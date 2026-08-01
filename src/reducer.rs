use crate::model::{
    ParameterId, ParameterLocks, ParameterValue, Percent, ProjectV1, STEP_COUNT, StepEvent,
    TrackKind, Waveform, tie_source,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Base,
    Lock,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidTrack,
    InvalidStep,
    NotSynth,
    NotDrum,
    EmptyLock,
    InvalidParameter,
    InvalidTie,
}
impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::InvalidTrack => "invalid track",
                Self::InvalidStep => "invalid step",
                Self::NotSynth => "action requires a synth track",
                Self::NotDrum => "action requires a drum track",
                Self::EmptyLock => "cannot lock an empty step",
                Self::InvalidParameter => "parameter is incompatible with this track",
                Self::InvalidTie => "tie requires a preceding note",
            }
        )
    }
}

#[derive(Clone)]
struct Revision {
    before: ProjectV1,
    after: ProjectV1,
    coalesce: Option<CoalesceKey>,
    at: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceKey(pub usize, pub usize, pub u8);

pub struct Editor {
    pub project: ProjectV1,
    saved: ProjectV1,
    undo: VecDeque<Revision>,
    redo: Vec<Revision>,
}
impl Editor {
    pub fn new(project: ProjectV1) -> Self {
        Self {
            saved: project.clone(),
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.project != self.saved
    }
    pub fn mark_saved(&mut self) {
        self.saved = self.project.clone();
    }
    pub fn replace_loaded(&mut self, p: ProjectV1) {
        self.project = p.clone();
        self.saved = p;
        self.undo.clear();
        self.redo.clear();
    }
    pub fn edit<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut ProjectV1) -> Result<(), EditError>,
    {
        let before = self.project.clone();
        f(&mut self.project)?;
        if before == self.project {
            return Ok(false);
        }
        let now = Instant::now();
        let merge = key.is_some()
            && self.undo.back().is_some_and(|r| {
                r.coalesce == key && now.duration_since(r.at) <= Duration::from_millis(300)
            });
        if merge {
            let r = self.undo.back_mut().unwrap();
            r.after = self.project.clone();
            r.at = now;
        } else {
            if self.undo.len() == 256 {
                self.undo.pop_front();
            }
            self.undo.push_back(Revision {
                before,
                after: self.project.clone(),
                coalesce: key,
                at: now,
            });
        }
        self.redo.clear();
        Ok(true)
    }
    pub fn undo(&mut self) -> bool {
        if let Some(r) = self.undo.pop_back() {
            self.project = r.before.clone();
            self.redo.push(r);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(r) = self.redo.pop() {
            self.project = r.after.clone();
            self.undo.push_back(r);
            true
        } else {
            false
        }
    }
    pub fn toggle_event(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if step >= STEP_COUNT {
                return Err(EditError::InvalidStep);
            }
            if t.steps[step].is_some() {
                clear_with_ties(t, step);
                return Ok(());
            }
            t.steps[step] = Some(if t.kind == TrackKind::Synth {
                StepEvent::Note {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    locks: Default::default(),
                }
            } else {
                StepEvent::Trigger {
                    locks: Default::default(),
                }
            });
            Ok(())
        })
    }
    pub fn set_note(&mut self, track: usize, step: usize, degree: u8) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if t.kind != TrackKind::Synth {
                return Err(EditError::NotSynth);
            }
            if step >= 16 || !(1..=8).contains(&degree) {
                return Err(EditError::InvalidStep);
            }
            let locks = match t.steps[step].take() {
                Some(e) => e.locks().clone(),
                None => Default::default(),
            };
            let octave = t.input_octave.unwrap();
            t.input_degree = Some(degree);
            t.steps[step] = Some(StepEvent::Note {
                degree,
                octave,
                locks,
            });
            cleanup_invalid_ties(t);
            Ok(())
        })
    }
    pub fn toggle_tie(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if t.kind != TrackKind::Synth {
                return Err(EditError::NotSynth);
            }
            if step >= 16 {
                return Err(EditError::InvalidStep);
            }
            match t.steps[step].take() {
                Some(StepEvent::Tie { .. }) => {
                    cleanup_invalid_ties(t);
                    Ok(())
                }
                old => {
                    let locks = old.as_ref().map(|x| x.locks().clone()).unwrap_or_default();
                    t.steps[step] = Some(StepEvent::Tie { locks });
                    if tie_source(&t.steps, step).is_none() {
                        t.steps[step] = old;
                        return Err(EditError::InvalidTie);
                    }
                    Ok(())
                }
            }
        })
    }
    pub fn clear(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if step >= 16 {
                return Err(EditError::InvalidStep);
            }
            clear_with_ties(t, step);
            Ok(())
        })
    }
    pub fn set_level(
        &mut self,
        track: usize,
        step: usize,
        scope: Scope,
        value: Percent,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.set_parameter(
            track,
            step,
            scope,
            ParameterId::Level,
            ParameterValue::Percent(value),
            key,
        )
    }

    pub fn set_parameter(
        &mut self,
        track: usize,
        step: usize,
        scope: Scope,
        parameter: ParameterId,
        value: ParameterValue,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.edit(key, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if !parameter.is_valid_for(t.kind) {
                return Err(EditError::InvalidParameter);
            }
            match scope {
                Scope::Base => set_track_parameter(t, parameter, value),
                Scope::Lock => {
                    let event = t
                        .steps
                        .get_mut(step)
                        .ok_or(EditError::InvalidStep)?
                        .as_mut()
                        .ok_or(EditError::EmptyLock)?;
                    set_lock_parameter(event.locks_mut(), parameter, value)
                }
            }
        })
    }

    pub fn parameter_value(
        &self,
        track: usize,
        step: usize,
        scope: Scope,
        parameter: ParameterId,
    ) -> Result<ParameterValue, EditError> {
        let t = self
            .project
            .tracks
            .get(track)
            .ok_or(EditError::InvalidTrack)?;
        if !parameter.is_valid_for(t.kind) {
            return Err(EditError::InvalidParameter);
        }
        let base = track_parameter(t, parameter)?;
        if scope == Scope::Base {
            return Ok(base);
        }
        let event = t
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .ok_or(EditError::EmptyLock)?;
        Ok(lock_parameter(event.locks(), parameter).unwrap_or(base))
    }

    pub fn clear_parameter_lock(
        &mut self,
        track: usize,
        step: usize,
        parameter: ParameterId,
    ) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if !parameter.is_valid_for(t.kind) {
                return Err(EditError::InvalidParameter);
            }
            let event = t
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::EmptyLock)?;
            clear_lock_parameter(event.locks_mut(), parameter);
            Ok(())
        })
    }

    pub fn toggle_waveform(
        &mut self,
        track: usize,
        step: usize,
        scope: Scope,
    ) -> Result<bool, EditError> {
        let next = match self.parameter_value(track, step, scope, ParameterId::Waveform)? {
            ParameterValue::Waveform(Waveform::Square) => Waveform::Saw,
            ParameterValue::Waveform(Waveform::Saw) => Waveform::Square,
            ParameterValue::Percent(_) => return Err(EditError::InvalidParameter),
        };
        self.set_parameter(
            track,
            step,
            scope,
            ParameterId::Waveform,
            ParameterValue::Waveform(next),
            None,
        )
    }
}

