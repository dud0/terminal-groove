use super::render::ValueOrigin;
use crate::tui::DIRECT_PARAMETER_RAMP;
use crate::{
    audio::RecordingState,
    generator::{ChordShapePool, Target as GeneratorTarget},
    model::{
        ChordShape, DrumRecipeSlot, GlobalParameterId, ParameterId, Percent, Project, TRACK_COUNT,
        TrackKind,
    },
    reducer::{Editor, Scope},
};
use std::{path::PathBuf, time::Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Navigation,
    PatternDialog,
    GeneratorDialog(GeneratorDialog),
    ParameterEdit(ParameterId),
    LfoEdit {
        parameter: ParameterId,
        field: LfoField,
    },
    ChordEdit {
        shape: ChordShape,
    },
    TriggerEdit {
        field: TriggerField,
    },
    SwingEdit,
    TrackProbabilityEdit,
    GlobalEdit(GlobalParameterId),
    SidechainEdit {
        field: SidechainField,
    },
    TempoInput(String),
    TrackLengthInput(String),
    ProjectBrowser {
        entries: Vec<PathBuf>,
        selected: usize,
    },
    PresetBrowser {
        track: usize,
        entries: Vec<PathBuf>,
        selected: usize,
    },
    FileInput(FileAction, String),
    PresetNameInput {
        track: usize,
        input: String,
    },
    OverwriteConfirm {
        path: PathBuf,
        input: String,
    },
    PresetOverwriteConfirm {
        track: usize,
        path: PathBuf,
        input: String,
    },
    DefaultPresetConfirm {
        track: usize,
        has_default: bool,
    },
    OpenConfirm(PathBuf),
    NewConfirm,
    Error(String),
    Help,
    QuitConfirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ParameterFocus {
    pub(super) parameter: ParameterId,
    pub(super) recipe: DrumRecipeSlot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatternPage {
    Patterns,
    Song,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParameterBank {
    Params,
    Effects,
}

impl ParameterBank {
    pub(super) const fn index(self) -> usize {
        match self {
            Self::Params => 0,
            Self::Effects => 1,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidechainField {
    Depth,
    Attack,
    Release,
}

impl SidechainField {
    pub(super) const ALL: [Self; 3] = [Self::Depth, Self::Attack, Self::Release];
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoField {
    Enabled,
    Waveform,
    TriggerReset,
    StartPhase,
    RateMode,
    Rate,
    Depth,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChordField {
    Shape,
    Arp,
    Type,
    Rate,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriggerField {
    Microtiming,
    Mode,
    CyclePosition,
    CycleLength,
    Chance,
    Retrigger,
}

impl TriggerField {
    pub(super) const ALL: [Self; 6] = [
        Self::Microtiming,
        Self::Mode,
        Self::CyclePosition,
        Self::CycleLength,
        Self::Chance,
        Self::Retrigger,
    ];
}

impl ChordField {
    pub(super) const ALL: [Self; 4] = [Self::Shape, Self::Arp, Self::Type, Self::Rate];
}

impl LfoField {
    pub(super) const ALL: [Self; 7] = [
        Self::Enabled,
        Self::Waveform,
        Self::TriggerReset,
        Self::StartPhase,
        Self::RateMode,
        Self::Rate,
        Self::Depth,
    ];
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileAction {
    SaveAs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GeneratorDialog {
    pub(crate) target: GeneratorTarget,
    pub(crate) track: usize,
    pub(crate) seed: String,
    pub(crate) density: Percent,
    pub(crate) range_low: u8,
    pub(crate) range_high: u8,
    pub(crate) chord_shapes: ChordShapePool,
    pub(crate) ties: Percent,
    pub(crate) accents: Percent,
    pub(crate) slides: Percent,
    pub(crate) field: usize,
}

impl GeneratorDialog {
    pub(crate) fn field_is_applicable(&self, project: &Project, field: usize) -> bool {
        let GeneratorTarget::Track(track) = self.target else {
            return true;
        };
        let Some(kind) = project.tracks.get(track).map(|track| track.kind) else {
            return false;
        };
        match field {
            6 => kind == TrackKind::Chord,
            9 => matches!(kind, TrackKind::Bass | TrackKind::Lead),
            _ => true,
        }
    }
}
pub struct App {
    pub editor: Editor,
    pub(super) row: usize,
    pub(super) step: usize,
    pub(super) global: usize,
    pub(super) scope: Scope,
    pub(super) parameter_bank: ParameterBank,
    pub(super) parameter_recipe: DrumRecipeSlot,
    pub(super) remembered_parameters: [[Option<ParameterFocus>; 2]; TRACK_COUNT],
    pub(super) mode: Mode,
    pub(super) chord_field: ChordField,
    pub(super) status: String,
    pub(super) path: Option<PathBuf>,
    pub(super) pending_open: Option<PathBuf>,
    pub(super) pending_new: bool,
    pub(super) pending_quit: bool,
    pub(super) quit: bool,
    pub(super) playheads: [Option<usize>; TRACK_COUNT],
    pub(super) playing: bool,
    pub(super) paused: bool,
    pub(super) pattern_cursor: usize,
    pub(super) active_pattern: usize,
    pub(super) queued_pattern: Option<usize>,
    pub(super) pattern_page: PatternPage,
    pub(super) song_cursor: usize,
    pub(super) song_mode: bool,
    pub(super) active_song: usize,
    pub(super) queued_song: Option<usize>,
    pub(super) song_bar: u8,
    pub(super) callback_overruns: u64,
    pub(super) max_callback_load_per_mille: u64,
    pub(super) recording_state: RecordingState,
    pub(super) fader_animations: Vec<FaderAnimation>,
}

#[derive(Clone, Copy)]
pub(super) struct FaderAnimation {
    pub(super) track: usize,
    pub(super) step: usize,
    pub(super) scope: Scope,
    pub(super) parameter: ParameterId,
    pub(super) recipe: DrumRecipeSlot,
    pub(super) from: Percent,
    pub(super) to: Percent,
    pub(super) started: Instant,
}
impl FaderAnimation {
    pub(super) fn value_at(self, now: Instant) -> Percent {
        let elapsed = now.saturating_duration_since(self.started);
        let progress = (elapsed.as_secs_f32() / DIRECT_PARAMETER_RAMP.as_secs_f32()).min(1.0);
        Percent::new(
            (self.from.get() as f32 + (self.to.get() as f32 - self.from.get() as f32) * progress)
                .round() as u8,
        )
        .unwrap()
    }
    pub(super) fn is_complete(self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= DIRECT_PARAMETER_RAMP
    }
}
impl App {
    pub fn new(project: Project, path: Option<PathBuf>) -> Self {
        Self {
            editor: Editor::new(project),
            row: 0,
            step: 0,
            global: 0,
            scope: Scope::Base,
            parameter_bank: ParameterBank::Params,
            parameter_recipe: DrumRecipeSlot::ONE,
            remembered_parameters: [[None; 2]; TRACK_COUNT],
            mode: Mode::Navigation,
            chord_field: ChordField::Shape,
            status: "Ready".into(),
            path,
            pending_open: None,
            pending_new: false,
            pending_quit: false,
            quit: false,
            playheads: [None; TRACK_COUNT],
            playing: false,
            paused: false,
            pattern_cursor: 0,
            active_pattern: 0,
            queued_pattern: None,
            pattern_page: PatternPage::Patterns,
            song_cursor: 0,
            song_mode: false,
            active_song: 0,
            queued_song: None,
            song_bar: 0,
            callback_overruns: 0,
            max_callback_load_per_mille: 0,
            recording_state: RecordingState::Idle,
            fader_animations: Vec::new(),
        }
    }
    pub(super) fn start_fader_animation(
        &mut self,
        track: usize,
        step: usize,
        parameter: ParameterId,
        from: Percent,
        to: Percent,
    ) {
        let now = Instant::now();
        let scope = self.scope;
        if let Some(existing) = self.fader_animations.iter_mut().find(|animation| {
            animation.track == track
                && animation.step == step
                && animation.scope == scope
                && animation.parameter == parameter
                && animation.recipe == self.parameter_recipe
        }) {
            existing.from = existing.value_at(now);
            existing.to = to;
            existing.started = now;
        } else {
            self.fader_animations.push(FaderAnimation {
                track,
                step,
                scope,
                parameter,
                recipe: self.parameter_recipe,
                from,
                to,
                started: now,
            });
        }
    }
    pub(super) fn animated_percent(
        &self,
        track: usize,
        step: usize,
        parameter: ParameterId,
        recipe: DrumRecipeSlot,
        origin: ValueOrigin,
        target: Percent,
    ) -> Percent {
        let now = Instant::now();
        self.fader_animations
            .iter()
            .rev()
            .find(|animation| {
                animation.track == track
                    && animation.parameter == parameter
                    && animation.recipe == recipe
                    && animation.to == target
                    && match animation.scope {
                        Scope::Base => origin == ValueOrigin::Base,
                        Scope::Lock => {
                            self.scope == Scope::Lock
                                && animation.step == step
                                && origin == ValueOrigin::Lock
                        }
                    }
            })
            .map(|animation| animation.value_at(now))
            .unwrap_or(target)
    }
}
