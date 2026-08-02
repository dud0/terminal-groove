use crate::model::{DelayDivision, ProjectV5, StepEvent, TRACK_COUNT, TrackKind, tie_source};

#[derive(Clone, Debug)]
pub enum EngineCommand {
    ReplaceProject(ProjectV5),
    SetProject(ProjectV5),
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
            self.next_step = s.wrapping_add(1);
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
    Trigger {
        degree: u8,
        octave: u8,
        accent: bool,
        slide: bool,
    },
    Hold,
    Release,
}
pub fn synth_action(steps: &[crate::model::Step], step: usize, voice_active: bool) -> GateAction {
    match &steps[step] {
        Some(StepEvent::Note {
            degree,
            octave,
            accent,
            ..
        }) => GateAction::Trigger {
            degree: *degree,
            octave: *octave,
            accent: *accent,
            slide: false,
        },
        Some(StepEvent::BassNote {
            degree,
            octave,
            accent,
            slide,
            ..
        }) => GateAction::Trigger {
            degree: *degree,
            octave: *octave,
            accent: *accent,
            slide: *slide,
        },
        Some(StepEvent::Tie { .. }) if voice_active => GateAction::Hold,
        Some(StepEvent::Tie { .. }) => tie_source(steps, step)
            .and_then(|i| match steps[i] {
                Some(StepEvent::Note {
                    degree,
                    octave,
                    accent,
                    ..
                }) => Some(GateAction::Trigger {
                    degree,
                    octave,
                    accent,
                    slide: false,
                }),
                Some(StepEvent::BassNote {
                    degree,
                    octave,
                    accent,
                    slide,
                    ..
                }) => Some(GateAction::Trigger {
                    degree,
                    octave,
                    accent,
                    slide,
                }),
                _ => None,
            })
            .unwrap_or(GateAction::Release),
        _ => GateAction::Release,
    }
}

pub fn effective_level(project: &ProjectV5, track: usize, step: usize) -> f32 {
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
    pub project: ProjectV5,
    pub transport: Transport,
    pub playheads: [Option<usize>; TRACK_COUNT],
    clock: StepClock,
    next_steps: [usize; TRACK_COUNT],
    voices: [bool; TRACK_COUNT],
}
impl Engine {
    pub fn new(project: ProjectV5, sr: u32) -> Self {
        let bpm = project.globals.tempo_bpm;
        Self {
            project,
            transport: Transport::Stopped,
            playheads: [None; TRACK_COUNT],
            clock: StepClock::new(sr, bpm),
            next_steps: [0; TRACK_COUNT],
            voices: [false; TRACK_COUNT],
        }
    }
    pub fn command(&mut self, c: EngineCommand) {
        match c {
            EngineCommand::ReplaceProject(p) | EngineCommand::SetProject(p) => {
                self.project = p;
                self.clock.set_bpm(self.project.globals.tempo_bpm);
                for i in 0..TRACK_COUNT {
                    if self.next_steps[i] >= self.project.tracks[i].steps.len() {
                        self.next_steps[i] = 0;
                    }
                    if self.playheads[i]
                        .is_some_and(|step| step >= self.project.tracks[i].steps.len())
                    {
                        self.playheads[i] = None;
                    }
                }
            }
            EngineCommand::PlayPause => match self.transport {
                Transport::Stopped | Transport::Paused => {
                    self.transport = Transport::Playing;
                    self.clock.phase = 0.
                }
                Transport::Playing => {
                    self.transport = Transport::Paused;
                    self.voices = [false; TRACK_COUNT]
                }
            },
            EngineCommand::Stop => {
                self.transport = Transport::Stopped;
                self.playheads = [None; TRACK_COUNT];
                self.clock.reset();
                self.next_steps = [0; TRACK_COUNT];
                self.voices = [false; TRACK_COUNT]
            }
        }
    }
    pub fn tick(&mut self) -> Option<[usize; TRACK_COUNT]> {
        if self.transport != Transport::Playing {
            return None;
        }
        self.clock.set_bpm(self.project.globals.tempo_bpm);
        self.clock.tick()?;
        let mut played = [0; TRACK_COUNT];
        for (i, played_step) in played.iter_mut().enumerate() {
            let step = self.next_steps[i];
            *played_step = step;
            self.playheads[i] = Some(step);
            if matches!(
                self.project.tracks[i].kind,
                TrackKind::Bass | TrackKind::Synth
            ) {
                match synth_action(&self.project.tracks[i].steps, step, self.voices[i]) {
                    GateAction::Trigger { .. } | GateAction::Hold => self.voices[i] = true,
                    GateAction::Release => self.voices[i] = false,
                    GateAction::None => {}
                }
            }
            self.next_steps[i] = (step + 1) % self.project.tracks[i].steps.len();
        }
        Some(played)
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
        let mut p = ProjectV5::new();
        p.tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: crate::model::ParameterLocks {
                level: crate::model::Percent::new(20),
                ..Default::default()
            },
        });
        assert!((effective_level(&p, 0, 0) - 0.04).abs() < 0.001);
        assert!((effective_level(&p, 0, 1) - 0.64).abs() < 0.001)
    }

    #[test]
    fn cold_tie_trigger_carries_the_source_note_accent() {
        let steps = vec![
            Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                locks: Default::default(),
            }),
            Some(StepEvent::Tie {
                locks: Default::default(),
            }),
        ];
        assert_eq!(
            synth_action(&steps, 1, false),
            GateAction::Trigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
            }
        );
        assert_eq!(synth_action(&steps, 1, true), GateAction::Hold);
    }

    #[test]
    fn tracks_cycle_at_independent_lengths_and_live_resize_keeps_position() {
        let mut project = ProjectV5::new();
        project.globals.tempo_bpm = 60;
        project.tracks[0].steps.resize(3, None);
        project.tracks[1].steps.resize(5, None);
        let mut engine = Engine::new(project, 4);
        engine.command(EngineCommand::PlayPause);
        assert_eq!(engine.tick().unwrap()[..2], [0, 0]);
        assert_eq!(engine.tick().unwrap()[..2], [1, 1]);

        let mut grown = engine.project.clone();
        grown.tracks[0].steps.resize(6, None);
        engine.command(EngineCommand::SetProject(grown));
        assert_eq!(engine.tick().unwrap()[..2], [2, 2]);

        let mut shrunk = engine.project.clone();
        shrunk.tracks[0].steps.resize(2, None);
        engine.command(EngineCommand::SetProject(shrunk));
        assert_eq!(engine.tick().unwrap()[..2], [0, 3]);
        assert_eq!(engine.tick().unwrap()[..2], [1, 4]);
        assert_eq!(engine.tick().unwrap()[..2], [0, 0]);
    }
}
