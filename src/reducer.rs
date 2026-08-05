use crate::model::{
    ChordShape, LfoConfig, MAX_STEP_COUNT, MIN_STEP_COUNT, ParameterId, ParameterValue, Pattern,
    Percent, Project, StepEvent, TrackKind, Waveform, tie_source,
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
    InvalidLength,
    CannotDouble,
    NoAccent,
    NoSlide,
    NoChordShape,
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
                Self::InvalidLength => "track length must be between 1 and 64 steps",
                Self::CannotDouble => "track is longer than 32 steps and cannot be doubled",
                Self::NoAccent => "accent requires a trigger or note",
                Self::NoSlide => "slide requires a Bass note",
                Self::NoChordShape => "chord shape requires a Chord note or empty Chord step",
            }
        )
    }
}

#[derive(Clone)]
struct Revision {
    before: Project,
    after: Project,
    before_pattern: usize,
    after_pattern: usize,
    coalesce: Option<CoalesceKey>,
    at: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceKey(pub usize, pub usize, pub u8);

pub struct Editor {
    pub project: Project,
    pattern: usize,
    saved: Project,
    undo: VecDeque<Revision>,
    redo: Vec<Revision>,
    clipboard: Option<Pattern>,
}
impl Editor {
    pub fn new(project: Project) -> Self {
        Self {
            saved: project.clone(),
            project,
            pattern: 0,
            undo: VecDeque::new(),
            redo: Vec::new(),
            clipboard: None,
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.project != self.saved
    }
    pub fn mark_saved(&mut self) {
        self.saved = self.project.clone();
    }
    pub fn end_coalescing(&mut self) {
        if let Some(revision) = self.undo.back_mut() {
            revision.coalesce = None;
        }
    }
    pub fn replace_loaded(&mut self, p: Project) {
        self.project = p.clone();
        self.saved = p;
        self.undo.clear();
        self.redo.clear();
        self.pattern = 0;
        self.clipboard = None;
    }
    pub fn pattern(&self) -> usize {
        self.pattern
    }
    pub fn select_pattern(&mut self, pattern: usize) -> bool {
        if pattern >= self.project.patterns.len() || !self.project.activate_pattern(pattern) {
            return false;
        }
        self.pattern = pattern;
        true
    }
    fn empty_pattern() -> Pattern {
        Pattern {
            tracks: (0..crate::model::TRACK_COUNT)
                .map(|_| crate::model::PatternTrack {
                    steps: vec![None; crate::model::STEP_BANK_SIZE],
                })
                .collect(),
        }
    }
    fn current_pattern(&self) -> Pattern {
        let mut pattern = self.project.patterns[self.pattern].clone();
        for (dst, track) in pattern.tracks.iter_mut().zip(&self.project.tracks) {
            dst.steps = track.steps.clone();
        }
        pattern
    }
    fn pattern_structure_edit<F>(&mut self, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<usize, EditError>,
    {
        self.project.store_active_pattern(self.pattern);
        let before = self.project.clone();
        let before_pattern = self.pattern;
        let next_pattern = f(&mut self.project, self.pattern)?;
        self.pattern = next_pattern;
        self.project.activate_pattern(self.pattern);
        if before == self.project {
            return Ok(false);
        }
        self.push_revision(
            before,
            before_pattern,
            self.project.clone(),
            self.pattern,
            None,
        );
        Ok(true)
    }

    fn pattern_structure_edit_at<F>(
        &mut self,
        cursor: usize,
        f: F,
    ) -> Result<(bool, usize), EditError>
    where
        F: FnOnce(&mut Project, usize, usize) -> Result<(usize, usize), EditError>,
    {
        if cursor >= self.project.patterns.len() {
            return Ok((false, self.pattern));
        }
        self.project.store_active_pattern(self.pattern);
        let before = self.project.clone();
        let before_pattern = self.pattern;
        let (next_cursor, next_active) = f(&mut self.project, cursor, before_pattern)?;
        self.pattern = next_active;
        self.project.activate_pattern(self.pattern);
        if before == self.project {
            return Ok((false, next_cursor));
        }
        self.push_revision(
            before,
            before_pattern,
            self.project.clone(),
            self.pattern,
            None,
        );
        Ok((true, next_cursor))
    }
    fn push_revision(
        &mut self,
        before: Project,
        before_pattern: usize,
        after: Project,
        after_pattern: usize,
        coalesce: Option<CoalesceKey>,
    ) {
        if self.undo.len() == 256 {
            self.undo.pop_front();
        }
        self.undo.push_back(Revision {
            before,
            after,
            before_pattern,
            after_pattern,
            coalesce,
            at: Instant::now(),
        });
        self.redo.clear();
    }
    pub fn insert_pattern(&mut self) -> Result<bool, EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok(false);
        }
        self.pattern_structure_edit(|p, cursor| {
            p.patterns.insert(cursor + 1, Self::empty_pattern());
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            Ok(cursor + 1)
        })
    }

