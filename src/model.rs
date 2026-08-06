use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub const MIN_STEP_COUNT: usize = 1;
pub const MAX_STEP_COUNT: usize = 64;
pub const STEP_BANK_SIZE: usize = 16;
pub const STEP_ROW_SIZE: usize = 32;
pub const TRACK_COUNT: usize = 6;
pub const MIN_PATTERN_COUNT: usize = 1;
pub const MAX_PATTERN_COUNT: usize = 100;

/// Deterministic old-pattern-index to new-pattern-index transform.
///
/// `u8::MAX` represents a removed index and is resolved to the nearest valid
/// index by the consumer.  This belongs to the project/model layer because it
/// is used to keep editor and transport pattern selections aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternIndexMap {
    forward: [u8; MAX_PATTERN_COUNT],
}

impl PatternIndexMap {
    pub fn identity() -> Self {
        Self {
            forward: std::array::from_fn(|i| i as u8),
        }
    }

    pub fn insert(after: usize) -> Self {
        Self {
            forward: std::array::from_fn(|i| if i > after { (i + 1) as u8 } else { i as u8 }),
        }
    }

    pub fn insert_at(at: usize) -> Self {
        Self {
            forward: std::array::from_fn(|i| if i >= at { (i + 1) as u8 } else { i as u8 }),
        }
    }

    pub fn delete(at: usize) -> Self {
        Self {
            forward: std::array::from_fn(|i| {
                if i == at {
                    u8::MAX
                } else if i > at {
                    (i - 1) as u8
                } else {
                    i as u8
                }
            }),
        }
    }

