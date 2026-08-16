use super::overlays::{
    overwrite_destination, overwrite_popup_rect, popup, popup_at, probability_popup_rect,
    quit_popup_rect, render_chord_popup, render_fm_operator_popup, render_generator_popup,
    render_lfo_popup, render_lfo_selector, render_pattern_popup, render_preset_browser,
    render_preset_dialog, render_project_browser, render_sidechain_popup, render_trigger_popup,
    swing_popup_rect, tempo_popup_rect,
};
use super::{
    controller::{
        PRESET_EXTENSION, PROJECT_EXTENSION, global_name, preset_directory, preset_path_for_name,
        project_directory, project_path_for_name,
    },
    controls::{GLOBAL_CONTROLS, global_control},
    input::parameter_supports_direct_percentage,
    state::{App, DefaultPresetAction, Mode, ParameterBank},
};
use crate::tui::DIRECT_PERCENTAGE_HINT;
use crate::{
    audio::{Audio, RecordingState},
    model::{
        ChordShape, ChorusMode, DelayDivision, FmAlgorithm, FmRatio, GlobalParameterId,
        ParameterId, ParameterValue, PitchClass, STEP_BANK_SIZE, STEP_ROW_SIZE, Scale, StepEvent,
        TRACK_COUNT, TrackKind, TriggerCondition, Waveform,
    },
    reducer::Scope,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Row, Table},
};

pub(super) fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Base => "BASE",
        Scope::Lock => "LOCK",
    }
}

pub(super) fn mode_name(mode: &Mode) -> String {
    match mode {
        Mode::Navigation => "Sequencer".into(),
        Mode::PatternDialog => "Pattern dialog".into(),
        Mode::GeneratorDialog(_) => "Pattern idea generator".into(),
        Mode::ParameterEdit(parameter) => {
            format!("Parameter edit ({})", parameter.display_name())
        }
        Mode::FmOperatorEdit { operator, .. } => {
            format!("FM operator edit (OP{})", operator + 1)
        }
        Mode::LfoEdit { parameter, .. } => {
            format!("Track LFO edit ({})", parameter.display_name())
        }
        Mode::ChordEdit { shape } => format!("Voicing edit ({shape})"),
        Mode::TriggerEdit { .. } => "Trigger editor".into(),
        Mode::SwingEdit => "Track swing edit".into(),
        Mode::TrackProbabilityEdit => "Track probability edit".into(),
        Mode::GlobalEdit(id) => format!("Global edit ({})", global_name(*id)),
        Mode::SidechainEdit { .. } => "Ducking editor".into(),
        Mode::TempoInput(_) => "Tempo numeric input".into(),
        Mode::TrackLengthInput(_) => "Track length input".into(),
        Mode::ProjectBrowser { .. } => "Project browser".into(),
        Mode::PresetBrowser { .. } => "Preset browser".into(),
        Mode::PresetDialog { .. } => "Track preset dialog".into(),
        Mode::FileInput(_, _) => "Project-name input".into(),
        Mode::PresetNameInput { .. } => "Preset-name input".into(),
        Mode::OverwriteConfirm { .. } => "Overwrite confirmation".into(),
        Mode::PresetOverwriteConfirm { .. } => "Overwrite confirmation".into(),
        Mode::DefaultPresetConfirm { .. } => "Default preset confirmation".into(),
        Mode::OpenConfirm(_) => "Unsaved confirmation".into(),
        Mode::NewConfirm => "Unsaved confirmation".into(),
        Mode::Error(_) => "Error dialog".into(),
        Mode::Help => "Help".into(),
        Mode::QuitConfirm => "Unsaved confirmation".into(),
    }
}

pub(super) fn help_available(mode: &Mode) -> bool {
    matches!(
        mode,
        Mode::Navigation
            | Mode::ParameterEdit(_)
            | Mode::FmOperatorEdit { .. }
            | Mode::LfoEdit { .. }
            | Mode::ChordEdit { .. }
            | Mode::TriggerEdit { .. }
            | Mode::SwingEdit
            | Mode::TrackProbabilityEdit
    )
}

const HELP_TEXT: &str =
    "CORE  Space play/pause · . stop/reset · ? help · Esc close help
      Ctrl+N new · Ctrl+O open · Ctrl+S save · Ctrl+Shift+S save as · Ctrl+L track presets
      Ctrl+Q quit · Ctrl+R record WAV · Ctrl+Z undo · Ctrl+Y redo · Ctrl+C/X/V copy/cut/paste selected step
PATTERNS  Ctrl+P open dialog · ←/→ Home End move cursor · Enter select/queue
          N insert · D duplicate · C copy · X cut · V paste · Delete/Backspace remove · Esc close
SEQUENCER  ↑/↓ rows · ←/→ steps (global row: controls) · Shift+←/→ step bank
           Tab params · p LOCK params · ~ global · Shift+1..0 tracks · Enter event · Del/Bksp clear · Esc BASE
           g pattern generator · o audition selected step
           Shift+Delete/Backspace clear selected track
EVENTS & TRACKS  m mute · l length · Shift+D double
                 a accent/default · Shift+G Bass/Lead slide · Shift+T microtiming/condition/retrigger · Shift+S swing · Shift+Q probability
                 Hat 1/2 Closed/Open · Tom 1/2/3 Low/Medium/High · 0 clear recipe locks
                 1–8 note · [ / ] octave · t tie · C Voicing editor (Chord/FM)
