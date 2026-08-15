//! Immutable, source-controlled sound presets.

use crate::{
    model::{
        ChordParameters, ChorusMode, DistortionParameters, FmAlgorithm, FmOperator, FmParameters,
        FmRatio, Instrument, LeadParameters, LeadSubMode, LfoConfig, LfoDivision, LfoRate,
        LfoWaveform, ParameterId, Percent, PhaserParameters, Project, TrackKind,
    },
    persistence::TrackPreset,
};

#[derive(Clone, Copy, Debug)]
pub struct BuiltInPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: TrackKind,
    factory: fn() -> TrackPreset,
}

impl BuiltInPreset {
    pub fn preset(self) -> TrackPreset {
        (self.factory)()
    }
}

fn p(value: u8) -> Percent {
    Percent::new(value).expect("built-in percentage is bounded")
}

fn base(kind: TrackKind, level: u8, delay: u8, reverb: u8, instrument: Instrument) -> TrackPreset {
    let mut track = Project::new()
        .tracks
        .into_iter()
        .find(|t| t.kind == kind)
        .unwrap();
    track.level = p(level);
    track.pan = p(50);
    track.delay_send = p(delay);
    track.reverb_send = p(reverb);
    track.instrument = instrument;
    TrackPreset::from_track(track)
}

fn chord(values: [u8; 11], chorus: ChorusMode, sends: [u8; 3]) -> TrackPreset {
    let mut preset = base(
        TrackKind::Chord,
        sends[0],
        sends[1],
        sends[2],
        Instrument::Chord(ChordParameters {
            oscillator_mix: p(values[0]),
            pulse_width: p(values[1]),
            sub_oscillator: p(values[2]),
            noise: p(values[3]),
            cutoff: p(values[4]),
            resonance: p(values[5]),
            filter_envelope: p(values[6]),
            attack: p(values[7]),
            decay: p(values[8]),
            sustain: p(values[9]),
            release: p(values[10]),
        }),
    );
    preset.effects.chorus = chorus;
    preset
}
fn lead(values: [u8; 13], sub_mode: LeadSubMode, sends: [u8; 3]) -> TrackPreset {
    base(
        TrackKind::Lead,
        sends[0],
        sends[1],
        sends[2],
        Instrument::Lead(LeadParameters {
            oscillator_mix: p(values[0]),
            pulse_width: p(values[1]),
            sub_oscillator: p(values[2]),
            noise: p(values[3]),
            sub_mode,
            keyboard_tracking: p(values[4]),
            portamento_time: p(values[5]),
            cutoff: p(values[6]),
            resonance: p(values[7]),
            filter_envelope: p(values[8]),
            attack: p(values[9]),
            decay: p(values[10]),
            sustain: p(values[11]),
            release: p(values[12]),
        }),
    )
}
fn op(ratio: FmRatio, level: u8, feedback: u8) -> FmOperator {
    FmOperator {
        ratio,
        level: p(level),
        feedback: p(feedback),
    }
}
fn fm(
    algorithm: FmAlgorithm,
    operators: [FmOperator; 4],
    values: [u8; 5],
    sends: [u8; 3],
) -> TrackPreset {
    base(
        TrackKind::Fm,
        sends[0],
        sends[1],
        sends[2],
        Instrument::Fm(FmParameters {
            algorithm,
            operators,
            brightness: p(values[0]),
            attack: p(values[1]),
            decay: p(values[2]),
            sustain: p(values[3]),
            release: p(values[4]),
        }),
    )
}

