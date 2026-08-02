use crate::model::{StepEvent, tie_source};

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
}
