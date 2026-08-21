use crate::generator::{self, Config as GeneratorConfig, Target as GeneratorTarget};
use crate::model::{
    ArpeggioConfig, ArpeggioRate, ArpeggioType, ChordShape, DrumRecipeSlot, LfoConfig,
    MAX_SONG_ENTRY_COUNT, MAX_STEP_COUNT, MIN_STEP_COUNT, Microtiming, ParameterId, ParameterLocks,
    ParameterValue, Pattern, PatternIndexMap, Percent, Project, SongEntry, SongIndexMap, Step,
    StepEvent, TrackKind, TriggerCondition, tie_source,
};
use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
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
    NoTriggerSettings,
    InvalidDrumRecipe,
    EmptyStepClipboard,
    IncompatibleStepClipboard,
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
                Self::NoSlide => "slide requires a Bass or Lead note",
                Self::NoChordShape => {
                    "voicing requires a Chord/FM note or empty Chord/FM step"
                }
                Self::NoTriggerSettings => "trigger settings require a trigger or note",
                Self::InvalidDrumRecipe => "recipe is incompatible with this drum track",
                Self::EmptyStepClipboard => "step clipboard is empty",
                Self::IncompatibleStepClipboard => {
                    "copied step requires the same destination track kind"
                }
            }
        )
    }
}

#[derive(Clone)]
struct Revision {
    delta: ProjectDelta,
    before_state_id: u64,
    after_state_id: u64,
    pattern: usize,
    coalesce: Option<CoalesceKey>,
    at: Instant,
    pattern_map: PatternIndexMap,
    inverse_pattern_map: PatternIndexMap,
    song_map: SongIndexMap,
    inverse_song_map: SongIndexMap,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditImpact {
    pub(crate) globals: bool,
    pub(crate) tracks: u16,
    pub(crate) sequences: Vec<(usize, usize)>,
    pub(crate) patterns_structural: bool,
    pub(crate) song: bool,
}

impl EditImpact {
    fn full() -> Self {
        Self {
            globals: true,
            tracks: (1_u16 << crate::model::TRACK_COUNT) - 1,
            sequences: Vec::new(),
            patterns_structural: true,
            song: true,
        }
    }
    fn from_delta(delta: &ProjectDelta) -> Self {
        Self {
            globals: delta.globals.is_some(),
            tracks: delta
                .tracks
                .iter()
                .fold(0, |mask, (track, _)| mask | (1_u16 << track)),
            sequences: delta
                .sequences
                .iter()
                .map(|(pattern, track, _)| (*pattern, *track))
                .collect(),
            patterns_structural: delta.patterns.is_some(),
            song: delta.song.is_some(),
        }
    }

    fn merge(&mut self, newer: Self) {
        self.globals |= newer.globals;
        self.tracks |= newer.tracks;
        self.patterns_structural |= newer.patterns_structural;
        self.song |= newer.song;
        if self.patterns_structural {
            self.sequences.clear();
        } else {
            for sequence in newer.sequences {
                if !self.sequences.contains(&sequence) {
                    self.sequences.push(sequence);
                }
            }
        }
    }

    pub(crate) fn track_changed(&self, track: usize) -> bool {
        self.tracks & (1_u16 << track) != 0
    }
}

#[derive(Clone, Default)]
struct ProjectDelta {
    globals: Option<crate::model::Globals>,
    tracks: Vec<(usize, crate::model::Track)>,
    sequences: Vec<(usize, usize, Vec<Step>)>,
    patterns: Option<Vec<Pattern>>,
    song: Option<Vec<crate::model::SongEntry>>,
}

impl ProjectDelta {
    fn between(before: Project, after: &Project) -> Self {
        let globals = (before.globals != after.globals).then_some(before.globals);
        let tracks = before
            .tracks
            .into_iter()
            .enumerate()
            .filter_map(|(index, track)| (track != after.tracks[index]).then_some((index, track)))
            .collect();
        let structural = before.patterns.len() != after.patterns.len()
            || before
                .patterns
                .iter()
                .zip(&after.patterns)
                .any(|(left, right)| left.tracks.len() != right.tracks.len());
        let (patterns, sequences) = if structural {
            (Some(before.patterns), Vec::new())
        } else {
            let mut sequences = Vec::new();
            for (pattern, (left, right)) in
                before.patterns.into_iter().zip(&after.patterns).enumerate()
            {
                for (track, (left, right)) in left.tracks.into_iter().zip(&right.tracks).enumerate()
                {
                    if left.steps != right.steps {
                        sequences.push((pattern, track, left.steps));
                    }
                }
            }
            (None, sequences)
        };
        let song = (before.song != after.song).then_some(before.song);
        Self {
            globals,
            tracks,
            sequences,
            patterns,
            song,
        }
    }

    fn swap(&mut self, project: &mut Project) {
        if let Some(globals) = &mut self.globals {
            std::mem::swap(globals, &mut project.globals);
        }
        for (index, track) in &mut self.tracks {
            std::mem::swap(track, &mut project.tracks[*index]);
        }
        if let Some(patterns) = &mut self.patterns {
            std::mem::swap(patterns, &mut project.patterns);
        } else {
            for (pattern, track, steps) in &mut self.sequences {
                std::mem::swap(steps, &mut project.patterns[*pattern].tracks[*track].steps);
            }
        }
        if let Some(song) = &mut self.song {
            std::mem::swap(song, &mut project.song);
        }
    }

    fn prune_unchanged(&mut self, project: &Project) {
        if self
            .globals
            .is_some_and(|globals| globals == project.globals)
        {
            self.globals = None;
        }
        self.tracks
            .retain(|(index, track)| *track != project.tracks[*index]);
        if let Some(patterns) = &self.patterns {
            if *patterns == project.patterns {
                self.patterns = None;
            }
        } else {
            self.sequences.retain(|(pattern, track, steps)| {
                *steps != project.patterns[*pattern].tracks[*track].steps
            });
        }
        if self.song.as_ref().is_some_and(|song| *song == project.song) {
            self.song = None;
        }
    }

    fn is_empty(&self) -> bool {
        self.globals.is_none()
            && self.tracks.is_empty()
            && self.sequences.is_empty()
            && self.patterns.is_none()
            && self.song.is_none()
    }

    /// Extend a coalesced revision with regions touched by a newer edit while
    /// retaining the oldest value for regions already represented.
    fn merge_earliest(&mut self, newer: Self) {
        if self.globals.is_none() {
            self.globals = newer.globals;
        }
        for (index, track) in newer.tracks {
            if !self.tracks.iter().any(|(existing, _)| *existing == index) {
                self.tracks.push((index, track));
            }
        }

        match (&mut self.patterns, newer.patterns) {
            (Some(_), _) => {}
            (slot @ None, Some(mut patterns)) => {
                for (pattern, track, steps) in self.sequences.drain(..) {
                    patterns[pattern].tracks[track].steps = steps;
                }
                *slot = Some(patterns);
            }
            (None, None) => {
                for (pattern, track, steps) in newer.sequences {
                    if !self
                        .sequences
                        .iter()
                        .any(|(existing_pattern, existing_track, _)| {
                            *existing_pattern == pattern && *existing_track == track
                        })
                    {
                        self.sequences.push((pattern, track, steps));
                    }
                }
            }
        }

        if self.song.is_none() {
            self.song = newer.song;
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceKey(pub usize, pub usize, pub u8);

pub struct Editor {
    pub(crate) project: Project,
    pattern: usize,
    state_id: u64,
    saved_state_id: u64,
    next_state_id: u64,
    undo: VecDeque<Revision>,
    redo: Vec<Revision>,
    pattern_clipboard: Option<Pattern>,
    song_clipboard: Option<SongEntry>,
    step_clipboard: Option<StepClipboard>,
    pending_pattern_map: PatternIndexMap,
    pending_song_map: SongIndexMap,
    pending_impact: EditImpact,
}

#[derive(Clone, Copy)]
struct StepClipboard {
    kind: TrackKind,
    step: Step,
}

struct ActiveTrack<'a> {
    track: &'a crate::model::Track,
    steps: &'a Vec<Step>,
}

impl Deref for ActiveTrack<'_> {
    type Target = crate::model::Track;
    fn deref(&self) -> &Self::Target {
        self.track
    }
}

struct ActiveTrackMut<'a> {
    track: &'a mut crate::model::Track,
    steps: &'a mut Vec<Step>,
}

impl Deref for ActiveTrackMut<'_> {
    type Target = crate::model::Track;
    fn deref(&self) -> &Self::Target {
        self.track
    }
}

impl DerefMut for ActiveTrackMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.track
    }
}

fn active_track(
    project: &Project,
    pattern: usize,
    track: usize,
) -> Result<ActiveTrack<'_>, EditError> {
    let track_data = project.tracks.get(track).ok_or(EditError::InvalidTrack)?;
    let steps = &project
        .patterns
        .get(pattern)
        .ok_or(EditError::InvalidStep)?
        .tracks
        .get(track)
        .ok_or(EditError::InvalidTrack)?
        .steps;
    Ok(ActiveTrack {
        track: track_data,
        steps,
    })
}

fn active_track_mut(
    project: &mut Project,
    pattern: usize,
    track: usize,
) -> Result<ActiveTrackMut<'_>, EditError> {
    let track_data = project
        .tracks
        .get_mut(track)
        .ok_or(EditError::InvalidTrack)?;
    let steps = &mut project
        .patterns
        .get_mut(pattern)
        .ok_or(EditError::InvalidStep)?
        .tracks
        .get_mut(track)
        .ok_or(EditError::InvalidTrack)?
        .steps;
    Ok(ActiveTrackMut {
        track: track_data,
        steps,
    })
}

fn read_parameter(
    track: &ActiveTrack<'_>,
    step: usize,
    scope: Scope,
    parameter: ParameterId,
) -> Result<ParameterValue, EditError> {
    let base = track
        .parameter(parameter)
        .ok_or(EditError::InvalidParameter)?;
    if scope == Scope::Base {
        return Ok(base);
    }
    track
        .steps
        .get(step)
        .ok_or(EditError::InvalidStep)?
        .as_ref()
        .ok_or(EditError::EmptyLock)?;
    let locks =
        crate::model::effective_event_locks(track.steps, step).ok_or(EditError::EmptyLock)?;
    Ok(track.effective_parameter(parameter, locks).unwrap_or(base))
}