fn warm_poly() -> TrackPreset {
    chord(
        [65, 50, 8, 0, 58, 16, 28, 18, 42, 74, 48],
        ChorusMode::I,
        [76, 0, 18],
    )
}
fn velvet_pad() -> TrackPreset {
    chord(
        [82, 55, 12, 2, 40, 18, 20, 72, 68, 84, 80],
        ChorusMode::Ii,
        [68, 0, 38],
    )
}
fn glass_keys() -> TrackPreset {
    chord(
        [90, 48, 0, 3, 72, 12, 58, 0, 38, 28, 34],
        ChorusMode::I,
        [74, 12, 24],
    )
}
fn pulse_stab() -> TrackPreset {
    let mut x = chord(
        [12, 32, 18, 0, 44, 48, 72, 0, 26, 14, 16],
        ChorusMode::Off,
        [76, 8, 10],
    );
    x.effects.distortion = DistortionParameters {
        drive: p(18),
        tone: p(68),
        mix: p(16),
    };
    x
}
fn house_organ() -> TrackPreset {
    let mut x = chord(
        [42, 50, 30, 0, 68, 8, 6, 0, 28, 92, 22],
        ChorusMode::Ii,
        [72, 0, 18],
    );
    x.effects.phaser = PhaserParameters {
        rate: p(20),
        depth: p(38),
        feedback: p(12),
        mix: p(12),
    };
    x
}
fn dream_motion() -> TrackPreset {
    let mut x = chord(
        [68, 42, 6, 5, 46, 28, 34, 62, 58, 70, 72],
        ChorusMode::Ii,
        [66, 10, 35],
    );
    x.effects.phaser = PhaserParameters {
        rate: p(15),
        depth: p(55),
        feedback: p(25),
        mix: p(18),
    };
    x.lfos.set(
        ParameterId::Cutoff,
        Some(LfoConfig {
            enabled: true,
            waveform: LfoWaveform::Sine,
            reset_on_trigger: false,
            start_phase: p(0),
            rate: LfoRate::Synced {
                division: LfoDivision::Bar,
            },
            depth: p(7),
        }),
    );
    x.lfos.set(
        ParameterId::Pan,
        Some(LfoConfig {
            enabled: true,
            waveform: LfoWaveform::Triangle,
            reset_on_trigger: false,
            start_phase: p(0),
            rate: LfoRate::Synced {
                division: LfoDivision::TwoBars,
            },
            depth: p(15),
        }),
    );
    x
}

fn solid_mono() -> TrackPreset {
    lead(
        [72, 50, 22, 0, 55, 22, 58, 24, 42, 0, 34, 68, 18],
        LeadSubMode::TwoOctaveSquare,
        [78, 8, 12],
    )
}
fn acid_pulse() -> TrackPreset {
    let mut x = lead(
        [5, 36, 12, 0, 68, 12, 34, 78, 86, 0, 24, 8, 8],
        LeadSubMode::OneOctaveSquare,
        [75, 10, 8],
    );
    x.effects.distortion = DistortionParameters {
        drive: p(22),
        tone: p(62),
        mix: p(20),
    };
    x
}
fn rubber_glide() -> TrackPreset {
    lead(
        [38, 46, 45, 0, 48, 72, 42, 52, 64, 0, 44, 48, 25],
        LeadSubMode::TwoOctaveSquare,
        [74, 18, 14],
    )
}
fn bright_saw() -> TrackPreset {
    let mut x = lead(
        [100, 50, 8, 0, 72, 15, 78, 16, 38, 0, 24, 72, 14],
        LeadSubMode::TwoOctaveSquare,
        [74, 18, 14],
    );
    x.effects.distortion = DistortionParameters {
        drive: p(10),
        tone: p(75),
        mix: p(8),
    };
    x
}
fn soft_flute() -> TrackPreset {
    let mut x = lead(
        [22, 62, 0, 14, 42, 30, 48, 12, 18, 18, 38, 82, 34],
        LeadSubMode::OneOctaveSquare,
        [72, 4, 30],
    );
    x.lfos.set(
        ParameterId::Pitch,
        Some(LfoConfig {
            enabled: true,
            waveform: LfoWaveform::Sine,
            reset_on_trigger: true,
            start_phase: p(0),
            rate: LfoRate::Free {
                rate_percent: p(82),
            },
            depth: p(8),
        }),
    );
    x
}
fn arcade_pulse() -> TrackPreset {
    let mut x = lead(
        [8, 18, 28, 0, 60, 18, 66, 32, 58, 0, 20, 46, 10],
        LeadSubMode::TwoOctaveNarrowPulse,
        [72, 24, 8],
    );
    x.effects.bit_crusher = crate::model::BitCrusherParameters {
        bits: p(35),
        rate: p(55),
        mix: p(22),
    };
    x
}

