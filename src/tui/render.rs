use super::overlays::{
    popup, popup_at, quit_popup_rect, render_chord_popup, render_generator_popup, render_lfo_popup,
    render_lfo_selector, render_pattern_popup, render_trigger_popup, swing_popup_rect,
    tempo_popup_rect,
};
use super::{
    controller::{global_name, resolved_path},
    input::parameter_supports_direct_percentage,
    state::{App, FileAction, Mode, ParameterBank},
};
use crate::tui::DIRECT_PERCENTAGE_HINT;
use crate::{
    audio::Audio,
    model::{
        ChordShape, ChorusMode, DelayDivision, GlobalParameterId, ParameterId, ParameterValue,
        PitchClass, STEP_BANK_SIZE, STEP_ROW_SIZE, Scale, StepEvent, TRACK_COUNT, TrackKind,
        TriggerCondition, Waveform,
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
        Mode::Navigation => "Navigation".into(),
        Mode::PatternDialog => "Pattern dialog".into(),
        Mode::GeneratorDialog(_) => "Pattern idea generator".into(),
        Mode::ParameterEdit(parameter) => {
            format!("Parameter edit ({})", parameter.display_name())
        }
        Mode::LfoEdit { parameter, .. } => {
            format!("Track LFO edit ({})", parameter.display_name())
        }
        Mode::ChordEdit { shape } => format!("Chord trigger edit ({shape})"),
        Mode::TriggerEdit { .. } => "Trigger editor".into(),
        Mode::SwingEdit => "Track swing edit".into(),
        Mode::GlobalEdit(id) => format!("Global edit ({})", global_name(*id)),
        Mode::TempoInput(_) => "Tempo numeric input".into(),
        Mode::TrackLengthInput(_) => "Track length input".into(),
        Mode::FileInput(_, _) => "File-path input".into(),
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
            | Mode::LfoEdit { .. }
            | Mode::ChordEdit { .. }
            | Mode::TriggerEdit { .. }
            | Mode::SwingEdit
    )
}

const HELP_TEXT: &str =
    "CORE  Space play/pause · . stop/reset · ? help · Esc close help
      Ctrl+N new project · Ctrl+O open · Ctrl+S save · Ctrl+Shift+S save as
      Ctrl+Q quit · Ctrl+Z undo · Ctrl+Y redo
PATTERNS  Ctrl+P open dialog · ←/→ Home End move cursor · Enter select/queue
          N insert · D duplicate · C copy · X cut · V paste · Delete remove · Esc close
NAVIGATION  ↑/↓ rows · ←/→ steps (global row: controls) · Shift+←/→ step bank
           ~ select global row · Shift+1..6 select track · Enter toggle/insert · Backspace/Delete clear
           g pattern generator · o audition selected step
EVENTS & TRACKS  p BASE/LOCK · m mute · l length · Shift+D double
                 A accent/default · Shift+G Bass slide · Shift+T condition/retrigger · Shift+S swing
                 1–8 note · [ / ] octave · t tie · C Chord trigger editor