fn track_parameter(
    t: &crate::model::Track,
    parameter: ParameterId,
) -> Result<ParameterValue, EditError> {
    let value = match parameter {
        ParameterId::Level => ParameterValue::Percent(t.level),
        ParameterId::DelaySend => ParameterValue::Percent(t.delay_send),
        ParameterId::ReverbSend => ParameterValue::Percent(t.reverb_send),
        ParameterId::Tone => match &t.instrument {
            crate::model::Instrument::Drum(p) => ParameterValue::Percent(p.tone),
            crate::model::Instrument::Synth(_) => return Err(EditError::InvalidParameter),
        },
        ParameterId::Decay => match &t.instrument {
            crate::model::Instrument::Drum(p) => ParameterValue::Percent(p.decay),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.decay),
        },
        ParameterId::Waveform => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Waveform(p.waveform),
        },
        ParameterId::Cutoff => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.cutoff),
        },
        ParameterId::Resonance => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.resonance),
        },
        ParameterId::FilterEnvelope => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.filter_envelope),
        },
        ParameterId::Attack => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.attack),
        },
        ParameterId::Sustain => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.sustain),
        },
        ParameterId::Release => match &t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => ParameterValue::Percent(p.release),
        },
    };
    Ok(value)
}

fn set_track_parameter(
    t: &mut crate::model::Track,
    parameter: ParameterId,
    value: ParameterValue,
) -> Result<(), EditError> {
    match (parameter, value) {
        (ParameterId::Level, ParameterValue::Percent(v)) => t.level = v,
        (ParameterId::DelaySend, ParameterValue::Percent(v)) => t.delay_send = v,
        (ParameterId::ReverbSend, ParameterValue::Percent(v)) => t.reverb_send = v,
        (ParameterId::Tone, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(p) => p.tone = v,
            crate::model::Instrument::Synth(_) => return Err(EditError::InvalidParameter),
        },
        (ParameterId::Decay, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(p) => p.decay = v,
            crate::model::Instrument::Synth(p) => p.decay = v,
        },
        (ParameterId::Waveform, ParameterValue::Waveform(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.waveform = v,
        },
        (ParameterId::Cutoff, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.cutoff = v,
        },
        (ParameterId::Resonance, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.resonance = v,
        },
        (ParameterId::FilterEnvelope, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.filter_envelope = v,
        },
        (ParameterId::Attack, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.attack = v,
        },
        (ParameterId::Sustain, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.sustain = v,
        },
        (ParameterId::Release, ParameterValue::Percent(v)) => match &mut t.instrument {
            crate::model::Instrument::Drum(_) => return Err(EditError::InvalidParameter),
            crate::model::Instrument::Synth(p) => p.release = v,
        },
        _ => return Err(EditError::InvalidParameter),
    }
    Ok(())
}

fn lock_parameter(l: &ParameterLocks, parameter: ParameterId) -> Option<ParameterValue> {
    match parameter {
        ParameterId::Level => l.level.map(ParameterValue::Percent),
        ParameterId::DelaySend => l.delay_send.map(ParameterValue::Percent),
        ParameterId::ReverbSend => l.reverb_send.map(ParameterValue::Percent),
        ParameterId::Tone => l.tone.map(ParameterValue::Percent),
        ParameterId::Decay => l.decay.map(ParameterValue::Percent),
        ParameterId::Waveform => l.waveform.map(ParameterValue::Waveform),
        ParameterId::Cutoff => l.cutoff.map(ParameterValue::Percent),
        ParameterId::Resonance => l.resonance.map(ParameterValue::Percent),
        ParameterId::FilterEnvelope => l.filter_envelope.map(ParameterValue::Percent),
        ParameterId::Attack => l.attack.map(ParameterValue::Percent),
        ParameterId::Sustain => l.sustain.map(ParameterValue::Percent),
        ParameterId::Release => l.release.map(ParameterValue::Percent),
    }
}

fn set_lock_parameter(
    l: &mut ParameterLocks,
    parameter: ParameterId,
    value: ParameterValue,
) -> Result<(), EditError> {
    match (parameter, value) {
        (ParameterId::Level, ParameterValue::Percent(v)) => l.level = Some(v),
        (ParameterId::DelaySend, ParameterValue::Percent(v)) => l.delay_send = Some(v),
        (ParameterId::ReverbSend, ParameterValue::Percent(v)) => l.reverb_send = Some(v),
        (ParameterId::Tone, ParameterValue::Percent(v)) => l.tone = Some(v),
        (ParameterId::Decay, ParameterValue::Percent(v)) => l.decay = Some(v),
        (ParameterId::Waveform, ParameterValue::Waveform(v)) => l.waveform = Some(v),
        (ParameterId::Cutoff, ParameterValue::Percent(v)) => l.cutoff = Some(v),
        (ParameterId::Resonance, ParameterValue::Percent(v)) => l.resonance = Some(v),
        (ParameterId::FilterEnvelope, ParameterValue::Percent(v)) => l.filter_envelope = Some(v),
        (ParameterId::Attack, ParameterValue::Percent(v)) => l.attack = Some(v),
        (ParameterId::Sustain, ParameterValue::Percent(v)) => l.sustain = Some(v),
        (ParameterId::Release, ParameterValue::Percent(v)) => l.release = Some(v),
        _ => return Err(EditError::InvalidParameter),
    }
    Ok(())
}

fn clear_lock_parameter(l: &mut ParameterLocks, parameter: ParameterId) {
    match parameter {
        ParameterId::Level => l.level = None,
        ParameterId::DelaySend => l.delay_send = None,
        ParameterId::ReverbSend => l.reverb_send = None,
        ParameterId::Tone => l.tone = None,
        ParameterId::Decay => l.decay = None,
        ParameterId::Waveform => l.waveform = None,
        ParameterId::Cutoff => l.cutoff = None,
        ParameterId::Resonance => l.resonance = None,
        ParameterId::FilterEnvelope => l.filter_envelope = None,
        ParameterId::Attack => l.attack = None,
        ParameterId::Sustain => l.sustain = None,
        ParameterId::Release => l.release = None,
    }
}
fn clear_with_ties(t: &mut crate::model::Track, step: usize) {
    t.steps[step] = None;
    cleanup_invalid_ties(t)
}
fn cleanup_invalid_ties(t: &mut crate::model::Track) {
    loop {
        let bad = (0..16).find(|&i| {
            matches!(t.steps[i], Some(StepEvent::Tie { .. })) && tie_source(&t.steps, i).is_none()
        });
        if let Some(i) = bad {
            t.steps[i] = None
        } else {
            break;
        }
    }
}

pub fn percentage_key(c: char) -> Option<Percent> {
    match c {
        '`' => Percent::new(0),
        '1'..='9' => Percent::new(c.to_digit(10).unwrap() as u8 * 10),
        '0' => Percent::new(100),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undo_dirty_redo() {
        let mut e = Editor::new(ProjectV1::new());
        e.toggle_event(0, 0).unwrap();
        assert!(e.is_dirty());
        assert!(e.undo());
        assert!(!e.is_dirty());
        assert!(e.redo());
        assert!(e.is_dirty());
    }
    #[test]
    fn edit_invalidates_redo() {
        let mut e = Editor::new(ProjectV1::new());
        e.toggle_event(0, 0).unwrap();
        e.undo();
        e.toggle_event(0, 1).unwrap();
        assert!(!e.redo());
    }
    #[test]
    fn tie_cleanup_is_atomic() {
        let mut e = Editor::new(ProjectV1::new());
        e.set_note(3, 0, 1).unwrap();
        e.toggle_tie(3, 1).unwrap();
        e.toggle_tie(3, 2).unwrap();
        e.clear(3, 0).unwrap();
        assert!(e.project.tracks[3].steps[1].is_none());
        e.undo();
        assert!(matches!(
            e.project.tracks[3].steps[2],
            Some(StepEvent::Tie { .. })
        ));
    }
    #[test]
    fn direct_percent() {
        assert_eq!(percentage_key('`').unwrap().get(), 0);
        assert_eq!(percentage_key('0').unwrap().get(), 100);
    }

    #[test]
    fn edits_all_track_parameter_kinds() {
        let mut e = Editor::new(ProjectV1::new());
        let p = |value| ParameterValue::Percent(Percent::new(value).unwrap());

        e.set_parameter(0, 0, Scope::Base, ParameterId::Level, p(61), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Base, ParameterId::Tone, p(72), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Base, ParameterId::Decay, p(34), None)
            .unwrap();
        assert_eq!(e.project.tracks[0].level.get(), 61);
        let crate::model::Instrument::Drum(drum) = &e.project.tracks[0].instrument else {
            panic!("expected drum")
        };
        assert_eq!(drum.tone.get(), 72);
        assert_eq!(drum.decay.get(), 34);

        for (parameter, value) in [
            (ParameterId::Cutoff, 41),
            (ParameterId::Resonance, 22),
            (ParameterId::FilterEnvelope, 63),
            (ParameterId::Attack, 4),
            (ParameterId::Decay, 31),
            (ParameterId::Sustain, 74),
            (ParameterId::Release, 18),
        ] {
            e.set_parameter(3, 0, Scope::Base, parameter, p(value), None)
                .unwrap();
        }
        e.set_parameter(
            3,
            0,
            Scope::Base,
            ParameterId::Waveform,
            ParameterValue::Waveform(Waveform::Square),
            None,
        )
        .unwrap();
        let crate::model::Instrument::Synth(synth) = &e.project.tracks[3].instrument else {
            panic!("expected synth")
        };
        assert_eq!(synth.waveform, Waveform::Square);
        assert_eq!(synth.cutoff.get(), 41);
        assert_eq!(synth.resonance.get(), 22);
        assert_eq!(synth.filter_envelope.get(), 63);
        assert_eq!(synth.attack.get(), 4);
        assert_eq!(synth.decay.get(), 31);
        assert_eq!(synth.sustain.get(), 74);
        assert_eq!(synth.release.get(), 18);
    }

    #[test]
    fn lock_edits_inherit_and_clear_one_parameter() {
        let mut e = Editor::new(ProjectV1::new());
        let p = |value| ParameterValue::Percent(Percent::new(value).unwrap());
        e.toggle_event(0, 0).unwrap();
        assert_eq!(
            e.parameter_value(0, 0, Scope::Lock, ParameterId::Level),
            Ok(p(80))
        );
        e.set_parameter(0, 0, Scope::Lock, ParameterId::Level, p(20), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Lock, ParameterId::Tone, p(90), None)
            .unwrap();
        assert_eq!(
            e.parameter_value(0, 0, Scope::Lock, ParameterId::Level),
            Ok(p(20))
        );
        assert_eq!(
            e.project.tracks[0].steps[0]
                .as_ref()
                .unwrap()
                .locks()
                .tone
                .unwrap()
                .get(),
            90
        );
        e.clear_parameter_lock(0, 0, ParameterId::Level).unwrap();
        assert_eq!(
            e.parameter_value(0, 0, Scope::Lock, ParameterId::Level),
            Ok(p(80))
        );
        assert!(
            e.project.tracks[0].steps[0]
                .as_ref()
                .unwrap()
                .locks()
                .tone
                .is_some()
        );
    }

    #[test]
    fn incompatible_and_empty_locks_are_rejected() {
        let mut e = Editor::new(ProjectV1::new());
        let value = ParameterValue::Percent(Percent::new(10).unwrap());
        assert_eq!(
            e.set_parameter(0, 0, Scope::Lock, ParameterId::Cutoff, value, None),
            Err(EditError::InvalidParameter)
        );
        assert_eq!(
            e.set_parameter(0, 0, Scope::Lock, ParameterId::Level, value, None),
            Err(EditError::EmptyLock)
        );
        e.toggle_event(3, 0).unwrap();
        assert_eq!(
            e.set_parameter(3, 0, Scope::Lock, ParameterId::Tone, value, None),
            Err(EditError::InvalidParameter)
        );
    }
}
