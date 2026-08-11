use crate::model::GlobalParameterId;

#[derive(Clone, Copy)]
pub(super) struct GlobalControl {
    pub(super) id: GlobalParameterId,
    pub(super) name: &'static str,
    pub(super) label: &'static str,
    pub(super) shortcut: &'static str,
}

pub(super) const GLOBAL_CONTROLS: [GlobalControl; 10] = [
    GlobalControl {
        id: GlobalParameterId::Tempo,
        name: "tempo",
        label: "Tempo",
        shortcut: "t",
    },
    GlobalControl {
        id: GlobalParameterId::DelayDivision,
        name: "delay division",
        label: "Delay div.",
        shortcut: "y",
    },
    GlobalControl {
        id: GlobalParameterId::DelayFeedback,
        name: "delay feedback",
        label: "Feedback",
        shortcut: "f",
    },
    GlobalControl {
        id: GlobalParameterId::ReverbTime,
        name: "reverb time",
        label: "Reverb time",
        shortcut: "r",
    },
    GlobalControl {
        id: GlobalParameterId::ReverbTone,
        name: "reverb tone",
        label: "Tone",
        shortcut: "b",
    },
    GlobalControl {
        id: GlobalParameterId::ReverbPreDelay,
        name: "reverb pre-delay",
        label: "Pre-delay",
        shortcut: "p",
    },
    GlobalControl {
        id: GlobalParameterId::ReverbReturn,
        name: "reverb return",
        label: "Return",
        shortcut: "m",
    },
    GlobalControl {
        id: GlobalParameterId::Ducking,
        name: "ducking",
        label: "Ducking",
        shortcut: "d",
    },
    GlobalControl {
        id: GlobalParameterId::Key,
        name: "key",
        label: "Key",
        shortcut: "k",
    },
    GlobalControl {
        id: GlobalParameterId::Scale,
        name: "scale",
        label: "Scale",
        shortcut: "s",
    },
];

pub(super) fn global_control(id: GlobalParameterId) -> &'static GlobalControl {
    &GLOBAL_CONTROLS[id as usize]
}
