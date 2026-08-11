use crate::model::GlobalParameterId;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GlobalParameterGroup {
    Clock,
    Delay,
    Reverb,
    Ducking,
    Scale,
}

impl GlobalParameterGroup {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Clock => "CLOCK",
            Self::Delay => "DELAY",
            Self::Reverb => "REVERB",
            Self::Ducking => "DUCKING",
            Self::Scale => "SCALE",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::Clock => Color::Cyan,
            Self::Delay => Color::LightBlue,
            Self::Reverb => Color::Magenta,
            Self::Ducking => Color::Red,
            Self::Scale => Color::Green,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct GlobalControl {
    pub(super) id: GlobalParameterId,
    pub(super) name: &'static str,
    pub(super) label: &'static str,
    pub(super) shortcut: &'static str,
    pub(super) group: GlobalParameterGroup,
}

pub(super) const GLOBAL_CONTROLS: [GlobalControl; 10] = [
    GlobalControl {
        id: GlobalParameterId::Tempo,
        name: "tempo",
        label: "Tempo",
        shortcut: "t",
        group: GlobalParameterGroup::Clock,
    },
    GlobalControl {
        id: GlobalParameterId::DelayDivision,
        name: "delay division",
        label: "Delay div.",
        shortcut: "y",
        group: GlobalParameterGroup::Delay,
    },
    GlobalControl {
        id: GlobalParameterId::DelayFeedback,
        name: "delay feedback",
        label: "Feedback",
        shortcut: "f",
        group: GlobalParameterGroup::Delay,
    },
    GlobalControl {
        id: GlobalParameterId::ReverbTime,
        name: "reverb time",
        label: "Reverb time",
        shortcut: "r",
        group: GlobalParameterGroup::Reverb,
    },
    GlobalControl {
        id: GlobalParameterId::ReverbTone,
        name: "reverb tone",
        label: "Tone",
        shortcut: "b",
        group: GlobalParameterGroup::Reverb,
    },
    GlobalControl {
        id: GlobalParameterId::ReverbPreDelay,
        name: "reverb pre-delay",
        label: "Pre-delay",
        shortcut: "p",
        group: GlobalParameterGroup::Reverb,
    },
    GlobalControl {
        id: GlobalParameterId::ReverbReturn,
        name: "reverb return",
        label: "Return",
        shortcut: "m",
        group: GlobalParameterGroup::Reverb,
    },
    GlobalControl {
        id: GlobalParameterId::Ducking,
        name: "ducking",
        label: "Ducking",
        shortcut: "d",
        group: GlobalParameterGroup::Ducking,
    },
    GlobalControl {
        id: GlobalParameterId::Key,
        name: "key",
        label: "Key",
        shortcut: "k",
        group: GlobalParameterGroup::Scale,
    },
    GlobalControl {
        id: GlobalParameterId::Scale,
        name: "scale",
        label: "Scale",
        shortcut: "s",
        group: GlobalParameterGroup::Scale,
    },
];

pub(super) fn global_control(id: GlobalParameterId) -> &'static GlobalControl {
    &GLOBAL_CONTROLS[id as usize]
}