fn electric_piano() -> TrackPreset {
    fm(
        FmAlgorithm::Pairs,
        [
            op(FmRatio::One, 100, 0),
            op(FmRatio::Two, 28, 0),
            op(FmRatio::One, 72, 0),
            op(FmRatio::Three, 22, 4),
        ],
        [62, 0, 56, 34, 46],
        [72, 6, 24],
    )
}
fn tubular_bell() -> TrackPreset {
    fm(
        FmAlgorithm::FanIn,
        [
            op(FmRatio::One, 100, 0),
            op(FmRatio::Two, 36, 0),
            op(FmRatio::Three, 30, 0),
            op(FmRatio::Five, 22, 18),
        ],
        [92, 0, 72, 0, 76],
        [68, 8, 38],
    )
}
fn fm_bass() -> TrackPreset {
    let mut x = fm(
        FmAlgorithm::Cascade,
        [
            op(FmRatio::Half, 100, 0),
            op(FmRatio::One, 42, 5),
            op(FmRatio::Two, 24, 0),
            op(FmRatio::One, 12, 0),
        ],
        [40, 0, 32, 58, 12],
        [78, 0, 8],
    );
    x.effects.distortion = DistortionParameters {
        drive: p(15),
        tone: p(55),
        mix: p(12),
    };
    x
}
fn digital_pluck() -> TrackPreset {
    fm(
        FmAlgorithm::Converge,
        [
            op(FmRatio::One, 100, 0),
            op(FmRatio::Two, 40, 0),
            op(FmRatio::Three, 26, 0),
            op(FmRatio::Five, 18, 12),
        ],
        [80, 0, 30, 4, 26],
        [72, 20, 18],
    )
}
fn drawbar_organ() -> TrackPreset {
    let mut x = fm(
        FmAlgorithm::Additive,
        [
            op(FmRatio::One, 100, 0),
            op(FmRatio::Two, 62, 0),
            op(FmRatio::Three, 36, 0),
            op(FmRatio::Six, 18, 0),
        ],
        [86, 2, 22, 92, 20],
        [68, 0, 24],
    );
    x.effects.phaser = PhaserParameters {
        rate: p(22),
        depth: p(40),
        feedback: p(12),
        mix: p(14),
    };
    x
}
fn brass_stack() -> TrackPreset {
    let mut x = fm(
        FmAlgorithm::FanOut,
        [
            op(FmRatio::One, 100, 0),
            op(FmRatio::One, 68, 0),
            op(FmRatio::Two, 32, 0),
            op(FmRatio::One, 34, 8),
        ],
        [68, 4, 42, 72, 28],
        [72, 10, 18],
    );
    x.effects.distortion = DistortionParameters {
        drive: p(10),
        tone: p(70),
        mix: p(8),
    };
    x
}

macro_rules! entry {
    ($id:literal,$name:literal,$desc:literal,$kind:ident,$factory:ident) => {
        BuiltInPreset {
            id: $id,
            name: $name,
            description: $desc,
            kind: TrackKind::$kind,
            factory: $factory,
        }
    };
}
pub const BUILT_INS: [BuiltInPreset; 18] = [
    entry!(
        "warm-poly",
        "Warm Poly",
        "Warm, balanced polyphonic chords",
        Chord,
        warm_poly
    ),
    entry!(
        "velvet-pad",
        "Velvet Pad",
        "Slow, wide and spacious pad",
        Chord,
        velvet_pad
    ),
    entry!(
        "glass-keys",
        "Glass Keys",
        "Bright, delicate chord keys",
        Chord,
        glass_keys
    ),
    entry!(
        "pulse-stab",
        "Pulse Stab",
        "Short distorted pulse stab",
        Chord,
        pulse_stab
    ),
    entry!(
        "house-organ",
        "House Organ",
        "Wide organ with subtle phaser",
        Chord,
        house_organ
    ),
    entry!(
        "dream-motion",
        "Dream Motion",
        "Evolving pad with cutoff and pan motion",
        Chord,
        dream_motion
    ),
    entry!(
        "solid-mono",
        "Solid Mono",
        "Balanced monophonic lead",
        Lead,
        solid_mono
    ),
    entry!(
        "acid-pulse",
        "Acid Pulse",
        "Resonant distorted acid pulse",
        Lead,
        acid_pulse
    ),
    entry!(
        "rubber-glide",
        "Rubber Glide",
        "Elastic lead with a long glide",
        Lead,
        rubber_glide
    ),
    entry!(
        "bright-saw",
        "Bright Saw",
        "Cutting saw lead",
        Lead,
        bright_saw
    ),
    entry!(
        "soft-flute",
        "Soft Flute",
        "Breathy lead with pitch vibrato",
        Lead,
        soft_flute
    ),
    entry!(
        "arcade-pulse",
        "Arcade Pulse",
        "Crushed narrow-pulse game lead",
        Lead,
        arcade_pulse
    ),
    entry!(
        "electric-piano",
        "Electric Piano",
        "Rounded, expressive FM piano",
        Fm,
        electric_piano
    ),
    entry!(
        "tubular-bell",
        "Tubular Bell",
        "Bright metallic bell",
        Fm,
        tubular_bell
    ),
    entry!(
        "fm-bass",
        "FM Bass",
        "Compact, lightly driven FM bass",
        Fm,
        fm_bass
    ),
    entry!(
        "digital-pluck",
        "Digital Pluck",
        "Crisp digital pluck",
        Fm,
        digital_pluck
    ),
    entry!(
        "drawbar-organ",
        "Drawbar Organ",
        "Additive organ with subtle phaser",
        Fm,
        drawbar_organ
    ),
    entry!(
        "brass-stack",
        "Brass Stack",
        "Layered, lightly driven brass",
        Fm,
        brass_stack
    ),
];