PARAMETERS  v level · n pan · y delay send · b reverb send
           Tab/Shift+Tab sequencer mode · Shift+← PARAMS · Shift+→ EFFECTS
           p BASE/LOCK · PageUp/Down step · Shift+PageUp/Down step bank
           EFFECTS: d/t/x distortion drive/tone/mix · b/s/m crusher bits/rate/mix
           EFFECTS: r/e/f/M phaser rate/depth/feedback/mix
           EFFECTS: R rate · q delay · E depth · F feedback · N flanger mix · h chorus
           Kick: u tune · d decay · a attack
           Snare: u tune · t tone · s snappy · Hat recipes: u tune · d decay
           Tom recipes/Cymbal/Rimshot: u tune · t tone · d decay
           Bass: w waveform · c cutoff · R resonance · f filter env · d decay
           Chord/Lead: w mix · P pulse · u sub · O noise · c cutoff · R resonance · f filter · i pitch · ADSR
           FM: q algorithm · O operators · c brightness · i pitch · ADSR
           FM operator: ←/→ select · Tab field · ↑/↓ edit · [/] algorithm
           Shift+L LFO · [`/-/1–9/0] percent · ↑/↓ adjust · ←/→ switch parameter
           Enter/Esc finish · Backspace/Delete remove lock/LFO
GLOBAL  t tempo · y delay division · f feedback · r reverb time
        b reverb tone · p pre-delay · m reverb return · k key · s scale · ←/→ select · ↑/↓ adjust";

pub(super) fn track_label(t: &crate::model::Track) -> String {
    if t.kind.supports_voicing() {
        let mode = if t.input_voicing_shape() == Some(ChordShape::Single) {
            'M'
        } else {
            'C'
        };
        format!("{} {mode} O{}", t.name, t.input_octave.unwrap_or(3))
    } else if matches!(t.kind, TrackKind::Bass | TrackKind::Lead) {
        format!("{} O{}", t.name, t.input_octave.unwrap_or(3))
    } else {
        t.name.clone()
    }
}

fn octave_superscript(octave: u8) -> char {
    match octave {
        0 => '⁰',
        1 => '¹',
        2 => '²',
        3 => '³',
        4 => '⁴',
        5 => '⁵',
        6 => '⁶',
        7 => '⁷',
        _ => unreachable!("model validation bounds octaves to 0 through 7"),
    }
}

pub(super) fn step_cell(event: Option<&StepEvent>) -> String {
    match event {
        // Empty steps should stay visible for navigation without competing with
        // events. The middle dot is deliberately quieter than a period.
        None => " · ".into(),
        Some(StepEvent::Trigger {
            accent,
            recipe,
            condition,
            retrigger_count,
            locks,
            ..
        }) if *condition != TriggerCondition::Always || *retrigger_count != 1 => {
            if recipe.get() > 1 {
                format!("{}+{}", if *accent { 'X' } else { 'x' }, recipe.get())
            } else if *accent {
                "X+ ".into()
            } else {
                "x+ ".into()
            }
        }
        Some(StepEvent::Trigger {
            accent,
            recipe,
            locks,
            ..
        }) if locks.is_empty() => {
            if recipe.get() > 1 {
                format!("{}{} ", if *accent { 'X' } else { 'x' }, recipe.get())
            } else if *accent {
                " X ".into()
            } else {
                " x ".into()
            }
        }
        Some(StepEvent::Trigger { accent, recipe, .. }) => {
            if recipe.get() > 1 {
                format!("{}*{}", if *accent { 'X' } else { 'x' }, recipe.get())
            } else if *accent {
                "X* ".into()
            } else {
                "x* ".into()
            }
        }
        Some(StepEvent::BassNote {
            degree,
            octave,
            accent,
            locks,
            ..
        })
        | Some(StepEvent::Note {
            degree,
            octave,
            accent,
            locks,
            ..
        })
        | Some(StepEvent::LeadNote {
            degree,
            octave,
            accent,
            locks,
            ..
        }) => {
            let octave = octave_superscript(*octave);
            match (*accent, locks.is_empty()) {
                // The trailing space creates a gutter between adjacent normal
                // note chips while retaining the fixed three-column cell.
                (false, true) => format!("{degree}{octave} "),
                (true, true) => format!("{degree}{octave}!"),
                (false, false) => format!("{degree}{octave}*"),
                (true, false) => format!("{degree}{octave}#"),
            }
        }
        Some(StepEvent::Tie { locks }) if locks.is_empty() => " ─ ".into(),
        Some(StepEvent::Tie { .. }) => "─* ".into(),
    }
}

pub(super) fn selected_accent(a: &App, track: usize) -> Option<(bool, Option<usize>)> {
    let t = a.editor.project.tracks.get(track)?;
    let steps = a.editor.active_steps(track)?;
    let Some(event) = steps[a.step].as_ref() else {
        return Some((t.input_accent, None));
    };
    if let Some(accent) = event.accent() {
        return Some((accent, None));
    }
    let source = crate::model::tie_source(steps, a.step)?;
    steps[source]
        .as_ref()?
        .accent()
        .map(|accent| (accent, Some(source)))
}

pub(super) fn articulation_title(a: &App, track: usize) -> String {
    match selected_accent(a, track) {
        Some((accent, None)) => {
            let steps = a.editor.active_steps(track).unwrap();
            let is_default = steps[a.step].is_none();
            let mut text = format!(
                "Accent {}{}",
                if is_default { "default " } else { "" },
                if accent { "on" } else { "off" }
            );
            if let Some(StepEvent::BassNote { slide, .. } | StepEvent::LeadNote { slide, .. }) =
                steps[a.step]
            {
                text.push_str(&format!(" · Slide {}", if slide { "on" } else { "off" }));
            }
            let kind = a.editor.project.tracks[track].kind;
            if matches!(kind, TrackKind::Hat | TrackKind::Tom)
                && let Some(recipe) = steps[a.step].as_ref().and_then(StepEvent::drum_recipe)
            {
                text.push_str(" · ");
                text.push_str(recipe_label(kind, recipe));
            }
            text
        }
        Some((accent, Some(source))) => {
            format!(
                "Accent {} from step {}",
                if accent { "on" } else { "off" },
                source + 1
            )
        }
        None => "Accent —".into(),
    }
}

pub(super) fn selected_chord_shape(a: &App, track: usize) -> Option<ChordShape> {
    let track_config = a.editor.project.tracks.get(track)?;
    if !track_config.kind.supports_voicing() {
        return None;
    }
    let default_shape = track_config.default_voicing_shape()?;
    let steps = a.editor.active_steps(track)?;
    match steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => Some(chord_shape.unwrap_or(default_shape)),
        Some(StepEvent::Tie { .. }) => crate::model::tie_source(steps, a.step).and_then(|source| {
            match steps[source].as_ref() {
                Some(StepEvent::Note { chord_shape, .. }) => {
                    Some(chord_shape.unwrap_or(default_shape))
                }
                _ => None,
            }
        }),
        None => track_config.input_voicing_shape(),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) struct ParameterDescriptor {
    pub(super) id: ParameterId,
    pub(super) label: &'static str,
    pub(super) shortcut: &'static str,
    pub(super) group: ParameterGroup,
}

#[derive(Clone, Copy)]
struct ParameterCardRender {
    active: bool,
    group_color: Color,
    style: Style,
    segment_count: u16,
    show_shortcut: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParameterGroup {
    Mixer,
    Instrument,
    Filter,
    Envelope,
    Distortion,
    BitCrusher,
    Phaser,
    Flanger,
    Chorus,
}

impl ParameterGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Mixer => "MIXER",
            Self::Instrument => "INSTRUMENT",
            Self::Filter => "FILTER",
            Self::Envelope => "ENVELOPE",
            Self::Distortion => "DISTORTION",
            Self::BitCrusher => "BIT CRUSHER",
            Self::Phaser => "PHASER",
            Self::Flanger => "FLANGER",
            Self::Chorus => "CHORUS",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::Mixer => Color::Cyan,
            Self::Instrument => Color::Green,
            Self::Filter => Color::Magenta,
            Self::Envelope => Color::Yellow,
            Self::Distortion => Color::Red,
            Self::BitCrusher => Color::LightMagenta,
            Self::Phaser => Color::Blue,
            Self::Flanger => Color::LightBlue,
            Self::Chorus => Color::LightGreen,
        }
    }
}

const LEVEL_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Level,
    label: "Level",
    shortcut: "v",
    group: ParameterGroup::Mixer,
};
const DELAY_SEND_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::DelaySend,
    label: "Delay",
    shortcut: "y",
    group: ParameterGroup::Mixer,
};
const REVERB_SEND_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::ReverbSend,
    label: "Reverb",
    shortcut: "b",
    group: ParameterGroup::Mixer,
};
const PAN_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Pan,
    label: "Pan",
    shortcut: "n",
    group: ParameterGroup::Mixer,
};
const TUNE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Tune,
    label: "Tune",
    shortcut: "u",
    group: ParameterGroup::Instrument,
};
const TONE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Tone,
    label: "Tone",
    shortcut: "t",
    group: ParameterGroup::Instrument,
};
const DECAY_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Decay,
    label: "Decay",
    shortcut: "d",
    group: ParameterGroup::Instrument,
};
const OSCILLATOR_MIX_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::OscillatorMix,
    label: "Osc Mix",
    shortcut: "w",
    group: ParameterGroup::Instrument,
};
const PULSE_WIDTH_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::PulseWidth,
    label: "Pulse W",
    shortcut: "P",
    group: ParameterGroup::Instrument,
};
const SUB_OSCILLATOR_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::SubOscillator,
    label: "Sub",
    shortcut: "u",
    group: ParameterGroup::Instrument,
};
const NOISE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Noise,
    label: "Noise",
    shortcut: "O",
    group: ParameterGroup::Instrument,
};
const PITCH_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Pitch,
    label: "Pitch",
    shortcut: "i",
    group: ParameterGroup::Instrument,
};
const CUTOFF_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Cutoff,
    label: "Cutoff",
    shortcut: "c",
    group: ParameterGroup::Filter,
};
const RESONANCE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Resonance,
    label: "Reson",
    shortcut: "R",
    group: ParameterGroup::Filter,
};
const FILTER_ENVELOPE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::FilterEnvelope,
    label: "Filt Env",
    shortcut: "f",
    group: ParameterGroup::Filter,
};
const ATTACK_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Attack,
    label: "Attack",
    shortcut: "a",
    group: ParameterGroup::Envelope,
};
const SYNTH_DECAY_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Decay,
    label: "Decay",
    shortcut: "d",
    group: ParameterGroup::Envelope,
};
const SUSTAIN_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Sustain,
    label: "Sustain",
    shortcut: "s",
    group: ParameterGroup::Envelope,
};
const RELEASE_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    id: ParameterId::Release,
    label: "Release",
    shortcut: "r",
    group: ParameterGroup::Envelope,
};

const KICK_PARAMETERS: [ParameterDescriptor; 7] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Attack,
        label: "Attack",
        shortcut: "a",
        group: ParameterGroup::Instrument,
    },
];

const SNARE_PARAMETERS: [ParameterDescriptor; 7] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Snappy,
        label: "Snappy",
        shortcut: "s",
        group: ParameterGroup::Instrument,
    },
];

const HAT_PARAMETERS: [ParameterDescriptor; 8] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
];

const TOM_PARAMETERS: [ParameterDescriptor; 13] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
];

const CYMBAL_PARAMETERS: [ParameterDescriptor; 7] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    TUNE_PARAMETER,
    TONE_PARAMETER,
    DECAY_PARAMETER,
];
const RIMSHOT_PARAMETERS: [ParameterDescriptor; 7] = CYMBAL_PARAMETERS;

const BASS_PARAMETERS: [ParameterDescriptor; 9] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::Waveform,
        label: "Wave",
        shortcut: "w",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Cutoff,
        label: "Cutoff",
        shortcut: "c",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Resonance,
        label: "Reson",
        shortcut: "R",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::FilterEnvelope,
        label: "Filt Env",
        shortcut: "f",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Envelope,
    },
];

const CHORD_PARAMETERS: [ParameterDescriptor; 16] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::OscillatorMix,
        label: "Osc Mix",
        shortcut: "w",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::PulseWidth,
        label: "Pulse W",
        shortcut: "P",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::SubOscillator,
        label: "Sub",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Noise,
        label: "Noise",
        shortcut: "O",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Pitch,
        label: "Pitch",
        shortcut: "i",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Cutoff,
        label: "Cutoff",
        shortcut: "c",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Resonance,
        label: "Reson",
        shortcut: "R",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::FilterEnvelope,
        label: "Filt Env",
        shortcut: "f",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Attack,
        label: "Attack",
        shortcut: "a",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Sustain,
        label: "Sustain",
        shortcut: "s",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Release,
        label: "Release",
        shortcut: "r",
        group: ParameterGroup::Envelope,
    },
];

const LEAD_PARAMETERS: [ParameterDescriptor; 19] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    OSCILLATOR_MIX_PARAMETER,
    PULSE_WIDTH_PARAMETER,
    SUB_OSCILLATOR_PARAMETER,
    NOISE_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::LeadSubMode,
        label: "Sub mode",
        shortcut: "U",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::PortamentoTime,
        label: "Porta",
        shortcut: "g",
        group: ParameterGroup::Instrument,
    },
    PITCH_PARAMETER,
    CUTOFF_PARAMETER,
    RESONANCE_PARAMETER,
    FILTER_ENVELOPE_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::KeyboardTracking,
        label: "KYBD",
        shortcut: "k",
        group: ParameterGroup::Filter,
    },
    ATTACK_PARAMETER,
    SYNTH_DECAY_PARAMETER,
    SUSTAIN_PARAMETER,
    RELEASE_PARAMETER,
];

const FM_PARAMETERS: [ParameterDescriptor; 15] = [
    LEVEL_PARAMETER,
    DELAY_SEND_PARAMETER,
    REVERB_SEND_PARAMETER,
    PAN_PARAMETER,
    ParameterDescriptor {
        id: ParameterId::FmAlgorithm,
        label: "Algo",
        shortcut: "q",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::FmOp1Level,
        label: "OP1",
        shortcut: "O",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::FmOp2Level,
        label: "OP2",
        shortcut: "O",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::FmOp3Level,
        label: "OP3",
        shortcut: "O",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::FmOp4Level,
        label: "OP4",
        shortcut: "O",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Brightness,
        label: "Bright",
        shortcut: "c",
        group: ParameterGroup::Filter,
    },
    PITCH_PARAMETER,
    ATTACK_PARAMETER,
    SYNTH_DECAY_PARAMETER,
    SUSTAIN_PARAMETER,
    RELEASE_PARAMETER,
];

pub(super) fn parameter_descriptors(kind: TrackKind) -> &'static [ParameterDescriptor] {
    match kind {
        TrackKind::Kick => &KICK_PARAMETERS,
        TrackKind::Snare => &SNARE_PARAMETERS,
        TrackKind::Hat => &HAT_PARAMETERS,
        TrackKind::Tom => &TOM_PARAMETERS,
        TrackKind::Cymbal => &CYMBAL_PARAMETERS,
        TrackKind::Rimshot => &RIMSHOT_PARAMETERS,
        TrackKind::Bass => &BASS_PARAMETERS,
        TrackKind::Chord => &CHORD_PARAMETERS,
        TrackKind::Lead => &LEAD_PARAMETERS,
        TrackKind::Fm => &FM_PARAMETERS,
    }
}

const EFFECT_PARAMETERS: [ParameterDescriptor; 16] = [
    ParameterDescriptor {
        id: ParameterId::DistortionDrive,
        label: "Drive",
        shortcut: "d",
        group: ParameterGroup::Distortion,
    },
    ParameterDescriptor {
        id: ParameterId::DistortionTone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Distortion,
    },
    ParameterDescriptor {
        id: ParameterId::DistortionMix,
        label: "Mix",
        shortcut: "x",
        group: ParameterGroup::Distortion,
    },
    ParameterDescriptor {
        id: ParameterId::BitCrusherBits,
        label: "Bits",
        shortcut: "b",
        group: ParameterGroup::BitCrusher,
    },
    ParameterDescriptor {
        id: ParameterId::BitCrusherRate,
        label: "Rate",
        shortcut: "s",
        group: ParameterGroup::BitCrusher,
    },
    ParameterDescriptor {
        id: ParameterId::BitCrusherMix,
        label: "Mix",
        shortcut: "m",
        group: ParameterGroup::BitCrusher,
    },
    ParameterDescriptor {
        id: ParameterId::PhaserRate,
        label: "Rate",
        shortcut: "r",
        group: ParameterGroup::Phaser,
    },
    ParameterDescriptor {
        id: ParameterId::PhaserDepth,
        label: "Depth",
        shortcut: "e",
        group: ParameterGroup::Phaser,
    },
    ParameterDescriptor {
        id: ParameterId::PhaserFeedback,
        label: "Feedbk",
        shortcut: "f",
        group: ParameterGroup::Phaser,
    },
    ParameterDescriptor {
        id: ParameterId::PhaserMix,
        label: "Mix",
        shortcut: "M",
        group: ParameterGroup::Phaser,
    },
    ParameterDescriptor {
        id: ParameterId::FlangerRate,
        label: "Rate",
        shortcut: "R",
        group: ParameterGroup::Flanger,
    },
    ParameterDescriptor {
        id: ParameterId::FlangerDelay,
        label: "Delay",
        shortcut: "q",
        group: ParameterGroup::Flanger,
    },
    ParameterDescriptor {
        id: ParameterId::FlangerDepth,
        label: "Depth",
        shortcut: "E",
        group: ParameterGroup::Flanger,
    },
    ParameterDescriptor {
        id: ParameterId::FlangerFeedback,
        label: "Feedbk",
        shortcut: "F",
        group: ParameterGroup::Flanger,
    },
    ParameterDescriptor {
        id: ParameterId::FlangerMix,
        label: "Mix",
        shortcut: "N",
        group: ParameterGroup::Flanger,
    },
    ParameterDescriptor {
        id: ParameterId::Chorus,
        label: "Chorus",
        shortcut: "h",
        group: ParameterGroup::Chorus,
    },
];

pub(super) fn effect_descriptors() -> &'static [ParameterDescriptor] {
    &EFFECT_PARAMETERS
}

pub(super) fn visible_parameter_descriptors(
    bank: ParameterBank,
    kind: TrackKind,
) -> &'static [ParameterDescriptor] {
    match bank {
        ParameterBank::Params => parameter_descriptors(kind),
        ParameterBank::Effects => effect_descriptors(),
    }
}

pub(super) fn parameter_recipe(kind: TrackKind, index: usize) -> crate::model::DrumRecipeSlot {
    use crate::model::DrumRecipeSlot;
    match kind {
        TrackKind::Hat if index >= 6 => DrumRecipeSlot::TWO,
        TrackKind::Tom if (7..10).contains(&index) => DrumRecipeSlot::TWO,
        TrackKind::Tom if index >= 10 => DrumRecipeSlot::THREE,
        _ => DrumRecipeSlot::ONE,
    }
}

pub(super) fn is_recipe_parameter(kind: TrackKind, parameter: ParameterId) -> bool {
    match kind {
        TrackKind::Hat => matches!(parameter, ParameterId::Tune | ParameterId::Decay),
        TrackKind::Tom => matches!(
            parameter,
            ParameterId::Tune | ParameterId::Tone | ParameterId::Decay
        ),
        _ => false,
    }
}

pub(super) fn recipe_label(kind: TrackKind, recipe: crate::model::DrumRecipeSlot) -> &'static str {
    use crate::model::DrumRecipeSlot;
    match (kind, recipe) {
        (TrackKind::Hat, DrumRecipeSlot::ONE) => "CLOSED",
        (TrackKind::Hat, DrumRecipeSlot::TWO) => "OPEN",
        (TrackKind::Tom, DrumRecipeSlot::ONE) => "LOW",
        (TrackKind::Tom, DrumRecipeSlot::TWO) => "MEDIUM",
        (TrackKind::Tom, DrumRecipeSlot::THREE) => "HIGH",
        _ => "INSTRUMENT",
    }
}

pub(super) fn selected_drum_recipe(a: &App, track: usize) -> crate::model::DrumRecipeSlot {
    a.editor
        .active_steps(track)
        .unwrap()
        .get(a.step)
        .and_then(Option::as_ref)
        .and_then(StepEvent::drum_recipe)
        .unwrap_or(crate::model::DrumRecipeSlot::ONE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueOrigin {
    Base,
    Lock,
}

pub(super) fn displayed_parameter(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> Option<(ParameterValue, ValueOrigin)> {
    displayed_parameter_for_recipe(a, track, step, parameter, crate::model::DrumRecipeSlot::ONE)
}

pub(super) fn displayed_parameter_for_recipe(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
    recipe: crate::model::DrumRecipeSlot,
) -> Option<(ParameterValue, ValueOrigin)> {
    let kind = a.editor.project.tracks.get(track)?.kind;
    let recipe_parameter = is_recipe_parameter(kind, parameter);
    let base = if recipe_parameter {
        a.editor
            .drum_recipe_parameter_value(track, step, Scope::Base, recipe, parameter)
            .ok()?
    } else {
        a.editor
            .parameter_value(track, step, Scope::Base, parameter)
            .ok()?
    };
    if a.scope == Scope::Base {
        let value = match base {
            ParameterValue::Percent(target) => ParameterValue::Percent(a.animated_percent(
                track,
                step,
                parameter,
                recipe,
                ValueOrigin::Base,
                target,
            )),
            value => value,
        };
        return Some((value, ValueOrigin::Base));
    }
    let locked = a
        .editor
        .active_steps(track)
        .and_then(|steps| crate::model::effective_event_locks(steps, step))
        .is_some_and(|locks| locks.get(parameter).is_some());
    if recipe_parameter && selected_drum_recipe(a, track) != recipe {
        return Some((base, ValueOrigin::Base));
    }
    let value = if recipe_parameter {
        a.editor
            .drum_recipe_parameter_value(track, step, Scope::Lock, recipe, parameter)
            .unwrap_or(base)
    } else {
        a.editor
            .parameter_value(track, step, Scope::Lock, parameter)
            .unwrap_or(base)
    };
    let origin = if locked {
        ValueOrigin::Lock
    } else {
        ValueOrigin::Base
    };
    let value = match value {
        ParameterValue::Percent(target) => ParameterValue::Percent(
            a.animated_percent(track, step, parameter, recipe, origin, target),
        ),
        value => value,
    };
    Some((value, origin))
}

pub(super) fn fader_segments(value: u8) -> usize {
    fader_segments_for(value, 10)
}

pub(super) fn fader_segments_for(value: u8, segment_count: u16) -> usize {
    ((usize::from(value) * usize::from(segment_count) + 50) / 100).min(usize::from(segment_count))
}

pub(super) fn physical_parameter_readout(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> String {
    if parameter == ParameterId::Pitch {
        return a.editor.project.tracks[track]
            .lfos
            .get(parameter)
            .map(|config| {
                format!(
                    "{}% · ±{:.1} semitones · LFO",
                    config.depth.get(),
                    config.depth.get() as f32 * 0.02
                )
            })
            .unwrap_or_else(|| "unassigned · LFO".into());
    }
    let Some((value, origin)) =
        displayed_parameter_for_recipe(a, track, step, parameter, a.parameter_recipe)
    else {
        return "unavailable".into();
    };
    let origin = match origin {
        ValueOrigin::Base => "BASE",
        ValueOrigin::Lock => "LOCK",
    };
    let flanger_geometry = || {
        let percent = |id| match displayed_parameter(a, track, step, id) {
            Some((ParameterValue::Percent(value), _)) => value.get() as f32,
            _ => 0.0,
        };
        crate::dsp::flanger_delay_geometry(
            percent(ParameterId::FlangerDelay),
            percent(ParameterId::FlangerDepth),
        )
    };
    let physical = match value {
        ParameterValue::Waveform(Waveform::Square) => "Square".into(),
        ParameterValue::Waveform(Waveform::Saw) => "Saw".into(),
        ParameterValue::Chorus(ChorusMode::Off) => "Off".into(),
        ParameterValue::Chorus(ChorusMode::I) => "Mode I".into(),
        ParameterValue::Chorus(ChorusMode::Ii) => "Mode II".into(),
        ParameterValue::LeadSubMode(value) => format!("{value:?}"),
        ParameterValue::FmAlgorithm(value) => format!("{value} · {}", value.diagram()),
        ParameterValue::FmRatio(value) => format!("{value}:1"),
        ParameterValue::Percent(value) => {
            let value = value.get();
            match (a.editor.project.tracks[track].kind, parameter) {
                (_, ParameterId::DistortionDrive) => {
                    format!(
                        "{:.1}× pre-gain",
                        crate::dsp::exp_map(value, 1.0, 31.622_776)
                    )
                }
                (_, ParameterId::DistortionTone) => {
                    format!(
                        "{:.0} Hz low-pass",
                        crate::dsp::exp_map(value, 700.0, 18_000.0)
                    )
                }
                (_, ParameterId::DistortionMix) => format!("{value}% wet"),
                (_, ParameterId::BitCrusherBits) => {
                    format!(
                        "{}-bit quantization",
                        crate::dsp::bit_crusher_bit_depth(value as f32)
                    )
                }
                (_, ParameterId::BitCrusherRate) => format!(
                    "÷{:.1} sample rate",
                    crate::dsp::bit_crusher_rate_divisor(value as f32)
                ),
                (_, ParameterId::BitCrusherMix) => format!("{value}% wet"),
                (_, ParameterId::PhaserRate) => {
                    format!("{:.2} Hz", crate::dsp::exp_map(value, 0.05, 8.0))
                }
                (_, ParameterId::PhaserDepth) => {
                    format!(
                        "{:.0}–{:.0} Hz sweep",
                        300.0,
                        crate::dsp::exp_map(value, 300.0, 8_000.0)
                    )
                }
                (_, ParameterId::PhaserFeedback) => format!("{}% feedback", value),
                (_, ParameterId::PhaserMix) => format!("{value}% wet"),
                (_, ParameterId::FlangerRate) => {
                    format!("{:.2} Hz", crate::dsp::exp_map(value, 0.05, 8.0))
                }
                (_, ParameterId::FlangerDelay) => {
                    let (center, _, low, high) = flanger_geometry();
                    format!("{center:.1} ms center · {low:.1}–{high:.1} ms range")
                }
                (_, ParameterId::FlangerDepth) => {
                    let (_, depth, low, high) = flanger_geometry();
                    format!("±{depth:.1} ms effective · {low:.1}–{high:.1} ms range")
                }
                (_, ParameterId::FlangerFeedback) => format!("{}% feedback", value),
                (_, ParameterId::FlangerMix) => format!("{value}% wet"),
                (TrackKind::Kick, ParameterId::Tune) => format!(
                    "peak {:.0} Hz · fundamental {:.0} Hz",
                    110.0 + value as f32 * 1.70,
                    45.0 + value as f32 * 0.25
                ),
                (TrackKind::Kick, ParameterId::Decay) => {
                    format!("{:.0} ms", crate::dsp::exp_map(value, 80.0, 1_200.0))
                }
                (TrackKind::Snare, ParameterId::Tone) => format!(
                    "body {:.0} Hz · noise {:.0} Hz",
                    145.0 + value as f32 * 1.7,
                    800.0 + value as f32 * 52.0
                ),
                (TrackKind::Snare, ParameterId::Tune) => {
                    format!("{:.0} Hz body", 150.0 + value as f32 * 1.5)
                }
                (TrackKind::Hat, ParameterId::Tune) => {
                    format!("{:.0} Hz source", 310.0 + value as f32 * 3.6)
                }
                (TrackKind::Hat, ParameterId::Decay) => {
                    format!("{:.0} ms", 25.0 * 32.0_f32.powf(value as f32 / 100.0))
                }
                (TrackKind::Tom, ParameterId::Tune) => format!(
                    "peak {:.0} Hz · body {:.0} Hz",
                    180.0 + value as f32 * 2.4,
                    80.0 + value as f32 * 1.4
                ),
                (TrackKind::Tom, ParameterId::Tone) => {
                    format!("lower {:.0}% · upper {:.0}%", 100 - value, value)
                }
                (TrackKind::Tom, ParameterId::Decay) => {
                    format!("{:.0} ms", 90.0 * 8.888_889_f32.powf(value as f32 / 100.0))
                }
                (TrackKind::Cymbal, ParameterId::Tune) => {
                    format!("{:.0} Hz source", 240.0 + value as f32 * 4.8)
                }
                (TrackKind::Cymbal, ParameterId::Tone) => {
                    format!("{:.0}% metallic · {:.0}% noise", 100 - value, value)
                }
                (TrackKind::Cymbal, ParameterId::Decay) => {
                    format!("{:.0} ms", 80.0 * 22.5_f32.powf(value as f32 / 100.0))
                }
                (TrackKind::Rimshot, ParameterId::Tune) => {
                    let multiplier = 2.0_f32.powf(value as f32 / 50.0 - 1.0);
                    format!(
                        "{:.0}/{:.0}/{:.0} Hz modes",
                        222.0 * multiplier,
                        500.0 * multiplier,
                        1_000.0 * multiplier
                    )
                }
                (TrackKind::Rimshot, ParameterId::Tone) => {
                    format!("{}% body · {}% crack", 100 - value, value)
                }
                (TrackKind::Rimshot, ParameterId::Decay) => {
                    let multiplier = 4.0_f32.powf(value as f32 / 50.0 - 1.0);
                    format!("{:.0} ms longest mode", 45.0 * multiplier)
                }
                (TrackKind::Bass, ParameterId::Cutoff) => {
                    format!("{:.0} Hz", crate::dsp::exp_map(value, 80.0, 8_000.0))
                }
                (TrackKind::Bass, ParameterId::Resonance) => {
                    format!("Q {:.2}", 0.707 + value as f32 / 100.0 * (14.0 - 0.707))
                }
                (TrackKind::Bass, ParameterId::Decay) => {
                    format!("{:.0} ms", crate::dsp::exp_map(value, 80.0, 2_000.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::Cutoff) => {
                    format!("{:.0} Hz", crate::dsp::exp_map(value, 20.0, 20_000.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::Resonance) => {
                    format!("{}% feedback", value)
                }
                (TrackKind::Chord, ParameterId::Attack) => {
                    let seconds = if value == 0 {
                        0.0
                    } else {
                        crate::dsp::exp_map(value, 0.001, 3.0)
                    };
                    format!("{seconds:.3} s")
                }
                (TrackKind::Lead, ParameterId::Attack) => {
                    let seconds = if value == 0 {
                        0.0
                    } else {
                        crate::dsp::exp_map(value, 0.0015, 4.0)
                    };
                    format!("{seconds:.3} s")
                }
                (TrackKind::Chord, ParameterId::Decay | ParameterId::Release) => {
                    format!("{:.3} s", crate::dsp::exp_map(value, 0.002, 12.0))
                }
                (TrackKind::Lead, ParameterId::Decay | ParameterId::Release) => {
                    format!("{:.3} s", crate::dsp::exp_map(value, 0.002, 10.0))
                }
                (TrackKind::Fm, parameter)
                    if matches!(
                        parameter.fm_operator_field(),
                        Some((_, crate::model::FmOperatorField::Level))
                    ) =>
                {
                    let operator = parameter.fm_operator_field().unwrap().0;
                    let algorithm =
                        match displayed_parameter(a, track, step, ParameterId::FmAlgorithm) {
                            Some((ParameterValue::FmAlgorithm(value), _)) => value,
                            _ => FmAlgorithm::default(),
                        };
                    if algorithm.is_carrier(operator) {
                        format!("{}% carrier gain", value)
                    } else {
                        format!("index {:.2} rad", 12.0 * (value as f32 / 100.0).powi(2))
                    }
                }
                (TrackKind::Fm, parameter)
                    if matches!(
                        parameter.fm_operator_field(),
                        Some((_, crate::model::FmOperatorField::Feedback))
                    ) =>
                {
                    format!("{:.2}π rad", 0.95 * value as f32 / 100.0)
                }
                (TrackKind::Fm, ParameterId::Brightness) => {
                    format!("{:.0} Hz", crate::dsp::exp_map(value, 200.0, 20_000.0))
                }
                (TrackKind::Fm, ParameterId::Attack) => {
                    let seconds = if value == 0 {
                        0.0
                    } else {
                        crate::dsp::exp_map(value, 0.0015, 4.0)
                    };
                    format!("{seconds:.3} s")
                }
                (TrackKind::Fm, ParameterId::Decay | ParameterId::Release) => {
                    format!("{:.3} s", crate::dsp::exp_map(value, 0.002, 10.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::OscillatorMix) => {
                    format!("Pulse {}% · Saw {}%", 100 - value, value)
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::PulseWidth) => {
                    format!("{:.0}% duty", 5.0 + value as f32 * 0.9)
                }
                _ => format!("{value}%"),
            }
        }
    };
    format!("{physical} · {origin}")
}

pub(super) fn global_shortcut_text(id: GlobalParameterId) -> &'static str {
    global_control(id).shortcut
}

pub(super) fn global_value_text(g: &crate::model::Globals, id: GlobalParameterId) -> String {
    match id {
        GlobalParameterId::Tempo => format!("{} BPM", g.tempo_bpm),
        GlobalParameterId::DelayDivision => g.delay_division.to_string(),
        GlobalParameterId::DelayFeedback => format!("{}%", g.delay_feedback.get()),
        GlobalParameterId::ReverbTime => format!("{:.1} s", g.reverb_time_seconds),
        GlobalParameterId::ReverbTone => format!("{}%", g.reverb_tone.get()),
        GlobalParameterId::ReverbPreDelay => format!("{} ms", g.reverb_pre_delay_ms),
        GlobalParameterId::ReverbReturn => format!("{}%", g.reverb_return.get()),
        GlobalParameterId::Ducking => {
            if g.sidechain.depth == crate::model::Percent::ZERO {
                "Off".into()
            } else {
                format!("{}%", g.sidechain.depth.get())
            }
        }
        GlobalParameterId::Key => g.key.to_string(),
        GlobalParameterId::Scale => g.scale.to_string(),
    }
}

fn global_fader_value(g: &crate::model::Globals, id: GlobalParameterId) -> Option<f32> {
    match id {
        GlobalParameterId::DelayFeedback => Some(g.delay_feedback.get() as f32),
        GlobalParameterId::ReverbTime => Some(g.reverb_time_seconds),
        GlobalParameterId::ReverbTone => Some(g.reverb_tone.get() as f32),
        GlobalParameterId::ReverbPreDelay => Some(g.reverb_pre_delay_ms as f32),
        GlobalParameterId::ReverbReturn => Some(g.reverb_return.get() as f32),
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayDivision
        | GlobalParameterId::Key
        | GlobalParameterId::Scale
        | GlobalParameterId::Ducking => None,
    }
}

fn global_fader_bounds(id: GlobalParameterId) -> Option<(f32, f32)> {
    match id {
        GlobalParameterId::DelayFeedback => Some((0.0, 95.0)),
        GlobalParameterId::ReverbTime => Some((0.2, 10.0)),
        GlobalParameterId::ReverbTone => Some((0.0, 100.0)),
        GlobalParameterId::ReverbPreDelay => Some((0.0, 200.0)),
        GlobalParameterId::ReverbReturn => Some((0.0, 100.0)),
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayDivision
        | GlobalParameterId::Key
        | GlobalParameterId::Scale
        | GlobalParameterId::Ducking => None,
    }
}

pub(super) fn global_fader_segments(
    g: &crate::model::Globals,
    id: GlobalParameterId,
) -> Option<usize> {
    let value = global_fader_value(g, id)?;
    let (minimum, maximum) = global_fader_bounds(id)?;
    let normalized = if value.is_finite() {
        ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((normalized * 10.0).round() as usize)
}

pub(super) fn global_selector_data(
    g: &crate::model::Globals,
    id: GlobalParameterId,
) -> Option<(Vec<String>, usize)> {
    match id {
        GlobalParameterId::DelayDivision => Some((
            DelayDivision::ALL.iter().map(ToString::to_string).collect(),
            DelayDivision::ALL
                .iter()
                .position(|division| *division == g.delay_division)
                .unwrap_or_default(),
        )),
        GlobalParameterId::Key => Some((
            PitchClass::ALL.iter().map(ToString::to_string).collect(),
            PitchClass::ALL
                .iter()
                .position(|key| *key == g.key)
                .unwrap_or_default(),
        )),
        GlobalParameterId::Scale => Some((
            Scale::ALL.iter().map(ToString::to_string).collect(),
            Scale::ALL
                .iter()
                .position(|scale| *scale == g.scale)
                .unwrap_or_default(),
        )),
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayFeedback
        | GlobalParameterId::ReverbTime
        | GlobalParameterId::ReverbTone
        | GlobalParameterId::ReverbPreDelay
        | GlobalParameterId::ReverbReturn
        | GlobalParameterId::Ducking => None,
    }
}

pub(super) fn global_control_text(g: &crate::model::Globals) -> Vec<String> {
    GLOBAL_CONTROLS
        .iter()
        .map(|control| format!("{} {}", control.label, global_value_text(g, control.id)))
        .collect()
}

pub(super) fn render_global_cards(f: &mut ratatui::Frame, area: Rect, a: &App) {
    let panel = Block::bordered().title("Global detail");
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let g = &a.editor.project.globals;

    let slot_width = (inner.width / GLOBAL_CONTROLS.len() as u16).min(10);
    let bank_width = slot_width.saturating_mul(GLOBAL_CONTROLS.len() as u16);
    let bank = Rect {
        x: inner.x + inner.width.saturating_sub(bank_width) / 2,
        y: inner.y + 1,
        width: bank_width,
        height: inner.height.saturating_sub(1),
    };

    let mut group_start = 0;
    while group_start < GLOBAL_CONTROLS.len() {
        let group = GLOBAL_CONTROLS[group_start].group;
        let group_end = GLOBAL_CONTROLS[group_start..]
            .iter()
            .position(|control| control.group != group)
            .map(|offset| group_start + offset)
            .unwrap_or(GLOBAL_CONTROLS.len());
        let group_area = Rect {
            x: bank.x + slot_width * group_start as u16,
            y: inner.y,
            width: slot_width * (group_end - group_start) as u16,
            height: 1,
        };
        render_centered(
            f,
            group.label(),
            group_area,
            Style::default()
                .fg(group.color())
                .add_modifier(Modifier::BOLD),
        );
        group_start = group_end;
    }

    for (index, control) in GLOBAL_CONTROLS.iter().enumerate() {
        let id = control.id;
        let slot = Rect {
            x: bank.x + slot_width * index as u16,
            y: bank.y,
            width: slot_width,
            height: bank.height,
        };
        let active = matches!(&a.mode, Mode::GlobalEdit(active_id) if *active_id == id)
            || matches!(&a.mode, Mode::TempoInput(_) if id == GlobalParameterId::Tempo)
            || matches!(&a.mode, Mode::SidechainEdit { .. } if id == GlobalParameterId::Ducking);
        let block = if active {
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Double)
                .border_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().reversed())
        } else {
            Block::default()
        };
        let content = if active {
            block.inner(slot)
        } else {
            Rect {
                x: slot.x + 1,
                y: slot.y,
                width: slot.width.saturating_sub(2),
                height: slot.height,
            }
        };
        f.render_widget(block, slot);
        let group_color = control.group.color();
        let style = if active {
            Style::default()
                .fg(group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(group_color)
                .add_modifier(Modifier::BOLD)
        };
        if let Some(filled) = global_fader_segments(g, id) {
            render_centered(
                f,
                &global_value_text(g, id),
                Rect {
                    height: 1,
                    ..content
                },
                style,
            );
            for segment in 0..10 {
                let is_filled = usize::from(segment) >= 10usize.saturating_sub(filled);
                let segment_style = if active || is_filled {
                    style
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                render_centered(
                    f,
                    if is_filled { "███" } else { "···" },
                    Rect {
                        x: content.x,
                        y: content.y + 1 + segment,
                        width: content.width,
                        height: 1,
                    },
                    segment_style,
                );
            }
        } else if let Some((choices, selected)) = global_selector_data(g, id) {
            render_lfo_selector(
                f,
                Rect {
                    height: 11,
                    ..content
                },
                &choices,
                selected,
                style,
            );
        } else {
            render_centered(
                f,
                &global_value_text(g, id),
                Rect {
                    height: 1,
                    ..content
                },
                style,
            );
        }
        render_centered(
            f,
            control.detail_label,
            Rect {
                y: content.y + 11,
                height: 1,
                ..content
            },
            style,
        );
        render_centered(
            f,
            &format!("[{}]", global_shortcut_text(id)),
            Rect {
                y: content.y + 12,
                height: 1,
                ..content
            },
            style,
        );
    }
}

pub(super) fn render_parameter_bank(f: &mut ratatui::Frame, area: Rect, a: &App, track: usize) {
    let t = &a.editor.project.tracks[track];
    let parameter_editing = matches!(a.mode, Mode::ParameterEdit(_));
    let lock_editing = a.scope == Scope::Lock && parameter_editing;
    let compact = !parameter_editing;
    let segment_count = if compact { 5 } else { 10 };
    let descriptors = visible_parameter_descriptors(a.parameter_bank, t.kind);
    let chord_shape = a
        .editor
        .chord_shape_value(track, a.step)
        .ok()
        .or_else(|| selected_chord_shape(a, track))
        .map(|shape| {
            let mode = if shape == ChordShape::Single {
                "MONO"
            } else {
                "CHORD"
            };
            format!(" · Voicing {shape} {mode}")
        })
        .unwrap_or_default();
    let context = if matches!(
        t.kind,
        TrackKind::Bass | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
    ) {
        format!(
            "{} · Step {} · {}{} · {} · Mute {}",
            track_label(t),
            a.step + 1,
            articulation_title(a, track),
            chord_shape,
            scope_name(a.scope),
            if t.muted { "on" } else { "off" }
        )
    } else {
        format!(
            "{} · Step {} · {} · {} · Mute {}",
            t.name,
            a.step + 1,
            articulation_title(a, track),
            scope_name(a.scope),
            if t.muted { "on" } else { "off" }
        )
    };
    let selected_tab_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);
    let inactive_tab_style = Style::default().fg(Color::DarkGray);
    let editing_badge = if lock_editing {
        Span::styled(
            " LOCK PARAMETER EDITING ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if parameter_editing {
        Span::styled(
            " PARAMETER EDITING ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" TRACK DETAIL ", Style::default().fg(Color::DarkGray))
    };
    let panel = Block::bordered()
        .border_style(if lock_editing {
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        })
        .title(Line::from(vec![
            Span::styled(
                if a.parameter_bank == ParameterBank::Params {
                    "[PARAMS]"
                } else {
                    " PARAMS "
                },
                if a.parameter_bank == ParameterBank::Params {
                    selected_tab_style
                } else {
                    inactive_tab_style
                },
            ),
            Span::raw(" "),
            Span::styled(
                if a.parameter_bank == ParameterBank::Effects {
                    "[EFFECTS]"
                } else {
                    " EFFECTS "
                },
                if a.parameter_bank == ParameterBank::Effects {
                    selected_tab_style
                } else {
                    inactive_tab_style
                },
            ),
            Span::raw(" · "),
            editing_badge,
            Span::raw(format!(" · {context}")),
        ]))
        .title_bottom(
            "LOCK values show effective output · LOCK = step override · BASE = inherited/base",
        );
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    let slot_width = (inner.width / descriptors.len() as u16).min(10);
    let bank_width = slot_width.saturating_mul(descriptors.len() as u16);
    let bank = Rect {
        x: inner.x + inner.width.saturating_sub(bank_width) / 2,
        y: inner.y + 1,
        width: bank_width,
        height: inner.height.saturating_sub(1),
    };

    let mut group_start = 0;
    while group_start < descriptors.len() {
        let group = descriptors[group_start].group;
        let group_recipe = parameter_recipe(t.kind, group_start);
        let group_end = descriptors[group_start..]
            .iter()
            .enumerate()
            .position(|(offset, descriptor)| {
                descriptor.group != group
                    || (group == ParameterGroup::Instrument
                        && parameter_recipe(t.kind, group_start + offset) != group_recipe)
            })
            .map(|offset| group_start + offset)
            .unwrap_or(descriptors.len());
        let group_area = Rect {
            x: bank.x + slot_width * group_start as u16,
            y: inner.y,
            width: slot_width * (group_end - group_start) as u16,
            height: 1,
        };
        let group_enabled = a.scope == Scope::Base
            || group != ParameterGroup::Instrument
            || !matches!(t.kind, TrackKind::Hat | TrackKind::Tom)
            || selected_drum_recipe(a, track) == group_recipe;
        render_centered(
            f,
            if group == ParameterGroup::Instrument
                && matches!(t.kind, TrackKind::Hat | TrackKind::Tom)
            {
                recipe_label(t.kind, group_recipe)
            } else {
                group.label()
            },
            group_area,
            Style::default()
                .fg(if group_enabled {
                    group.color()
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        );
        group_start = group_end;
    }

    for (index, descriptor) in descriptors.iter().enumerate() {
        let recipe = parameter_recipe(t.kind, index);
        let enabled = a.scope == Scope::Base
            || !is_recipe_parameter(t.kind, descriptor.id)
            || selected_drum_recipe(a, track) == recipe;
        let slot = Rect {
            x: bank.x + slot_width * index as u16,
            y: bank.y,
            width: slot_width,
            height: bank.height,
        };
        let active = matches!(
            a.mode,
            Mode::ParameterEdit(parameter)
                if parameter == descriptor.id
                    && (!is_recipe_parameter(t.kind, descriptor.id)
                        || a.parameter_recipe == recipe)
        ) || matches!(
            a.mode,
            Mode::LfoEdit { parameter, .. }
                if parameter == descriptor.id
                    && (!is_recipe_parameter(t.kind, descriptor.id)
                        || a.parameter_recipe == recipe)
        );
        let group_color = descriptor.group.color();
        let block = if active {
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Double)
                .border_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().reversed())
        } else {
            Block::default()
        };
        let content = if active {
            block.inner(slot)
        } else {
            Rect {
                x: slot.x + 1,
                y: slot.y,
                width: slot.width.saturating_sub(2),
                height: slot.height,
            }
        };
        f.render_widget(block, slot);
        if t.kind == TrackKind::Fm
            && let Some((operator, crate::model::FmOperatorField::Level)) =
                descriptor.id.fm_operator_field()
        {
            render_fm_operator_summary(f, content, a, track, operator, descriptor, active, compact);
            continue;
        }
        let style = if active {
            Style::default()
                .fg(group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else if !enabled {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
                .fg(group_color)
                .add_modifier(Modifier::BOLD)
        };
        if descriptor.id == ParameterId::Pitch {
            render_pitch_lfo_card(
                f,
                content,
                t,
                descriptor,
                ParameterCardRender {
                    active,
                    group_color,
                    style,
                    segment_count,
                    show_shortcut: !compact,
                },
            );
            continue;
        }
        let Some((value, origin)) =
            displayed_parameter_for_recipe(a, track, a.step, descriptor.id, recipe)
        else {
            continue;
        };
        let value_label = match value {
            ParameterValue::Percent(value) => format!("{}%", value.get()),
            ParameterValue::Waveform(Waveform::Square) => "SQR".into(),
            ParameterValue::Waveform(Waveform::Saw) => "SAW".into(),
            ParameterValue::Chorus(ChorusMode::Off) => "OFF".into(),
            ParameterValue::Chorus(ChorusMode::I) => "I".into(),
            ParameterValue::Chorus(ChorusMode::Ii) => "II".into(),
            ParameterValue::LeadSubMode(value) => format!("{value:?}"),
            ParameterValue::FmAlgorithm(value) => value.to_string(),
            ParameterValue::FmRatio(value) => format!("{value}:1"),
        };
        let has_lfo = t.lfos.get(descriptor.id).is_some();
        let compact_origin = match origin {
            ValueOrigin::Base => "B",
            ValueOrigin::Lock => "L",
        };
        if compact {
            let origin_style = if origin == ValueOrigin::Lock {
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD)
            } else {
                style
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(value_label.clone(), style),
                    Span::styled(compact_origin, origin_style),
                    Span::styled(
                        if has_lfo { "~" } else { "" },
                        Style::default()
                            .fg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
                .alignment(Alignment::Center),
                Rect {
                    x: content.x,
                    y: content.y,
                    width: content.width,
                    height: 1,
                },
            );
        } else {
            render_centered(f, &value_label, content, style);
        }
        for segment in 0..segment_count {
            let segment_area = Rect {
                x: content.x,
                y: content.y + 1 + segment,
                width: content.width,
                height: 1,
            };
            let symbol = match value {
                ParameterValue::Percent(value) => {
                    let filled = fader_segments_for(value.get(), segment_count);
                    if usize::from(segment) >= usize::from(segment_count) - filled {
                        "███"
                    } else {
                        "···"
                    }
                }
                ParameterValue::Waveform(Waveform::Saw) => {
                    if segment == 0 {
                        "●"
                    } else if segment == segment_count - 1 {
                        "○"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Waveform(Waveform::Square) => {
                    if segment == 0 {
                        "○"
                    } else if segment == segment_count - 1 {
                        "●"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Chorus(mode) => {
                    let selected = match mode {
                        ChorusMode::Off => segment_count - 1,
                        ChorusMode::I => segment_count / 2,
                        ChorusMode::Ii => 0,
                    };
                    if segment == selected { "●" } else { "│" }
                }
                ParameterValue::LeadSubMode(mode) => {
                    let selected = match mode {
                        crate::model::LeadSubMode::OneOctaveSquare => segment_count - 1,
                        crate::model::LeadSubMode::TwoOctaveSquare => segment_count / 2,
                        crate::model::LeadSubMode::TwoOctaveNarrowPulse => 0,
                    };
                    if segment == selected { "●" } else { "│" }
                }
                ParameterValue::FmAlgorithm(mode) => {
                    let index = FmAlgorithm::ALL
                        .iter()
                        .position(|choice| *choice == mode)
                        .unwrap_or(0);
                    let selected = ((FmAlgorithm::ALL.len() - 1 - index)
                        * usize::from(segment_count.saturating_sub(1))
                        / (FmAlgorithm::ALL.len() - 1)) as u16;
                    if segment == selected { "●" } else { "│" }
                }
                ParameterValue::FmRatio(mode) => {
                    let index = FmRatio::ALL
                        .iter()
                        .position(|choice| *choice == mode)
                        .unwrap_or(0);
                    let selected = ((FmRatio::ALL.len() - 1 - index)
                        * usize::from(segment_count.saturating_sub(1))
                        / (FmRatio::ALL.len() - 1)) as u16;
                    if segment == selected { "●" } else { "│" }
                }
            };
            let segment_style = if active {
                Style::default()
                    .fg(group_color)
                    .reversed()
                    .add_modifier(Modifier::BOLD)
            } else if symbol == "···" {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
                    .fg(group_color)
                    .add_modifier(Modifier::BOLD)
            };
            render_centered(f, symbol, segment_area, segment_style);
        }
        let label_area = Rect {
            x: content.x,
            y: content.y + 1 + segment_count,
            width: content.width,
            height: 1,
        };
        render_centered(f, descriptor.label, label_area, style);
        if compact {
            continue;
        }
        let full_origin_label = match origin {
            ValueOrigin::Base => "BASE",
            ValueOrigin::Lock => "LOCK",
        };
        let shortcut_area = Rect {
            x: content.x,
            y: content.y + 12,
            width: content.width,
            height: 1,
        };
        let shortcut_label = format!("[{}]", descriptor.shortcut);
        let full_label_width =
            shortcut_label.len() + full_origin_label.len() + usize::from(has_lfo);
        let origin_label = if full_label_width <= usize::from(shortcut_area.width) {
            full_origin_label
        } else {
            match origin {
                ValueOrigin::Base => "B",
                ValueOrigin::Lock => "L",
            }
        };
        let shortcut_style = if active {
            Style::default()
                .fg(group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(group_color)
                .add_modifier(Modifier::BOLD)
        };
        let origin_style = if origin == ValueOrigin::Lock {
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD)
                .add_modifier(if active {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                })
        } else {
            shortcut_style
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(shortcut_label, shortcut_style),
                Span::styled(origin_label, origin_style),
                Span::styled(
                    if has_lfo { "~" } else { "" },
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .alignment(Alignment::Center),
            shortcut_area,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_fm_operator_summary(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    track: usize,
    operator: usize,
    descriptor: &ParameterDescriptor,
    active: bool,
    compact: bool,
) {
    use crate::model::FmOperatorField;
    let style = if active {
        Style::default()
            .fg(Color::LightCyan)
            .reversed()
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    };
    let value = |field| {
        let id = ParameterId::fm_operator(operator, field).unwrap();
        displayed_parameter(a, track, a.step, id).map(|(value, origin)| (id, value, origin))
    };
    let Some((_, ParameterValue::FmRatio(ratio), ratio_origin)) = value(FmOperatorField::Ratio)
    else {
        return;
    };
    let Some((level_id, ParameterValue::Percent(level), level_origin)) =
        value(FmOperatorField::Level)
    else {
        return;
    };
    let Some((feedback_id, ParameterValue::Percent(feedback), feedback_origin)) =
        value(FmOperatorField::Feedback)
    else {
        return;
    };
    let algorithm = match displayed_parameter(a, track, a.step, ParameterId::FmAlgorithm) {
        Some((ParameterValue::FmAlgorithm(value), _)) => value,
        _ => FmAlgorithm::default(),
    };
    let origin = |origin| {
        if origin == ValueOrigin::Lock {
            "L"
        } else {
            "B"
        }
    };
    let lfo = |id| {
        if a.editor.project.tracks[track].lfos.get(id).is_some() {
            "~"
        } else {
            ""
        }
    };
    let lines = [
        format!("R{ratio}{}", origin(ratio_origin)),
        format!("L{}{}{}", level.get(), origin(level_origin), lfo(level_id)),
        format!(
            "F{}{}{}",
            feedback.get(),
            origin(feedback_origin),
            lfo(feedback_id)
        ),
        algorithm.role(operator).to_string(),
    ];
    let available = area.height.saturating_sub(if compact { 1 } else { 2 });
    let start = area.y + available.saturating_sub(lines.len() as u16) / 2;
    for (row, line) in lines.iter().take(usize::from(available)).enumerate() {
        render_centered(
            f,
            line,
            Rect {
                y: start + row as u16,
                height: 1,
                ..area
            },
            style,
        );
    }
    render_centered(
        f,
        descriptor.label,
        Rect {
            y: area.y + area.height.saturating_sub(if compact { 1 } else { 2 }),
            height: 1,
            ..area
        },
        style,
    );
    if !compact {
        render_centered(
            f,
            "[O]",
            Rect {
                y: area.y + area.height.saturating_sub(1),
                height: 1,
                ..area
            },
            style,
        );
    }
}

fn render_pitch_lfo_card(
    f: &mut ratatui::Frame,
    content: Rect,
    track: &crate::model::Track,
    descriptor: &ParameterDescriptor,
    render: ParameterCardRender,
) {
    let config = track.lfos.get(ParameterId::Pitch);
    let value_label = config
        .map(|config| {
            format!(
                "{}%±{:.0}st",
                config.depth.get(),
                config.depth.get() as f32 * 0.02
            )
        })
        .unwrap_or_else(|| "—".into());
    let value_label = if render.show_shortcut || config.is_none() {
        value_label
    } else {
        format!("{value_label}~")
    };
    render_centered(f, &value_label, content, render.style);
    for segment in 0..render.segment_count {
        let segment_area = Rect {
            x: content.x,
            y: content.y + 1 + segment,
            width: content.width,
            height: 1,
        };
        let symbol = config.map_or("···", |config| {
            if usize::from(segment)
                >= usize::from(render.segment_count)
                    - fader_segments_for(config.depth.get(), render.segment_count)
            {
                "███"
            } else {
                "···"
            }
        });
        let segment_style = if render.active {
            Style::default()
                .fg(render.group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else if symbol == "···" {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
                .fg(render.group_color)
                .add_modifier(Modifier::BOLD)
        };
        render_centered(f, symbol, segment_area, segment_style);
    }
    render_centered(
        f,
        descriptor.label,
        Rect {
            x: content.x,
            y: content.y + 1 + render.segment_count,
            width: content.width,
            height: 1,
        },
        render.style,
    );
    if !render.show_shortcut {
        return;
    }
    let shortcut_style = if render.active {
        Style::default()
            .fg(render.group_color)
            .reversed()
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(render.group_color)
            .add_modifier(Modifier::BOLD)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("[{}]", descriptor.shortcut), shortcut_style),
            Span::styled("LFO", shortcut_style),
            Span::styled(
                if config.is_some() { "~" } else { "" },
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Center),
        Rect {
            x: content.x,
            y: content.y + 2 + render.segment_count,
            width: content.width,
            height: 1,
        },
    );
}

pub(super) fn render_centered(f: &mut ratatui::Frame, text: &str, area: Rect, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_owned(), style)))
            .alignment(Alignment::Center),
        area,
    );
}

pub(super) fn draw(f: &mut ratatui::Frame, a: &App, audio: &Audio) {
    draw_with_device(f, a, &audio.device_name);
}

pub(super) fn draw_with_device(f: &mut ratatui::Frame, a: &App, device_name: &str) {
    let area = f.area();
    if area.width < 120 || area.height < 34 {
        let help_hint = if help_available(&a.mode) {
            "  [?] Help"
        } else {
            ""
        };
        let recording = match a.recording_state {
            RecordingState::Idle => "",
            RecordingState::Recording => "\n● REC  [Ctrl+R] Stop recording",
            RecordingState::Finalizing => "\nWAV FINALIZING  [Ctrl+R] unavailable",
        };
        f.render_widget(
            Paragraph::new(format!(
                "terminal-groove needs 120x34\nCurrent: {}x{}\n[Ctrl+Q] Quit  [Ctrl+R] Record WAV{help_hint}{recording}",
                area.width, area.height,
            ))
            .block(Block::bordered().title("Terminal too small")),
            area,
        );
        return;
    }
    let details_height =
        if a.row > 0 && !matches!(a.mode, Mode::ParameterEdit(_) | Mode::ChordEdit { .. }) {
            10
        } else {
            16
        };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(9),
            Constraint::Length(details_height),
            Constraint::Length(2),
        ])
        .split(area);
    let dirty = if a.editor.is_dirty() { " *" } else { "" };
    let file = a
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled");
    let transport = if a.playing {
        "PLAY"
    } else if a.paused {
        "PAUSE"
    } else {
        "STOP"
    };
    let pattern_state = if a.song_mode {
        let entry = &a.editor.project.song[a.active_song];
        format!(
            "SONG {} / {} · P{} · bar {}/{}",
            a.active_song + 1,
            a.editor.project.song.len(),
            entry.pattern,
            a.song_bar + 1,
            entry.bars
        )
    } else {
        format!(
            "P{} / {}",
            a.editor.pattern() + 1,
            a.editor.project.patterns.len()
        )
    };
    let mut header_spans = vec![
        Span::styled(
            " terminal-groove ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        match a.recording_state {
            RecordingState::Idle => Span::raw(""),
            RecordingState::Recording => Span::styled(
                " ● REC ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            RecordingState::Finalizing => Span::styled(
                " WAV FINALIZING ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        },
        Span::raw(format!(
            " {file}{dirty} | audio: {} | {transport} | {pattern_state} | {} BPM",
            device_name, a.editor.project.globals.tempo_bpm
        )),
    ];
    if a.callback_overruns > 0 {
        header_spans.push(Span::styled(
            format!(
                " | ⚠ audio overload: {}, max {}%",
                a.callback_overruns,
                a.max_callback_load_per_mille.div_ceil(10)
            ),
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let header = Line::from(header_spans);
    f.render_widget(Paragraph::new(header), chunks[0]);
    let g = &a.editor.project.globals;
    let globals = global_control_text(g);
    let line = globals
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Span::styled(
                format!(" {s} "),
                if a.row == 0 && a.global == i {
                    Style::default().reversed()
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(Line::from(line)).block(Block::bordered().title("Globals")),
        chunks[1],
    );
    let available_rows = chunks[2].height.saturating_sub(3) as usize;
    let heights = a.editor.project.patterns[a.editor.pattern()]
        .tracks
        .iter()
        .map(|track| track.steps.len().div_ceil(STEP_ROW_SIZE))
        .collect::<Vec<_>>();
    let selected_track = a.row.saturating_sub(1).min(TRACK_COUNT - 1);
    let mut first_track = selected_track;
    let mut used_rows = heights[selected_track];
    while first_track > 0 && used_rows + heights[first_track - 1] <= available_rows {
        first_track -= 1;
        used_rows += heights[first_track];
    }
    let mut last_track = selected_track + 1;
    while last_track < TRACK_COUNT && used_rows + heights[last_track] <= available_rows {
        used_rows += heights[last_track];
        last_track += 1;
    }
    let mut rows = Vec::new();
    for (ti, &track_height) in heights
        .iter()
        .enumerate()
        .take(last_track)
        .skip(first_track)
    {
        let track = &a.editor.project.tracks[ti];
        let steps = a.editor.active_steps(ti).unwrap();
        let length = steps.len();
        for line_index in 0..track_height {
            let line_start = line_index * STEP_ROW_SIZE;
            let line_end = (line_start + STEP_ROW_SIZE).min(length);
            let mut cells: Vec<ratatui::widgets::Cell> = Vec::with_capacity(37);
            let lane_marker = if line_index == 0 && track.muted {
                "M"
            } else if line_index == 0 && a.playheads[ti].is_some() {
                "▶"
            } else if line_index == 0 && a.row == ti + 1 {
                "›"
            } else {
                " "
            };
            cells.push(ratatui::widgets::Cell::from(format!("{lane_marker} ")));
            cells.push(if line_index == 0 {
                let style = if a.row == ti + 1 {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ratatui::widgets::Cell::from(track_label(track)).style(style)
            } else {
                "↳".into()
            });
            cells.push(format!(" {:02}–{:02}", line_start + 1, line_end).into());
            cells.push(ratatui::widgets::Cell::from("│"));
            for slot in 0..STEP_ROW_SIZE {
                if slot == STEP_BANK_SIZE {
                    cells.push(ratatui::widgets::Cell::from("│"));
                }
                let step = line_start + slot;
                if step >= length {
                    cells.push(ratatui::widgets::Cell::from("   "));
                    continue;
                }
                // Beat starts are called out in the header; occupancy is shown
                // with a muted tint while cursor and playhead styles take precedence.
                let selected = a.row == ti + 1 && a.step == step;
                let playing = a.playheads[ti] == Some(step);
                let populated = steps[step].is_some();
                let mut style = match (selected, playing, populated) {
                    (true, true, _) => Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                    (true, false, _) => Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                    (false, true, _) => Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    // Do not rely on the terminal's default foreground here:
                    // light themes commonly make that black, which is hard to
                    // read on the occupied-cell tint.
                    (false, false, true) => Style::default().fg(Color::White).bg(Color::DarkGray),
                    (false, false, false) => Style::default(),
                };
                if matches!(
                    steps[step],
                    Some(
                        StepEvent::BassNote { slide: true, .. }
                            | StepEvent::LeadNote { slide: true, .. }
                    )
                ) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if matches!(
                    steps[step].as_ref(),
                    Some(StepEvent::BassNote {
                        condition,
                        retrigger_count,
                        ..
                    }
                    | StepEvent::Note {
                        condition,
                        retrigger_count,
                        ..
                    }
                    | StepEvent::LeadNote {
                        condition,
                        retrigger_count,
                        ..
                    }) if *condition != TriggerCondition::Always || *retrigger_count != 1
                ) && !selected
                    && !playing
                {
                    style = style.fg(Color::Magenta).add_modifier(Modifier::BOLD);
                }
                cells.push(
                    ratatui::widgets::Cell::from(step_cell(steps[step].as_ref())).style(style),
                );
            }
            rows.push(Row::new(cells));
        }
    }
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(1),
    ];
    widths.extend((0..STEP_BANK_SIZE).map(|_| Constraint::Length(3)));
    widths.push(Constraint::Length(1));
    widths.extend((0..STEP_BANK_SIZE).map(|_| Constraint::Length(3)));
    let mut header_cells = vec![
        ratatui::widgets::Cell::from(" "),
        ratatui::widgets::Cell::from("Track"),
        ratatui::widgets::Cell::from(" Steps"),
        ratatui::widgets::Cell::from("│"),
    ];
    let selected_column = (a.row > 0).then_some(a.step % STEP_ROW_SIZE);
    header_cells.extend((1..=STEP_BANK_SIZE).map(|n| {
        let selected = selected_column == Some(n - 1);
        let label = if selected {
            format!("▾{n:02}")
        } else {
            format!(" {n:02}")
        };
        let style = if (n - 1) % 4 == 0 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        ratatui::widgets::Cell::from(label).style(style)
    }));
    header_cells.push(ratatui::widgets::Cell::from("│"));
    header_cells.extend(((STEP_BANK_SIZE + 1)..=STEP_ROW_SIZE).map(|n| {
        let selected = selected_column == Some(n - 1);
        let label = if selected {
            format!("▾{n:02}")
        } else {
            format!(" {n:02}")
        };
        let style = if (n - 1) % 4 == 0 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        ratatui::widgets::Cell::from(label).style(style)
    }));
    let trigger_summary = if a.row > 0 {
        let track = a.row - 1;
        a.editor.active_steps(track).unwrap()[a.step]
            .as_ref()
            .and_then(|event| event.condition().zip(event.retrigger_count()))
            .map(|(condition, count)| format!(" · {condition}, x{count}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let swing_summary = if a.row > 0 {
        format!(" · Swing {}", a.editor.project.tracks[a.row - 1].swing)
    } else {
        String::new()
    };
    let probability_summary = if a.row > 0 {
        format!(
            " · Probability {}",
            a.editor.project.tracks[a.row - 1].probability
        )
    } else {
        String::new()
    };
    let cursor_summary = if a.row > 0 {
        format!(" · Cursor {:02}", a.step + 1)
    } else {
        String::new()
    };
    let pattern_title = Line::from(format!(
        "Pattern{cursor_summary}{trigger_summary}{swing_summary}{probability_summary}"
    ));
    f.render_widget(
        Table::new(rows, widths)
            .column_spacing(0)
            .header(Row::new(header_cells))
            .block(
                Block::bordered().title(pattern_title).title_bottom(
                    "▾ cursor  ▶ active lane  · empty  ▰ populated  x/X hit/accent  + condition/retrigger  Dᴼ note  */# lock  underline slide  ─ tie",
                ),
            ),
        chunks[2],
    );
    if a.row == 0 {
        render_global_cards(f, chunks[3], a);
    } else {
        let track = a.row - 1;
        render_parameter_bank(f, chunks[3], a, track);
    }
    let lock_editing = a.scope == Scope::Lock
        && matches!(
            a.mode,
            Mode::ParameterEdit(_) | Mode::LfoEdit { .. } | Mode::FmOperatorEdit { .. }
        );
    let mode_line = if lock_editing {
        let parameter = match a.mode {
            Mode::ParameterEdit(parameter) | Mode::LfoEdit { parameter, .. } => {
                parameter.display_name().to_owned()
            }
            Mode::FmOperatorEdit {
                operator, field, ..
            } => format!("operator {} {field:?}", operator + 1).to_lowercase(),
            _ => unreachable!(),
        };
        Line::from(vec![
            Span::styled(
                " LOCK PARAMETER EDITING ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" ({parameter}) | {}", a.status)),
        ])
    } else {
        Line::from(format!("Mode: {} | {}", mode_name(&a.mode), a.status))
    };
    let mut status_lines = vec![mode_line];
    if let Mode::ParameterEdit(parameter) = a.mode {
        let track = a.row.saturating_sub(1);
        let kind = a.editor.project.tracks[track].kind;
        let percentage_hint = parameter_supports_direct_percentage(parameter)
            .then_some(format!("  {DIRECT_PERCENTAGE_HINT}"))
            .unwrap_or_default();
        let lfo_hint = if parameter.supports_lfo(kind) {
            "  [Shift+L] LFO"
        } else {
            ""
        };
        let removal_hint = if parameter == ParameterId::Pitch {
            "  [Backspace/Del] remove LFO"
        } else if a.scope == Scope::Lock {
            "  [Backspace/Del] remove lock"
        } else {
            ""
        };
        let extended_hints =
            (chunks[4].width >= 180).then(|| format!("{percentage_hint}{lfo_hint}{removal_hint}"));
        status_lines.push(Line::from(format!(
            "{} · ↑↓ ±1/10 · ←→ param · S+←→ bank · Pg step · S+Pg bank · Tab/Esc{}",
            physical_parameter_readout(a, track, a.step, parameter),
            extended_hints.unwrap_or_default(),
        )));
    } else if matches!(a.mode, Mode::LfoEdit { .. }) {
        status_lines.push(Line::from(
            "Track-level LFO · [←/→] field  [↑/↓] adjust  [Shift+↑/↓] ±10% fields  [`/-/1–9/0] phase/free rate/depth  [Backspace/Del] remove  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::ChordEdit { .. }) {
        status_lines.push(Line::from(
            "Voicing · [←/→] field  [↑/↓] adjust  [PageUp/Down] step  [1–8] note  [ / ] octave  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::GlobalEdit(_) | Mode::TempoInput(_)) {
        status_lines.push(Line::from(
            "[↑/↓] adjust  [←/→] select another control  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::SidechainEdit { .. }) {
        status_lines.push(Line::from(
            "Ducking · [←/→] field  [↑/↓] ±1%  [Shift+↑/↓] ±10%  [`/-/1–9/0] depth  [Enter/Esc] close",
        ));
    } else if matches!(a.mode, Mode::TrackLengthInput(_)) {
        status_lines.push(Line::from(
            "Type 1–64 and press Enter; [↑/↓] ±1  [Shift+↑/↓] ±16  [Esc] finish",
        ));
    }
    if matches!(a.mode, Mode::TriggerEdit { .. }) {
        status_lines.push(Line::from(
            "Trigger editor · [←/→] field  [↑/↓] adjust  [Shift+↑/↓] ±10%  [Enter/Esc] finish",
        ));
    } else if a.mode == Mode::SwingEdit {
        status_lines.push(Line::from(
            "Track swing · [↑/↓] ±1%  [Shift+↑/↓] ±10%  (0–75%)  [Enter/Esc] finish",
        ));
    } else if a.mode == Mode::TrackProbabilityEdit {
        status_lines.push(Line::from(
            "Track probability · [↑/↓] ±1%  [Shift+↑/↓] ±10%  (0–100%)  [Enter/Esc/Shift+Q] finish",
        ));
    }
    f.render_widget(Paragraph::new(status_lines), chunks[4]);
    if a.mode == Mode::Help {
        let help_area = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        popup_at(f, help_area, "Help", HELP_TEXT)
    }
    if a.mode == Mode::QuitConfirm {
        popup_at(
            f,
            quit_popup_rect(area),
            "Unsaved changes",
            "Save [S]  Discard [D]  Cancel [Esc]",
        )
    }
    if a.mode == Mode::NewConfirm {
        popup_at(
            f,
            quit_popup_rect(area),
            "New project",
            "Save [S]  Discard [D]  Cancel [Esc]",
        )
    }
    match &a.mode {
        Mode::PatternDialog => render_pattern_popup(f, area, a),
        Mode::GeneratorDialog(dialog) => render_generator_popup(f, area, dialog, a),
        Mode::FmOperatorEdit {
            operator, field, ..
        } => render_fm_operator_popup(f, area, a, *operator, *field),
        Mode::LfoEdit { parameter, field } => {
            let track = a.row - 1;
            if let Ok(Some(config)) = a.editor.lfo(track, *parameter) {
                render_lfo_popup(
                    f,
                    area,
                    *parameter,
                    config,
                    *field,
                    a.editor.project.globals.tempo_bpm,
                );
            }
        }
        Mode::ChordEdit { shape } => render_chord_popup(f, chunks[3], *shape, a),
        Mode::TriggerEdit { field } => render_trigger_popup(f, area, a, *field),
        Mode::SidechainEdit { field } => render_sidechain_popup(f, area, a, *field),
        Mode::SwingEdit => popup_at(
            f,
            swing_popup_rect(area),
            "Track swing",
            &format!(
                "{}: {}\n\n[↑/↓] ±1%   [Shift+↑/↓] ±10%\n0–75% · applies to offbeat sixteenths",
                a.editor.project.tracks[a.row - 1].name,
                a.editor.project.tracks[a.row - 1].swing,
            ),
        ),
        Mode::TrackProbabilityEdit => popup_at(
            f,
            probability_popup_rect(area),
            "Track probability",
            &format!(
                "{}: {}\n\n[↑/↓] ±1%   [Shift+↑/↓] ±10%\n0–100% · evaluated after event conditions",
                a.editor.project.tracks[a.row - 1].name,
                a.editor.project.tracks[a.row - 1].probability,
            ),
        ),
        Mode::TempoInput(input) => popup_at(
            f,
            tempo_popup_rect(area),
            "Tempo numeric input",
            &format!(
                "Tempo: {input}_\nEnter confirms  Esc closes (keeps arrow changes)  ↑/↓ adjusts current tempo"
            ),
        ),
        Mode::TrackLengthInput(input) => {
            let current = a.editor.active_steps(a.row - 1).unwrap().len();
            popup(
                f,
                area,
                "Track length",
                &format!(
                    "Current: {current}\nLength: {input}_\nEnter confirms  Esc finishes  ↑/↓ ±1  Shift+↑/↓ ±16"
                ),
            )
        }
        Mode::ProjectBrowser { entries, selected } => {
            render_project_browser(f, area, entries, *selected);
        }
        Mode::PresetBrowser {
            entries, selected, ..
        } => render_preset_browser(f, area, entries, *selected),
        Mode::PresetDialog {
            track,
            selected,
            has_default,
        } => render_preset_dialog(
            f,
            area,
            &a.editor.project.tracks[*track].name,
            *selected,
            *has_default,
        ),
        Mode::FileInput(_, input) => {
            let directory_path = project_directory().ok();
            let directory = directory_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Terminal Groove Projects folder unavailable".into());
            let destination = if input.is_empty() {
                format!("{directory}/<name>{PROJECT_EXTENSION}")
            } else {
                project_path_for_name(
                    directory_path.as_deref().unwrap_or_else(|| {
                        std::path::Path::new("Terminal Groove Projects folder unavailable")
                    }),
                    input,
                )
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| format!("{directory}/{input}{PROJECT_EXTENSION}"))
            };
            popup(
                f,
                area,
                "Save project",
                &format!("Name: {input}_\nDestination: {destination}\nEnter confirms  Esc cancels"),
            );
        }
        Mode::PresetNameInput { track, input } => {
            let kind = a.editor.project.tracks[*track].kind;
            let directory_path = preset_directory(kind).ok();
            let directory = directory_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Terminal Groove Presets folder unavailable".into());
            let destination = if input.is_empty() {
                format!("{directory}/<name>{PRESET_EXTENSION}")
            } else {
                preset_path_for_name(kind, input)
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| format!("{directory}/{input}{PRESET_EXTENSION}"))
            };
            popup(
                f,
                area,
                "Save track preset",
                &format!(
                    "Track: {}\nName: {input}_\nDestination: {destination}\nEnter confirms  Esc cancels",
                    a.editor.project.tracks[*track].name
                ),
            );
        }
        Mode::OverwriteConfirm { path, .. } => {
            let destination = path.display().to_string();
            let popup_area = overwrite_popup_rect(area, &destination);
            let destination = overwrite_destination(&destination, popup_area);
            popup_at(
                f,
                popup_area,
                "Overwrite existing project?",
                &format!("{destination}\n\nOverwrite [Enter/O]  Cancel [Esc]"),
            )
        }
        Mode::PresetOverwriteConfirm { path, .. } => {
            let destination = path.display().to_string();
            let popup_area = overwrite_popup_rect(area, &destination);
            let destination = overwrite_destination(&destination, popup_area);
            popup_at(
                f,
                popup_area,
                "Overwrite existing preset?",
                &format!("{destination}\n\nOverwrite [Enter/O]  Cancel [Esc]"),
            )
        }
        Mode::DefaultPresetConfirm { track, action } => {
            let name = &a.editor.project.tracks[*track].name;
            let (title, text) = if *action == DefaultPresetAction::Save {
                (
                    "Set track default preset?",
                    format!(
                        "Set {name} as the default for new projects only.\n\nSet [Enter/S]  Cancel [Esc]"
                    ),
                )
            } else {
                (
                    "Clear track default preset?",
                    format!(
                        "Clear the {name} default used by new projects.\n\nClear [Enter/D]  Cancel [Esc]"
                    ),
                )
            };
            popup(f, area, title, &text)
        }
        Mode::OpenConfirm(path) => popup(
            f,
            area,
            "Unsaved changes",
            &format!(
                "Open {}?\nSave [S]  Discard [D]  Cancel [Esc]",
                path.display()
            ),
        ),
        Mode::Error(message) => popup(
            f,
            area,
            "Error",
            &format!("{message}\n\nEnter or Esc closes"),
        ),
        _ => {}
    }
}