fn write_parameter(
    track: &mut ActiveTrackMut<'_>,
    step: usize,
    scope: Scope,
    parameter: ParameterId,
    value: ParameterValue,
) -> Result<(), EditError> {
    if track.parameter(parameter).is_none() {
        return Err(EditError::InvalidParameter);
    }
    match scope {
        Scope::Base => track
            .set_parameter(parameter, value)
            .then_some(())
            .ok_or(EditError::InvalidParameter),
        Scope::Lock => {
            let event = track
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
}

fn clear_parameter_lock(
    track: &mut ActiveTrackMut<'_>,
    step: usize,
    parameter: ParameterId,
) -> Result<(), EditError> {
    if track.parameter(parameter).is_none() {
        return Err(EditError::InvalidParameter);
    }
    let event = track
        .steps
        .get_mut(step)
        .ok_or(EditError::InvalidStep)?
        .as_mut()
        .ok_or(EditError::EmptyLock)?;
    event.locks_mut().clear(parameter);
    Ok(())
}

impl Editor {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            state_id: 0,
            saved_state_id: 0,
            next_state_id: 1,
            pattern: 0,
            undo: VecDeque::new(),
            redo: Vec::new(),
            pattern_clipboard: None,
            song_clipboard: None,
            step_clipboard: None,
            pending_pattern_map: PatternIndexMap::identity(),
            pending_song_map: SongIndexMap::identity(),
            pending_impact: EditImpact::default(),
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.state_id != self.saved_state_id
    }
    pub fn mark_saved(&mut self) {
        self.saved_state_id = self.state_id;
        self.end_coalescing();
    }
    pub fn end_coalescing(&mut self) {
        if let Some(revision) = self.undo.back_mut() {
            revision.coalesce = None;
        }
    }
    pub fn replace_loaded(&mut self, p: Project) {
        self.project = p;
        self.state_id = 0;
        self.saved_state_id = 0;
        self.next_state_id = 1;
        self.undo.clear();
        self.redo.clear();
        self.pattern = 0;
        self.pattern_clipboard = None;
        self.song_clipboard = None;
        self.step_clipboard = None;
        self.pending_pattern_map = PatternIndexMap::identity();
        self.pending_song_map = SongIndexMap::identity();
        self.pending_impact = EditImpact::full();
    }
    pub fn pattern(&self) -> usize {
        self.pattern
    }
    pub fn take_pattern_map(&mut self) -> PatternIndexMap {
        std::mem::replace(&mut self.pending_pattern_map, PatternIndexMap::identity())
    }
    pub fn take_song_map(&mut self) -> SongIndexMap {
        std::mem::replace(&mut self.pending_song_map, SongIndexMap::identity())
    }
    pub(crate) fn take_edit_impact(&mut self) -> EditImpact {
        std::mem::take(&mut self.pending_impact)
    }
    pub(crate) fn discard_pending_sync(&mut self) {
        self.pending_pattern_map = PatternIndexMap::identity();
        self.pending_song_map = SongIndexMap::identity();
        self.pending_impact = EditImpact::default();
    }
    pub fn select_pattern(&mut self, pattern: usize) -> bool {
        if pattern >= self.project.patterns.len() {
            return false;
        }
        self.pattern = pattern;
        true
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Read-only access to the active editor workspace.
    pub fn active_steps(&self, track: usize) -> Option<&[crate::model::Step]> {
        self.project.pattern_steps(self.pattern, track)
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
    fn pattern_structure_edit<F>(&mut self, cursor: usize, f: F) -> Result<(bool, usize), EditError>
    where
        F: FnOnce(&mut Project, usize, usize) -> Result<(usize, usize), EditError>,
    {
        if cursor >= self.project.patterns.len() {
            return Ok((false, self.pattern));
        }
        let before_patterns = self.project.patterns.clone();
        let before_song = self.project.song.clone();
        let before_count = before_patterns.len();
        let before_pattern = self.pattern;
        let (next_cursor, next_active) = match f(&mut self.project, cursor, before_pattern) {
            Ok(result) => result,
            Err(error) => {
                self.project.patterns = before_patterns;
                self.project.song = before_song;
                return Err(error);
            }
        };
        self.pattern = next_active;
        let mut delta = ProjectDelta {
            patterns: Some(before_patterns),
            song: Some(before_song),
            ..ProjectDelta::default()
        };
        delta.prune_unchanged(&self.project);
        if delta.is_empty() {
            return Ok((false, next_cursor));
        }
        self.push_revision(delta, before_pattern, None);
        if self.project.patterns.len() == before_count + 1 {
            self.record_pattern_map(
                PatternIndexMap::insert(cursor),
                PatternIndexMap::delete(cursor + 1),
            );
        } else if self.project.patterns.len() + 1 == before_count {
            self.record_pattern_map(
                PatternIndexMap::delete(cursor),
                PatternIndexMap::insert_at(cursor),
            );
        }
        Ok((true, next_cursor))
    }
    fn push_revision(
        &mut self,
        delta: ProjectDelta,
        before_pattern: usize,
        coalesce: Option<CoalesceKey>,
    ) {
        self.pending_impact.merge(EditImpact::from_delta(&delta));
        let before_state_id = self.state_id;
        let after_state_id = self.next_state_id;
        self.next_state_id = self.next_state_id.wrapping_add(1);
        self.state_id = after_state_id;
        if self.undo.len() == 256 {
            self.undo.pop_front();
        }
        self.undo.push_back(Revision {
            delta,
            before_state_id,
            after_state_id,
            pattern: before_pattern,
            coalesce,
            at: Instant::now(),
            pattern_map: PatternIndexMap::identity(),
            inverse_pattern_map: PatternIndexMap::identity(),
            song_map: SongIndexMap::identity(),
            inverse_song_map: SongIndexMap::identity(),
        });
        self.redo.clear();
    }
    fn record_pattern_map(&mut self, map: PatternIndexMap, inverse: PatternIndexMap) {
        self.pending_pattern_map = map;
        if let Some(revision) = self.undo.back_mut() {
            revision.pattern_map = map;
            revision.inverse_pattern_map = inverse;
        }
    }
    fn record_song_map(&mut self, map: SongIndexMap, inverse: SongIndexMap) {
        self.pending_song_map = map;
        if let Some(revision) = self.undo.back_mut() {
            revision.song_map = map;
            revision.inverse_song_map = inverse;
        }
    }
    pub fn insert_pattern(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit(cursor, |p, cursor, active| {
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
    pub fn duplicate_pattern(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit(cursor, |p, cursor, active| {
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
    pub fn copy_pattern(&mut self, cursor: usize) -> bool {
        let Some(pattern) = self.project.patterns.get(cursor).cloned() else {
            return false;
        };
        self.pattern_clipboard = Some(pattern);
        true
    }
    pub fn cut_pattern(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        if !self.copy_pattern(cursor) {
            return Ok((false, cursor));
        }
        self.delete_pattern(cursor)
    }
    pub fn paste_pattern(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        let Some(pattern) = self.pattern_clipboard.clone() else {
            return Ok((false, cursor));
        };
        if self.project.patterns.len() >= crate::model::MAX_PATTERN_COUNT {
            return Ok((false, cursor));
        }
        self.pattern_structure_edit(cursor, |p, cursor, active| {
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

    pub fn copy_step(&mut self, track: usize, step: usize) -> Result<(), EditError> {
        let track = active_track(&self.project, self.pattern, track)?;
        let copied = *track.steps.get(step).ok_or(EditError::InvalidStep)?;
        self.step_clipboard = Some(StepClipboard {
            kind: track.kind,
            step: copied,
        });
        Ok(())
    }

    pub fn cut_step(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.copy_step(track, step)?;
        self.clear(track, step)
    }

    pub fn paste_step(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        let clipboard = self.step_clipboard.ok_or(EditError::EmptyStepClipboard)?;
        self.edit_active_track(track, None, move |project, pattern| {
            let destination = active_track_mut(project, pattern, track)?;
            if destination.kind != clipboard.kind {
                return Err(EditError::IncompatibleStepClipboard);
            }
            if step >= destination.steps.len() {
                return Err(EditError::InvalidStep);
            }

            let mut candidate = destination.steps.clone();
            candidate[step] = clipboard.step;
            if matches!(clipboard.step, Some(StepEvent::Tie { .. }))
                && tie_source(&candidate, step).is_none()
            {
                return Err(EditError::InvalidTie);
            }
            cleanup_invalid_ties(&mut candidate);
            *destination.steps = candidate;
            Ok(())
        })
    }
    pub fn delete_pattern(&mut self, cursor: usize) -> Result<(bool, usize), EditError> {
        self.pattern_structure_edit(cursor, |p, cursor, active| {
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

    pub fn insert_song_entry(&mut self, cursor: usize) -> Result<bool, EditError> {
        if cursor >= self.project.song.len() || self.project.song.len() >= MAX_SONG_ENTRY_COUNT {
            return Ok(false);
        }
        let entry = SongEntry {
            pattern: (self.pattern + 1) as u8,
            bars: 1,
        };
        let changed = self.edit_song(None, |project, _| {
            project.song.insert(cursor + 1, entry);
            Ok(())
        })?;
        if changed {
            self.record_song_map(
                SongIndexMap::insert(cursor),
                SongIndexMap::delete(cursor + 1),
            );
        }
        Ok(changed)
    }

    pub fn duplicate_song_entry(&mut self, cursor: usize) -> Result<bool, EditError> {
        let Some(entry) = self.project.song.get(cursor).copied() else {
            return Ok(false);
        };
        if self.project.song.len() >= MAX_SONG_ENTRY_COUNT {
            return Ok(false);
        }
        let changed = self.edit_song(None, |project, _| {
            project.song.insert(cursor + 1, entry);
            Ok(())
        })?;
        if changed {
            self.record_song_map(
                SongIndexMap::insert(cursor),
                SongIndexMap::delete(cursor + 1),
            );
        }
        Ok(changed)
    }

    pub fn copy_song_entry(&mut self, cursor: usize) -> bool {
        let Some(entry) = self.project.song.get(cursor).copied() else {
            return false;
        };
        self.song_clipboard = Some(entry);
        true
    }

    pub fn paste_song_entry(&mut self, cursor: usize) -> Result<bool, EditError> {
        let Some(entry) = self.song_clipboard else {
            return Ok(false);
        };
        if cursor >= self.project.song.len() || self.project.song.len() >= MAX_SONG_ENTRY_COUNT {
            return Ok(false);
        }
        let changed = self.edit_song(None, |project, _| {
            project.song.insert(cursor + 1, entry);
            Ok(())
        })?;
        if changed {
            self.record_song_map(
                SongIndexMap::insert(cursor),
                SongIndexMap::delete(cursor + 1),
            );
        }
        Ok(changed)
    }

    pub fn delete_song_entry(&mut self, cursor: usize) -> Result<bool, EditError> {
        if cursor >= self.project.song.len() {
            return Ok(false);
        }
        if self.project.song.len() == 1 {
            return self.edit_song(None, |project, _| {
                project.song[0] = SongEntry {
                    pattern: 1,
                    bars: 1,
                };
                Ok(())
            });
        }
        let changed = self.edit_song(None, |project, _| {
            project.song.remove(cursor);
            Ok(())
        })?;
        if changed {
            self.record_song_map(
                SongIndexMap::delete(cursor),
                SongIndexMap::insert_at(cursor),
            );
        }
        Ok(changed)
    }

    pub fn cut_song_entry(&mut self, cursor: usize) -> Result<bool, EditError> {
        if !self.copy_song_entry(cursor) {
            return Ok(false);
        }
        self.delete_song_entry(cursor)
    }

    pub fn change_song_pattern(&mut self, cursor: usize, delta: i8) -> Result<bool, EditError> {
        let count = self.project.patterns.len() as i16;
        self.edit_song(None, |project, _| {
            let entry = project.song.get_mut(cursor).ok_or(EditError::InvalidStep)?;
            entry.pattern = (i16::from(entry.pattern) + i16::from(delta)).clamp(1, count) as u8;
            Ok(())
        })
    }

    pub fn change_song_bars(&mut self, cursor: usize, delta: i8) -> Result<bool, EditError> {
        self.edit_song(None, |project, _| {
            let entry = project.song.get_mut(cursor).ok_or(EditError::InvalidStep)?;
            entry.bars = (i16::from(entry.bars) + i16::from(delta)).clamp(1, 64) as u8;
            Ok(())
        })
    }
    pub fn edit<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let before = self.project.clone();
        if let Err(error) = f(&mut self.project, self.pattern) {
            self.project = before;
            return Err(error);
        }
        if before == self.project {
            return Ok(false);
        }
        let delta = ProjectDelta::between(before, &self.project);
        self.commit_delta(key, delta);
        Ok(true)
    }

    fn edit_delta<F>(
        &mut self,
        key: Option<CoalesceKey>,
        mut delta: ProjectDelta,
        f: F,
    ) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        if let Err(error) = f(&mut self.project, self.pattern) {
            delta.swap(&mut self.project);
            return Err(error);
        }
        delta.prune_unchanged(&self.project);
        if delta.is_empty() {
            return Ok(false);
        }
        self.commit_delta(key, delta);
        Ok(true)
    }

    fn commit_delta(&mut self, key: Option<CoalesceKey>, delta: ProjectDelta) {
        self.pending_impact.merge(EditImpact::from_delta(&delta));
        let now = Instant::now();
        let merge = key.is_some()
            && self.undo.back().is_some_and(|r| {
                r.coalesce == key && now.duration_since(r.at) <= Duration::from_millis(300)
            });
        if merge {
            let r = self.undo.back_mut().unwrap();
            r.delta.merge_earliest(delta);
            r.at = now;
            r.after_state_id = self.next_state_id;
        } else {
            if self.undo.len() == 256 {
                self.undo.pop_front();
            }
            self.undo.push_back(Revision {
                delta,
                before_state_id: self.state_id,
                after_state_id: self.next_state_id,
                pattern: self.pattern,
                coalesce: key,
                at: now,
                pattern_map: PatternIndexMap::identity(),
                inverse_pattern_map: PatternIndexMap::identity(),
                song_map: SongIndexMap::identity(),
                inverse_song_map: SongIndexMap::identity(),
            });
        }
        self.state_id = self.next_state_id;
        self.next_state_id = self.next_state_id.wrapping_add(1);
        self.redo.clear();
    }

    pub fn edit_globals<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let delta = ProjectDelta {
            globals: Some(self.project.globals),
            ..ProjectDelta::default()
        };
        self.edit_delta(key, delta, f)
    }

    pub fn edit_track<F>(
        &mut self,
        track: usize,
        key: Option<CoalesceKey>,
        f: F,
    ) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let before = self
            .project
            .tracks
            .get(track)
            .cloned()
            .ok_or(EditError::InvalidTrack)?;
        let delta = ProjectDelta {
            tracks: vec![(track, before)],
            ..ProjectDelta::default()
        };
        self.edit_delta(key, delta, f)
    }

    /// Replaces one stable track slot with the factory state for another
    /// instrument and clears that slot's sequence in every pattern.
    pub fn assign_instrument(&mut self, track: usize, kind: TrackKind) -> Result<bool, EditError> {
        let before_track = self
            .project
            .tracks
            .get(track)
            .cloned()
            .ok_or(EditError::InvalidTrack)?;
        if before_track.kind == kind {
            return Ok(false);
        }
        let sequences = self
            .project
            .patterns
            .iter()
            .enumerate()
            .map(|(pattern, value)| {
                value
                    .tracks
                    .get(track)
                    .map(|sequence| (pattern, track, sequence.steps.clone()))
                    .ok_or(EditError::InvalidTrack)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let replacement = Project::new().tracks[kind.index()].clone();
        self.edit_delta(
            None,
            ProjectDelta {
                tracks: vec![(track, before_track)],
                sequences,
                ..ProjectDelta::default()
            },
            move |project, _| {
                project.tracks[track] = replacement;
                for pattern in &mut project.patterns {
                    pattern.tracks[track].steps = vec![None; crate::model::STEP_BANK_SIZE];
                }
                Ok(())
            },
        )
    }

    fn edit_active_track<F>(
        &mut self,
        track: usize,
        key: Option<CoalesceKey>,
        f: F,
    ) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let before_track = self
            .project
            .tracks
            .get(track)
            .cloned()
            .ok_or(EditError::InvalidTrack)?;
        let before_steps = self
            .project
            .patterns
            .get(self.pattern)
            .and_then(|pattern| pattern.tracks.get(track))
            .map(|sequence| sequence.steps.clone())
            .ok_or(EditError::InvalidTrack)?;
        let delta = ProjectDelta {
            tracks: vec![(track, before_track)],
            sequences: vec![(self.pattern, track, before_steps)],
            ..ProjectDelta::default()
        };
        self.edit_delta(key, delta, f)
    }

    fn edit_song<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let delta = ProjectDelta {
            song: Some(self.project.song.clone()),
            ..ProjectDelta::default()
        };
        self.edit_delta(key, delta, f)
    }

    fn edit_active_pattern<F>(&mut self, key: Option<CoalesceKey>, f: F) -> Result<bool, EditError>
    where
        F: FnOnce(&mut Project, usize) -> Result<(), EditError>,
    {
        let sequences = self.project.patterns[self.pattern]
            .tracks
            .iter()
            .enumerate()
            .map(|(track, sequence)| (self.pattern, track, sequence.steps.clone()))
            .collect();
        self.edit_delta(
            key,
            ProjectDelta {
                sequences,
                ..ProjectDelta::default()
            },
            f,
        )
    }
    pub fn undo(&mut self) -> bool {
        if let Some(mut r) = self.undo.pop_back() {
            self.pending_impact.merge(EditImpact::from_delta(&r.delta));
            r.delta.swap(&mut self.project);
            self.state_id = r.before_state_id;
            std::mem::swap(&mut r.pattern, &mut self.pattern);
            self.pending_pattern_map = r.inverse_pattern_map;
            self.pending_song_map = r.inverse_song_map;
            std::mem::swap(&mut r.pattern_map, &mut r.inverse_pattern_map);
            std::mem::swap(&mut r.song_map, &mut r.inverse_song_map);
            self.redo.push(r);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(mut r) = self.redo.pop() {
            self.pending_impact.merge(EditImpact::from_delta(&r.delta));
            r.delta.swap(&mut self.project);
            self.state_id = r.after_state_id;
            std::mem::swap(&mut r.pattern, &mut self.pattern);
            self.pending_pattern_map = r.inverse_pattern_map;
            self.pending_song_map = r.inverse_song_map;
            std::mem::swap(&mut r.pattern_map, &mut r.inverse_pattern_map);
            std::mem::swap(&mut r.song_map, &mut r.inverse_song_map);
            self.undo.push_back(r);
            true
        } else {
            false
        }
    }

    /// Fill empty cells in the active pattern as one undoable revision.
    pub fn generate_pattern(&mut self, config: GeneratorConfig) -> Result<usize, EditError> {
        let generated = generator::generate_for_pattern(&self.project, self.pattern, config);
        if generated.inserted == 0 {
            return Ok(0);
        }
        self.edit_active_pattern(None, |project, pattern| {
            for (index, steps) in generated.tracks.iter().enumerate() {
                if matches!(config.target, GeneratorTarget::Track(track) if track != index) {
                    continue;
                }
                project.patterns[pattern].tracks[index]
                    .steps
                    .clone_from(steps);
            }
            Ok(())
        })?;
        Ok(generated.inserted)
    }
    pub fn toggle_event(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            if t.steps[step].is_some() {
                clear_with_ties(t.steps, step);
                return Ok(());
            }
            t.steps[step] = Some(if t.kind == TrackKind::Bass {
                StepEvent::BassNote {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: t.input_accent,
                    slide: false,
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: Default::default(),
                }
            } else if t.kind.supports_voicing() {
                StepEvent::Note {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: t.input_accent,
                    chord_shape: t.input_chord_shape,
                    arpeggio: t.input_chord_arpeggio.unwrap_or_default(),
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: Default::default(),
                }
            } else if t.kind == TrackKind::Lead {
                StepEvent::LeadNote {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: t.input_accent,
                    slide: false,
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: Default::default(),
                }
            } else if t.kind == TrackKind::Fm {
                StepEvent::Note {
                    degree: t.input_degree.unwrap(),
                    octave: t.input_octave.unwrap(),
                    accent: t.input_accent,
                    chord_shape: None,
                    arpeggio: ArpeggioConfig::default(),
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: Default::default(),
                }
            } else {
                StepEvent::Trigger {
                    accent: t.input_accent,
                    recipe: crate::model::DrumRecipeSlot::ONE,
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: Default::default(),
                }
            });
            Ok(())
        })
    }

    pub fn drum_recipe_value(
        &self,
        track: usize,
        step: usize,
    ) -> Result<DrumRecipeSlot, EditError> {
        let track = active_track(&self.project, self.pattern, track)?;
        track
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .and_then(StepEvent::drum_recipe)
            .ok_or(EditError::InvalidDrumRecipe)
    }

    pub fn set_drum_recipe(
        &mut self,
        track: usize,
        step: usize,
        recipe: DrumRecipeSlot,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let track = active_track_mut(project, pattern, track)?;
            if !matches!(track.kind, TrackKind::Hat | TrackKind::Tom)
                || recipe.get() > track.drum_recipe_count()
            {
                return Err(EditError::InvalidDrumRecipe);
            }
            let input_accent = track.input_accent;
            let kind = track.kind;
            let event = track.steps.get_mut(step).ok_or(EditError::InvalidStep)?;
            if event.is_none() {
                *event = Some(StepEvent::Trigger {
                    accent: input_accent,
                    recipe,
                    condition: TriggerCondition::Always,
                    retrigger_count: 1,
                    microtiming: crate::model::Microtiming::ZERO,
                    locks: ParameterLocks::default(),
                });
                return Ok(());
            }
            let trigger = event.as_mut().ok_or(EditError::InvalidDrumRecipe)?;
            *trigger
                .drum_recipe_mut()
                .ok_or(EditError::InvalidDrumRecipe)? = recipe;
            clear_drum_sound_locks(trigger.locks_mut(), kind);
            Ok(())
        })
    }

    pub fn clear_drum_recipe_overrides(
        &mut self,
        track: usize,
        step: usize,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let track = active_track_mut(project, pattern, track)?;
            if !matches!(track.kind, TrackKind::Hat | TrackKind::Tom) {
                return Err(EditError::InvalidDrumRecipe);
            }
            let kind = track.kind;
            let trigger = track
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::InvalidDrumRecipe)?;
            if trigger.drum_recipe().is_none() {
                return Err(EditError::InvalidDrumRecipe);
            }
            clear_drum_sound_locks(trigger.locks_mut(), kind);
            Ok(())
        })
    }
    pub fn set_note(&mut self, track: usize, step: usize, degree: u8) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let mut t = active_track_mut(p, pattern, track)?;
            if !matches!(
                t.kind,
                TrackKind::Bass | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
            ) {
                return Err(EditError::NotSynth);
            }
            if step >= t.steps.len() || !(1..=8).contains(&degree) {
                return Err(EditError::InvalidStep);
            }
            let (
                locks,
                accent,
                slide,
                chord_shape,
                arpeggio,
                condition,
                retrigger_count,
                microtiming,
                existing_note,
            ) = match t.steps[step].take() {
                Some(StepEvent::BassNote {
                    accent,
                    slide,
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                    ..
                }) => (
                    locks,
                    accent,
                    slide,
                    None,
                    ArpeggioConfig::default(),
                    condition,
                    retrigger_count,
                    microtiming,
                    false,
                ),
                Some(StepEvent::LeadNote {
                    accent,
                    slide,
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                    ..
                }) => (
                    locks,
                    accent,
                    slide,
                    None,
                    ArpeggioConfig::default(),
                    condition,
                    retrigger_count,
                    microtiming,
                    true,
                ),
                Some(StepEvent::Note {
                    accent,
                    chord_shape,
                    arpeggio,
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                    ..
                }) => (
                    locks,
                    accent,
                    false,
                    chord_shape,
                    arpeggio,
                    condition,
                    retrigger_count,
                    microtiming,
                    true,
                ),
                Some(event) => (
                    *event.locks(),
                    t.input_accent,
                    false,
                    None,
                    ArpeggioConfig::default(),
                    TriggerCondition::Always,
                    1,
                    crate::model::Microtiming::ZERO,
                    false,
                ),
                None => (
                    Default::default(),
                    t.input_accent,
                    false,
                    None,
                    ArpeggioConfig::default(),
                    TriggerCondition::Always,
                    1,
                    crate::model::Microtiming::ZERO,
                    false,
                ),
            };
            let octave = t.input_octave.unwrap();
            t.input_degree = Some(degree);
            t.steps[step] = Some(if t.kind == TrackKind::Bass {
                StepEvent::BassNote {
                    degree,
                    octave,
                    accent,
                    slide,
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                }
            } else if t.kind == TrackKind::Lead {
                StepEvent::LeadNote {
                    degree,
                    octave,
                    accent,
                    slide,
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                }
            } else {
                StepEvent::Note {
                    degree,
                    octave,
                    accent,
                    chord_shape: if t.kind.supports_voicing() {
                        if existing_note {
                            chord_shape
                        } else {
                            t.input_chord_shape
                        }
                    } else {
                        None
                    },
                    arpeggio: if t.kind.supports_voicing() {
                        if existing_note {
                            arpeggio
                        } else {
                            t.input_chord_arpeggio.unwrap_or_default()
                        }
                    } else {
                        ArpeggioConfig::default()
                    },
                    condition,
                    retrigger_count,
                    microtiming,
                    locks,
                }
            });
            cleanup_invalid_ties(t.steps);
            Ok(())
        })
    }

    pub fn set_chord_shape(
        &mut self,
        track: usize,
        step: usize,
        shape: ChordShape,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let mut t = active_track_mut(project, pattern, track)?;
            if !t.kind.supports_voicing() {
                return Err(EditError::NoChordShape);
            }
            let shape = t.canonical_voicing_shape(shape);
            match t.steps.get_mut(step).ok_or(EditError::InvalidStep)? {
                Some(StepEvent::Note { chord_shape, .. }) => {
                    *chord_shape = shape;
                }
                None => t.input_chord_shape = shape,
                Some(StepEvent::Tie { .. }) | Some(_) => return Err(EditError::NoChordShape),
            }
            Ok(())
        })
    }

    pub fn chord_shape_value(&self, track: usize, step: usize) -> Result<ChordShape, EditError> {
        let t = active_track(&self.project, self.pattern, track)?;
        if !t.kind.supports_voicing() {
            return Err(EditError::NoChordShape);
        }
        let default_shape = t.default_voicing_shape().ok_or(EditError::NoChordShape)?;
        let base = match t.steps.get(step).ok_or(EditError::InvalidStep)?.as_ref() {
            Some(StepEvent::Note { chord_shape, .. }) => chord_shape.unwrap_or(default_shape),
            Some(StepEvent::Tie { .. }) => {
                let source = tie_source(t.steps, step).ok_or(EditError::InvalidTie)?;
                match t.steps[source] {
                    Some(StepEvent::Note { chord_shape, .. }) => {
                        chord_shape.unwrap_or(default_shape)
                    }
                    _ => return Err(EditError::NoChordShape),
                }
            }
            None => t.input_voicing_shape().ok_or(EditError::NoChordShape)?,
            Some(_) => return Err(EditError::NoChordShape),
        };
        Ok(base)
    }

    pub fn arpeggio_config_value(
        &self,
        track: usize,
        step: usize,
    ) -> Result<ArpeggioConfig, EditError> {
        let t = active_track(&self.project, self.pattern, track)?;
        if !t.kind.supports_voicing() {
            return Err(EditError::InvalidParameter);
        }
        match t.steps.get(step).ok_or(EditError::InvalidStep)?.as_ref() {
            Some(StepEvent::Note { arpeggio, .. }) => Ok(*arpeggio),
            Some(StepEvent::Tie { .. }) => {
                let source = tie_source(t.steps, step).ok_or(EditError::InvalidTie)?;
                match t.steps[source] {
                    Some(StepEvent::Note { arpeggio, .. }) => Ok(arpeggio),
                    _ => Err(EditError::InvalidParameter),
                }
            }
            None => Ok(t.input_chord_arpeggio.unwrap_or_default()),
            Some(_) => Err(EditError::InvalidParameter),
        }
    }

    pub fn set_arpeggio_enabled(
        &mut self,
        track: usize,
        step: usize,
        value: bool,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let mut t = active_track_mut(project, pattern, track)?;
            if !t.kind.supports_voicing() {
                return Err(EditError::InvalidParameter);
            }
            match t.steps.get_mut(step).ok_or(EditError::InvalidStep)? {
                Some(StepEvent::Note { arpeggio, .. }) => arpeggio.enabled = value,
                None => {
                    let mut config = t.input_chord_arpeggio.unwrap_or_default();
                    config.enabled = value;
                    t.input_chord_arpeggio = (!config.is_default()).then_some(config);
                }
                Some(StepEvent::Tie { .. }) | Some(_) => return Err(EditError::NoChordShape),
            }
            Ok(())
        })
    }

    pub fn set_arpeggio_type(
        &mut self,
        track: usize,
        step: usize,
        value: ArpeggioType,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let mut t = active_track_mut(project, pattern, track)?;
            if !t.kind.supports_voicing() {
                return Err(EditError::InvalidParameter);
            }
            match t.steps.get_mut(step).ok_or(EditError::InvalidStep)? {
                Some(StepEvent::Note { arpeggio, .. }) => arpeggio.r#type = value,
                None => {
                    let mut config = t.input_chord_arpeggio.unwrap_or_default();
                    config.r#type = value;
                    t.input_chord_arpeggio = (!config.is_default()).then_some(config);
                }
                Some(StepEvent::Tie { .. }) | Some(_) => return Err(EditError::NoChordShape),
            }
            Ok(())
        })
    }

    pub fn set_arpeggio_rate(
        &mut self,
        track: usize,
        step: usize,
        value: ArpeggioRate,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let mut t = active_track_mut(project, pattern, track)?;
            if !t.kind.supports_voicing() {
                return Err(EditError::InvalidParameter);
            }
            match t.steps.get_mut(step).ok_or(EditError::InvalidStep)? {
                Some(StepEvent::Note { arpeggio, .. }) => arpeggio.rate = value,
                None => {
                    let mut config = t.input_chord_arpeggio.unwrap_or_default();
                    config.rate = value;
                    t.input_chord_arpeggio = (!config.is_default()).then_some(config);
                }
                Some(StepEvent::Tie { .. }) | Some(_) => return Err(EditError::NoChordShape),
            }
            Ok(())
        })
    }
    pub fn toggle_tie(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            if !matches!(
                t.kind,
                TrackKind::Bass | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
            ) {
                return Err(EditError::NotSynth);
            }
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            match t.steps[step].take() {
                Some(StepEvent::Tie { .. }) => {
                    cleanup_invalid_ties(t.steps);
                    Ok(())
                }
                old => {
                    let locks = old.as_ref().map(|x| *x.locks()).unwrap_or_default();
                    t.steps[step] = Some(StepEvent::Tie { locks });
                    if tie_source(t.steps, step).is_none() {
                        t.steps[step] = old;
                        return Err(EditError::InvalidTie);
                    }
                    Ok(())
                }
            }
        })
    }
    pub fn clear(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            if step >= t.steps.len() {
                return Err(EditError::InvalidStep);
            }
            clear_with_ties(t.steps, step);
            Ok(())
        })
    }

    /// Clear every event and lock from one track in the active pattern.
    ///
    /// The returned count is the number of populated steps removed. The
    /// whole operation is recorded as one undoable edit.
    pub fn clear_track(&mut self, track: usize) -> Result<usize, EditError> {
        let mut cleared = 0;
        self.edit_active_track(track, None, |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            cleared = t.steps.iter().filter(|step| step.is_some()).count();
            t.steps.fill(None);
            Ok(())
        })?;
        Ok(cleared)
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
        self.edit_active_track(track, key, move |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            t.steps.resize(length, None);
            cleanup_invalid_ties(t.steps);
            Ok(())
        })
    }

    pub fn duplicate_track(&mut self, track: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let t = active_track_mut(p, pattern, track)?;
            if t.steps.len() > MAX_STEP_COUNT / 2 {
                return Err(EditError::CannotDouble);
            }
            let copy = t.steps.clone();
            t.steps.extend(copy);
            Ok(())
        })
    }
    pub fn accent_value(&self, track: usize, step: usize) -> Result<bool, EditError> {
        let t = active_track(&self.project, self.pattern, track)?;
        match t.steps.get(step).ok_or(EditError::InvalidStep)?.as_ref() {
            Some(event) => event.accent().ok_or(EditError::NoAccent),
            None => Ok(t.input_accent),
        }
    }

    pub fn toggle_accent(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let mut t = active_track_mut(project, pattern, track)?;
            match t.steps.get_mut(step).ok_or(EditError::InvalidStep)? {
                Some(event) => {
                    let accent = event.accent_mut().ok_or(EditError::NoAccent)?;
                    *accent = !*accent;
                }
                None => t.input_accent = !t.input_accent,
            }
            Ok(())
        })
    }

    pub fn toggle_slide(&mut self, track: usize, step: usize) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |project, pattern| {
            let t = active_track_mut(project, pattern, track)?;
            if !matches!(t.kind, TrackKind::Bass | TrackKind::Lead) {
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

    pub fn trigger_condition_value(
        &self,
        track: usize,
        step: usize,
    ) -> Result<TriggerCondition, EditError> {
        active_track(&self.project, self.pattern, track)?
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .and_then(StepEvent::condition)
            .ok_or(EditError::NoTriggerSettings)
    }

    pub fn retrigger_count_value(&self, track: usize, step: usize) -> Result<u8, EditError> {
        active_track(&self.project, self.pattern, track)?
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .and_then(StepEvent::retrigger_count)
            .ok_or(EditError::NoTriggerSettings)
    }

    pub fn set_trigger_condition(
        &mut self,
        track: usize,
        step: usize,
        condition: TriggerCondition,
    ) -> Result<bool, EditError> {
        if !condition.valid() {
            return Err(EditError::NoTriggerSettings);
        }
        self.edit_active_track(track, None, move |project, pattern| {
            let event = active_track_mut(project, pattern, track)?
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::NoTriggerSettings)?;
            *event.condition_mut().ok_or(EditError::NoTriggerSettings)? = condition;
            Ok(())
        })
    }

    pub fn set_retrigger_count(
        &mut self,
        track: usize,
        step: usize,
        count: u8,
    ) -> Result<bool, EditError> {
        if !(1..=4).contains(&count) {
            return Err(EditError::NoTriggerSettings);
        }
        self.edit_active_track(track, None, move |project, pattern| {
            let event = active_track_mut(project, pattern, track)?
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::NoTriggerSettings)?;
            *event
                .retrigger_count_mut()
                .ok_or(EditError::NoTriggerSettings)? = count;
            Ok(())
        })
    }

    pub fn microtiming_value(&self, track: usize, step: usize) -> Result<Microtiming, EditError> {
        active_track(&self.project, self.pattern, track)?
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .and_then(StepEvent::microtiming)
            .ok_or(EditError::NoTriggerSettings)
    }

    pub fn set_microtiming(
        &mut self,
        track: usize,
        step: usize,
        value: Microtiming,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, key, move |project, pattern| {
            let event = active_track_mut(project, pattern, track)?
                .steps
                .get_mut(step)
                .ok_or(EditError::InvalidStep)?
                .as_mut()
                .ok_or(EditError::NoTriggerSettings)?;
            *event
                .microtiming_mut()
                .ok_or(EditError::NoTriggerSettings)? = value;
            Ok(())
        })
    }

    pub fn set_track_swing(
        &mut self,
        track: usize,
        value: Percent,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        if value.get() > 75 {
            return Err(EditError::InvalidParameter);
        }
        self.edit_track(track, key, move |project, _pattern| {
            project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?
                .swing = value;
            Ok(())
        })
    }

    pub fn set_track_probability(
        &mut self,
        track: usize,
        value: Percent,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.edit_track(track, key, move |project, _pattern| {
            project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?
                .probability = value;
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
        self.edit_active_track(track, key, move |p, pattern| {
            let mut t = active_track_mut(p, pattern, track)?;
            write_parameter(&mut t, step, scope, parameter, value)
        })
    }

    pub fn set_drum_recipe_parameter(
        &mut self,
        track: usize,
        step: usize,
        scope: Scope,
        recipe_parameter: (DrumRecipeSlot, ParameterId),
        value: ParameterValue,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        let (recipe, parameter) = recipe_parameter;
        self.edit_active_track(track, key, move |project, pattern| {
            let mut track = active_track_mut(project, pattern, track)?;
            if recipe.get() > track.drum_recipe_count()
                || !matches!(
                    parameter,
                    ParameterId::Tune | ParameterId::Tone | ParameterId::Decay
                )
            {
                return Err(EditError::InvalidDrumRecipe);
            }
            match scope {
                Scope::Base => track
                    .set_drum_recipe_parameter(recipe, parameter, value)
                    .then_some(())
                    .ok_or(EditError::InvalidParameter),
                Scope::Lock => {
                    let event = track
                        .steps
                        .get_mut(step)
                        .ok_or(EditError::InvalidStep)?
                        .as_mut()
                        .ok_or(EditError::EmptyLock)?;
                    if event.drum_recipe() != Some(recipe) {
                        return Err(EditError::InvalidDrumRecipe);
                    }
                    event
                        .locks_mut()
                        .set(parameter, value)
                        .then_some(())
                        .ok_or(EditError::InvalidParameter)
                }
            }
        })
    }

    pub fn drum_recipe_parameter_value(
        &self,
        track: usize,
        step: usize,
        scope: Scope,
        recipe: DrumRecipeSlot,
        parameter: ParameterId,
    ) -> Result<ParameterValue, EditError> {
        let track = active_track(&self.project, self.pattern, track)?;
        let base = track
            .drum_recipe_parameter(recipe, parameter)
            .ok_or(EditError::InvalidParameter)?;
        if scope == Scope::Base {
            return Ok(base);
        }
        let event = track
            .steps
            .get(step)
            .ok_or(EditError::InvalidStep)?
            .as_ref()
            .ok_or(EditError::EmptyLock)?;
        if event.drum_recipe() != Some(recipe) {
            return Err(EditError::InvalidDrumRecipe);
        }
        let locks =
            crate::model::effective_event_locks(track.steps, step).ok_or(EditError::EmptyLock)?;
        Ok(locks.get(parameter).unwrap_or(base))
    }

    pub fn parameter_value(
        &self,
        track: usize,
        step: usize,
        scope: Scope,
        parameter: ParameterId,
    ) -> Result<ParameterValue, EditError> {
        let t = active_track(&self.project, self.pattern, track)?;
        read_parameter(&t, step, scope, parameter)
    }

    pub fn clear_parameter_lock(
        &mut self,
        track: usize,
        step: usize,
        parameter: ParameterId,
    ) -> Result<bool, EditError> {
        self.edit_active_track(track, None, move |p, pattern| {
            let mut t = active_track_mut(p, pattern, track)?;
            clear_parameter_lock(&mut t, step, parameter)
        })
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
        if !track.supports_lfo(parameter) {
            return Err(EditError::InvalidParameter);
        }
        Ok(track.lfo(parameter))
    }

    pub fn set_lfo(
        &mut self,
        track: usize,
        parameter: ParameterId,
        config: Option<LfoConfig>,
        key: Option<CoalesceKey>,
    ) -> Result<bool, EditError> {
        self.edit_track(track, key, move |project, _pattern| {
            let track = project
                .tracks
                .get_mut(track)
                .ok_or(EditError::InvalidTrack)?;
            if !track.set_lfo(parameter, config) {
                return Err(EditError::InvalidParameter);
            }
            Ok(())
        })
    }
}
fn clear_with_ties(steps: &mut [Step], step: usize) {
    steps[step] = None;
    cleanup_invalid_ties(steps)
}

fn clear_drum_sound_locks(locks: &mut crate::model::ParameterLocks, kind: TrackKind) {
    locks.clear(ParameterId::Tune);
    locks.clear(ParameterId::Decay);
    if kind == TrackKind::Tom {
        locks.clear(ParameterId::Tone);
    }
}

fn cleanup_invalid_ties(steps: &mut [Step]) {
    loop {
        let bad = (0..steps.len()).find(|&i| {
            matches!(steps[i], Some(StepEvent::Tie { .. })) && tie_source(steps, i).is_none()
        });
        if let Some(i) = bad {
            steps[i] = None
        } else {
            break;
        }
    }
}

pub fn percentage_key(c: char) -> Option<Percent> {
    match c {
        '`' | '-' => Percent::new(0),
        '1'..='9' => Percent::new(c.to_digit(10).unwrap() as u8 * 10),
        '0' => Percent::new(100),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        CHORD_TRACK_INDEX, LEAD_TRACK_INDEX, RIMSHOT_TRACK_INDEX, SYNTH_TRACK_START, TRACK_COUNT,
    };
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
    fn assigning_an_instrument_resets_one_slot_in_every_pattern_as_one_edit() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.tracks[0].level = Percent::new(17).unwrap();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: TriggerCondition::Always,
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: ParameterLocks::default(),
        });
        project.patterns[1].tracks[0].steps.resize(32, None);
        project.patterns[1].tracks[0].steps[31] = project.patterns[0].tracks[0].steps[0];
        let before = project.clone();
        let mut editor = Editor::new(project);

        assert!(editor.assign_instrument(0, TrackKind::Chord).unwrap());
        assert_eq!(editor.project.tracks[0].kind, TrackKind::Chord);
        assert_eq!(
            editor.project.tracks[0],
            Project::new().tracks[CHORD_TRACK_INDEX]
        );
        assert!(editor.project.patterns.iter().all(|pattern| {
            pattern.tracks[0].steps.len() == crate::model::STEP_BANK_SIZE
                && pattern.tracks[0].steps.iter().all(Option::is_none)
        }));
        assert!(editor.is_dirty());
        let impact = editor.take_edit_impact();
        assert_eq!(impact.tracks, 1);
        assert_eq!(impact.sequences, vec![(0, 0), (1, 0)]);

        assert!(editor.undo());
        assert_eq!(editor.project, before);
        assert!(editor.redo());
        assert_eq!(editor.project.tracks[0].kind, TrackKind::Chord);
    }

    #[test]
    fn assigning_the_current_instrument_is_a_noop() {
        let mut editor = Editor::new(Project::new());

        assert!(!editor.assign_instrument(0, TrackKind::Kick).unwrap());
        assert!(!editor.is_dirty());
        assert_eq!(editor.take_edit_impact(), EditImpact::default());
        assert!(!editor.undo());
    }

    #[test]
    fn scoped_edits_and_history_report_exact_audio_impacts() {
        let mut editor = Editor::new(Project::new());
        assert_eq!(editor.take_edit_impact(), EditImpact::default());

        editor.toggle_event(0, 0).unwrap();
        let impact = editor.take_edit_impact();
        assert_eq!(impact.tracks, 0);
        assert_eq!(impact.sequences, vec![(0, 0)]);
        assert!(!impact.patterns_structural);

        editor
            .set_track_probability(0, Percent::new(75).unwrap(), None)
            .unwrap();
        let impact = editor.take_edit_impact();
        assert_eq!(impact.tracks, 1);
        assert!(impact.sequences.is_empty());

        assert!(editor.undo());
        let impact = editor.take_edit_impact();
        assert_eq!(impact.tracks, 1);
        assert!(impact.sequences.is_empty());
    }

    #[test]
    fn saving_ends_coalescing_so_undo_restores_the_saved_state() {
        let mut editor = Editor::new(Project::new());
        let key = Some(CoalesceKey(0, 0, u8::MAX));
        editor
            .set_track_probability(0, Percent::new(90).unwrap(), key)
            .unwrap();
        editor.mark_saved();
        editor
            .set_track_probability(0, Percent::new(80).unwrap(), key)
            .unwrap();
        assert!(editor.is_dirty());

        assert!(editor.undo());
        assert_eq!(
            editor.project.tracks[0].probability,
            Percent::new(90).unwrap()
        );
        assert!(!editor.is_dirty());
    }

    #[test]
    fn editing_pattern_zero_after_saving_pattern_one_stays_dirty() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        let mut editor = Editor::new(project);
        editor.select_pattern(1);
        editor.toggle_event(0, 0).unwrap();
        editor.mark_saved();

        editor.select_pattern(0);
        editor.toggle_event(0, 1).unwrap();
        editor.select_pattern(1);

        assert!(editor.is_dirty());
    }

    #[test]
    fn dynamic_pattern_operations_shift_cursor_and_are_undoable() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        let (changed, cursor) = editor.insert_pattern(0).unwrap();
        assert!(changed);
        assert_eq!(editor.project.patterns.len(), 2);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.project.patterns[0].tracks[0].steps[0].is_some());
        assert!(editor.project.patterns[1].tracks[0].steps[0].is_none());

        let (changed, cursor) = editor.duplicate_pattern(cursor).unwrap();
        assert!(changed);
        assert_eq!(editor.project.patterns.len(), 3);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.delete_pattern(cursor).unwrap().0);
        assert_eq!(editor.project.patterns.len(), 2);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.undo());
        assert_eq!(editor.project.patterns.len(), 3);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.redo());
        assert_eq!(editor.project.patterns.len(), 2);
    }

    #[test]
    fn copy_cut_paste_and_final_pattern_reset() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        assert!(editor.copy_pattern(0));
        assert!(editor.delete_pattern(0).unwrap().0);
        assert!(editor.paste_pattern(0).unwrap().0);
        assert!(editor.project.patterns[1].tracks[0].steps[0].is_some());
        assert!(editor.cut_pattern(1).unwrap().0);
        assert_eq!(editor.project.patterns.len(), 1);
        assert!(editor.active_steps(0).unwrap().iter().all(Option::is_none));
        assert!(!editor.delete_pattern(0).unwrap().0);
        assert_eq!(editor.project.patterns.len(), 1);
        assert!(editor.active_steps(0).unwrap().iter().all(Option::is_none));
    }

    #[test]
    fn cursor_pattern_operations_preserve_the_committed_editor_pattern() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.duplicate_pattern(0).unwrap();
        editor.duplicate_pattern(1).unwrap();
        editor.select_pattern(0);

        let (changed, cursor) = editor.insert_pattern(1).unwrap();
        assert!(changed);
        assert_eq!(cursor, 2);
        assert_eq!(editor.pattern(), 0);
        assert!(editor.active_steps(0).unwrap()[0].is_some());
        assert_eq!(editor.project.patterns.len(), 4);

        let (changed, cursor) = editor.delete_pattern(cursor).unwrap();
        assert!(changed);
        assert_eq!(cursor, 2);
        assert_eq!(editor.pattern(), 0);
        assert_eq!(editor.project.patterns.len(), 3);
    }

    #[test]
    fn cursor_clipboard_operations_use_the_cursor_pattern() {
        let mut editor = Editor::new(Project::new());
        editor.duplicate_pattern(0).unwrap();
        editor.select_pattern(0);
        editor.project.patterns[1].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });

        assert!(editor.copy_pattern(1));
        let (changed, cursor) = editor.paste_pattern(0).unwrap();
        assert!(changed);
        assert_eq!(cursor, 1);
        assert!(editor.project.patterns[1].tracks[0].steps[0].is_some());
        assert!(editor.cut_pattern(1).unwrap().0);
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
        e.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
        e.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
        e.toggle_tie(SYNTH_TRACK_START, 2).unwrap();
        e.clear(SYNTH_TRACK_START, 0).unwrap();
        assert!(e.active_steps(SYNTH_TRACK_START).unwrap()[1].is_none());
        e.undo();
        assert!(matches!(
            e.active_steps(SYNTH_TRACK_START).unwrap()[2],
            Some(StepEvent::Tie { .. })
        ));
    }

    #[test]
    fn clear_track_removes_events_and_locks_as_one_undoable_edit() {
        let mut editor = Editor::new(Project::new());
        editor.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
        editor
            .set_parameter(
                SYNTH_TRACK_START,
                0,
                Scope::Lock,
                ParameterId::Cutoff,
                ParameterValue::Percent(Percent::new(25).unwrap()),
                None,
            )
            .unwrap();
        editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
        editor.toggle_event(0, 0).unwrap();
        let before = editor.project.clone();
        let length = editor.active_steps(SYNTH_TRACK_START).unwrap().len();

        assert_eq!(editor.clear_track(SYNTH_TRACK_START).unwrap(), 2);
        assert!(
            editor
                .active_steps(SYNTH_TRACK_START)
                .unwrap()
                .iter()
                .all(Option::is_none)
        );
        assert_eq!(
            editor.active_steps(SYNTH_TRACK_START).unwrap().len(),
            length
        );
        assert!(editor.active_steps(0).unwrap()[0].is_some());

        assert!(editor.undo());
        assert_eq!(editor.project, before);
        assert!(editor.redo());
        assert!(
            editor
                .active_steps(SYNTH_TRACK_START)
                .unwrap()
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn clearing_an_empty_track_is_a_no_op() {
        let mut editor = Editor::new(Project::new());

        assert_eq!(editor.clear_track(SYNTH_TRACK_START).unwrap(), 0);
        assert!(!editor.undo());
    }

    #[test]
    fn direct_percentage_keys_cover_the_complete_mapping() {
        for (key, expected) in [
            ('`', 0),
            ('-', 0),
            ('1', 10),
            ('2', 20),
            ('3', 30),
            ('4', 40),
            ('5', 50),
            ('6', 60),
            ('7', 70),
            ('8', 80),
            ('9', 90),
            ('0', 100),
        ] {
            assert_eq!(percentage_key(key).map(Percent::get), Some(expected));
        }
        for key in ['=', 'a', ' '] {
            assert_eq!(percentage_key(key), None);
        }
    }

    #[test]
    fn undo_history_records_only_the_changed_project_regions() {
        let mut editor = Editor::new(Project::new());

        editor.toggle_event(0, 0).unwrap();
        let delta = &editor.undo.back().unwrap().delta;
        assert_eq!(delta.sequences.len(), 1);
        assert!(delta.globals.is_none());
        assert!(delta.tracks.is_empty());
        assert!(delta.patterns.is_none());
        assert!(delta.song.is_none());

        editor
            .set_track_swing(0, Percent::new(25).unwrap(), None)
            .unwrap();
        let delta = &editor.undo.back().unwrap().delta;
        assert_eq!(delta.tracks.len(), 1);
        assert!(delta.sequences.is_empty());
        assert!(delta.patterns.is_none());

        editor
            .edit(None, |project, _| {
                project.globals.tempo_bpm = 121;
                Ok(())
            })
            .unwrap();
        let delta = &editor.undo.back().unwrap().delta;
        assert!(delta.globals.is_some());
        assert!(delta.tracks.is_empty());
        assert!(delta.sequences.is_empty());

        editor.insert_pattern(0).unwrap();
        let delta = &editor.undo.back().unwrap().delta;
        assert!(delta.patterns.is_some());
        assert!(delta.sequences.is_empty());
    }

    #[test]
    fn new_events_inherit_the_input_accent_and_bass_notes_do_not_slide() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_accent(0, 0).unwrap();
        editor.toggle_event(0, 0).unwrap();
        assert_eq!(editor.accent_value(0, 0), Ok(true));

        editor.toggle_accent(0, 0).unwrap();
        assert_eq!(editor.accent_value(0, 0), Ok(false));
        editor.clear(0, 0).unwrap();

        editor.toggle_accent(SYNTH_TRACK_START, 0).unwrap();
        editor.toggle_event(SYNTH_TRACK_START, 0).unwrap();
        assert!(matches!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[0],
            Some(StepEvent::BassNote {
                accent: true,
                slide: false,
                ..
            })
        ));
        editor.toggle_accent(SYNTH_TRACK_START, 0).unwrap();
        assert!(editor.project.tracks[SYNTH_TRACK_START].input_accent);
        editor.clear(SYNTH_TRACK_START, 0).unwrap();
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(true));
        editor.set_note(SYNTH_TRACK_START, 1, 2).unwrap();
        assert!(matches!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[1],
            Some(StepEvent::BassNote { accent: true, .. })
        ));
    }

    #[test]
    fn empty_accent_defaults_are_undoable_and_ties_reject_direct_editing() {
        let mut editor = Editor::new(Project::new());
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(false));
        editor.toggle_accent(SYNTH_TRACK_START, 0).unwrap();
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(true));
        assert!(editor.undo());
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(false));
        assert!(editor.redo());
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(true));

        editor.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
        editor.toggle_slide(SYNTH_TRACK_START, 0).unwrap();
        editor.set_note(SYNTH_TRACK_START, 0, 5).unwrap();
        assert_eq!(editor.accent_value(SYNTH_TRACK_START, 0), Ok(true));
        assert!(matches!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[0],
            Some(StepEvent::BassNote { slide: true, .. })
        ));
        editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
        assert_eq!(
            editor.toggle_accent(SYNTH_TRACK_START, 1),
            Err(EditError::NoAccent)
        );
        assert_eq!(
            editor.toggle_slide(CHORD_TRACK_INDEX, 0),
            Err(EditError::NoSlide)
        );
        assert!(editor.undo());
    }

    #[test]
    fn lead_notes_support_slide_and_other_tracks_reject_it() {
        let mut editor = Editor::new(Project::new());
        let lead = LEAD_TRACK_INDEX;
        editor.set_note(lead, 0, 1).unwrap();

        assert!(editor.toggle_slide(lead, 0).unwrap());
        assert!(matches!(
            editor.active_steps(lead).unwrap()[0],
            Some(StepEvent::LeadNote { slide: true, .. })
        ));
        assert!(editor.undo());
        assert!(matches!(
            editor.active_steps(lead).unwrap()[0],
            Some(StepEvent::LeadNote { slide: false, .. })
        ));
        assert_eq!(
            editor.toggle_slide(CHORD_TRACK_INDEX, 0),
            Err(EditError::NoSlide)
        );
    }

    #[test]
    fn replacing_ties_uses_the_current_input_accent_but_existing_notes_keep_theirs() {
        let mut editor = Editor::new(Project::new());
        editor.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
        editor.toggle_accent(SYNTH_TRACK_START, 0).unwrap();
        editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();

        editor.toggle_accent(SYNTH_TRACK_START, 1).unwrap_err();
        editor.set_note(SYNTH_TRACK_START, 1, 2).unwrap();
        assert!(matches!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[1],
            Some(StepEvent::BassNote { accent: false, .. })
        ));

        editor.set_note(SYNTH_TRACK_START, 0, 5).unwrap();
        assert!(matches!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[0],
            Some(StepEvent::BassNote { accent: true, .. })
        ));
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
            (ParameterId::Tune, 64),
            (ParameterId::Tone, 37),
            (ParameterId::Decay, 81),
        ] {
            e.set_parameter(
                RIMSHOT_TRACK_INDEX,
                0,
                Scope::Base,
                parameter,
                p(value),
                None,
            )
            .unwrap();
        }
        let crate::model::Instrument::Rimshot(rimshot) =
            &e.project.tracks[RIMSHOT_TRACK_INDEX].instrument
        else {
            panic!("expected rimshot")
        };
        assert_eq!(rimshot.tune.get(), 64);
        assert_eq!(rimshot.tone.get(), 37);
        assert_eq!(rimshot.decay.get(), 81);

        for (parameter, value) in [
            (ParameterId::Cutoff, 41),
            (ParameterId::Resonance, 22),
            (ParameterId::FilterEnvelope, 63),
            (ParameterId::Attack, 4),
            (ParameterId::Decay, 31),
            (ParameterId::Sustain, 74),
            (ParameterId::Release, 18),
        ] {
            e.set_parameter(CHORD_TRACK_INDEX, 0, Scope::Base, parameter, p(value), None)
                .unwrap();
        }
        e.set_parameter(
            CHORD_TRACK_INDEX,
            0,
            Scope::Base,
            ParameterId::OscillatorMix,
            p(82),
            None,
        )
        .unwrap();
        let crate::model::Instrument::Chord(synth) =
            &e.project.tracks[CHORD_TRACK_INDEX].instrument
        else {
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
    fn effect_base_and_lock_edits_are_undoable_and_coalescible() {
        let mut editor = Editor::new(Project::new());
        let value = |n| ParameterValue::Percent(Percent::new(n).unwrap());
        editor
            .set_parameter(
                0,
                0,
                Scope::Base,
                ParameterId::DistortionDrive,
                value(70),
                Some(CoalesceKey(0, 0, ParameterId::DistortionDrive as u8)),
            )
            .unwrap();
        editor
            .set_parameter(
                0,
                0,
                Scope::Base,
                ParameterId::DistortionDrive,
                value(80),
                Some(CoalesceKey(0, 0, ParameterId::DistortionDrive as u8)),
            )
            .unwrap();
        assert_eq!(
            editor.project.tracks[0].effects.distortion.drive,
            Percent::new(80).unwrap()
        );
        editor
            .set_parameter(0, 0, Scope::Base, ParameterId::FlangerMix, value(45), None)
            .unwrap();
        assert_eq!(
            editor.project.tracks[0].effects.flanger.mix,
            Percent::new(45).unwrap()
        );
        editor
            .set_parameter(
                0,
                0,
                Scope::Base,
                ParameterId::BitCrusherMix,
                value(55),
                None,
            )
            .unwrap();
        assert_eq!(
            editor.project.tracks[0].effects.bit_crusher.mix,
            Percent::new(55).unwrap()
        );
        editor
            .set_parameter(
                0,
                0,
                Scope::Base,
                ParameterId::Chorus,
                ParameterValue::Chorus(crate::model::ChorusMode::I),
                None,
            )
            .unwrap();
        assert_eq!(
            editor.project.tracks[0].effects.chorus,
            crate::model::ChorusMode::I
        );
        editor.toggle_event(0, 0).unwrap();
        editor
            .set_parameter(
                0,
                0,
                Scope::Lock,
                ParameterId::FlangerFeedback,
                value(60),
                None,
            )
            .unwrap();
        editor
            .set_parameter(
                0,
                0,
                Scope::Lock,
                ParameterId::Chorus,
                ParameterValue::Chorus(crate::model::ChorusMode::Ii),
                None,
            )
            .unwrap();
        assert_eq!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .chorus(),
            Some(crate::model::ChorusMode::Ii)
        );
        assert_eq!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .percent(ParameterId::FlangerFeedback),
            Percent::new(60)
        );
        assert!(editor.undo());
        assert!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .chorus()
                .is_none()
        );
        assert!(editor.undo());
        assert!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .percent(ParameterId::FlangerFeedback)
                .is_none()
        );
        editor
            .set_parameter(
                0,
                0,
                Scope::Lock,
                ParameterId::BitCrusherBits,
                value(80),
                None,
            )
            .unwrap();
        assert_eq!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .percent(ParameterId::BitCrusherBits),
            Percent::new(80)
        );
    }

    #[test]
    fn coalescing_preserves_the_earliest_value_for_every_touched_region() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.end_coalescing();
        let key = Some(CoalesceKey(0, 0, ParameterId::Level as u8));

        editor
            .set_parameter(
                0,
                0,
                Scope::Base,
                ParameterId::Level,
                ParameterValue::Percent(Percent::new(30).unwrap()),
                key,
            )
            .unwrap();
        editor
            .set_parameter(
                0,
                0,
                Scope::Lock,
                ParameterId::Level,
                ParameterValue::Percent(Percent::new(40).unwrap()),
                key,
            )
            .unwrap();

        assert!(editor.undo());
        assert_eq!(editor.project.tracks[0].level, Percent::new(80).unwrap());
        assert_eq!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .get(ParameterId::Level),
            None
        );

        assert!(editor.redo());
        assert_eq!(editor.project.tracks[0].level, Percent::new(30).unwrap());
        assert_eq!(
            editor.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .get(ParameterId::Level),
            Some(ParameterValue::Percent(Percent::new(40).unwrap()))
        );
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
            e.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .percent(ParameterId::Tune)
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
            e.active_steps(0).unwrap()[0]
                .as_ref()
                .unwrap()
                .locks()
                .percent(ParameterId::Tune)
                .is_some()
        );
    }

    #[test]
    fn tied_lock_reads_follow_the_source_and_intermediate_overrides() {
        let value = |n| ParameterValue::Percent(Percent::new(n).unwrap());
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: Default::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: ParameterLocks::from_pairs([
                (ParameterId::Level, value(50)),
                (ParameterId::Cutoff, value(60)),
            ]),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks::from_pairs([(ParameterId::Level, value(30))]),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[2] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let mut editor = Editor::new(project);

        assert_eq!(
            editor.parameter_value(SYNTH_TRACK_START, 2, Scope::Lock, ParameterId::Level),
            Ok(value(30))
        );
        assert_eq!(
            editor.parameter_value(SYNTH_TRACK_START, 2, Scope::Lock, ParameterId::Cutoff),
            Ok(value(60))
        );
        editor
            .clear_parameter_lock(SYNTH_TRACK_START, 1, ParameterId::Level)
            .unwrap();
        assert_eq!(
            editor.parameter_value(SYNTH_TRACK_START, 2, Scope::Lock, ParameterId::Level),
            Ok(value(50))
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
        e.toggle_event(SYNTH_TRACK_START, 0).unwrap();
        assert_eq!(
            e.set_parameter(
                SYNTH_TRACK_START,
                0,
                Scope::Lock,
                ParameterId::Tone,
                value,
                None
            ),
            Err(EditError::InvalidParameter)
        );
    }

    #[test]
    fn resize_cleans_wrapped_ties_and_undo_restores_them() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(SYNTH_TRACK_START, 4, None).unwrap();
        e.set_note(SYNTH_TRACK_START, 3, 1).unwrap();
        e.toggle_tie(SYNTH_TRACK_START, 0).unwrap();
        e.set_track_length(SYNTH_TRACK_START, 3, None).unwrap();
        assert_eq!(e.active_steps(SYNTH_TRACK_START).unwrap().len(), 3);
        assert!(e.active_steps(SYNTH_TRACK_START).unwrap()[0].is_none());
        assert!(e.undo());
        assert_eq!(e.active_steps(SYNTH_TRACK_START).unwrap().len(), 4);
        assert!(matches!(
            e.active_steps(SYNTH_TRACK_START).unwrap()[0],
            Some(StepEvent::Tie { .. })
        ));
        assert!(matches!(
            e.active_steps(SYNTH_TRACK_START).unwrap()[3],
            Some(StepEvent::BassNote { .. })
        ));
    }

    #[test]
    fn duplicate_track_copies_events_locks_and_is_one_undo_step() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(SYNTH_TRACK_START, 4, None).unwrap();
        e.set_note(SYNTH_TRACK_START, 3, 2).unwrap();
        e.set_parameter(
            SYNTH_TRACK_START,
            3,
            Scope::Lock,
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(42).unwrap()),
            None,
        )
        .unwrap();
        e.toggle_tie(SYNTH_TRACK_START, 0).unwrap();
        let original = e.active_steps(SYNTH_TRACK_START).unwrap().to_vec();
        e.duplicate_track(SYNTH_TRACK_START).unwrap();
        assert_eq!(e.active_steps(SYNTH_TRACK_START).unwrap().len(), 8);
        assert_eq!(
            &e.active_steps(SYNTH_TRACK_START).unwrap()[..4],
            original.as_slice()
        );
        assert_eq!(
            &e.active_steps(SYNTH_TRACK_START).unwrap()[4..],
            original.as_slice()
        );
        assert!(e.undo());
        assert_eq!(e.active_steps(SYNTH_TRACK_START).unwrap(), original);
    }

    #[test]
    fn duplicate_rejects_lengths_above_32_without_state_change() {
        let mut e = Editor::new(Project::new());
        e.set_track_length(0, 33, None).unwrap();
        e.mark_saved();
        assert_eq!(e.duplicate_track(0), Err(EditError::CannotDouble));
        assert_eq!(e.active_steps(0).unwrap().len(), 33);
        assert!(!e.is_dirty());
    }

    #[test]
    fn lfo_assignment_is_validated_and_undoable() {
        let mut editor = Editor::new(Project::new());
        let config = LfoConfig::default();
        editor
            .set_lfo(SYNTH_TRACK_START, ParameterId::Cutoff, Some(config), None)
            .unwrap();
        assert_eq!(
            editor.lfo(SYNTH_TRACK_START, ParameterId::Cutoff),
            Ok(Some(config))
        );
        assert!(editor.is_dirty());
        assert!(editor.undo());
        assert_eq!(editor.lfo(SYNTH_TRACK_START, ParameterId::Cutoff), Ok(None));
        assert_eq!(
            editor.set_lfo(SYNTH_TRACK_START, ParameterId::Waveform, Some(config), None),
            Err(EditError::InvalidParameter)
        );
        assert_eq!(
            editor.set_lfo(0, ParameterId::Cutoff, Some(config), None),
            Err(EditError::InvalidParameter)
        );
        editor
            .set_lfo(CHORD_TRACK_INDEX, ParameterId::Pitch, Some(config), None)
            .unwrap();
        assert_eq!(
            editor.lfo(CHORD_TRACK_INDEX, ParameterId::Pitch),
            Ok(Some(config))
        );
        assert!(editor.undo());
        assert_eq!(editor.lfo(CHORD_TRACK_INDEX, ParameterId::Pitch), Ok(None));
        assert!(editor.redo());
        assert_eq!(
            editor.lfo(CHORD_TRACK_INDEX, ParameterId::Pitch),
            Ok(Some(config))
        );
        assert_eq!(
            editor.set_lfo(SYNTH_TRACK_START, ParameterId::Pitch, Some(config), None),
            Err(EditError::InvalidParameter)
        );
        assert_eq!(
            editor.set_parameter(
                CHORD_TRACK_INDEX,
                0,
                Scope::Base,
                ParameterId::Pitch,
                ParameterValue::Percent(crate::model::Percent::new(50).unwrap()),
                None
            ),
            Err(EditError::InvalidParameter)
        );
        assert_eq!(
            editor.set_parameter(
                CHORD_TRACK_INDEX,
                0,
                Scope::Lock,
                ParameterId::Pitch,
                ParameterValue::Percent(crate::model::Percent::new(50).unwrap()),
                None
            ),
            Err(EditError::InvalidParameter)
        );
    }

    #[test]
    fn chord_shape_edits_selected_notes_and_empty_step_defaults() {
        let mut editor = Editor::new(Project::new());
        editor
            .set_chord_shape(CHORD_TRACK_INDEX, 0, ChordShape::Single)
            .unwrap();
        assert_eq!(
            editor.project.tracks[CHORD_TRACK_INDEX].input_chord_shape,
            Some(ChordShape::Single)
        );
        editor.set_note(CHORD_TRACK_INDEX, 0, 1).unwrap();
        assert!(matches!(
            editor.active_steps(CHORD_TRACK_INDEX).unwrap()[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::Single),
                ..
            })
        ));
        editor
            .set_chord_shape(CHORD_TRACK_INDEX, 0, ChordShape::DyadFifth)
            .unwrap();
        assert!(matches!(
            editor.active_steps(CHORD_TRACK_INDEX).unwrap()[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::DyadFifth),
                ..
            })
        ));
        assert!(editor.undo());
        assert!(matches!(
            editor.active_steps(CHORD_TRACK_INDEX).unwrap()[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::Single),
                ..
            })
        ));
    }

    #[test]
    fn fm_voicing_defaults_to_single_and_preserves_polyphonic_articulation() {
        let mut editor = Editor::new(Project::new());
        assert_eq!(
            editor.chord_shape_value(crate::model::FM_TRACK_INDEX, 0),
            Ok(ChordShape::Single)
        );
        editor
            .set_chord_shape(
                crate::model::FM_TRACK_INDEX,
                0,
                ChordShape::SeventhFirstInversion,
            )
            .unwrap();
        editor
            .set_arpeggio_enabled(crate::model::FM_TRACK_INDEX, 0, true)
            .unwrap();
        editor.set_note(crate::model::FM_TRACK_INDEX, 0, 4).unwrap();
        assert!(matches!(
            editor.active_steps(crate::model::FM_TRACK_INDEX).unwrap()[0],
            Some(StepEvent::Note {
                chord_shape: Some(ChordShape::SeventhFirstInversion),
                arpeggio: ArpeggioConfig { enabled: true, .. },
                ..
            })
        ));
        editor.toggle_tie(crate::model::FM_TRACK_INDEX, 1).unwrap();
        assert_eq!(
            editor.chord_shape_value(crate::model::FM_TRACK_INDEX, 1),
            Ok(ChordShape::SeventhFirstInversion)
        );
    }

    #[test]
    fn chord_trigger_articulation_and_tie_inheritance_are_undoable() {
        let mut editor = Editor::new(Project::new());
        editor
            .set_arpeggio_type(CHORD_TRACK_INDEX, 0, crate::model::ArpeggioType::Down)
            .unwrap();
        editor
            .set_arpeggio_rate(
                CHORD_TRACK_INDEX,
                0,
                crate::model::ArpeggioRate::EighthTriplet,
            )
            .unwrap();
        editor
            .set_arpeggio_enabled(CHORD_TRACK_INDEX, 0, true)
            .unwrap();
        assert_eq!(
            editor.arpeggio_config_value(CHORD_TRACK_INDEX, 0).unwrap(),
            crate::model::ArpeggioConfig {
                enabled: true,
                r#type: crate::model::ArpeggioType::Down,
                rate: crate::model::ArpeggioRate::EighthTriplet,
            }
        );
        editor.set_note(CHORD_TRACK_INDEX, 0, 1).unwrap();
        editor
            .set_chord_shape(CHORD_TRACK_INDEX, 0, ChordShape::SeventhRoot)
            .unwrap();
        editor
            .set_arpeggio_rate(
                CHORD_TRACK_INDEX,
                0,
                crate::model::ArpeggioRate::ThirtySecond,
            )
            .unwrap();
        editor.toggle_tie(CHORD_TRACK_INDEX, 1).unwrap();
        assert_eq!(
            editor.chord_shape_value(CHORD_TRACK_INDEX, 1),
            Ok(ChordShape::SeventhRoot)
        );
        assert_eq!(
            editor
                .arpeggio_config_value(CHORD_TRACK_INDEX, 1)
                .map(|c| c.rate),
            Ok(crate::model::ArpeggioRate::ThirtySecond)
        );
        assert_eq!(
            editor.set_arpeggio_rate(CHORD_TRACK_INDEX, 1, crate::model::ArpeggioRate::Quarter,),
            Err(EditError::NoChordShape)
        );
        assert!(editor.undo());
        assert!(editor.redo());
    }

    #[test]
    fn trigger_settings_and_swing_are_atomic_and_reject_ties() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        let condition = TriggerCondition::Cycle {
            position: 2,
            length: 3,
        };
        editor.set_trigger_condition(0, 0, condition).unwrap();
        editor.set_retrigger_count(0, 0, 3).unwrap();
        editor
            .set_microtiming(0, 0, Microtiming::new(-25).unwrap(), None)
            .unwrap();
        editor
            .set_track_swing(0, Percent::new(42).unwrap(), None)
            .unwrap();
        assert_eq!(editor.trigger_condition_value(0, 0), Ok(condition));
        assert_eq!(editor.retrigger_count_value(0, 0), Ok(3));
        assert_eq!(
            editor.microtiming_value(0, 0),
            Ok(Microtiming::new(-25).unwrap())
        );
        assert_eq!(editor.project.tracks[0].swing, Percent::new(42).unwrap());
        assert!(editor.undo());
        assert_eq!(editor.project.tracks[0].swing, Percent::ZERO);
        editor.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
        editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
        assert_eq!(
            editor.set_retrigger_count(SYNTH_TRACK_START, 1, 2),
            Err(EditError::NoTriggerSettings)
        );
        assert_eq!(
            editor.set_microtiming(SYNTH_TRACK_START, 1, Microtiming::ZERO, None),
            Err(EditError::NoTriggerSettings)
        );
    }

    #[test]
    fn track_probability_is_undoable_dirty_and_coalescible() {
        let mut editor = Editor::new(Project::new());
        let key = Some(CoalesceKey(0, 0, 0xfe));
        editor
            .set_track_probability(0, Percent::new(90).unwrap(), key)
            .unwrap();
        editor
            .set_track_probability(0, Percent::new(80).unwrap(), key)
            .unwrap();
        assert_eq!(editor.project.tracks[0].probability.get(), 80);
        assert!(editor.is_dirty());
        assert!(editor.undo());
        assert_eq!(editor.project.tracks[0].probability.get(), 100);
        assert!(editor.redo());
        assert_eq!(editor.project.tracks[0].probability.get(), 80);
        assert_eq!(
            editor.set_track_probability(TRACK_COUNT, Percent::new(50).unwrap(), None),
            Err(EditError::InvalidTrack)
        );
    }

    #[test]
    fn generator_fills_only_empty_steps_and_is_one_undoable_edit() {
        let mut editor = Editor::new(Project::new());
        editor.set_note(SYNTH_TRACK_START, 0, 8).unwrap();
        let original = editor.active_steps(SYNTH_TRACK_START).unwrap()[0];
        let config = crate::generator::Config {
            density: Percent::new(100).unwrap(),
            ..Default::default()
        };
        let inserted = editor.generate_pattern(config).unwrap();
        assert!(inserted > 0);
        assert_eq!(editor.active_steps(SYNTH_TRACK_START).unwrap()[0], original);
        assert!(editor.undo());
        assert_eq!(editor.active_steps(SYNTH_TRACK_START).unwrap()[0], original);
        assert!(editor.redo());
        assert_eq!(editor.active_steps(SYNTH_TRACK_START).unwrap()[0], original);
    }

    #[test]
    fn drum_recipe_selection_is_referenced_and_preserves_unrelated_trigger_data() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(3, 0).unwrap();
        editor.toggle_accent(3, 0).unwrap();
        editor
            .set_trigger_condition(
                3,
                0,
                TriggerCondition::Cycle {
                    position: 2,
                    length: 3,
                },
            )
            .unwrap();
        editor
            .set_parameter(
                3,
                0,
                Scope::Lock,
                ParameterId::Level,
                ParameterValue::Percent(Percent::new(25).unwrap()),
                None,
            )
            .unwrap();
        editor
            .set_parameter(
                3,
                0,
                Scope::Lock,
                ParameterId::Tune,
                ParameterValue::Percent(Percent::new(99).unwrap()),
                None,
            )
            .unwrap();

        editor.set_drum_recipe(3, 0, DrumRecipeSlot::TWO).unwrap();
        let Some(StepEvent::Trigger {
            accent,
            recipe,
            condition,
            locks,
            ..
        }) = editor.active_steps(3).unwrap()[0]
        else {
            panic!("expected trigger")
        };
        assert!(accent);
        assert_eq!(recipe, DrumRecipeSlot::TWO);
        assert_eq!(
            condition,
            TriggerCondition::Cycle {
                position: 2,
                length: 3
            }
        );
        assert_eq!(locks.percent(ParameterId::Level), Percent::new(25));
        assert_eq!(
            (
                locks.percent(ParameterId::Tune),
                locks.percent(ParameterId::Tone),
                locks.percent(ParameterId::Decay),
            ),
            (None, None, None)
        );

        editor
            .set_drum_recipe_parameter(
                3,
                0,
                Scope::Base,
                (DrumRecipeSlot::TWO, ParameterId::Tune),
                ParameterValue::Percent(Percent::new(63).unwrap()),
                None,
            )
            .unwrap();
        assert_eq!(
            editor.drum_recipe_parameter_value(
                3,
                0,
                Scope::Lock,
                DrumRecipeSlot::TWO,
                ParameterId::Tune
            ),
            Ok(ParameterValue::Percent(Percent::new(63).unwrap()))
        );
        assert!(editor.undo());
    }

    #[test]
    fn recipe_selection_creates_triggers_and_rejects_incompatible_slots() {
        let mut editor = Editor::new(Project::new());
        editor.project.tracks[2].input_accent = true;
        editor.set_drum_recipe(2, 4, DrumRecipeSlot::TWO).unwrap();
        assert!(matches!(
            editor.active_steps(2).unwrap()[4],
            Some(StepEvent::Trigger {
                accent: true,
                recipe: DrumRecipeSlot::TWO,
                ..
            })
        ));
        assert_eq!(
            editor.set_drum_recipe(2, 4, DrumRecipeSlot::THREE),
            Err(EditError::InvalidDrumRecipe)
        );
        assert_eq!(
            editor.set_drum_recipe(0, 0, DrumRecipeSlot::ONE),
            Err(EditError::InvalidDrumRecipe)
        );
    }

    #[test]
    fn step_clipboard_is_exact_same_kind_and_validates_ties_atomically() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        let mut editor = Editor::new(project);
        editor.set_note(SYNTH_TRACK_START, 0, 3).unwrap();
        editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
        editor.copy_step(SYNTH_TRACK_START, 0).unwrap();
        editor.paste_step(SYNTH_TRACK_START, 2).unwrap();
        assert_eq!(
            editor.active_steps(SYNTH_TRACK_START).unwrap()[2],
            editor.active_steps(SYNTH_TRACK_START).unwrap()[0]
        );
        assert_eq!(
            editor.paste_step(CHORD_TRACK_INDEX, 2),
            Err(EditError::IncompatibleStepClipboard)
        );

        editor.copy_step(SYNTH_TRACK_START, 1).unwrap();
        editor.select_pattern(1);
        let before = editor.project.clone();
        assert_eq!(
            editor.paste_step(SYNTH_TRACK_START, 1),
            Err(EditError::InvalidTie)
        );
        assert_eq!(editor.project, before);
    }

    #[test]
    fn empty_step_clipboard_clears_destination_and_cut_is_undoable() {
        let mut editor = Editor::new(Project::new());
        editor.toggle_event(0, 0).unwrap();
        editor.copy_step(0, 1).unwrap();
        editor.paste_step(0, 0).unwrap();
        assert!(editor.active_steps(0).unwrap()[0].is_none());
        assert!(editor.undo());
        assert!(editor.active_steps(0).unwrap()[0].is_some());
        assert!(editor.cut_step(0, 0).unwrap());
        assert!(editor.active_steps(0).unwrap()[0].is_none());
        assert!(editor.undo());
        assert!(editor.active_steps(0).unwrap()[0].is_some());
    }

    #[test]
    fn song_entries_edit_clipboard_and_undo_are_atomic() {
        let mut editor = Editor::new(Project::new());
        editor.insert_song_entry(0).unwrap();
        assert_eq!(editor.project.song.len(), 2);
        assert_eq!(
            editor.project.song[1],
            SongEntry {
                pattern: 1,
                bars: 1
            }
        );
        editor.change_song_bars(1, 3).unwrap();
        editor.change_song_pattern(1, 1).unwrap();
        assert_eq!(
            editor.project.song[1],
            SongEntry {
                pattern: 1,
                bars: 4
            }
        );
        editor.copy_song_entry(1);
        editor.paste_song_entry(1).unwrap();
        assert_eq!(editor.project.song.len(), 3);
        assert!(editor.undo());
        assert_eq!(editor.project.song.len(), 2);
        assert!(editor.delete_song_entry(0).unwrap());
        assert_eq!(editor.project.song.len(), 1);
        assert!(editor.delete_song_entry(0).unwrap());
        assert_eq!(
            editor.project.song,
            vec![SongEntry {
                pattern: 1,
                bars: 1
            }]
        );
    }
}