    pub fn insert_pattern_at(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit_at(cursor, |p, cursor, active| {
            p.patterns.insert(cursor + 1, Self::empty_pattern());
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            let active = if active > cursor { active + 1 } else { active };
            Ok((cursor + 1, active))
        })
    }
    pub fn duplicate_pattern(&mut self) -> Result<bool, EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok(false);
        }
        self.pattern_structure_edit(|p, cursor| {
            p.patterns.insert(cursor + 1, p.patterns[cursor].clone());
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            Ok(cursor + 1)
        })
    }

    pub fn duplicate_pattern_at(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit_at(cursor, |p, cursor, active| {
            p.patterns.insert(cursor + 1, p.patterns[cursor].clone());
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            let active = if active > cursor { active + 1 } else { active };
            Ok((cursor + 1, active))
        })
    }
    pub fn copy_pattern(&mut self) {
        self.clipboard = Some(self.current_pattern());
    }

    pub fn copy_pattern_at(&mut self, cursor: usize) -> bool {
        let Some(pattern) = self.project.patterns.get(cursor).cloned() else {
            return false;
        };
        self.clipboard = Some(if cursor == self.pattern {
            self.current_pattern()
        } else {
            pattern
        });
        true
    }
    pub fn cut_pattern(&mut self) -> Result<bool, EditError> {
        self.clipboard = Some(self.current_pattern());
        self.delete_pattern()
    }

    pub fn cut_pattern_at(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if !self.copy_pattern_at(cursor) {
            return Ok((false, cursor));
        }
        self.delete_pattern_at(cursor)
    }
    pub fn paste_pattern(&mut self) -> Result<bool, EditError> {
        let Some(pattern) = self.clipboard.clone() else {
            return Ok(false);
        };
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok(false);
        }
        self.pattern_structure_edit(|p, cursor| {
            p.patterns.insert(cursor + 1, pattern);
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            Ok(cursor + 1)
        })
    }

    pub fn paste_pattern_at(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        let Some(pattern) = self.clipboard.clone() else {
            return Ok((false, cursor));
        };
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit_at(cursor, |p, cursor, active| {
            p.patterns.insert(cursor + 1, pattern);
            for entry in &mut p.song {
                if usize::from(entry.pattern) > cursor + 1 {
                    entry.pattern += 1;
                }
            }
            let active = if active > cursor { active + 1 } else { active };
            Ok((cursor + 1, active))
        })
    }
    pub fn delete_pattern(&mut self) -> Result<bool, EditError> {
        self.pattern_structure_edit(|p, cursor| {
            if p.patterns.len() == crate::model::MIN_PATTERN_COUNT {
                p.patterns[cursor] = Self::empty_pattern();
                return Ok(cursor);
            }
            p.patterns.remove(cursor);
            let fallback = cursor.min(p.patterns.len() - 1) + 1;
            let removed = cursor + 1;
            for entry in &mut p.song {
                if entry.pattern == removed as u8 {
                    entry.pattern = fallback as u8;
                } else if entry.pattern > removed as u8 {
                    entry.pattern -= 1;
                }
            }
            Ok(fallback - 1)
        })
    }

    pub fn delete_pattern_at(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        self.pattern_structure_edit_at(cursor, |p, cursor, active| {
            if p.patterns.len() == crate::model::MIN_PATTERN_COUNT {
                p.patterns[cursor] = Self::empty_pattern();
                return Ok((cursor, active));
            }
            p.patterns.remove(cursor);
            let fallback = cursor.min(p.patterns.len() - 1);
            let removed = cursor + 1;
            for entry in &mut p.song {
                if entry.pattern == removed as u8 {
                    entry.pattern = (fallback + 1) as u8;
                } else if entry.pattern > removed as u8 {
                    entry.pattern -= 1;
                }
            }
            let active = if active > cursor {
                active - 1
            } else if active == cursor {
                fallback
            } else {
                active
            };
            Ok((fallback, active))
        })
    }
    pub fn clear_pattern(&mut self) -> Result<bool, EditError> {
        self.pattern_structure_edit(|p, cursor| {
            p.patterns[cursor] = Self::empty_pattern();
            Ok(cursor)
        })
    }
    pub fn edit<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project) -> Result<(), EditError>,
    {
        let before = self.project.clone();
        f(&mut self.project)?;
        self.project.store_active_pattern(self.pattern);
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
            r.after_pattern = self.pattern;
            r.at = now;
        } else {
            if self.undo.len() == 256 {
                self.undo.pop_front();
            }
            self.undo.push_back(Revision {
                before,
                after: self.project.clone(),
                before_pattern: self.pattern,
                after_pattern: self.pattern,
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
            self.pattern = r.before_pattern;
            self.redo.push(r);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(r) = self.redo.pop() {
            self.project = r.after.clone();
            self.pattern = r.after_pattern;
            self.undo.push_back(r);
            true
        } else {
            false
        }
    }
    pub fn toggle_event(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            if t.steps[step].is_some() {
                clear_with_ties(t, step);
                return Ok(());
            }
            t.steps[step] = Some(if t.kind == TrackKind::Bass {
                StepEvent::BassNote {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: false,
                    slide: false,
                    locks: Default::default(),
                }
            } else if matches!(t.kind, TrackKind::Chord | TrackKind::Lead) {
                StepEvent::Note {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: false,
                    chord_shape: (t.kind == TrackKind::Chord)
                        .then_some(t.input_chord_shape.unwrap_or_default()),
                    locks: Default::default(),
                }
            } else {
                StepEvent::Trigger {
                    accent: false,
                    locks: Default::default(),
                }
            });
            Ok(())
        })
    }
    pub fn set_note(&mut self, track: usize, step: usize, degree: u8) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if !matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
                return Err(EditError::NotSynth);
            }
            if step >= t.steps.len() || !(1..=8).contains(&degree) {
                return Err(EditError::InvalidStep);
            }
            let (locks, accent, slide, chord_shape, existing_note) = match t.steps[step].take() {
                Some(StepEvent::BassNote {
                    accent,
                    slide,
                    locks,
                    ..
                }) => (locks, accent, slide, None, false),
                Some(StepEvent::Note {
                    accent,
                    chord_shape,
                    locks,
                    ..
                }) => (locks, accent, false, chord_shape, true),
                Some(event) => (*event.locks(), false, false, None, false),
                None => (Default::default(), false, false, None, false),
            };
            let octave = t.input_octave.unwrap();
            t.input_degree = Some(degree);
            t.steps[step] = Some(if t.kind == TrackKind::Bass {
                StepEvent::BassNote {
                    degree,
                    octave,
                    accent,
                    slide,
                    locks,
                }
            } else {
                StepEvent::Note {
                    degree,
                    octave,
                    accent,
                    chord_shape: (t.kind == TrackKind::Chord).then_some(if existing_note {
                        chord_shape.unwrap_or_default()
                    } else {
                        t.input_chord_shape.unwrap_or_default()
                    }),
                    locks,
                }
            });
            cleanup_invalid_ties(t);
            Ok(())
        })
    }

    pub fn set_chord_shape(
        &mut self,
        track: usize,
        step: usize,
        shape: ChordShape,
    ) -> Result<bool, EditError> {
        self.edit(None, move |project| {
            let t = project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?;
            if t.kind != TrackKind::Chord {
                return Err(EditError::NoChordShape);
            }
            let event = t.steps.get_mut(step).ok_or(EditError::InvalidStep)?;
            match event {
                Some(StepEvent::Note { chord_shape, .. }) => {
                    *chord_shape = (shape != ChordShape::default()).then_some(shape);
                }
                None => {
                    t.input_chord_shape = (shape != ChordShape::default()).then_some(shape);
                }
                Some(StepEvent::Tie { .. }) => return Err(EditError::NoChordShape),
                Some(_) => return Err(EditError::NoChordShape),
            }
            Ok(())
        })
    }
    pub fn toggle_tie(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if !matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
                return Err(EditError::NotSynth);
            }
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            match t.steps[step].take() {
                Some(StepEvent::Tie { .. }) => {
                    cleanup_invalid_ties(t);
                    Ok(())
                }
                old => {
                    let locks = old.as_ref().map(|x| *x.locks()).unwrap_or_default();
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
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            clear_with_ties(t, step);
            Ok(())
        })
    }

    pub fn set_track_length(
        &mut self,
        track: usize,
        length: usize,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        if !(MIN_STEP_COUNT..=MAX_STEP_COUNT).contains(&length) {
            return Err(EditError::InvalidLength);
        }
        self.edit(key, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            t.steps.resize(length, None);
            cleanup_invalid_ties(t);
            Ok(())
        })
    }

    pub fn duplicate_track(&mut self, track: usize) -> Result<bool, EditError> {
        self.edit(None, move |p| {
            let t = p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;
            if t.steps.len() > MAX_STEP_COUNT / 2 {
                return Err(EditError::CannotDouble);
            }
            let copy = t.steps.clone();
            t.steps.extend(copy);
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

    pub fn set_pan(
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
            ParameterId::Pan,
            ParameterValue::Percent(value),
            key,
        )
    }

    pub fn accent_value(&self, track: usize, step: usize) -> Result<bool, EditError> {
        self.project
            .tracks
            .get(track)
            .ok_or(EditError::InvalidTrack)?
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .and_then(StepEvent::accent)
            .ok_or(EditError::NoAccent)
    }

    pub fn toggle_accent(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |project| {
            let event = project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::NoAccent)?;
            let accent = event.accent_mut().ok_or(EditError::NoAccent)?;
            *accent = !*accent;
            Ok(())
        })
    }

    pub fn toggle_slide(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit(None, move |project| {
            let t = project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?;
            if t.kind != TrackKind::Bass {
                return Err(EditError::NoSlide);
            }
            let event = t
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::NoSlide)?;
            let slide = event.slide_mut().ok_or(EditError::NoSlide)?;
            *slide = !*slide;
            Ok(())
        })
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
                Scope::Base => t
                    .set_parameter(parameter, value)
                    .then_some(())
                    .ok_or(EditError::InvalidParameter),
                Scope::Lock => {
                    let event = t
                        .steps
                        .get_mut(step)
                        .ok_or(EditError::InvalidStep)?
                        .as_mut()
                        .ok_or(EditError::EmptyLock)?;
                    event
                        .locks_mut()
                        .set(parameter, value)
                        .then_some(())
                        .ok_or(EditError::InvalidParameter)
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
        let base = t.parameter(parameter).ok_or(EditError::InvalidParameter)?;
        if scope == Scope::Base {
            return Ok(base);
        }
        let event = t
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .ok_or(EditError::EmptyLock)?;
        Ok(event.locks().get(parameter).unwrap_or(base))
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
            event.locks_mut().clear(parameter);
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
            ParameterValue::Percent(_) | ParameterValue::Chorus(_) | ParameterValue::Spread(_) => {
                return Err(EditError::InvalidParameter);
            }
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

    pub fn lfo(
        &self,
        track: usize,
        parameter: ParameterId,
    ) -> Result<Option<LfoConfig>, EditError> {
        let track = self
            .project
            .tracks
            .get(track)
            .ok_or(EditError::InvalidTrack)?;
        if !parameter.supports_lfo(track.kind) {
            return Err(EditError::InvalidParameter);
        }
        Ok(track.lfos.get(parameter))
    }

    pub fn set_lfo(
        &mut self,
        track: usize,
        parameter: ParameterId,
        config: Option<LfoConfig>,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.edit(key, move |project| {
            let track = project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?;
            if !parameter.supports_lfo(track.kind) || !track.lfos.set(parameter, config) {
                return Err(EditError::InvalidParameter);
            }
            Ok(())
        })
    }
}
fn clear_with_ties(t: &mut crate::model::Track, step: usize) {
    t.steps[step] = None;
    cleanup_invalid_ties(t)
}
fn cleanup_invalid_ties(t: &mut crate::model::Track) {
    loop {
        let bad = (0..t.steps.len()).find(|&i| {
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
        let mut e = Editor::new(Project::new());
        e.toggle_event(0, 0).unwrap();
        assert!(e.is_dirty());
        assert!(e.undo());
        assert!(!e.is_dirty());
        assert!(e.redo());
        assert!(e.is_dirty());
    }

    #[test]
    fn dynamic_pattern_operations_shift_cursor_and_are_undoable() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        assert!(editor.insert_pattern().unwrap());
        assert_eq!(editor.project.patterns.len(), 2);
        assert_eq!(editor.pattern(), 1);
        assert!(editor.project.patterns[0].tracks[0].steps[0].is_some());
        assert!(editor.project.patterns[1].tracks[0].steps[0].is_none());

        assert!(editor.duplicate_pattern().unwrap());
        assert_eq!(editor.project.patterns.len(), 3);
        assert_eq!(editor.pattern(), 2);
        assert!(editor.delete_pattern().unwrap());
        assert_eq!(editor.project.patterns.len(), 2);
        assert_eq!(editor.pattern(), 1);
        assert!(editor.undo());
        assert_eq!(editor.project.patterns.len(), 3);
        assert_eq!(editor.pattern(), 2);
        assert!(editor.redo());
        assert_eq!(editor.project.patterns.len(), 2);
    }

    #[test]
    fn copy_cut_paste_and_final_pattern_reset() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.copy_pattern();
        editor.clear_pattern().unwrap();
        assert!(editor.paste_pattern().unwrap());
        assert!(editor.project.tracks[0].steps[0].is_some());
        assert!(editor.cut_pattern().unwrap());
        assert_eq!(editor.project.patterns.len(), 1);
        assert!(editor.project.tracks[0].steps.iter().all(Option::is_none));
        assert!(!editor.delete_pattern().unwrap());
        assert_eq!(editor.project.patterns.len(), 1);
        assert!(editor.project.tracks[0].steps.iter().all(Option::is_none));
    }

    #[test]
    fn cursor_pattern_operations_preserve_the_committed_editor_pattern() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.duplicate_pattern().unwrap();
        editor.duplicate_pattern().unwrap();
        editor.select_pattern(0);

        let (changed, cursor) = editor.insert_pattern_at(1).unwrap();
        assert!(changed);
        assert_eq!(cursor, 2);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.project.tracks[0].steps[0].is_some());
        assert_eq!(editor.project.patterns.len(), 4);

        let (changed, cursor) = editor.delete_pattern_at(cursor).unwrap();
        assert!(changed);
        assert_eq!(cursor, 2);
        assert_eq!(editor.pattern(), 0);
        assert_eq!(editor.project.patterns.len(), 3);
    }

    #[test]
    fn cursor_clipboard_operations_use_the_cursor_pattern() {
        let mut editor = Editor::new(Project::new());
        editor.duplicate_pattern().unwrap();
        editor.select_pattern(0);
        editor.project.patterns[1].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: Default::default(),
        });

        assert!(editor.copy_pattern_at(1));
        let (changed, cursor) = editor.paste_pattern_at(0).unwrap();
        assert!(changed);
        assert_eq!(cursor, 1);
        assert!(editor.project.patterns[1].tracks[0].steps[0].is_some());
        assert!(editor.cut_pattern_at(1).unwrap().0);
        assert_eq!(editor.project.patterns.len(), 2);
    }
    #[test]
    fn edit_invalidates_redo() {
        let mut e = Editor::new(Project::new());
        e.toggle_event(0, 0).unwrap();
        e.undo();
        e.toggle_event(0, 1).unwrap();
        assert!(!e.redo());
    }
    #[test]
    fn tie_cleanup_is_atomic() {
        let mut e = Editor::new(Project::new());
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
    fn new_events_are_unaccented_and_bass_notes_do_not_slide() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.toggle_event(3, 0).unwrap();
        assert_eq!(editor.accent_value(0, 0), Ok(false));
        assert!(matches!(
            editor.project.tracks[3].steps[0],
            Some(StepEvent::BassNote {
                accent: false,
                slide: false,
                ..
            })
        ));
    }

    #[test]
    fn accent_and_slide_are_undoable_and_rejected_where_incompatible() {
        let mut editor = Editor::new(Project::new());
        editor.set_note(3, 0, 1).unwrap();
        editor.toggle_accent(3, 0).unwrap();
        editor.toggle_slide(3, 0).unwrap();
        editor.set_note(3, 0, 5).unwrap();
        assert_eq!(editor.accent_value(3, 0), Ok(true));
        assert!(matches!(
            editor.project.tracks[3].steps[0],
            Some(StepEvent::BassNote { slide: true, .. })
        ));
        editor.toggle_tie(3, 1).unwrap();
        assert_eq!(editor.toggle_accent(3, 1), Err(EditError::NoAccent));
        assert_eq!(editor.toggle_slide(4, 0), Err(EditError::NoSlide));
        assert!(editor.undo());
    }

    #[test]
    fn edits_all_track_parameter_kinds() {
        let mut e = Editor::new(Project::new());
        let p = |value| ParameterValue::Percent(Percent::new(value).unwrap());

        e.set_parameter(0, 0, Scope::Base, ParameterId::Level, p(61), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Base, ParameterId::Tune, p(72), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Base, ParameterId::Decay, p(34), None)
            .unwrap();
        assert_eq!(e.project.tracks[0].level.get(), 61);
        let crate::model::Instrument::Kick(kick) = &e.project.tracks[0].instrument else {
            panic!("expected kick")
        };
        assert_eq!(kick.tune.get(), 72);
        assert_eq!(kick.decay.get(), 34);

        for (parameter, value) in [
            (ParameterId::Cutoff, 41),
            (ParameterId::Resonance, 22),
            (ParameterId::FilterEnvelope, 63),
            (ParameterId::Attack, 4),
            (ParameterId::Decay, 31),
            (ParameterId::Sustain, 74),
            (ParameterId::Release, 18),
        ] {
            e.set_parameter(4, 0, Scope::Base, parameter, p(value), None)
                .unwrap();
        }
        e.set_parameter(4, 0, Scope::Base, ParameterId::OscillatorMix, p(82), None)
            .unwrap();
        let crate::model::Instrument::Chord(synth) = &e.project.tracks[4].instrument else {
            panic!("expected synth")
        };
        assert_eq!(synth.oscillator_mix.get(), 82);
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
        let mut e = Editor::new(Project::new());
        let p = |value| ParameterValue::Percent(Percent::new(value).unwrap());
        e.toggle_event(0, 0).unwrap();
        assert_eq!(
            e.parameter_value(0, 0, Scope::Lock, ParameterId::Level),
            Ok(p(80))
        );
        e.set_parameter(0, 0, Scope::Lock, ParameterId::Level, p(20), None)
            .unwrap();
        e.set_parameter(0, 0, Scope::Lock, ParameterId::Tune, p(90), None)
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
                .tune
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
                .tune
                .is_some()
        );
    }

    #[test]
    fn incompatible_and_empty_locks_are_rejected() {
        let mut e = Editor::new(Project::new());
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

    #[test]
    fn resize_cleans_wrapped_ties_and_undo_restores_them() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(3, 4, None).unwrap();
        e.set_note(3, 3, 1).unwrap();
        e.toggle_tie(3, 0).unwrap();
        e.set_track_length(3, 3, None).unwrap();
        assert_eq!(e.project.tracks[3].steps.len(), 3);
        assert!(e.project.tracks[3].steps[0].is_none());
        assert!(e.undo());
        assert_eq!(e.project.tracks[3].steps.len(), 4);
        assert!(matches!(
            e.project.tracks[3].steps[0],
            Some(StepEvent::Tie { .. })
        ));
        assert!(matches!(
            e.project.tracks[3].steps[3],
            Some(StepEvent::BassNote { .. })
        ));
    }

    #[test]
    fn duplicate_track_copies_events_locks_and_is_one_undo_step() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(3, 4, None).unwrap();
        e.set_note(3, 3, 2).unwrap();
        e.set_parameter(
            3,
            3,
            Scope::Lock,
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(42).unwrap()),
            None,
        )
        .unwrap();
        e.toggle_tie(3, 0).unwrap();
        let original = e.project.tracks[3].steps.clone();
        e.duplicate_track(3).unwrap();
        assert_eq!(e.project.tracks[3].steps.len(), 8);
        assert_eq!(&e.project.tracks[3].steps[..4], original.as_slice());
        assert_eq!(&e.project.tracks[3].steps[4..], original.as_slice());
        assert!(e.undo());
        assert_eq!(e.project.tracks[3].steps, original);
    }

    #[test]
    fn duplicate_rejects_lengths_above_32_without_state_change() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(0, 33, None).unwrap();
        e.mark_saved();
        assert_eq!(e.duplicate_track(0), Err(EditError::CannotDouble));
        assert_eq!(e.project.tracks[0].steps.len(), 33);
        assert!(!e.is_dirty());
    }

    #[test]
    fn lfo_assignment_is_validated_and_undoable() {
        let mut editor = Editor::new(Project::new());
        let config = LfoConfig::default();
        editor
            .set_lfo(3, ParameterId::Cutoff, Some(config), None)
            .unwrap();
        assert_eq!(editor.lfo(3, ParameterId::Cutoff), Ok(Some(config)));
        assert!(editor.is_dirty());
        assert!(editor.undo());
        assert_eq!(editor.lfo(3, ParameterId::Cutoff), Ok(None));
        assert_eq!(
            editor.set_lfo(3, ParameterId::Waveform, Some(config), None),
            Err(EditError::InvalidParameter)
        );
        assert_eq!(
            editor.set_lfo(0, ParameterId::Cutoff, Some(config), None),
            Err(EditError::InvalidParameter)
        );
    }

    #[test]
    fn chord_shape_edits_selected_notes_and_empty_step_defaults() {
        let mut editor = Editor::new(Project::new());
        editor
            .set_chord_shape(4, 0, ChordShape::SeventhRoot)
            .unwrap();
        assert_eq!(
            editor.project.tracks[4].input_chord_shape,
            Some(ChordShape::SeventhRoot)
        );
        editor.set_note(4, 0, 1).unwrap();
        assert!(matches!(
            editor.project.tracks[4].steps[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::SeventhRoot),
                ..
            })
        ));
        editor
            .set_chord_shape(4, 0, ChordShape::Sus4FirstInversion)
            .unwrap();
        assert!(matches!(
            editor.project.tracks[4].steps[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::Sus4FirstInversion),
                ..
            })
        ));
        assert!(editor.undo());
        assert!(matches!(
            editor.project.tracks[4].steps[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::SeventhRoot),
                ..
            })
        ));
    }
}