pub fn for_kind(kind: TrackKind) -> impl Iterator<Item = &'static BuiltInPreset> {
    BUILT_INS.iter().filter(move |p| p.kind == kind)
}
pub fn find(id: &str) -> Option<&'static BuiltInPreset> {
    BUILT_INS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_unique_ordered_and_valid() {
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for item in BUILT_INS {
            assert!(ids.insert(item.id));
            assert!(names.insert((format!("{:?}", item.kind), item.name)));
            item.preset().validate().unwrap();
        }
        assert_eq!(for_kind(TrackKind::Chord).count(), 6);
        assert_eq!(for_kind(TrackKind::Lead).count(), 6);
        assert_eq!(for_kind(TrackKind::Fm).count(), 6);
    }

    #[test]
    fn catalog_regression_settings_match_the_builtin_library() {
        let expected = [
            ("warm-poly", [76, 0, 18]),
            ("velvet-pad", [68, 0, 38]),
            ("glass-keys", [74, 12, 24]),
            ("pulse-stab", [76, 8, 10]),
            ("house-organ", [72, 0, 18]),
            ("dream-motion", [66, 10, 35]),
            ("solid-mono", [78, 8, 12]),
            ("acid-pulse", [75, 10, 8]),
            ("rubber-glide", [74, 18, 14]),
            ("bright-saw", [74, 18, 14]),
            ("soft-flute", [72, 4, 30]),
            ("arcade-pulse", [72, 24, 8]),
            ("electric-piano", [72, 6, 24]),
            ("tubular-bell", [68, 8, 38]),
            ("fm-bass", [78, 0, 8]),
            ("digital-pluck", [72, 20, 18]),
            ("drawbar-organ", [68, 0, 24]),
            ("brass-stack", [72, 10, 18]),
        ];
        for ((id, sends), item) in expected.into_iter().zip(BUILT_INS) {
            let preset = item.preset();
            assert_eq!(item.id, id);
            assert_eq!(
                [
                    preset.level.get(),
                    preset.delay_send.get(),
                    preset.reverb_send.get()
                ],
                sends
            );
            assert_eq!(preset.pan, p(50));
        }
        let dream = find("dream-motion").unwrap().preset();
        assert_eq!(
            for_kind(TrackKind::Chord)
                .map(|item| item.preset().effects.chorus)
                .collect::<Vec<_>>(),
            vec![
                ChorusMode::I,
                ChorusMode::Ii,
                ChorusMode::I,
                ChorusMode::Off,
                ChorusMode::Ii,
                ChorusMode::Ii,
            ]
        );
        assert_eq!(
            dream.effects.phaser,
            PhaserParameters {
                rate: p(15),
                depth: p(55),
                feedback: p(25),
                mix: p(18)
            }
        );
        assert_eq!(dream.lfos.get(ParameterId::Cutoff).unwrap().depth, p(7));
        assert_eq!(dream.lfos.get(ParameterId::Pan).unwrap().depth, p(15));
        let flute = find("soft-flute").unwrap().preset();
        assert_eq!(
            flute.lfos.get(ParameterId::Pitch).unwrap(),
            LfoConfig {
                enabled: true,
                waveform: LfoWaveform::Sine,
                reset_on_trigger: true,
                start_phase: p(0),
                rate: LfoRate::Free {
                    rate_percent: p(82)
                },
                depth: p(8)
            }
        );
        let organ = find("drawbar-organ").unwrap().preset();
        assert!(
            matches!(organ.instrument, Instrument::Fm(FmParameters { algorithm:FmAlgorithm::Additive, operators:[FmOperator { ratio:FmRatio::One, level, .. }, FmOperator { ratio:FmRatio::Two, .. }, FmOperator { ratio:FmRatio::Three, .. }, FmOperator { ratio:FmRatio::Six, .. }], brightness, .. }) if level==p(100) && brightness==p(86))
        );
    }
}
