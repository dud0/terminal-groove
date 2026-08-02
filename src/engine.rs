use crate::model::{DelayDivision, ProjectV1, STEP_COUNT, StepEvent, TrackKind, tie_source};

#[derive(Clone, Debug)]
pub enum EngineCommand {
    ReplaceProject(ProjectV1),
    SetProject(ProjectV1),
    PlayPause,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Paused,
}

pub struct StepClock {
    sample_rate: f64,
    bpm: u16,
    phase: f64,
    pub next_step: usize,
}
impl StepClock {
    pub fn new(sample_rate: u32, bpm: u16) -> Self {
        Self {
            sample_rate: sample_rate as f64,
            bpm,
            phase: 0.0,
            next_step: 0,
        }
    }
    pub fn set_bpm(&mut self, bpm: u16) {
        self.bpm = bpm
    }
    pub fn reset(&mut self) {
        self.phase = 0.;
        self.next_step = 0
    }
    pub fn restart_timing(&mut self) {
        self.phase = 0.;
    }
    pub fn step_samples(&self) -> f64 {
        self.sample_rate * 60.0 / (self.bpm as f64 * 4.0)
    }
    pub fn tick(&mut self) -> Option<usize> {
        self.phase -= 1.0;
        if self.phase <= 0.0 {
            let s = self.next_step;
            self.next_step = (s + 1) % STEP_COUNT;
            self.phase += self.step_samples();
            Some(s)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAction {
    None,
    Trigger { degree: u8, octave: u8 },
    Hold,
    Release,
}
pub fn synth_action(steps: &[crate::model::Step], step: usize, voice_active: bool) -> GateAction {
    match &steps[step] {
        Some(StepEvent::Note { degree, octave, .. }) => GateAction::Trigger {
            degree: *degree,
            octave: *octave,
        },
        Some(StepEvent::Tie { .. }) if voice_active => GateAction::Hold,
        Some(StepEvent::Tie { .. }) => tie_source(steps, step)
            .and_then(|i| match steps[i] {
                Some(StepEvent::Note { degree, octave, .. }) => {
                    Some(GateAction::Trigger { degree, octave })
                }
                _ => None,
            })
            .unwrap_or(GateAction::Release),
        _ => GateAction::Release,
    }
}

pub fn effective_level(project: &ProjectV1, track: usize, step: usize) -> f32 {
    let t = &project.tracks[track];
    if t.muted {
        return 0.0;
    }
    let p = t.steps[step]
        .as_ref()
        .and_then(|e| e.locks().level)
        .unwrap_or(t.level)
        .normalized();
    p * p
}
pub fn delay_samples(d: DelayDivision, bpm: u16, sr: u32) -> usize {
    d.samples(bpm, sr).round() as usize
}

pub struct Engine {
    pub project: ProjectV1,
    pub transport: Transport,
    pub playhead: Option<usize>,
    clock: StepClock,
    voices: [bool; 6],
}
impl Engine {
    pub fn new(project: ProjectV1, sr: u32) -> Self {
        let bpm = project.globals.tempo_bpm;
        Self {
            project,
            transport: Transport::Stopped,
            playhead: None,
            clock: StepClock::new(sr, bpm),
            voices: [false; 6],
        }
    }
    pub fn command(&mut self, c: EngineCommand) {
        match c {
            EngineCommand::ReplaceProject(p) | EngineCommand::SetProject(p) => {
                self.project = p;
                self.clock.set_bpm(self.project.globals.tempo_bpm)
            }
            EngineCommand::PlayPause => match self.transport {
                Transport::Stopped | Transport::Paused => {
                    self.transport = Transport::Playing;
                    self.clock.phase = 0.
                }
                Transport::Playing => {
                    self.transport = Transport::Paused;
                    self.voices = [false; 6]
                }
            },
            EngineCommand::Stop => {
                self.transport = Transport::Stopped;
                self.playhead = None;
                self.clock.reset();
                self.voices = [false; 6]
            }
        }
    }
    pub fn tick(&mut self) -> Option<usize> {
        if self.transport != Transport::Playing {
            return None;
        }
        self.clock.set_bpm(self.project.globals.tempo_bpm);
        let step = self.clock.tick()?;
        self.playhead = Some(step);
        for i in 0..6 {
            if self.project.tracks[i].kind == TrackKind::Synth {
                match synth_action(&self.project.tracks[i].steps, step, self.voices[i]) {
                    GateAction::Trigger { .. } | GateAction::Hold => self.voices[i] = true,
                    GateAction::Release => self.voices[i] = false,
                    GateAction::None => {}
                }
            }
        }
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_clock_has_no_drift() {
        let mut c = StepClock::new(44100, 123);
        let mut hits = Vec::new();
        for n in 0..1_000_000 {
            if c.tick().is_some() {
                hits.push(n)
            }
        }
        let ideal = 44100.0 * 60.0 / (123.0 * 4.0);
        for (i, n) in hits.iter().enumerate().take(1000) {
            assert!((*n as f64 - i as f64 * ideal).abs() <= 1.0)
        }
    }
    #[test]
    fn lock_restores() {
        let mut p = ProjectV1::new();
        p.tracks[0].steps[0] = Some(StepEvent::Trigger {
            locks: crate::model::ParameterLocks {
                level: crate::model::Percent::new(20),
                ..Default::default()
            },
        });
        assert!((effective_level(&p, 0, 0) - 0.04).abs() < 0.001);
        assert!((effective_level(&p, 0, 1) - 0.64).abs() < 0.001)
    }
}