PARAMETERS  v level · n pan · y delay send · b reverb send
           Tab PARAMS/EFFECTS · EFFECTS: d drive · t tone · x distortion mix
           EFFECTS: r/e/f/M phaser rate/depth/feedback/mix
           EFFECTS: R rate · q delay · E depth · F feedback · N flanger mix
           Kick: u tune · d decay · a attack
           Snare: u tune · t tone · s snappy · Hat: u tune · d decay
           Bass: w waveform · c cutoff · R resonance · f filter env · d decay
           Chord/Lead: w osc mix · Shift+P pulse · u sub · i pitch LFO
           Chord/Lead: c cutoff · R resonance · f filter env · a/d/s/r ADSR
           Chord: h chorus · e spread
           Parameter edit: PageUp/Down step · Shift+L LFO · [`/1–9/0] percent
           ↑/↓ adjust · ←/→ switch parameter · Enter/Esc finish · Backspace/Delete remove lock/LFO
GLOBAL  t tempo · y delay division · f feedback · r reverb time
        b reverb tone · p pre-delay · k key · s scale · ←/→ select · ↑/↓ adjust
Help is available from navigation and track editors with ?; Esc in navigation resets scope to BASE.";

pub(super) fn track_label(t: &crate::model::Track) -> String {
    if matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
        format!("{} O{}", t.name, t.input_octave.unwrap_or(3))
    } else {
        t.name.clone()
    }
}

pub(super) fn step_cell(event: Option<&StepEvent>) -> String {
    match event {
        None => " . ".into(),
        Some(StepEvent::Trigger {
            accent,
            condition,
            retrigger_count,
            locks,
        }) if *condition != TriggerCondition::Always || *retrigger_count != 1 => {
            if *accent {
                "X+ ".into()
            } else {
                "x+ ".into()
            }
        }
        Some(StepEvent::Trigger { accent, locks, .. }) if locks.is_empty() => {
            if *accent {
                " X ".into()
            } else {
                " x ".into()
            }
        }
        Some(StepEvent::Trigger { accent, .. }) => {
            if *accent {
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
        }) => format!(
            "{degree}{}{octave}",
            match (*accent, locks.is_empty()) {
                (false, true) => ':',
                (true, true) => '!',
                (false, false) => '*',
                (true, false) => '#',
            }
        ),
        Some(StepEvent::Tie { locks }) if locks.is_empty() => " - ".into(),
        Some(StepEvent::Tie { .. }) => "-* ".into(),
    }
}

pub(super) fn selected_accent(a: &App, track: usize) -> Option<(bool, Option<usize>)> {
    let t = a.editor.project.tracks.get(track)?;
    let Some(event) = t.steps[a.step].as_ref() else {
        return Some((t.input_accent, None));
    };
    if let Some(accent) = event.accent() {
        return Some((accent, None));
    }
    let source = crate::model::tie_source(&a.editor.project.tracks[track].steps, a.step)?;
    a.editor.project.tracks[track].steps[source]
        .as_ref()?
        .accent()
        .map(|accent| (accent, Some(source)))
}

pub(super) fn articulation_title(a: &App, track: usize) -> String {
    match selected_accent(a, track) {
        Some((accent, None)) => {
            let is_default = a.editor.project.tracks[track].steps[a.step].is_none();
            let mut text = format!(
                "[A] Accent {}{}",
                if is_default { "default " } else { "" },
                if accent { "on" } else { "off" }
            );
            if let Some(StepEvent::BassNote { slide, .. }) =
                a.editor.project.tracks[track].steps[a.step]
            {
                text.push_str(&format!(
                    " · [Shift+G] Slide {}",
                    if slide { "on" } else { "off" }
                ));
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
    if a.editor.project.tracks.get(track)?.kind != TrackKind::Chord {
        return None;
    }
    match a.editor.project.tracks[track].steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => Some(chord_shape.unwrap_or_default()),
        Some(StepEvent::Tie { .. }) => {
            crate::model::tie_source(&a.editor.project.tracks[track].steps, a.step).and_then(
                |source| match a.editor.project.tracks[track].steps[source].as_ref() {
                    Some(StepEvent::Note { chord_shape, .. }) => {
                        Some(chord_shape.unwrap_or_default())
                    }
                    _ => None,
                },
            )
        }
        None => Some(
            a.editor.project.tracks[track]
                .input_chord_shape
                .unwrap_or_default(),
        ),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParameterGroup {
    Mixer,
    Instrument,
    Filter,
    Envelope,
    Distortion,
    Phaser,
    Flanger,
}

impl ParameterGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Mixer => "MIXER",
            Self::Instrument => "INSTRUMENT",
            Self::Filter => "FILTER",
            Self::Envelope => "ENVELOPE",
            Self::Distortion => "DISTORTION",
            Self::Phaser => "PHASER",
            Self::Flanger => "FLANGER",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::Mixer => Color::Cyan,
            Self::Instrument => Color::Green,
            Self::Filter => Color::Magenta,
            Self::Envelope => Color::Yellow,
            Self::Distortion => Color::Red,
            Self::Phaser => Color::Blue,
            Self::Flanger => Color::LightBlue,
        }
    }
}

const COMMON_PARAMETERS: [ParameterDescriptor; 4] = [
    ParameterDescriptor {
        id: ParameterId::Level,
        label: "Level",
        shortcut: "v",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::DelaySend,
        label: "Delay",
        shortcut: "y",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::ReverbSend,
        label: "Reverb",
        shortcut: "b",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::Pan,
        label: "Pan",
        shortcut: "n",
        group: ParameterGroup::Mixer,
    },
];

const KICK_PARAMETERS: [ParameterDescriptor; 7] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
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
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
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

const HAT_PARAMETERS: [ParameterDescriptor; 6] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
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

const BASS_PARAMETERS: [ParameterDescriptor; 9] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
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

const CHORD_PARAMETERS: [ParameterDescriptor; 17] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
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
        id: ParameterId::Chorus,
        label: "Chorus",
        shortcut: "h",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Spread,
        label: "Spread",
        shortcut: "e",
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

const LEAD_PARAMETERS: [ParameterDescriptor; 15] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    CHORD_PARAMETERS[4],
    CHORD_PARAMETERS[5],
    CHORD_PARAMETERS[6],
    CHORD_PARAMETERS[9],
    CHORD_PARAMETERS[10],
    CHORD_PARAMETERS[11],
    CHORD_PARAMETERS[12],
    CHORD_PARAMETERS[13],
    CHORD_PARAMETERS[14],
    CHORD_PARAMETERS[15],
    CHORD_PARAMETERS[16],
];

pub(super) fn parameter_descriptors(kind: TrackKind) -> &'static [ParameterDescriptor] {
    match kind {
        TrackKind::Kick => &KICK_PARAMETERS,
        TrackKind::Snare => &SNARE_PARAMETERS,
        TrackKind::Hat => &HAT_PARAMETERS,
        TrackKind::Bass => &BASS_PARAMETERS,
        TrackKind::Chord => &CHORD_PARAMETERS,
        TrackKind::Lead => &LEAD_PARAMETERS,
    }
}

const EFFECT_PARAMETERS: [ParameterDescriptor; 12] = [
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueOrigin {
    Base,
    Lock,
}

pub(super) fn lock_has_parameter(event: &StepEvent, parameter: ParameterId) -> bool {
    event.locks().get(parameter).is_some()
}

pub(super) fn displayed_parameter(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> Option<(ParameterValue, ValueOrigin)> {
    let base = a
        .editor
        .parameter_value(track, step, Scope::Base, parameter)
        .ok()?;
    if a.scope == Scope::Base {
        let value = match base {
            ParameterValue::Percent(target) => ParameterValue::Percent(a.animated_percent(
                track,
                step,
                parameter,
                ValueOrigin::Base,
                target,
            )),
            value => value,
        };
        return Some((value, ValueOrigin::Base));
    }
    let locked = a
        .editor
        .project
        .tracks
        .get(track)
        .and_then(|t| t.steps.get(step))
        .and_then(Option::as_ref)
        .is_some_and(|event| lock_has_parameter(event, parameter));
    let value = a
        .editor
        .parameter_value(track, step, Scope::Lock, parameter)
        .unwrap_or(base);
    let origin = if locked {
        ValueOrigin::Lock
    } else {
        ValueOrigin::Base
    };
    let value = match value {
        ParameterValue::Percent(target) => {
            ParameterValue::Percent(a.animated_percent(track, step, parameter, origin, target))
        }
        value => value,
    };
    Some((value, origin))
}

pub(super) fn fader_segments(value: u8) -> usize {
    ((value as usize * 10 + 50) / 100).min(10)
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
    let Some((value, origin)) = displayed_parameter(a, track, step, parameter) else {
        return "unavailable".into();
    };
    let origin = match origin {
        ValueOrigin::Base => "BASE",
        ValueOrigin::Lock => "LOCK",
    };
    let physical = match value {
        ParameterValue::Waveform(Waveform::Square) => "Square".into(),
        ParameterValue::Waveform(Waveform::Saw) => "Saw".into(),
        ParameterValue::Chorus(ChorusMode::Off) => "Off".into(),
        ParameterValue::Chorus(ChorusMode::I) => "Mode I".into(),
        ParameterValue::Chorus(ChorusMode::Ii) => "Mode II".into(),
        ParameterValue::Spread(value) => value.to_string(),
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
                    format!("{:.1} ms center", 0.2 + value as f32 * 0.098)
                }
                (_, ParameterId::FlangerDepth) => {
                    format!("±{:.1} ms sweep", value as f32 * 0.05)
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
    match id {
        GlobalParameterId::Tempo => "t",
        GlobalParameterId::DelayDivision => "y",
        GlobalParameterId::DelayFeedback => "f",
        GlobalParameterId::ReverbTime => "r",
        GlobalParameterId::ReverbTone => "b",
        GlobalParameterId::ReverbPreDelay => "p",
        GlobalParameterId::Key => "k",
        GlobalParameterId::Scale => "s",
    }
}

pub(super) const GLOBAL_IDS: [GlobalParameterId; 8] = [
    GlobalParameterId::Tempo,
    GlobalParameterId::DelayDivision,
    GlobalParameterId::DelayFeedback,
    GlobalParameterId::ReverbTime,
    GlobalParameterId::ReverbTone,
    GlobalParameterId::ReverbPreDelay,
    GlobalParameterId::Key,
    GlobalParameterId::Scale,
];

pub(super) fn global_display_name(id: GlobalParameterId) -> &'static str {
    match id {
        GlobalParameterId::Tempo => "Tempo",
        GlobalParameterId::DelayDivision => "Delay div.",
        GlobalParameterId::DelayFeedback => "Feedback",
        GlobalParameterId::ReverbTime => "Reverb time",
        GlobalParameterId::ReverbTone => "Tone",
        GlobalParameterId::ReverbPreDelay => "Pre-delay",
        GlobalParameterId::Key => "Key",
        GlobalParameterId::Scale => "Scale",
    }
}

pub(super) fn global_value_text(g: &crate::model::Globals, id: GlobalParameterId) -> String {
    match id {
        GlobalParameterId::Tempo => format!("{} BPM", g.tempo_bpm),
        GlobalParameterId::DelayDivision => g.delay_division.to_string(),
        GlobalParameterId::DelayFeedback => format!("{}%", g.delay_feedback.get()),
        GlobalParameterId::ReverbTime => format!("{:.1} s", g.reverb_time_seconds),
        GlobalParameterId::ReverbTone => format!("{}%", g.reverb_tone.get()),
        GlobalParameterId::ReverbPreDelay => format!("{} ms", g.reverb_pre_delay_ms),
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
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayDivision
        | GlobalParameterId::Key
        | GlobalParameterId::Scale => None,
    }
}

fn global_fader_bounds(id: GlobalParameterId) -> Option<(f32, f32)> {
    match id {
        GlobalParameterId::DelayFeedback => Some((0.0, 95.0)),
        GlobalParameterId::ReverbTime => Some((0.2, 10.0)),
        GlobalParameterId::ReverbTone => Some((0.0, 100.0)),
        GlobalParameterId::ReverbPreDelay => Some((0.0, 200.0)),
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayDivision
        | GlobalParameterId::Key
        | GlobalParameterId::Scale => None,
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

fn global_selector_data(
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
            [Scale::Major, Scale::NaturalMinor]
                .iter()
                .map(ToString::to_string)
                .collect(),
            usize::from(g.scale == Scale::NaturalMinor),
        )),
        GlobalParameterId::Tempo
        | GlobalParameterId::DelayFeedback
        | GlobalParameterId::ReverbTime
        | GlobalParameterId::ReverbTone
        | GlobalParameterId::ReverbPreDelay => None,
    }
}

pub(super) fn global_control_text(g: &crate::model::Globals) -> Vec<String> {
    GLOBAL_IDS
        .iter()
        .map(|id| format!("{} {}", global_display_name(*id), global_value_text(g, *id)))
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
    for (index, id) in GLOBAL_IDS.iter().enumerate() {
        let x = inner.x + inner.width * index as u16 / GLOBAL_IDS.len() as u16;
        let next_x = inner.x + inner.width * (index + 1) as u16 / GLOBAL_IDS.len() as u16;
        let slot = Rect {
            x,
            y: inner.y,
            width: next_x.saturating_sub(x),
            height: inner.height,
        };
        let active = matches!(&a.mode, Mode::GlobalEdit(active_id) if *active_id == *id)
            || matches!(&a.mode, Mode::TempoInput(_) if *id == GlobalParameterId::Tempo);
        let block = if active {
            Block::bordered()
                .border_type(BorderType::Double)
                .border_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .title_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .reversed()
                        .add_modifier(Modifier::BOLD),
                )
                .title(global_display_name(*id))
                .style(Style::default().reversed())
        } else {
            Block::bordered().title(global_display_name(*id))
        };
        let content = block.inner(slot);
        f.render_widget(block, slot);
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
        let shortcut_area = Rect {
            x: content.x,
            y: content.y + content.height.saturating_sub(1),
            width: content.width,
            height: content.height.min(1),
        };
        render_centered(
            f,
            &format!("[{}]", global_shortcut_text(*id)),
            shortcut_area,
            style,
        );

        let body = Rect {
            x: content.x,
            y: content.y,
            width: content.width,
            height: content.height.saturating_sub(1),
        };
        if let Some(filled) = global_fader_segments(g, *id) {
            render_centered(
                f,
                &global_value_text(g, *id),
                Rect { height: 1, ..body },
                style,
            );
            let fader_area = Rect {
                y: body.y + 1,
                height: body.height.saturating_sub(1),
                ..body
            };
            let height = fader_area.height.min(10);
            let start_y = fader_area.y + fader_area.height.saturating_sub(height) / 2;
            for segment in 0..height {
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
                        x: fader_area.x,
                        y: start_y + segment,
                        width: fader_area.width,
                        height: 1,
                    },
                    segment_style,
                );
            }
        } else if let Some((choices, selected)) = global_selector_data(g, *id) {
            render_lfo_selector(f, body, &choices, selected, style);
        } else {
            render_centered(f, &global_value_text(g, *id), body, style);
        }
    }
}

pub(super) fn render_parameter_bank(f: &mut ratatui::Frame, area: Rect, a: &App, track: usize) {
    let t = &a.editor.project.tracks[track];
    let lock_editing = a.scope == Scope::Lock && matches!(a.mode, Mode::ParameterEdit(_));
    let descriptors = visible_parameter_descriptors(a.parameter_bank, t.kind);
    let chord_shape = a
        .editor
        .chord_shape_value(track, a.step)
        .ok()
        .or_else(|| selected_chord_shape(a, track))
        .map(|shape| format!(" · [C] Chord trigger {shape}"))
        .unwrap_or_default();
    let bank_title = match a.parameter_bank {
        ParameterBank::Params => "PARAMS",
        ParameterBank::Effects => "EFFECTS",
    };
    let title = if matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
        format!(
            "{} · {} · Step {} · {}{} · [Tab] bank · [p] {} · [m] Mute {} · [o] Audition{}",
            track_label(t),
            bank_title,
            a.step + 1,
            articulation_title(a, track),
            chord_shape,
            scope_name(a.scope),
            if t.muted { "on" } else { "off" },
            if lock_editing {
                " · !! LOCK PARAMETER EDITING !!"
            } else {
                ""
            }
        )
    } else {
        format!(
            "{} · {} · Step {} · {} · [Tab] bank · [p] {} · [m] Mute {} · [o] Audition{}",
            t.name,
            bank_title,
            a.step + 1,
            articulation_title(a, track),
            scope_name(a.scope),
            if t.muted { "on" } else { "off" },
            if lock_editing {
                " · !! LOCK PARAMETER EDITING !!"
            } else {
                ""
            }
        )
    };
    let panel = Block::bordered()
        .border_style(if lock_editing {
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        })
        .title(Line::from(title).style(if lock_editing {
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }))
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
        let group_end = descriptors[group_start..]
            .iter()
            .position(|descriptor| descriptor.group != group)
            .map(|offset| group_start + offset)
            .unwrap_or(descriptors.len());
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

    for (index, descriptor) in descriptors.iter().enumerate() {
        let slot = Rect {
            x: bank.x + slot_width * index as u16,
            y: bank.y,
            width: slot_width,
            height: bank.height,
        };
        let active = matches!(
            a.mode,
            Mode::ParameterEdit(parameter) if parameter == descriptor.id
        ) || matches!(
            a.mode,
            Mode::LfoEdit { parameter, .. } if parameter == descriptor.id
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
        if descriptor.id == ParameterId::Pitch {
            render_pitch_lfo_card(f, content, t, descriptor, active, group_color, style);
            continue;
        }
        let Some((value, origin)) = displayed_parameter(a, track, a.step, descriptor.id) else {
            continue;
        };
        let value_label = match value {
            ParameterValue::Percent(value) => format!("{}%", value.get()),
            ParameterValue::Waveform(Waveform::Square) => "SQR".into(),
            ParameterValue::Waveform(Waveform::Saw) => "SAW".into(),
            ParameterValue::Chorus(ChorusMode::Off) => "OFF".into(),
            ParameterValue::Chorus(ChorusMode::I) => "I".into(),
            ParameterValue::Chorus(ChorusMode::Ii) => "II".into(),
            ParameterValue::Spread(value) => match value {
                crate::model::ChordSpread::Off => "OFF".into(),
                crate::model::ChordSpread::Narrow => "NAR".into(),
                crate::model::ChordSpread::Wide => "WIDE".into(),
            },
        };
        render_centered(f, &value_label, content, style);
        for segment in 0..10 {
            let segment_area = Rect {
                x: content.x,
                y: content.y + 1 + segment,
                width: content.width,
                height: 1,
            };
            let symbol = match value {
                ParameterValue::Percent(value) => {
                    let filled = fader_segments(value.get());
                    if usize::from(segment) >= 10 - filled {
                        "███"
                    } else {
                        "···"
                    }
                }
                ParameterValue::Waveform(Waveform::Saw) => {
                    if segment == 0 {
                        "●"
                    } else if segment == 9 {
                        "○"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Waveform(Waveform::Square) => {
                    if segment == 0 {
                        "○"
                    } else if segment == 9 {
                        "●"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Chorus(mode) => {
                    let selected = match mode {
                        ChorusMode::Off => 9,
                        ChorusMode::I => 5,
                        ChorusMode::Ii => 0,
                    };
                    if segment == selected { "●" } else { "│" }
                }
                ParameterValue::Spread(mode) => {
                    let selected = match mode {
                        crate::model::ChordSpread::Off => 9,
                        crate::model::ChordSpread::Narrow => 5,
                        crate::model::ChordSpread::Wide => 0,
                    };
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
            y: content.y + 11,
            width: content.width,
            height: 1,
        };
        render_centered(f, descriptor.label, label_area, style);
        let origin_label = match origin {
            ValueOrigin::Base => "BASE",
            ValueOrigin::Lock => "LOCK",
        };
        let shortcut_area = Rect {
            x: content.x,
            y: content.y + 12,
            width: content.width,
            height: 1,
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
                Span::styled(format!("[{}]", descriptor.shortcut), shortcut_style),
                Span::styled(origin_label, origin_style),
                Span::styled(
                    if t.lfos.get(descriptor.id).is_some() {
                        "~"
                    } else {
                        ""
                    },
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

pub(super) fn render_pitch_lfo_card(
    f: &mut ratatui::Frame,
    content: Rect,
    track: &crate::model::Track,
    descriptor: &ParameterDescriptor,
    active: bool,
    group_color: Color,
    style: Style,
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
    render_centered(f, &value_label, content, style);
    for segment in 0..10 {
        let segment_area = Rect {
            x: content.x,
            y: content.y + 1 + segment,
            width: content.width,
            height: 1,
        };
        let symbol = config.map_or("···", |config| {
            if usize::from(segment) >= 10 - fader_segments(config.depth.get()) {
                "███"
            } else {
                "···"
            }
        });
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
    render_centered(
        f,
        descriptor.label,
        Rect {
            x: content.x,
            y: content.y + 11,
            width: content.width,
            height: 1,
        },
        style,
    );
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
            y: content.y + 12,
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
        f.render_widget(
            Paragraph::new(format!(
                "terminal-groove needs 120x34\nCurrent: {}x{}\n[Ctrl+Q] Quit{help_hint}",
                area.width, area.height,
            ))
            .block(Block::bordered().title("Terminal too small")),
            area,
        );
        return;
    }
    let details_height = if a.row == 0 { 18 } else { 16 };
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
    let pattern_state = format!(
        "{} / {}",
        a.editor.pattern() + 1,
        a.editor.project.patterns.len()
    );
    let mut header_spans = vec![
        Span::styled(
            " terminal-groove ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
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
        Paragraph::new(Line::from(line))
            .block(Block::bordered().title("Globals [←→] select [Enter] edit")),
        chunks[1],
    );
    let available_rows = chunks[2].height.saturating_sub(3) as usize;
    let heights = a
        .editor
        .project
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
        let length = track.steps.len();
        for line_index in 0..track_height {
            let line_start = line_index * STEP_ROW_SIZE;
            let line_end = (line_start + STEP_ROW_SIZE).min(length);
            let mut cells: Vec<ratatui::widgets::Cell> = Vec::with_capacity(37);
            cells.push(if line_index == 0 && track.muted {
                "M".into()
            } else {
                " ".into()
            });
            cells.push(if line_index == 0 {
                track_label(track).into()
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
                let mut style = Style::default();
                if a.row == ti + 1 && a.step == step {
                    style = style.reversed();
                }
                if a.playheads[ti] == Some(step) {
                    style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
                }
                if matches!(
                    track.steps[step],
                    Some(StepEvent::BassNote { slide: true, .. })
                ) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if matches!(
                    track.steps[step].as_ref(),
                    Some(StepEvent::BassNote {
                        condition,
                        retrigger_count,
                        ..
                    }
                    | StepEvent::Note {
                        condition,
                        retrigger_count,
                        ..
                    }) if *condition != TriggerCondition::Always || *retrigger_count != 1
                ) {
                    style = style.fg(Color::Magenta).add_modifier(Modifier::BOLD);
                }
                cells.push(
                    ratatui::widgets::Cell::from(step_cell(track.steps[step].as_ref()))
                        .style(style),
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
        ratatui::widgets::Cell::from("M"),
        ratatui::widgets::Cell::from("Track / O"),
        ratatui::widgets::Cell::from(" Range"),
        ratatui::widgets::Cell::from("│"),
    ];
    header_cells.extend((1..=STEP_BANK_SIZE).map(|n| format!(" {n:02}").into()));
    header_cells.push(ratatui::widgets::Cell::from("│"));
    header_cells.extend(((STEP_BANK_SIZE + 1)..=STEP_ROW_SIZE).map(|n| format!(" {n:02}").into()));
    let scroll_hint = match (first_track > 0, last_track < TRACK_COUNT) {
        (true, true) => "  ↑↓ more tracks",
        (true, false) => "  ↑ more tracks",
        (false, true) => "  ↓ more tracks",
        (false, false) => "",
    };
    let pattern_help = format!(
        "[↑↓] vertical  [←→] step  [Shift+←→] bank  [Shift+1..6] track  [l] length  [Shift+D] double{scroll_hint}"
    );
    let trigger_summary = if a.row > 0 {
        let track = a.row - 1;
        a.editor.project.tracks[track].steps[a.step]
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
    let pattern_title = Line::from(format!(
        "Pattern  {pattern_help}{trigger_summary}{swing_summary}"
    ));
    f.render_widget(
        Table::new(rows, widths)
            .column_spacing(0)
            .header(Row::new(header_cells))
            .block(
                Block::bordered().title(pattern_title).title_bottom(
                    ". empty   x/X normal/accent   + condition/retrigger   D:O note   D!O accent   */# lock   underline slide   - tie",
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
    let lock_editing =
        a.scope == Scope::Lock && matches!(a.mode, Mode::ParameterEdit(_) | Mode::LfoEdit { .. });
    let mode_line = if lock_editing {
        let parameter = match a.mode {
            Mode::ParameterEdit(parameter) | Mode::LfoEdit { parameter, .. } => {
                parameter.display_name()
            }
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
        status_lines.push(Line::from(format!(
            "{} · [↑/↓] ±1  [Shift+↑/↓] ±10  [PageUp/Down] step  [Shift+1..6] track{percentage_hint}{lfo_hint}  [Enter/Esc] finish{removal_hint}",
            physical_parameter_readout(a, track, a.step, parameter),
        )));
    } else if matches!(a.mode, Mode::LfoEdit { .. }) {
        status_lines.push(Line::from(
            "Track-level LFO · [←/→] field  [↑/↓] adjust  [Shift+↑/↓] ±10% fields  [`/1–9/0] free rate/depth  [Backspace/Del] remove  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::ChordEdit { .. }) {
        status_lines.push(Line::from(
            "Chord trigger · [←/→] field  [↑/↓] adjust  [PageUp/Down] step  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::GlobalEdit(_) | Mode::TempoInput(_)) {
        status_lines.push(Line::from(
            "[↑/↓] adjust  [←/→] select another control  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::TrackLengthInput(_)) {
        status_lines.push(Line::from(
            "Type 1–64 and press Enter; [↑/↓] ±1  [Shift+↑/↓] ±16  [Esc] finish",
        ));
    }
    if matches!(a.mode, Mode::TriggerEdit { .. }) {
        status_lines.push(Line::from(
            "Trigger editor · [←/→] field  [↑/↓] adjust  [`/1–9/0] chance  [Enter/Esc] finish",
        ));
    } else if a.mode == Mode::SwingEdit {
        status_lines.push(Line::from(
            "Track swing · [↑/↓] ±1%  [Shift+↑/↓] ±10%  (0–75%)  [Enter/Esc] finish",
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
        Mode::TempoInput(input) => popup_at(
            f,
            tempo_popup_rect(area),
            "Tempo numeric input",
            &format!(
                "Tempo: {input}_\nEnter confirms  Esc closes (keeps arrow changes)  ↑/↓ adjusts current tempo"
            ),
        ),
        Mode::TrackLengthInput(input) => {
            let current = a.editor.project.tracks[a.row - 1].steps.len();
            popup(
                f,
                area,
                "Track length",
                &format!(
                    "Current: {current}\nLength: {input}_\nEnter confirms  Esc finishes  ↑/↓ ±1  Shift+↑/↓ ±16"
                ),
            )
        }
        Mode::FileInput(action, input) => {
            let title = if *action == FileAction::Open {
                "Open project"
            } else {
                "Save project as"
            };
            let resolved = resolved_path(input)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            popup(
                f,
                area,
                title,
                &format!("Path: {input}_\nResolved: {resolved}\nEnter confirms  Esc cancels"),
            );
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