    pub(crate) fn rebase(self, index: usize, next_count: usize) -> usize {
        if next_count == 0 {
            return 0;
        }
        let mapped = self.forward.get(index).copied().unwrap_or(u8::MAX);
        if mapped == u8::MAX {
            index.min(next_count - 1)
        } else {
            usize::from(mapped).min(next_count - 1)
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError {
    #[error("unsupported format_version {0}")]
    Version(u32),
    #[error("tracks: expected exactly six tracks")]
    TrackCount,
    #[error("patterns: expected between 1 and 100 patterns, got {0}")]
    PatternCount(usize),
    #[error("tracks[{0}]: expected {1}")]
    TrackOrder(usize, &'static str),
    #[error("tracks[{0}].steps: expected between 1 and 64 steps, got {1}")]
    StepCount(usize, usize),
    #[error("tracks[{0}].steps[{1}]: event is incompatible with track")]
    EventKind(usize, usize),
    #[error("tracks[{0}].steps[{1}].locks: incompatible lock `{2}`")]
    Lock(usize, usize, &'static str),
    #[error("tracks[{0}].steps[{1}]: tie has no source note")]
    Tie(usize, usize),
    #[error("tracks[{0}]: a sequence containing only ties is invalid")]
    AllTies(usize),
    #[error("tracks[{0}].lfos: incompatible destination `{1}`")]
    Lfo(usize, &'static str),
    #[error("tracks[{0}]: swing must be between 0 and 75%")]
    Swing(usize),
    #[error("tracks[{0}].effects: invalid {1} range")]
    Effect(usize, &'static str),
    #[error("tracks[{0}].steps[{1}]: invalid trigger condition")]
    Condition(usize, usize),
    #[error("tracks[{0}].steps[{1}]: retrigger_count must be between 1 and 4")]
    Retrigger(usize, usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Percent(u8);

impl Percent {
    pub const ZERO: Self = Self(0);
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 100 {
            Some(Self(value))
        } else {
            None
        }
    }
    pub const fn get(self) -> u8 {
        self.0
    }
    pub fn saturating_add(self, delta: i16) -> Self {
        Self((self.0 as i16 + delta).clamp(0, 100) as u8)
    }
    pub fn normalized(self) -> f32 {
        self.0 as f32 / 100.0
    }
}

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.0)
    }
}
impl Serialize for Percent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.0)
    }
}
impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u8::deserialize(d)?;
        Self::new(n).ok_or_else(|| serde::de::Error::custom("percentage must be between 0 and 100"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PitchClass {
    #[serde(rename = "C")]
    C,
    #[serde(rename = "C#")]
    CSharp,
    #[serde(rename = "D")]
    D,
    #[serde(rename = "D#")]
    DSharp,
    #[serde(rename = "E")]
    E,
    #[serde(rename = "F")]
    F,
    #[serde(rename = "F#")]
    FSharp,
    #[serde(rename = "G")]
    G,
    #[serde(rename = "G#")]
    GSharp,
    #[serde(rename = "A")]
    A,
    #[serde(rename = "A#")]
    ASharp,
    #[serde(rename = "B")]
    B,
}
impl PitchClass {
    pub const ALL: [Self; 12] = [
        Self::C,
        Self::CSharp,
        Self::D,
        Self::DSharp,
        Self::E,
        Self::F,
        Self::FSharp,
        Self::G,
        Self::GSharp,
        Self::A,
        Self::ASharp,
        Self::B,
    ];
    pub const fn semitone(self) -> i32 {
        self as i32
    }
    pub fn shifted(self, delta: i32) -> Self {
        Self::ALL[(self.semitone() + delta).rem_euclid(12) as usize]
    }
}
impl fmt::Display for PitchClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            [
                "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"
            ][*self as usize]
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    Major,
    NaturalMinor,
}
impl Scale {
    pub const fn offsets(self) -> [i32; 8] {
        match self {
            Self::Major => [0, 2, 4, 5, 7, 9, 11, 12],
            Self::NaturalMinor => [0, 2, 3, 5, 7, 8, 10, 12],
        }
    }
}
impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Major => "Major",
                Self::NaturalMinor => "Natural minor",
            }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Waveform {
    Square,
    Saw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoWaveform {
    Sine,
    Triangle,
    Square,
    Saw,
    SampleAndHold,
}

impl LfoWaveform {
    pub const ALL: [Self; 5] = [
        Self::Sine,
        Self::Triangle,
        Self::Square,
        Self::Saw,
        Self::SampleAndHold,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoDivision {
    FourBars,
    TwoBars,
    Bar,
    Half,
    QuarterDotted,
    Quarter,
    QuarterTriplet,
    EighthDotted,
    Eighth,
    EighthTriplet,
    Sixteenth,
    SixteenthTriplet,
    ThirtySecond,
}

impl LfoDivision {
    pub const ALL: [Self; 13] = [
        Self::FourBars,
        Self::TwoBars,
        Self::Bar,
        Self::Half,
        Self::QuarterDotted,
        Self::Quarter,
        Self::QuarterTriplet,
        Self::EighthDotted,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
    ];

    pub const fn beats(self) -> f32 {
        match self {
            Self::FourBars => 16.0,
            Self::TwoBars => 8.0,
            Self::Bar => 4.0,
            Self::Half => 2.0,
            Self::QuarterDotted => 1.5,
            Self::Quarter => 1.0,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::EighthDotted => 0.75,
            Self::Eighth => 0.5,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Sixteenth => 0.25,
            Self::SixteenthTriplet => 1.0 / 6.0,
            Self::ThirtySecond => 0.125,
        }
    }
}

impl fmt::Display for LfoDivision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::FourBars => "4 bars",
            Self::TwoBars => "2 bars",
            Self::Bar => "1 bar",
            Self::Half => "1/2",
            Self::QuarterDotted => "1/4D",
            Self::Quarter => "1/4",
            Self::QuarterTriplet => "1/4T",
            Self::EighthDotted => "1/8D",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
            Self::SixteenthTriplet => "1/16T",
            Self::ThirtySecond => "1/32",
        };
        write!(f, "{text}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LfoRate {
    Synced { division: LfoDivision },
    Free { rate_percent: Percent },
}

impl LfoRate {
    pub fn hz(self, tempo_bpm: u16) -> f32 {
        match self {
            Self::Synced { division } => tempo_bpm as f32 / (60.0 * division.beats()),
            Self::Free { rate_percent } => 0.01 * 2000.0_f32.powf(rate_percent.normalized()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfoConfig {
    pub enabled: bool,
    pub waveform: LfoWaveform,
    pub rate: LfoRate,
    pub depth: Percent,
}

impl Default for LfoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            waveform: LfoWaveform::Sine,
            rate: LfoRate::Synced {
                division: LfoDivision::Quarter,
            },
            depth: Percent(10),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayDivision {
    ThirtySecond,
    SixteenthTriplet,
    Sixteenth,
    EighthTriplet,
    Eighth,
    EighthDotted,
    QuarterTriplet,
    Quarter,
    QuarterDotted,
    Half,
    Bar,
}
impl DelayDivision {
    pub const ALL: [Self; 11] = [
        Self::ThirtySecond,
        Self::SixteenthTriplet,
        Self::Sixteenth,
        Self::EighthTriplet,
        Self::Eighth,
        Self::EighthDotted,
        Self::QuarterTriplet,
        Self::Quarter,
        Self::QuarterDotted,
        Self::Half,
        Self::Bar,
    ];
    pub const fn beats(self) -> f64 {
        match self {
            Self::ThirtySecond => 0.125,
            Self::SixteenthTriplet => 1.0 / 6.0,
            Self::Sixteenth => 0.25,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Eighth => 0.5,
            Self::EighthDotted => 0.75,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::Quarter => 1.0,
            Self::QuarterDotted => 1.5,
            Self::Half => 2.0,
            Self::Bar => 4.0,
        }
    }
    pub fn samples(self, bpm: u16, sample_rate: u32) -> f64 {
        sample_rate as f64 * 60.0 / bpm as f64 * self.beats()
    }
}
impl fmt::Display for DelayDivision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::ThirtySecond => "1/32",
            Self::SixteenthTriplet => "1/16T",
            Self::Sixteenth => "1/16",
            Self::EighthTriplet => "1/8T",
            Self::Eighth => "1/8",
            Self::EighthDotted => "1/8D",
            Self::QuarterTriplet => "1/4T",
            Self::Quarter => "1/4",
            Self::QuarterDotted => "1/4D",
            Self::Half => "1/2",
            Self::Bar => "1 bar",
        };
        write!(f, "{s}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Globals {
    pub tempo_bpm: u16,
    pub delay_division: DelayDivision,
    pub delay_feedback: Percent,
    pub reverb_time_seconds: f32,
    #[serde(default = "default_reverb_tone")]
    pub reverb_tone: Percent,
    #[serde(default = "default_reverb_pre_delay_ms")]
    pub reverb_pre_delay_ms: u16,
    pub key: PitchClass,
    pub scale: Scale,
}

fn default_reverb_tone() -> Percent {
    Percent(40)
}

fn default_reverb_pre_delay_ms() -> u16 {
    20
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalParameterId {
    Tempo,
    DelayDivision,
    DelayFeedback,
    ReverbTime,
    ReverbTone,
    ReverbPreDelay,
    Key,
    Scale,
}
impl Default for Globals {
    fn default() -> Self {
        Self {
            tempo_bpm: 120,
            delay_division: DelayDivision::Eighth,
            delay_feedback: Percent(30),
            reverb_time_seconds: 2.5,
            reverb_tone: default_reverb_tone(),
            reverb_pre_delay_ms: default_reverb_pre_delay_ms(),
            key: PitchClass::C,
            scale: Scale::Major,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Kick,
    Snare,
    Hat,
    Bass,
    Chord,
    Lead,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordSpread {
    #[default]
    Off,
    Narrow,
    Wide,
}

impl ChordSpread {
    pub const ALL: [Self; 3] = [Self::Off, Self::Narrow, Self::Wide];
    pub const fn percent(self) -> Percent {
        match self {
            Self::Off => Percent::ZERO,
            Self::Narrow => Percent(50),
            Self::Wide => Percent(100),
        }
    }
}

impl fmt::Display for ChordSpread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Off => "Off",
                Self::Narrow => "Narrow",
                Self::Wide => "Wide",
            }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordShape {
    #[default]
    TriadRoot,
    TriadFirstInversion,
    TriadSecondInversion,
    SeventhRoot,
    SeventhFirstInversion,
    SeventhSecondInversion,
    SeventhThirdInversion,
    SixthRoot,
    SixthFirstInversion,
    SixthSecondInversion,
    SixthThirdInversion,
    Sus2Root,
    Sus2FirstInversion,
    Sus2SecondInversion,
    Sus4Root,
    Sus4FirstInversion,
    Sus4SecondInversion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArpeggioType {
    #[default]
    Up,
    Down,
    UpDown,
    DownUp,
    Random,
}

impl ArpeggioType {
    pub const ALL: [Self; 5] = [
        Self::Up,
        Self::Down,
        Self::UpDown,
        Self::DownUp,
        Self::Random,
    ];
}

impl fmt::Display for ArpeggioType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Up => "Up",
                Self::Down => "Down",
                Self::UpDown => "Up-Down",
                Self::DownUp => "Down-Up",
                Self::Random => "Random",
            }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArpeggioRate {
    ThirtySecond,
    SixteenthTriplet,
    #[default]
    Sixteenth,
    EighthTriplet,
    Eighth,
    QuarterTriplet,
    Quarter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArpeggioConfig {
    pub enabled: bool,
    #[serde(rename = "type")]
    pub r#type: ArpeggioType,
    pub rate: ArpeggioRate,
}

impl ArpeggioConfig {
    pub fn is_default(&self) -> bool {
        !self.enabled && self.r#type == ArpeggioType::Up && self.rate == ArpeggioRate::Sixteenth
    }
}

impl ArpeggioRate {
    pub const ALL: [Self; 7] = [
        Self::ThirtySecond,
        Self::SixteenthTriplet,
        Self::Sixteenth,
        Self::EighthTriplet,
        Self::Eighth,
        Self::QuarterTriplet,
        Self::Quarter,
    ];

    pub const fn beats(self) -> f64 {
        match self {
            Self::ThirtySecond => 0.125,
            Self::SixteenthTriplet => 1.0 / 6.0,
            Self::Sixteenth => 0.25,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Eighth => 0.5,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::Quarter => 1.0,
        }
    }
}

impl fmt::Display for ArpeggioRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::ThirtySecond => "1/32",
                Self::SixteenthTriplet => "1/16T",
                Self::Sixteenth => "1/16",
                Self::EighthTriplet => "1/8T",
                Self::Eighth => "1/8",
                Self::QuarterTriplet => "1/4T",
                Self::Quarter => "1/4",
            }
        )
    }
}

impl ChordShape {
    pub const ALL: [Self; 17] = [
        Self::TriadRoot,
        Self::TriadFirstInversion,
        Self::TriadSecondInversion,
        Self::SeventhRoot,
        Self::SeventhFirstInversion,
        Self::SeventhSecondInversion,
        Self::SeventhThirdInversion,
        Self::SixthRoot,
        Self::SixthFirstInversion,
        Self::SixthSecondInversion,
        Self::SixthThirdInversion,
        Self::Sus2Root,
        Self::Sus2FirstInversion,
        Self::Sus2SecondInversion,
        Self::Sus4Root,
        Self::Sus4FirstInversion,
        Self::Sus4SecondInversion,
    ];

    pub const fn degrees(self) -> &'static [u8] {
        match self {
            Self::TriadRoot => &[1, 3, 5],
            Self::TriadFirstInversion => &[3, 5, 1],
            Self::TriadSecondInversion => &[5, 1, 3],
            Self::SeventhRoot => &[1, 3, 5, 7],
            Self::SeventhFirstInversion => &[3, 5, 7, 1],
            Self::SeventhSecondInversion => &[5, 7, 1, 3],
            Self::SeventhThirdInversion => &[7, 1, 3, 5],
            Self::SixthRoot => &[1, 3, 5, 6],
            Self::SixthFirstInversion => &[3, 5, 6, 1],
            Self::SixthSecondInversion => &[5, 6, 1, 3],
            Self::SixthThirdInversion => &[6, 1, 3, 5],
            Self::Sus2Root => &[1, 2, 5],
            Self::Sus2FirstInversion => &[2, 5, 1],
            Self::Sus2SecondInversion => &[5, 1, 2],
            Self::Sus4Root => &[1, 4, 5],
            Self::Sus4FirstInversion => &[4, 5, 1],
            Self::Sus4SecondInversion => &[5, 1, 4],
        }
    }
}

impl fmt::Display for ChordShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self
            .degrees()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join("-");
        write!(f, "{text}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KickParameters {
    pub tune: Percent,
    pub decay: Percent,
    pub attack: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnareParameters {
    pub tune: Percent,
    pub tone: Percent,
    pub snappy: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HatParameters {
    pub tune: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BassParameters {
    pub waveform: Waveform,
    pub cutoff: Percent,
    pub resonance: Percent,
    pub filter_envelope: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChordParameters {
    pub oscillator_mix: Percent,
    pub pulse_width: Percent,
    pub sub_oscillator: Percent,
    pub chorus: ChorusMode,
    #[serde(default)]
    pub spread: ChordSpread,
    pub cutoff: Percent,
    pub resonance: Percent,
    pub filter_envelope: Percent,
    pub attack: Percent,
    pub decay: Percent,
    pub sustain: Percent,
    pub release: Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeadParameters {
    pub oscillator_mix: Percent,
    pub pulse_width: Percent,
    pub sub_oscillator: Percent,
    pub cutoff: Percent,
    pub resonance: Percent,
    pub filter_envelope: Percent,
    pub attack: Percent,
    pub decay: Percent,
    pub sustain: Percent,
    pub release: Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistortionParameters {
    pub drive: Percent,
    pub tone: Percent,
    pub mix: Percent,
}

impl Default for DistortionParameters {
    fn default() -> Self {
        Self {
            drive: Percent::ZERO,
            tone: Percent(50),
            mix: Percent::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaserParameters {
    pub rate: Percent,
    pub depth: Percent,
    pub feedback: Percent,
    pub mix: Percent,
}

impl Default for PhaserParameters {
    fn default() -> Self {
        Self {
            rate: Percent(25),
            depth: Percent(50),
            feedback: Percent(20),
            mix: Percent::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackEffects {
    #[serde(default)]
    pub distortion: DistortionParameters,
    #[serde(default)]
    pub phaser: PhaserParameters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChorusMode {
    Off,
    I,
    Ii,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Instrument {
    Kick(KickParameters),
    Snare(SnareParameters),
    Hat(HatParameters),
    Bass(BassParameters),
    Chord(ChordParameters),
    Lead(LeadParameters),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterLocks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay_send: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverb_send: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distortion_drive: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distortion_tone: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distortion_mix: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phaser_rate: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phaser_depth: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phaser_feedback: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phaser_mix: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tune: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snappy: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Waveform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oscillator_mix: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pulse_width: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_oscillator: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chorus: Option<ChorusMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<ChordSpread>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resonance: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_envelope: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sustain: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<Percent>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfoAssignments {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tune: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snappy: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oscillator_mix: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pulse_width: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_oscillator: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resonance: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_envelope: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sustain: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<LfoConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<LfoConfig>,
}

impl LfoAssignments {
    pub fn get(&self, parameter: ParameterId) -> Option<LfoConfig> {
        match parameter {
            ParameterId::Pan => self.pan,
            ParameterId::Level => self.level,
            ParameterId::Tune => self.tune,
            ParameterId::Tone => self.tone,
            ParameterId::Snappy => self.snappy,
            ParameterId::Decay => self.decay,
            ParameterId::OscillatorMix => self.oscillator_mix,
            ParameterId::PulseWidth => self.pulse_width,
            ParameterId::SubOscillator => self.sub_oscillator,
            ParameterId::Cutoff => self.cutoff,
            ParameterId::Resonance => self.resonance,
            ParameterId::FilterEnvelope => self.filter_envelope,
            ParameterId::Attack => self.attack,
            ParameterId::Sustain => self.sustain,
            ParameterId::Release => self.release,
            ParameterId::Pitch => self.pitch,
            ParameterId::DelaySend
            | ParameterId::ReverbSend
            | ParameterId::DistortionDrive
            | ParameterId::DistortionTone
            | ParameterId::DistortionMix
            | ParameterId::PhaserRate
            | ParameterId::PhaserDepth
            | ParameterId::PhaserFeedback
            | ParameterId::PhaserMix
            | ParameterId::Waveform
            | ParameterId::Chorus
            | ParameterId::Spread => None,
        }
    }

    pub fn set(&mut self, parameter: ParameterId, config: Option<LfoConfig>) -> bool {
        let slot = match parameter {
            ParameterId::Pan => &mut self.pan,
            ParameterId::Level => &mut self.level,
            ParameterId::Tune => &mut self.tune,
            ParameterId::Tone => &mut self.tone,
            ParameterId::Snappy => &mut self.snappy,
            ParameterId::Decay => &mut self.decay,
            ParameterId::OscillatorMix => &mut self.oscillator_mix,
            ParameterId::PulseWidth => &mut self.pulse_width,
            ParameterId::SubOscillator => &mut self.sub_oscillator,
            ParameterId::Cutoff => &mut self.cutoff,
            ParameterId::Resonance => &mut self.resonance,
            ParameterId::FilterEnvelope => &mut self.filter_envelope,
            ParameterId::Attack => &mut self.attack,
            ParameterId::Sustain => &mut self.sustain,
            ParameterId::Release => &mut self.release,
            ParameterId::Pitch => &mut self.pitch,
            ParameterId::DelaySend
            | ParameterId::ReverbSend
            | ParameterId::DistortionDrive
            | ParameterId::DistortionTone
            | ParameterId::DistortionMix
            | ParameterId::PhaserRate
            | ParameterId::PhaserDepth
            | ParameterId::PhaserFeedback
            | ParameterId::PhaserMix
            | ParameterId::Waveform
            | ParameterId::Chorus
            | ParameterId::Spread => {
                return false;
            }
        };
        *slot = config;
        true
    }
}
impl ParameterLocks {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn get(&self, parameter: ParameterId) -> Option<ParameterValue> {
        match parameter {
            ParameterId::Pan => self.pan.map(ParameterValue::Percent),
            ParameterId::Level => self.level.map(ParameterValue::Percent),
            ParameterId::DelaySend => self.delay_send.map(ParameterValue::Percent),
            ParameterId::ReverbSend => self.reverb_send.map(ParameterValue::Percent),
            ParameterId::DistortionDrive => self.distortion_drive.map(ParameterValue::Percent),
            ParameterId::DistortionTone => self.distortion_tone.map(ParameterValue::Percent),
            ParameterId::DistortionMix => self.distortion_mix.map(ParameterValue::Percent),
            ParameterId::PhaserRate => self.phaser_rate.map(ParameterValue::Percent),
            ParameterId::PhaserDepth => self.phaser_depth.map(ParameterValue::Percent),
            ParameterId::PhaserFeedback => self.phaser_feedback.map(ParameterValue::Percent),
            ParameterId::PhaserMix => self.phaser_mix.map(ParameterValue::Percent),
            ParameterId::Tune => self.tune.map(ParameterValue::Percent),
            ParameterId::Tone => self.tone.map(ParameterValue::Percent),
            ParameterId::Snappy => self.snappy.map(ParameterValue::Percent),
            ParameterId::Decay => self.decay.map(ParameterValue::Percent),
            ParameterId::Waveform => self.waveform.map(ParameterValue::Waveform),
            ParameterId::OscillatorMix => self.oscillator_mix.map(ParameterValue::Percent),
            ParameterId::PulseWidth => self.pulse_width.map(ParameterValue::Percent),
            ParameterId::SubOscillator => self.sub_oscillator.map(ParameterValue::Percent),
            ParameterId::Chorus => self.chorus.map(ParameterValue::Chorus),
            ParameterId::Spread => self.spread.map(ParameterValue::Spread),
            ParameterId::Cutoff => self.cutoff.map(ParameterValue::Percent),
            ParameterId::Resonance => self.resonance.map(ParameterValue::Percent),
            ParameterId::FilterEnvelope => self.filter_envelope.map(ParameterValue::Percent),
            ParameterId::Attack => self.attack.map(ParameterValue::Percent),
            ParameterId::Sustain => self.sustain.map(ParameterValue::Percent),
            ParameterId::Release => self.release.map(ParameterValue::Percent),
            ParameterId::Pitch => None,
        }
    }

    pub fn set(&mut self, parameter: ParameterId, value: ParameterValue) -> bool {
        match (parameter, value) {
            (ParameterId::Pan, ParameterValue::Percent(v)) => self.pan = Some(v),
            (ParameterId::Level, ParameterValue::Percent(v)) => self.level = Some(v),
            (ParameterId::DelaySend, ParameterValue::Percent(v)) => self.delay_send = Some(v),
            (ParameterId::ReverbSend, ParameterValue::Percent(v)) => self.reverb_send = Some(v),
            (ParameterId::DistortionDrive, ParameterValue::Percent(v)) => {
                self.distortion_drive = Some(v)
            }
            (ParameterId::DistortionTone, ParameterValue::Percent(v)) => {
                self.distortion_tone = Some(v)
            }
            (ParameterId::DistortionMix, ParameterValue::Percent(v)) => {
                self.distortion_mix = Some(v)
            }
            (ParameterId::PhaserRate, ParameterValue::Percent(v)) => self.phaser_rate = Some(v),
            (ParameterId::PhaserDepth, ParameterValue::Percent(v)) => self.phaser_depth = Some(v),
            (ParameterId::PhaserFeedback, ParameterValue::Percent(v)) => {
                if v.get() > 90 {
                    return false;
                }
                self.phaser_feedback = Some(v)
            }
            (ParameterId::PhaserMix, ParameterValue::Percent(v)) => self.phaser_mix = Some(v),
            (ParameterId::Tune, ParameterValue::Percent(v)) => self.tune = Some(v),
            (ParameterId::Tone, ParameterValue::Percent(v)) => self.tone = Some(v),
            (ParameterId::Snappy, ParameterValue::Percent(v)) => self.snappy = Some(v),
            (ParameterId::Decay, ParameterValue::Percent(v)) => self.decay = Some(v),
            (ParameterId::Waveform, ParameterValue::Waveform(v)) => self.waveform = Some(v),
            (ParameterId::OscillatorMix, ParameterValue::Percent(v)) => {
                self.oscillator_mix = Some(v)
            }
            (ParameterId::PulseWidth, ParameterValue::Percent(v)) => self.pulse_width = Some(v),
            (ParameterId::SubOscillator, ParameterValue::Percent(v)) => {
                self.sub_oscillator = Some(v)
            }
            (ParameterId::Chorus, ParameterValue::Chorus(v)) => self.chorus = Some(v),
            (ParameterId::Spread, ParameterValue::Spread(v)) => self.spread = Some(v),
            (ParameterId::Cutoff, ParameterValue::Percent(v)) => self.cutoff = Some(v),
            (ParameterId::Resonance, ParameterValue::Percent(v)) => self.resonance = Some(v),
            (ParameterId::FilterEnvelope, ParameterValue::Percent(v)) => {
                self.filter_envelope = Some(v)
            }
            (ParameterId::Attack, ParameterValue::Percent(v)) => self.attack = Some(v),
            (ParameterId::Sustain, ParameterValue::Percent(v)) => self.sustain = Some(v),
            (ParameterId::Release, ParameterValue::Percent(v)) => self.release = Some(v),
            _ => return false,
        }
        true
    }

    pub fn clear(&mut self, parameter: ParameterId) {
        match parameter {
            ParameterId::Pan => self.pan = None,
            ParameterId::Level => self.level = None,
            ParameterId::DelaySend => self.delay_send = None,
            ParameterId::ReverbSend => self.reverb_send = None,
            ParameterId::DistortionDrive => self.distortion_drive = None,
            ParameterId::DistortionTone => self.distortion_tone = None,
            ParameterId::DistortionMix => self.distortion_mix = None,
            ParameterId::PhaserRate => self.phaser_rate = None,
            ParameterId::PhaserDepth => self.phaser_depth = None,
            ParameterId::PhaserFeedback => self.phaser_feedback = None,
            ParameterId::PhaserMix => self.phaser_mix = None,
            ParameterId::Tune => self.tune = None,
            ParameterId::Tone => self.tone = None,
            ParameterId::Snappy => self.snappy = None,
            ParameterId::Decay => self.decay = None,
            ParameterId::Waveform => self.waveform = None,
            ParameterId::OscillatorMix => self.oscillator_mix = None,
            ParameterId::PulseWidth => self.pulse_width = None,
            ParameterId::SubOscillator => self.sub_oscillator = None,
            ParameterId::Chorus => self.chorus = None,
            ParameterId::Spread => self.spread = None,
            ParameterId::Cutoff => self.cutoff = None,
            ParameterId::Resonance => self.resonance = None,
            ParameterId::FilterEnvelope => self.filter_envelope = None,
            ParameterId::Attack => self.attack = None,
            ParameterId::Sustain => self.sustain = None,
            ParameterId::Release => self.release = None,
            ParameterId::Pitch => {}
        }
    }

    pub fn overlay(&mut self, overlay: Self) {
        for parameter in ParameterId::ALL {
            if let Some(value) = overlay.get(parameter) {
                let set = self.set(parameter, value);
                debug_assert!(set);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TriggerCondition {
    #[default]
    Always,
    Cycle {
        position: u8,
        length: u8,
    },
    Chance {
        probability: Percent,
    },
}

impl TriggerCondition {
    pub const fn valid(self) -> bool {
        match self {
            Self::Always | Self::Chance { .. } => true,
            Self::Cycle { position, length } => {
                length >= 2 && length <= 4 && position >= 1 && position <= length
            }
        }
    }
}

impl fmt::Display for TriggerCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Always => write!(f, "Always"),
            Self::Cycle { position, length } => write!(f, "Cycle {position}/{length}"),
            Self::Chance { probability } => write!(f, "Chance {probability}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepEvent {
    Trigger {
        accent: bool,
        #[serde(default, skip_serializing_if = "trigger_condition_is_default")]
        condition: TriggerCondition,
        #[serde(
            default = "default_retrigger_count",
            skip_serializing_if = "retrigger_count_is_default"
        )]
        retrigger_count: u8,
        locks: ParameterLocks,
    },
    BassNote {
        degree: u8,
        octave: u8,
        accent: bool,
        slide: bool,
        #[serde(default, skip_serializing_if = "trigger_condition_is_default")]
        condition: TriggerCondition,
        #[serde(
            default = "default_retrigger_count",
            skip_serializing_if = "retrigger_count_is_default"
        )]
        retrigger_count: u8,
        locks: ParameterLocks,
    },
    Note {
        degree: u8,
        octave: u8,
        accent: bool,
        #[serde(default, skip_serializing_if = "chord_shape_is_default")]
        chord_shape: Option<ChordShape>,
        #[serde(default, skip_serializing_if = "ArpeggioConfig::is_default")]
        arpeggio: ArpeggioConfig,
        #[serde(default, skip_serializing_if = "trigger_condition_is_default")]
        condition: TriggerCondition,
        #[serde(
            default = "default_retrigger_count",
            skip_serializing_if = "retrigger_count_is_default"
        )]
        retrigger_count: u8,
        locks: ParameterLocks,
    },
    Tie {
        locks: ParameterLocks,
    },
}
fn trigger_condition_is_default(value: &TriggerCondition) -> bool {
    *value == TriggerCondition::Always
}
fn default_retrigger_count() -> u8 {
    1
}
fn retrigger_count_is_default(value: &u8) -> bool {
    *value == 1
}
impl StepEvent {
    pub fn locks(&self) -> &ParameterLocks {
        match self {
            Self::Trigger { locks, .. }
            | Self::BassNote { locks, .. }
            | Self::Note { locks, .. }
            | Self::Tie { locks } => locks,
        }
    }
    pub fn locks_mut(&mut self) -> &mut ParameterLocks {
        match self {
            Self::Trigger { locks, .. }
            | Self::BassNote { locks, .. }
            | Self::Note { locks, .. }
            | Self::Tie { locks } => locks,
        }
    }

    pub fn accent(&self) -> Option<bool> {
        match self {
            Self::Trigger { accent, .. }
            | Self::BassNote { accent, .. }
            | Self::Note { accent, .. } => Some(*accent),
            Self::Tie { .. } => None,
        }
    }

    pub fn accent_mut(&mut self) -> Option<&mut bool> {
        match self {
            Self::Trigger { accent, .. }
            | Self::BassNote { accent, .. }
            | Self::Note { accent, .. } => Some(accent),
            Self::Tie { .. } => None,
        }
    }

    pub fn slide_mut(&mut self) -> Option<&mut bool> {
        match self {
            Self::BassNote { slide, .. } => Some(slide),
            _ => None,
        }
    }
    pub fn condition(&self) -> Option<TriggerCondition> {
        match self {
            Self::Trigger { condition, .. }
            | Self::BassNote { condition, .. }
            | Self::Note { condition, .. } => Some(*condition),
            Self::Tie { .. } => None,
        }
    }
    pub fn condition_mut(&mut self) -> Option<&mut TriggerCondition> {
        match self {
            Self::Trigger { condition, .. }
            | Self::BassNote { condition, .. }
            | Self::Note { condition, .. } => Some(condition),
            Self::Tie { .. } => None,
        }
    }
    pub fn retrigger_count(&self) -> Option<u8> {
        match self {
            Self::Trigger {
                retrigger_count, ..
            }
            | Self::BassNote {
                retrigger_count, ..
            }
            | Self::Note {
                retrigger_count, ..
            } => Some(*retrigger_count),
            Self::Tie { .. } => None,
        }
    }
    pub fn retrigger_count_mut(&mut self) -> Option<&mut u8> {
        match self {
            Self::Trigger {
                retrigger_count, ..
            }
            | Self::BassNote {
                retrigger_count, ..
            }
            | Self::Note {
                retrigger_count, ..
            } => Some(retrigger_count),
            Self::Tie { .. } => None,
        }
    }
}
pub type Step = Option<StepEvent>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub kind: TrackKind,
    pub name: String,
    pub level: Percent,
    #[serde(default = "default_pan")]
    pub pan: Percent,
    pub muted: bool,
    pub delay_send: Percent,
    pub reverb_send: Percent,
    #[serde(default)]
    pub swing: Percent,
    pub instrument: Instrument,
    #[serde(default)]
    pub effects: TrackEffects,
    pub lfos: LfoAssignments,
    /// Transient editor cache for the selected pattern.  It is deliberately
    /// excluded from v11 JSON; canonical sequence data is `patterns`.
    #[serde(skip, default = "default_step_cache")]
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_degree: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_octave: Option<u8>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub input_accent: bool,
    #[serde(default, skip_serializing_if = "chord_shape_is_default")]
    pub input_chord_shape: Option<ChordShape>,
    #[serde(default, skip_serializing_if = "arpeggio_is_default")]
    pub input_chord_arpeggio: Option<ArpeggioConfig>,
}

/// A pattern owns only its six sequences. Instrument and mixer settings live
/// on the project tracks and are consequently shared by every pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub tracks: Vec<PatternTrack>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternTrack {
    pub steps: Vec<Step>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SongEntry {
    /// One-based, matching the pattern labels shown in the UI.
    pub pattern: u8,
    pub bars: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub format_version: u32,
    pub globals: Globals,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub song: Vec<SongEntry>,
}

impl PartialEq for Project {
    fn eq(&self, other: &Self) -> bool {
        self.format_version == other.format_version
            && self.globals == other.globals
            && self.tracks.len() == other.tracks.len()
            && self.tracks.iter().zip(&other.tracks).all(|(left, right)| {
                left.kind == right.kind
                    && left.name == right.name
                    && left.level == right.level
                    && left.pan == right.pan
                    && left.muted == right.muted
                    && left.delay_send == right.delay_send
                    && left.reverb_send == right.reverb_send
                    && left.swing == right.swing
                    && left.instrument == right.instrument
                    && left.effects == right.effects
                    && left.lfos == right.lfos
                    && left.input_degree == right.input_degree
                    && left.input_octave == right.input_octave
                    && left.input_accent == right.input_accent
                    && left.input_chord_shape == right.input_chord_shape
                    && left.input_chord_arpeggio == right.input_chord_arpeggio
            })
            && self.patterns == other.patterns
            && self.song == other.song
    }
}

fn p(n: u8) -> Percent {
    Percent(n)
}
fn default_pan() -> Percent {
    Percent(50)
}
fn default_step_cache() -> Vec<Step> {
    vec![None; STEP_BANK_SIZE]
}
fn bool_is_false(value: &bool) -> bool {
    !*value
}
fn chord_shape_is_default(value: &Option<ChordShape>) -> bool {
    value.is_none() || value == &Some(ChordShape::default())
}
fn arpeggio_is_default(value: &Option<ArpeggioConfig>) -> bool {
    value.is_none() || value.is_some_and(|config| config.is_default())
}
impl Project {
    pub fn new() -> Self {
        let track = |kind: TrackKind, name: &str, instrument: Instrument| Track {
            kind,
            name: name.into(),
            level: p(80),
            pan: default_pan(),
            muted: false,
            delay_send: p(0),
            reverb_send: p(0),
            swing: p(0),
            instrument,
            effects: TrackEffects::default(),
            lfos: LfoAssignments::default(),
            steps: vec![None; STEP_BANK_SIZE],
            input_degree: None,
            input_octave: None,
            input_accent: false,
            input_chord_shape: None,
            input_chord_arpeggio: None,
        };
        let synth = |kind: TrackKind, name: &str, instrument: Instrument| Track {
            kind,
            name: name.into(),
            level: p(80),
            pan: default_pan(),
            muted: false,
            delay_send: p(0),
            reverb_send: match kind {
                TrackKind::Chord | TrackKind::Lead => p(20),
                _ => p(0),
            },
            swing: p(0),
            instrument,
            effects: TrackEffects::default(),
            lfos: LfoAssignments::default(),
            steps: vec![None; STEP_BANK_SIZE],
            input_degree: Some(1),
            input_octave: Some(3),
            input_accent: false,
            input_chord_shape: None,
            input_chord_arpeggio: None,
        };
        Self {
            format_version: 13,
            globals: Globals::default(),
            tracks: vec![
                track(
                    TrackKind::Kick,
                    "Kick",
                    Instrument::Kick(KickParameters {
                        tune: p(50),
                        decay: p(35),
                        attack: p(35),
                    }),
                ),
                track(
                    TrackKind::Snare,
                    "Snare",
                    Instrument::Snare(SnareParameters {
                        tune: p(50),
                        tone: p(50),
                        snappy: p(55),
                    }),
                ),
                track(
                    TrackKind::Hat,
                    "Hi-hat",
                    Instrument::Hat(HatParameters {
                        tune: p(50),
                        decay: p(20),
                    }),
                ),
                synth(
                    TrackKind::Bass,
                    "Bass",
                    Instrument::Bass(BassParameters {
                        waveform: Waveform::Saw,
                        cutoff: p(45),
                        resonance: p(55),
                        filter_envelope: p(65),
                        decay: p(40),
                    }),
                ),
                synth(
                    TrackKind::Chord,
                    "Chord",
                    Instrument::Chord(ChordParameters {
                        oscillator_mix: p(70),
                        pulse_width: p(50),
                        sub_oscillator: p(0),
                        chorus: ChorusMode::I,
                        spread: ChordSpread::Off,
                        cutoff: p(55),
                        resonance: p(15),
                        filter_envelope: p(25),
                        attack: p(55),
                        decay: p(45),
                        sustain: p(75),
                        release: p(65),
                    }),
                ),
                synth(
                    TrackKind::Lead,
                    "Lead",
                    Instrument::Lead(LeadParameters {
                        oscillator_mix: p(75),
                        pulse_width: p(50),
                        sub_oscillator: p(25),
                        cutoff: p(50),
                        resonance: p(35),
                        filter_envelope: p(55),
                        attack: p(0),
                        decay: p(35),
                        sustain: p(55),
                        release: p(20),
                    }),
                ),
            ],
            patterns: vec![Pattern {
                tracks: (0..TRACK_COUNT)
                    .map(|_| PatternTrack {
                        steps: vec![None; STEP_BANK_SIZE],
                    })
                    .collect(),
            }],
            song: vec![SongEntry {
                pattern: 1,
                bars: 1,
            }],
        }
    }
    pub fn pattern_steps(&self, pattern: usize, track: usize) -> Option<&[Step]> {
        self.patterns
            .get(pattern)?
            .tracks
            .get(track)
            .map(|track| track.steps.as_slice())
    }

    /// Synchronize the non-persisted editor cache from a canonical pattern.
    pub fn activate_pattern(&mut self, pattern: usize) -> bool {
        let Some(source) = self.patterns.get(pattern) else {
            return false;
        };
        if source.tracks.len() != TRACK_COUNT {
            return false;
        }
        for (track, sequence) in self.tracks.iter_mut().zip(&source.tracks) {
            track.steps.clone_from(&sequence.steps);
        }
        true
    }

    /// Commit the transient editor cache to canonical sequence ownership.
    pub fn store_active_pattern(&mut self, pattern: usize) -> bool {
        let Some(destination) = self.patterns.get_mut(pattern) else {
            return false;
        };
        if destination.tracks.len() != TRACK_COUNT {
            return false;
        }
        for (sequence, track) in destination.tracks.iter_mut().zip(&self.tracks) {
            sequence.steps.clone_from(&track.steps);
        }
        true
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.format_version != 13 {
            return Err(ValidationError::Version(self.format_version));
        }
        if self.tracks.len() != TRACK_COUNT {
            return Err(ValidationError::TrackCount);
        }
        let expected = [
            (TrackKind::Kick, "Kick"),
            (TrackKind::Snare, "Snare"),
            (TrackKind::Hat, "Hi-hat"),
            (TrackKind::Bass, "Bass"),
            (TrackKind::Chord, "Chord"),
            (TrackKind::Lead, "Lead"),
        ];
        if !(40..=240).contains(&self.globals.tempo_bpm)
            || !self.globals.reverb_time_seconds.is_finite()
            || !(0.2..=10.0).contains(&self.globals.reverb_time_seconds)
            || self.globals.delay_feedback.get() > 95
            || self.globals.reverb_tone.get() > 100
            || self.globals.reverb_pre_delay_ms > 200
        {
            return Err(ValidationError::TrackOrder(0, "valid globals"));
        }
        if !(MIN_PATTERN_COUNT..=MAX_PATTERN_COUNT).contains(&self.patterns.len()) {
            return Err(ValidationError::PatternCount(self.patterns.len()));
        }
        if self.song.is_empty() || self.song.len() > 256 {
            return Err(ValidationError::TrackCount);
        }
        for entry in &self.song {
            if !(1..=self.patterns.len() as u8).contains(&entry.pattern)
                || !(1..=64).contains(&entry.bars)
            {
                return Err(ValidationError::TrackOrder(0, "valid song"));
            }
        }
        for (ti, t) in self.tracks.iter().enumerate() {
            if t.kind != expected[ti].0 || t.name != expected[ti].1 {
                return Err(ValidationError::TrackOrder(ti, expected[ti].1));
            }
            let pitched = matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead);
            let instrument_ok = matches!(
                (t.kind, t.instrument),
                (TrackKind::Kick, Instrument::Kick(_))
                    | (TrackKind::Snare, Instrument::Snare(_))
                    | (TrackKind::Hat, Instrument::Hat(_))
                    | (TrackKind::Bass, Instrument::Bass(_))
                    | (TrackKind::Chord, Instrument::Chord(_))
                    | (TrackKind::Lead, Instrument::Lead(_))
            );
            if !instrument_ok
                || pitched != t.input_degree.is_some()
                || pitched != t.input_octave.is_some()
                || (t.kind != TrackKind::Chord
                    && (t.input_chord_shape.is_some() || t.input_chord_arpeggio.is_some()))
            {
                return Err(ValidationError::TrackOrder(ti, expected[ti].1));
            }
            if t.swing.get() > 75 {
                return Err(ValidationError::Swing(ti));
            }
            if t.effects.phaser.feedback.get() > 90 {
                return Err(ValidationError::Effect(ti, "phaser_feedback"));
            }
            validate_lfos(ti, t)?;
            if let Some(d) = t.input_degree {
                if !(1..=8).contains(&d) {
                    return Err(ValidationError::TrackOrder(ti, "valid input degree"));
                }
            }
            if let Some(o) = t.input_octave {
                if o > 7 {
                    return Err(ValidationError::TrackOrder(ti, "valid input octave"));
                }
            }
        }
        for pattern in &self.patterns {
            if pattern.tracks.len() != TRACK_COUNT {
                return Err(ValidationError::TrackCount);
            }
            for (ti, sequence) in pattern.tracks.iter().enumerate() {
                let track = &self.tracks[ti];
                if !(MIN_STEP_COUNT..=MAX_STEP_COUNT).contains(&sequence.steps.len()) {
                    return Err(ValidationError::StepCount(ti, sequence.steps.len()));
                }
                for (si, event) in sequence.steps.iter().enumerate() {
                    if let Some(event) = event {
                        let event_ok = matches!(
                            (track.kind, event),
                            (
                                TrackKind::Kick | TrackKind::Snare | TrackKind::Hat,
                                StepEvent::Trigger { .. }
                            ) | (TrackKind::Bass, StepEvent::BassNote { .. })
                                | (TrackKind::Bass, StepEvent::Tie { .. })
                                | (TrackKind::Chord | TrackKind::Lead, StepEvent::Note { .. })
                                | (TrackKind::Chord | TrackKind::Lead, StepEvent::Tie { .. })
                        );
                        if !event_ok {
                            return Err(ValidationError::EventKind(ti, si));
                        }
                        if let Some(condition) = event.condition()
                            && !condition.valid()
                        {
                            return Err(ValidationError::Condition(ti, si));
                        }
                        if let Some(count) = event.retrigger_count()
                            && !(1..=4).contains(&count)
                        {
                            return Err(ValidationError::Retrigger(ti, si));
                        }
                        if let StepEvent::Note {
                            chord_shape,
                            arpeggio,
                            ..
                        } = event
                        {
                            if track.kind == TrackKind::Lead
                                && (chord_shape.is_some() || !arpeggio.is_default())
                            {
                                return Err(ValidationError::EventKind(ti, si));
                            }
                        }
                        if let StepEvent::Note { degree, octave, .. }
                        | StepEvent::BassNote { degree, octave, .. } = event
                        {
                            if !(1..=8).contains(degree) || *octave > 7 {
                                return Err(ValidationError::EventKind(ti, si));
                            }
                        }
                        validate_locks(ti, si, track.kind, event.locks())?;
                    }
                }
                if matches!(
                    track.kind,
                    TrackKind::Bass | TrackKind::Chord | TrackKind::Lead
                ) {
                    validate_ties(ti, &sequence.steps)?;
                }
            }
        }
        Ok(())
    }
    pub fn note_midi(&self, degree: u8, octave: u8) -> Option<i32> {
        if !(1..=8).contains(&degree) || octave > 7 {
            return None;
        }
        Some(
            12 * (octave as i32 + 1)
                + self.globals.key.semitone()
                + self.globals.scale.offsets()[degree as usize - 1],
        )
    }
    pub fn note_frequency(&self, degree: u8, octave: u8) -> Option<f32> {
        self.note_midi(degree, octave)
            .map(|m| 440.0 * 2.0_f32.powf((m as f32 - 69.0) / 12.0))
    }

    pub fn chord_midis_for(
        &self,
        degree: u8,
        octave: u8,
        shape: ChordShape,
    ) -> Option<([i32; 4], usize)> {
        if !(1..=8).contains(&degree) || octave > 7 {
            return None;
        }
        let scale = self.globals.scale.offsets();
        let root = degree as usize - 1;
        let mut previous = 0;
        let mut wraps = 0;
        let mut midis = [0; 4];
        for (voice, midi) in midis.iter_mut().enumerate().take(shape.degrees().len()) {
            let chord_degree = shape.degrees()[voice];
            if voice > 0 && chord_degree <= previous {
                wraps += 7;
            }
            previous = chord_degree;
            let scale_degree = root + usize::from(chord_degree - 1) + wraps;
            *midi = 12 * (octave as i32 + 1 + (scale_degree / 7) as i32)
                + self.globals.key.semitone()
                + scale[scale_degree % 7];
        }
        Some((midis, shape.degrees().len()))
    }

    pub fn chord_midis(&self, degree: u8, octave: u8) -> Option<[i32; 3]> {
        let (midis, _) = self.chord_midis_for(degree, octave, ChordShape::default())?;
        Some([midis[0], midis[1], midis[2]])
    }
}
impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_locks(
    ti: usize,
    si: usize,
    kind: TrackKind,
    l: &ParameterLocks,
) -> Result<(), ValidationError> {
    if l.phaser_feedback.is_some_and(|value| value.get() > 90) {
        return Err(ValidationError::Lock(ti, si, "phaser_feedback"));
    }
    let bad = ParameterId::ALL.into_iter().find_map(|parameter| {
        (l.get(parameter).is_some() && !parameter.is_valid_for(kind)).then_some(parameter.name())
    });
    bad.map_or(Ok(()), |name| Err(ValidationError::Lock(ti, si, name)))
}

fn validate_lfos(ti: usize, track: &Track) -> Result<(), ValidationError> {
    for parameter in ParameterId::ALL {
        if track.lfos.get(parameter).is_some() && !track.supports_lfo(parameter) {
            return Err(ValidationError::Lfo(ti, parameter.name()));
        }
    }
    Ok(())
}
pub fn tie_source(steps: &[Step], at: usize) -> Option<usize> {
    if steps.is_empty() || at >= steps.len() {
        return None;
    }
    let step_count = steps.len();
    let mut i = (at + step_count - 1) % step_count;
    for _ in 0..step_count {
        match &steps[i] {
            Some(StepEvent::Note { .. } | StepEvent::BassNote { .. }) => return Some(i),
            Some(StepEvent::Tie { .. }) => i = (i + step_count - 1) % step_count,
            _ => return None,
        }
    }
    None
}
fn validate_ties(track: usize, steps: &[Step]) -> Result<(), ValidationError> {
    let ties = steps
        .iter()
        .filter(|s| matches!(s, Some(StepEvent::Tie { .. })))
        .count();
    if ties == steps.len() {
        return Err(ValidationError::AllTies(track));
    }
    for (i, s) in steps.iter().enumerate() {
        if matches!(s, Some(StepEvent::Tie { .. })) && tie_source(steps, i).is_none() {
            return Err(ValidationError::Tie(track, i));
        }
    }
    Ok(())
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterId {
    Level,
    Pan,
    DelaySend,
    ReverbSend,
    DistortionDrive,
    DistortionTone,
    DistortionMix,
    PhaserRate,
    PhaserDepth,
    PhaserFeedback,
    PhaserMix,
    Tune,
    Tone,
    Snappy,
    Decay,
    Waveform,
    OscillatorMix,
    PulseWidth,
    SubOscillator,
    Chorus,
    Spread,
    Cutoff,
    Resonance,
    FilterEnvelope,
    Attack,
    Sustain,
    Release,
    Pitch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterValue {
    Percent(Percent),
    Waveform(Waveform),
    Chorus(ChorusMode),
    Spread(ChordSpread),
}

impl ParameterId {
    pub const ALL: [Self; 28] = [
        Self::Level,
        Self::Pan,
        Self::DelaySend,
        Self::ReverbSend,
        Self::DistortionDrive,
        Self::DistortionTone,
        Self::DistortionMix,
        Self::PhaserRate,
        Self::PhaserDepth,
        Self::PhaserFeedback,
        Self::PhaserMix,
        Self::Tune,
        Self::Tone,
        Self::Snappy,
        Self::Decay,
        Self::Waveform,
        Self::OscillatorMix,
        Self::PulseWidth,
        Self::SubOscillator,
        Self::Chorus,
        Self::Spread,
        Self::Cutoff,
        Self::Resonance,
        Self::FilterEnvelope,
        Self::Attack,
        Self::Sustain,
        Self::Release,
        Self::Pitch,
    ];

    pub const fn is_valid_for(self, kind: TrackKind) -> bool {
        match self {
            Self::Level
            | Self::Pan
            | Self::DelaySend
            | Self::ReverbSend
            | Self::DistortionDrive
            | Self::DistortionTone
            | Self::DistortionMix
            | Self::PhaserRate
            | Self::PhaserDepth
            | Self::PhaserFeedback
            | Self::PhaserMix => true,
            Self::Tune => matches!(kind, TrackKind::Kick | TrackKind::Snare | TrackKind::Hat),
            Self::Tone | Self::Snappy => matches!(kind, TrackKind::Snare),
            Self::Decay => matches!(
                kind,
                TrackKind::Kick
                    | TrackKind::Hat
                    | TrackKind::Bass
                    | TrackKind::Chord
                    | TrackKind::Lead
            ),
            Self::Attack => matches!(kind, TrackKind::Kick | TrackKind::Chord | TrackKind::Lead),
            Self::Waveform => matches!(kind, TrackKind::Bass),
            Self::OscillatorMix | Self::PulseWidth | Self::SubOscillator => {
                matches!(kind, TrackKind::Chord | TrackKind::Lead)
            }
            Self::Chorus => matches!(kind, TrackKind::Chord),
            Self::Spread => matches!(kind, TrackKind::Chord),
            Self::Cutoff | Self::Resonance | Self::FilterEnvelope => {
                matches!(kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead)
            }
            Self::Sustain | Self::Release => matches!(kind, TrackKind::Chord | TrackKind::Lead),
            Self::Pitch => false,
        }
    }

    pub const fn is_waveform(self) -> bool {
        matches!(self, Self::Waveform)
    }

    pub const fn is_chorus(self) -> bool {
        matches!(self, Self::Chorus)
    }

    pub const fn supports_lfo(self, kind: TrackKind) -> bool {
        (matches!(self, Self::Pitch) && matches!(kind, TrackKind::Chord | TrackKind::Lead))
            || (self.is_valid_for(kind)
                && !matches!(
                    self,
                    Self::DelaySend
                        | Self::ReverbSend
                        | Self::DistortionDrive
                        | Self::DistortionTone
                        | Self::DistortionMix
                        | Self::PhaserRate
                        | Self::PhaserDepth
                        | Self::PhaserFeedback
                        | Self::PhaserMix
                        | Self::Waveform
                        | Self::Chorus
                        | Self::Spread
                ))
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Level => "level",
            Self::Pan => "pan",
            Self::DelaySend => "delay_send",
            Self::ReverbSend => "reverb_send",
            Self::DistortionDrive => "distortion_drive",
            Self::DistortionTone => "distortion_tone",
            Self::DistortionMix => "distortion_mix",
            Self::PhaserRate => "phaser_rate",
            Self::PhaserDepth => "phaser_depth",
            Self::PhaserFeedback => "phaser_feedback",
            Self::PhaserMix => "phaser_mix",
            Self::Tune => "tune",
            Self::Tone => "tone",
            Self::Snappy => "snappy",
            Self::Decay => "decay",
            Self::Waveform => "waveform",
            Self::OscillatorMix => "oscillator_mix",
            Self::PulseWidth => "pulse_width",
            Self::SubOscillator => "sub_oscillator",
            Self::Chorus => "chorus",
            Self::Spread => "spread",
            Self::Cutoff => "cutoff",
            Self::Resonance => "resonance",
            Self::FilterEnvelope => "filter_envelope",
            Self::Attack => "attack",
            Self::Sustain => "sustain",
            Self::Release => "release",
            Self::Pitch => "pitch",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::DelaySend => "delay send",
            Self::ReverbSend => "reverb send",
            Self::Pan => "pan",
            Self::FilterEnvelope => "filter envelope",
            Self::OscillatorMix => "oscillator mix",
            Self::PulseWidth => "pulse width",
            Self::SubOscillator => "sub oscillator",
            Self::DistortionDrive => "distortion drive",
            Self::DistortionTone => "distortion tone",
            Self::DistortionMix => "distortion mix",
            Self::PhaserRate => "phaser rate",
            Self::PhaserDepth => "phaser depth",
            Self::PhaserFeedback => "phaser feedback",
            Self::PhaserMix => "phaser mix",
            _ => self.name(),
        }
    }
}

impl Track {
    pub fn parameter(&self, parameter: ParameterId) -> Option<ParameterValue> {
        let value = match parameter {
            ParameterId::Level => ParameterValue::Percent(self.level),
            ParameterId::Pan => ParameterValue::Percent(self.pan),
            ParameterId::DelaySend => ParameterValue::Percent(self.delay_send),
            ParameterId::ReverbSend => ParameterValue::Percent(self.reverb_send),
            ParameterId::DistortionDrive => ParameterValue::Percent(self.effects.distortion.drive),
            ParameterId::DistortionTone => ParameterValue::Percent(self.effects.distortion.tone),
            ParameterId::DistortionMix => ParameterValue::Percent(self.effects.distortion.mix),
            ParameterId::PhaserRate => ParameterValue::Percent(self.effects.phaser.rate),
            ParameterId::PhaserDepth => ParameterValue::Percent(self.effects.phaser.depth),
            ParameterId::PhaserFeedback => ParameterValue::Percent(self.effects.phaser.feedback),
            ParameterId::PhaserMix => ParameterValue::Percent(self.effects.phaser.mix),
            ParameterId::Tune => match self.instrument {
                Instrument::Kick(p) => ParameterValue::Percent(p.tune),
                Instrument::Snare(p) => ParameterValue::Percent(p.tune),
                Instrument::Hat(p) => ParameterValue::Percent(p.tune),
                _ => return None,
            },
            ParameterId::Tone => match self.instrument {
                Instrument::Snare(p) => ParameterValue::Percent(p.tone),
                _ => return None,
            },
            ParameterId::Snappy => match self.instrument {
                Instrument::Snare(p) => ParameterValue::Percent(p.snappy),
                _ => return None,
            },
            ParameterId::Decay => match self.instrument {
                Instrument::Kick(p) => ParameterValue::Percent(p.decay),
                Instrument::Hat(p) => ParameterValue::Percent(p.decay),
                Instrument::Bass(p) => ParameterValue::Percent(p.decay),
                Instrument::Chord(p) => ParameterValue::Percent(p.decay),
                Instrument::Lead(p) => ParameterValue::Percent(p.decay),
                _ => return None,
            },
            ParameterId::Waveform => match self.instrument {
                Instrument::Bass(p) => ParameterValue::Waveform(p.waveform),
                _ => return None,
            },
            ParameterId::OscillatorMix => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.oscillator_mix),
                Instrument::Lead(p) => ParameterValue::Percent(p.oscillator_mix),
                _ => return None,
            },
            ParameterId::PulseWidth => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.pulse_width),
                Instrument::Lead(p) => ParameterValue::Percent(p.pulse_width),
                _ => return None,
            },
            ParameterId::SubOscillator => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.sub_oscillator),
                Instrument::Lead(p) => ParameterValue::Percent(p.sub_oscillator),
                _ => return None,
            },
            ParameterId::Chorus => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Chorus(p.chorus),
                _ => return None,
            },
            ParameterId::Spread => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Spread(p.spread),
                _ => return None,
            },
            ParameterId::Cutoff => match self.instrument {
                Instrument::Bass(p) => ParameterValue::Percent(p.cutoff),
                Instrument::Chord(p) => ParameterValue::Percent(p.cutoff),
                Instrument::Lead(p) => ParameterValue::Percent(p.cutoff),
                _ => return None,
            },
            ParameterId::Resonance => match self.instrument {
                Instrument::Bass(p) => ParameterValue::Percent(p.resonance),
                Instrument::Chord(p) => ParameterValue::Percent(p.resonance),
                Instrument::Lead(p) => ParameterValue::Percent(p.resonance),
                _ => return None,
            },
            ParameterId::FilterEnvelope => match self.instrument {
                Instrument::Bass(p) => ParameterValue::Percent(p.filter_envelope),
                Instrument::Chord(p) => ParameterValue::Percent(p.filter_envelope),
                Instrument::Lead(p) => ParameterValue::Percent(p.filter_envelope),
                _ => return None,
            },
            ParameterId::Attack => match self.instrument {
                Instrument::Kick(p) => ParameterValue::Percent(p.attack),
                Instrument::Chord(p) => ParameterValue::Percent(p.attack),
                Instrument::Lead(p) => ParameterValue::Percent(p.attack),
                _ => return None,
            },
            ParameterId::Sustain => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.sustain),
                Instrument::Lead(p) => ParameterValue::Percent(p.sustain),
                _ => return None,
            },
            ParameterId::Release => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.release),
                Instrument::Lead(p) => ParameterValue::Percent(p.release),
                _ => return None,
            },
            ParameterId::Pitch => return None,
        };
        Some(value)
    }

    /// Return the value that applies to a parameter after a step lock has
    /// been overlaid on the track's base value.
    pub fn effective_parameter(
        &self,
        parameter: ParameterId,
        locks: ParameterLocks,
    ) -> Option<ParameterValue> {
        locks.get(parameter).or_else(|| self.parameter(parameter))
    }

    /// Read an LFO assignment only when its destination is compatible with
    /// this track.  Keeping this check beside the assignment access prevents
    /// callers from duplicating the model's compatibility rules.
    pub const fn supports_lfo(&self, parameter: ParameterId) -> bool {
        parameter.supports_lfo(self.kind)
    }

    pub fn lfo(&self, parameter: ParameterId) -> Option<LfoConfig> {
        self.supports_lfo(parameter)
            .then(|| self.lfos.get(parameter))
            .flatten()
    }

    /// Set an LFO assignment when its destination is compatible with this
    /// track.
    pub fn set_lfo(&mut self, parameter: ParameterId, config: Option<LfoConfig>) -> bool {
        self.supports_lfo(parameter) && self.lfos.set(parameter, config)
    }

    pub fn set_parameter(&mut self, parameter: ParameterId, value: ParameterValue) -> bool {
        match (parameter, value) {
            (ParameterId::Level, ParameterValue::Percent(v)) => self.level = v,
            (ParameterId::Pan, ParameterValue::Percent(v)) => self.pan = v,
            (ParameterId::DelaySend, ParameterValue::Percent(v)) => self.delay_send = v,
            (ParameterId::ReverbSend, ParameterValue::Percent(v)) => self.reverb_send = v,
            (ParameterId::DistortionDrive, ParameterValue::Percent(v)) => {
                self.effects.distortion.drive = v
            }
            (ParameterId::DistortionTone, ParameterValue::Percent(v)) => {
                self.effects.distortion.tone = v
            }
            (ParameterId::DistortionMix, ParameterValue::Percent(v)) => {
                self.effects.distortion.mix = v
            }
            (ParameterId::PhaserRate, ParameterValue::Percent(v)) => self.effects.phaser.rate = v,
            (ParameterId::PhaserDepth, ParameterValue::Percent(v)) => self.effects.phaser.depth = v,
            (ParameterId::PhaserFeedback, ParameterValue::Percent(v)) => {
                if v.get() > 90 {
                    return false;
                }
                self.effects.phaser.feedback = v
            }
            (ParameterId::PhaserMix, ParameterValue::Percent(v)) => self.effects.phaser.mix = v,
            (ParameterId::Tune, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Kick(p) => p.tune = v,
                Instrument::Snare(p) => p.tune = v,
                Instrument::Hat(p) => p.tune = v,
                _ => return false,
            },
            (ParameterId::Tone, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Snare(p) => p.tone = v,
                _ => return false,
            },
            (ParameterId::Snappy, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Snare(p) => p.snappy = v,
                _ => return false,
            },
            (ParameterId::Decay, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Kick(p) => p.decay = v,
                Instrument::Hat(p) => p.decay = v,
                Instrument::Bass(p) => p.decay = v,
                Instrument::Chord(p) => p.decay = v,
                Instrument::Lead(p) => p.decay = v,
                _ => return false,
            },
            (ParameterId::Waveform, ParameterValue::Waveform(v)) => match &mut self.instrument {
                Instrument::Bass(p) => p.waveform = v,
                _ => return false,
            },
            (ParameterId::OscillatorMix, ParameterValue::Percent(v)) => {
                match &mut self.instrument {
                    Instrument::Chord(p) => p.oscillator_mix = v,
                    Instrument::Lead(p) => p.oscillator_mix = v,
                    _ => return false,
                }
            }
            (ParameterId::PulseWidth, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.pulse_width = v,
                Instrument::Lead(p) => p.pulse_width = v,
                _ => return false,
            },
            (ParameterId::SubOscillator, ParameterValue::Percent(v)) => {
                match &mut self.instrument {
                    Instrument::Chord(p) => p.sub_oscillator = v,
                    Instrument::Lead(p) => p.sub_oscillator = v,
                    _ => return false,
                }
            }
            (ParameterId::Chorus, ParameterValue::Chorus(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.chorus = v,
                _ => return false,
            },
            (ParameterId::Spread, ParameterValue::Spread(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.spread = v,
                _ => return false,
            },
            (ParameterId::Cutoff, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Bass(p) => p.cutoff = v,
                Instrument::Chord(p) => p.cutoff = v,
                Instrument::Lead(p) => p.cutoff = v,
                _ => return false,
            },
            (ParameterId::Resonance, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Bass(p) => p.resonance = v,
                Instrument::Chord(p) => p.resonance = v,
                Instrument::Lead(p) => p.resonance = v,
                _ => return false,
            },
            (ParameterId::FilterEnvelope, ParameterValue::Percent(v)) => {
                match &mut self.instrument {
                    Instrument::Bass(p) => p.filter_envelope = v,
                    Instrument::Chord(p) => p.filter_envelope = v,
                    Instrument::Lead(p) => p.filter_envelope = v,
                    _ => return false,
                }
            }
            (ParameterId::Attack, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Kick(p) => p.attack = v,
                Instrument::Chord(p) => p.attack = v,
                Instrument::Lead(p) => p.attack = v,
                _ => return false,
            },
            (ParameterId::Sustain, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.sustain = v,
                Instrument::Lead(p) => p.sustain = v,
                _ => return false,
            },
            (ParameterId::Release, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.release = v,
                Instrument::Lead(p) => p.release = v,
                _ => return false,
            },
            (ParameterId::Pitch, _) => return false,
            _ => return false,
        }
        true
    }
}

impl FromStr for PitchClass {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|p| p.to_string() == s).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_valid() {
        let project = Project::new();
        assert_eq!(project.patterns.len(), MIN_PATTERN_COUNT);
        project.validate().unwrap();
    }
    #[test]
    fn project_equality_ignores_transient_step_caches() {
        let mut left = Project::new();
        let mut right = left.clone();
        left.tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        right.tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        assert_eq!(left, right);

        right.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        assert_ne!(left, right);
    }
    #[test]
    fn song_entries_accept_pattern_100_and_reject_pattern_101() {
        let mut project = Project::new();
        project.patterns.resize_with(100, || Pattern {
            tracks: (0..TRACK_COUNT)
                .map(|_| PatternTrack {
                    steps: vec![None; STEP_BANK_SIZE],
                })
                .collect(),
        });
        project.song[0].pattern = 100;
        project.validate().unwrap();

        project.song[0].pattern = 101;
        assert!(project.validate().is_err());
    }
    #[test]
    fn reverb_globals_validate_boundaries() {
        let mut project = Project::new();
        project.globals.reverb_tone = p(101);
        assert!(project.validate().is_err());

        project.globals.reverb_tone = p(100);
        project.globals.reverb_pre_delay_ms = 200;
        assert!(project.validate().is_ok());

        project.globals.reverb_pre_delay_ms = 201;
        assert!(project.validate().is_err());
    }
    #[test]
    fn scale_and_frequency() {
        let mut p = Project::new();
        assert_eq!(p.note_midi(8, 3), Some(60));
        p.globals.key = PitchClass::A;
        assert!((p.note_frequency(1, 4).unwrap() - 440.0).abs() < 0.001);
        p.globals.scale = Scale::NaturalMinor;
        assert_eq!(p.note_midi(3, 3), Some(60));
    }
    #[test]
    fn diatonic_triads_are_close_position_and_cross_octaves() {
        let mut project = Project::new();
        assert_eq!(project.chord_midis(1, 3), Some([48, 52, 55]));
        assert_eq!(project.chord_midis(7, 3), Some([59, 62, 65]));
        assert_eq!(project.chord_midis(8, 3), Some([60, 64, 67]));
        project.globals.scale = Scale::NaturalMinor;
        assert_eq!(project.chord_midis(1, 3), Some([48, 51, 55]));
        assert_eq!(project.chord_midis(2, 3), Some([50, 53, 56]));
        assert_eq!(project.chord_midis(0, 3), None);
        assert_eq!(project.chord_midis(1, 8), None);
    }

    #[test]
    fn chord_shapes_use_diatonic_degrees_and_lift_inversions() {
        let project = Project::new();
        assert_eq!(ChordShape::SeventhRoot.to_string(), "1-3-5-7");
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::SeventhRoot),
            Some(([48, 52, 55, 59], 4))
        );
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::SeventhFirstInversion),
            Some(([52, 55, 59, 60], 4))
        );
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::SeventhSecondInversion),
            Some(([55, 59, 60, 64], 4))
        );
    }
    #[test]
    fn percent_clamps() {
        assert_eq!(Percent::new(101), None);
        assert_eq!(p(95).saturating_add(10), p(100));
    }
    #[test]
    fn wrapped_tie() {
        let mut s = vec![None; 16];
        s[15] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        s[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        assert_eq!(tie_source(&s, 0), Some(15));
    }
    #[test]
    fn variable_step_counts_and_wrapped_ties_validate() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps = vec![None; 1];
        project.patterns[0].tracks[1].steps = vec![None; MAX_STEP_COUNT];
        project.patterns[0].tracks[3].steps = vec![None; 3];
        project.patterns[0].tracks[3].steps[2] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        assert_eq!(tie_source(&project.patterns[0].tracks[3].steps, 0), Some(2));
        project.validate().unwrap();

        project.patterns[0].tracks[0].steps.clear();
        assert_eq!(project.validate(), Err(ValidationError::StepCount(0, 0)));
        project.patterns[0].tracks[0].steps = vec![None; MAX_STEP_COUNT + 1];
        assert_eq!(
            project.validate(),
            Err(ValidationError::StepCount(0, MAX_STEP_COUNT + 1))
        );
    }
    #[test]
    fn pitch_class_json_is_stable() {
        assert_eq!(
            serde_json::to_string(&PitchClass::FSharp).unwrap(),
            "\"F#\""
        );
    }
    #[test]
    fn invalid_percent_is_rejected() {
        assert!(serde_json::from_str::<Percent>("101").is_err());
    }

    #[test]
    fn effects_have_shared_defaults_and_are_lockable_on_every_track() {
        let project = Project::new();
        assert_eq!(project.format_version, 13);
        assert_eq!(project.tracks[0].effects.distortion.drive, Percent::ZERO);
        assert_eq!(project.tracks[0].effects.distortion.tone, p(50));
        assert_eq!(project.tracks[0].effects.phaser.rate, p(25));
        assert!(ParameterId::PhaserMix.is_valid_for(TrackKind::Kick));
        assert!(!ParameterId::PhaserMix.supports_lfo(TrackKind::Kick));

        let mut locks = ParameterLocks::default();
        assert!(locks.set(ParameterId::DistortionDrive, ParameterValue::Percent(p(80))));
        assert_eq!(
            locks.get(ParameterId::DistortionDrive),
            Some(ParameterValue::Percent(p(80)))
        );
        assert!(locks.set(ParameterId::PhaserFeedback, ParameterValue::Percent(p(90))));
        assert_eq!(locks.phaser_feedback, Some(p(90)));
        assert!(!locks.set(ParameterId::PhaserFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(locks.phaser_feedback, Some(p(90)));

        let mut track = project.tracks[0].clone();
        assert!(track.set_parameter(ParameterId::PhaserFeedback, ParameterValue::Percent(p(90))));
        assert!(!track.set_parameter(ParameterId::PhaserFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(track.effects.phaser.feedback, p(90));

        let mut invalid = project;
        invalid.tracks[0].effects.phaser.feedback = p(91);
        assert_eq!(
            invalid.validate(),
            Err(ValidationError::Effect(0, "phaser_feedback"))
        );
    }
    #[test]
    fn accent_and_slide_event_fields_are_strict() {
        let trigger = StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        };
        let note = StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        };
        assert_eq!(serde_json::to_value(trigger).unwrap()["accent"], false);
        assert_eq!(serde_json::to_value(note).unwrap()["accent"], false);
        assert!(serde_json::from_str::<StepEvent>(r#"{"type":"trigger","locks":{}}"#).is_err());
        assert!(
            serde_json::from_str::<StepEvent>(
                r#"{"type":"note","degree":1,"octave":3,"accent":false,"slide":true,"locks":{}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<StepEvent>(r#"{"type":"tie","accent":true,"locks":{}}"#)
                .is_err()
        );
    }

    #[test]
    fn input_accent_omits_false_and_round_trips_true() {
        let mut project = Project::new();
        let value = serde_json::to_value(&project).unwrap();
        assert!(
            value["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|track| { track.get("input_accent").is_none() })
        );

        project.tracks[3].input_accent = true;
        let value = serde_json::to_value(&project).unwrap();
        assert_eq!(value["tracks"][3]["input_accent"], true);
        assert_eq!(serde_json::from_value::<Project>(value).unwrap(), project);
    }
    #[test]
    fn lock_compatibility_matches_track_kind() {
        let mut drum = Project::new();
        drum.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                cutoff: Percent::new(50),
                ..Default::default()
            },
        });
        assert!(matches!(
            drum.validate(),
            Err(ValidationError::Lock(0, 0, "cutoff"))
        ));

        let mut synth = Project::new();
        synth.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                tone: Percent::new(50),
                ..Default::default()
            },
        });
        assert!(matches!(
            synth.validate(),
            Err(ValidationError::Lock(4, 0, "tone"))
        ));
    }

    #[test]
    fn shared_parameter_access_matches_track_compatibility() {
        let mut project = Project::new();
        for track in &mut project.tracks {
            for parameter in ParameterId::ALL {
                let current = track.parameter(parameter);
                assert_eq!(
                    current.is_some(),
                    parameter.is_valid_for(track.kind),
                    "{} access disagrees with {:?} compatibility",
                    parameter.name(),
                    track.kind
                );
                if let Some(value) = current {
                    assert!(track.set_parameter(parameter, value));
                    assert_eq!(track.parameter(parameter), Some(value));
                }
            }
        }

        assert!(!project.tracks[0].set_parameter(
            ParameterId::Level,
            ParameterValue::Waveform(Waveform::Square)
        ));
    }

    #[test]
    fn shared_lock_access_sets_clears_and_overlays_every_parameter() {
        let percent = ParameterValue::Percent(p(42));
        let mut locks = ParameterLocks::default();
        for parameter in ParameterId::ALL {
            if parameter == ParameterId::Pitch {
                assert!(!locks.set(parameter, percent));
                assert_eq!(locks.get(parameter), None);
                locks.clear(parameter);
                continue;
            }
            let value = if parameter.is_waveform() {
                ParameterValue::Waveform(Waveform::Square)
            } else if parameter.is_chorus() {
                ParameterValue::Chorus(ChorusMode::Ii)
            } else if parameter == ParameterId::Spread {
                ParameterValue::Spread(ChordSpread::Wide)
            } else {
                percent
            };
            assert!(locks.set(parameter, value));
            assert_eq!(locks.get(parameter), Some(value));
            locks.clear(parameter);
            assert_eq!(locks.get(parameter), None);
        }

        locks.set(ParameterId::Level, ParameterValue::Percent(p(20)));
        locks.set(ParameterId::Cutoff, ParameterValue::Percent(p(30)));
        let mut overlay = ParameterLocks::default();
        overlay.set(ParameterId::Cutoff, ParameterValue::Percent(p(90)));
        locks.overlay(overlay);
        assert_eq!(
            locks.get(ParameterId::Level),
            Some(ParameterValue::Percent(p(20)))
        );
        assert_eq!(
            locks.get(ParameterId::Cutoff),
            Some(ParameterValue::Percent(p(90)))
        );
    }

    #[test]
    fn all_keys_map_degrees() {
        for key in PitchClass::ALL {
            for scale in [Scale::Major, Scale::NaturalMinor] {
                let mut p = Project::new();
                p.globals.key = key;
                p.globals.scale = scale;
                for degree in 1..=8 {
                    assert!(p.note_frequency(degree, 3).unwrap().is_finite());
                    let chord = p.chord_midis(degree, 3).unwrap();
                    assert!(chord[0] < chord[1] && chord[1] < chord[2]);
                }
                assert_eq!(p.note_midi(8, 3).unwrap() - p.note_midi(1, 3).unwrap(), 12)
            }
        }
    }

    #[test]
    fn lfo_targets_and_rates_are_bounded_and_contextual() {
        assert!(
            (LfoRate::Free {
                rate_percent: Percent::ZERO
            }
            .hz(120)
                - 0.01)
                .abs()
                < 0.0001
        );
        assert!(
            (LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            }
            .hz(120)
                - 20.0)
                .abs()
                < 0.001
        );
        assert!(
            (LfoRate::Synced {
                division: LfoDivision::Quarter
            }
            .hz(120)
                - 2.0)
                .abs()
                < 0.001
        );

        let mut project = Project::new();
        project.tracks[0].lfos.cutoff = Some(LfoConfig::default());
        assert_eq!(project.validate(), Err(ValidationError::Lfo(0, "cutoff")));
        project.tracks[0].lfos.cutoff = None;
        project.tracks[3].lfos.tone = Some(LfoConfig::default());
        assert_eq!(project.validate(), Err(ValidationError::Lfo(3, "tone")));
        project.tracks[3].lfos.tone = None;
        project.tracks[4].lfos.release = Some(LfoConfig::default());
        project.validate().unwrap();
        assert!(ParameterId::Pitch.supports_lfo(TrackKind::Chord));
        assert!(ParameterId::Pitch.supports_lfo(TrackKind::Lead));
        assert!(!ParameterId::Pitch.supports_lfo(TrackKind::Bass));
        assert!(!ParameterId::Pitch.is_valid_for(TrackKind::Chord));
        assert!(!project.tracks[4].set_parameter(
            ParameterId::Pitch,
            ParameterValue::Percent(Percent::new(50).unwrap())
        ));
        assert!(!ParameterLocks::default().set(
            ParameterId::Pitch,
            ParameterValue::Percent(Percent::new(50).unwrap())
        ));
        project.tracks[4].lfos.pitch = Some(LfoConfig::default());
        project.validate().unwrap();
        project.tracks[3].lfos.pitch = Some(LfoConfig::default());
        assert_eq!(project.validate(), Err(ValidationError::Lfo(3, "pitch")));
    }

    #[test]
    fn arpeggio_defaults_and_json_names_are_stable() {
        let config = ArpeggioConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.r#type, ArpeggioType::Up);
        assert_eq!(config.rate, ArpeggioRate::Sixteenth);
        assert_eq!(
            serde_json::to_string(&ArpeggioType::UpDown).unwrap(),
            "\"up_down\""
        );
        assert_eq!(
            serde_json::to_string(&ArpeggioRate::SixteenthTriplet).unwrap(),
            "\"sixteenth_triplet\""
        );
    }

    #[test]
    fn ordinary_locks_overlay_and_chord_articulation_is_not_a_lock() {
        let mut locks = ParameterLocks {
            cutoff: Some(Percent::new(20).unwrap()),
            ..Default::default()
        };
        let overlay = ParameterLocks {
            cutoff: Some(Percent::new(30).unwrap()),
            ..Default::default()
        };
        locks.overlay(overlay);
        assert_eq!(locks.cutoff, Some(Percent::new(30).unwrap()));

        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks,
        });
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lock(0, 0, "cutoff"))
        );
    }
}
