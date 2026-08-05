use super::*;

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
    GlobalEdit(GlobalParameterId),
    TempoInput(String),
    TrackLengthInput(String),
    FileInput(FileAction, String),
    OpenConfirm(PathBuf),
    NewConfirm,
    Error(String),
    Help,
    QuitConfirm,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoField {
    Enabled,
    Waveform,
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
    Mode,
    CyclePosition,
    CycleLength,
    Chance,
    Retrigger,
}

impl TriggerField {
    pub(crate) const ALL: [Self; 5] = [
        Self::Mode,
        Self::CyclePosition,
        Self::CycleLength,
        Self::Chance,
        Self::Retrigger,
    ];
}

impl ChordField {
    pub(crate) const ALL: [Self; 4] = [Self::Shape, Self::Arp, Self::Type, Self::Rate];
}

impl LfoField {
    pub(crate) const ALL: [Self; 5] = [
        Self::Enabled,
        Self::Waveform,
        Self::RateMode,
        Self::Rate,
        Self::Depth,
    ];
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileAction {
    SaveAs,
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GeneratorDialog {
    pub(crate) target: GeneratorTarget,
    pub(crate) track: usize,
    pub(crate) seed: String,
    pub(crate) density: Percent,
    pub(crate) ties: Percent,
    pub(crate) accents: Percent,
    pub(crate) field: usize,
}
pub struct App {
    pub editor: Editor,
    pub(crate) row: usize,
    pub(crate) step: usize,
    pub(crate) global: usize,
    pub(crate) scope: Scope,
    pub(crate) mode: Mode,
    pub(crate) chord_field: ChordField,
    pub(crate) status: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) pending_open: Option<PathBuf>,
    pub(crate) pending_new: bool,
    pub(crate) pending_quit: bool,
    pub(crate) quit: bool,
    pub(crate) playheads: [Option<usize>; TRACK_COUNT],
    pub(crate) playing: bool,
    pub(crate) paused: bool,
    pub(crate) pattern_cursor: usize,
    pub(crate) active_pattern: usize,
    pub(crate) queued_pattern: Option<usize>,
    pub(crate) fader_animations: Vec<FaderAnimation>,
}

#[derive(Clone, Copy)]
pub(crate) struct FaderAnimation {
    pub(crate) track: usize,
    pub(crate) step: usize,
    pub(crate) scope: Scope,
    pub(crate) parameter: ParameterId,
    pub(crate) from: Percent,
    pub(crate) to: Percent,
    pub(crate) started: Instant,
}
impl FaderAnimation {
    pub(crate) fn value_at(self, now: Instant) -> Percent {
        let elapsed = now.saturating_duration_since(self.started);
        let progress = (elapsed.as_secs_f32() / DIRECT_PARAMETER_RAMP.as_secs_f32()).min(1.0);
        Percent::new(
            (self.from.get() as f32 + (self.to.get() as f32 - self.from.get() as f32) * progress)
                .round() as u8,
        )
        .unwrap()
    }
    pub(crate) fn is_complete(self, now: Instant) -> bool {
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
            fader_animations: Vec::new(),
        }
    }
    pub(crate) fn start_fader_animation(
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
                from,
                to,
                started: now,
            });
        }
    }
    pub(crate) fn animated_percent(
        &self,
        track: usize,
        step: usize,
        parameter: ParameterId,
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
