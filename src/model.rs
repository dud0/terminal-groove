use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as _, MapAccess, Visitor},
    ser::SerializeMap,
};
use std::{fmt, str::FromStr};

pub const MIN_STEP_COUNT: usize = 1;
pub const MAX_STEP_COUNT: usize = 64;
pub const STEP_BANK_SIZE: usize = 16;
pub const STEP_ROW_SIZE: usize = 32;
pub const DRUM_TRACK_COUNT: usize = 6;
pub const RIMSHOT_TRACK_INDEX: usize = DRUM_TRACK_COUNT - 1;
pub const SYNTH_TRACK_START: usize = DRUM_TRACK_COUNT;
pub const CHORD_TRACK_INDEX: usize = SYNTH_TRACK_START + 1;
pub const LEAD_TRACK_INDEX: usize = SYNTH_TRACK_START + 2;
pub const FM_TRACK_INDEX: usize = SYNTH_TRACK_START + 3;
pub const TRACK_COUNT: usize = 10;
pub const MIN_PATTERN_COUNT: usize = 1;
pub const MAX_PATTERN_COUNT: usize = 100;
pub const MAX_SONG_ENTRY_COUNT: usize = 256;

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

/// Deterministic old-song-entry-index to new-song-entry-index transform.
///
/// Song entries are editor-visible transport positions, so insertions and
/// removals must keep the audio thread on the same entry where possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongIndexMap {
    kind: SongIndexMapKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SongIndexMapKind {
    Identity,
    InsertAfter(u8),
    InsertAt(u8),
    Delete(u8),
}

impl SongIndexMap {
    pub fn identity() -> Self {
        Self {
            kind: SongIndexMapKind::Identity,
        }
    }

    pub fn insert(after: usize) -> Self {
        Self {
            kind: SongIndexMapKind::InsertAfter(after as u8),
        }
    }

    pub fn insert_at(at: usize) -> Self {
        Self {
            kind: SongIndexMapKind::InsertAt(at as u8),
        }
    }

    pub fn delete(at: usize) -> Self {
        Self {
            kind: SongIndexMapKind::Delete(at as u8),
        }
    }

    pub(crate) fn rebase(self, index: usize, next_count: usize) -> usize {
        if next_count == 0 {
            return 0;
        }
        let mapped = match self.kind {
            SongIndexMapKind::Identity => index,
            SongIndexMapKind::InsertAfter(at) if index > usize::from(at) => index + 1,
            SongIndexMapKind::InsertAt(at) if index >= usize::from(at) => index + 1,
            SongIndexMapKind::Delete(at) if index > usize::from(at) => index - 1,
            SongIndexMapKind::Delete(at) if index == usize::from(at) => index,
            _ => index,
        };
        mapped.min(next_count - 1)
    }

    pub(crate) fn removes(self, index: usize) -> bool {
        matches!(self.kind, SongIndexMapKind::Delete(at) if index == usize::from(at))
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError {
    #[error("unsupported format_version {0}")]
    Version(u32),
    #[error("tracks: expected exactly ten tracks")]
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
    #[error("tracks[{0}]: probability must be between 0 and 100%")]
    Probability(usize),
    #[error("tracks[{0}].effects: invalid {1} range")]
    Effect(usize, &'static str),
    #[error("tracks[{0}].steps[{1}]: invalid trigger condition")]
    Condition(usize, usize),
    #[error("tracks[{0}].steps[{1}]: retrigger_count must be between 1 and 4")]
    Retrigger(usize, usize),
    #[error("tracks[{0}].steps[{1}]: invalid drum recipe")]
    DrumRecipe(usize, usize),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Microtiming(i8);

impl Microtiming {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: i8) -> Option<Self> {
        if value >= -50 && value <= 50 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> i8 {
        self.0
    }

    pub fn saturating_add(self, delta: i16) -> Self {
        Self((i16::from(self.0) + delta).clamp(-50, 50) as i8)
    }
}

impl fmt::Display for Microtiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 > 0 {
            write!(f, "+{}%", self.0)
        } else {
            write!(f, "{}%", self.0)
        }
    }
}

impl Serialize for Microtiming {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i8(self.0)
    }
}

impl<'de> Deserialize<'de> for Microtiming {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = i8::deserialize(d)?;
        Self::new(value)
            .ok_or_else(|| serde::de::Error::custom("microtiming must be between -50 and 50"))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DrumRecipeSlot(u8);

impl Default for DrumRecipeSlot {
    fn default() -> Self {
        Self::ONE
    }
}

impl DrumRecipeSlot {
    pub const ONE: Self = Self(1);
    pub const TWO: Self = Self(2);
    pub const THREE: Self = Self(3);

    pub const fn new(value: u8) -> Option<Self> {
        if value >= 1 && value <= 3 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for DrumRecipeSlot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u8::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("drum recipe must be 1, 2, or 3"))
    }
}

fn drum_recipe_is_default(value: &DrumRecipeSlot) -> bool {
    *value == DrumRecipeSlot::ONE
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
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    NaturalMinor,
    Locrian,
}
impl Scale {
    pub const ALL: [Self; 7] = [
        Self::Major,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::NaturalMinor,
        Self::Locrian,
    ];

    pub const fn offsets(self) -> [i32; 8] {
        match self {
            Self::Major => [0, 2, 4, 5, 7, 9, 11, 12],
            Self::Dorian => [0, 2, 3, 5, 7, 9, 10, 12],
            Self::Phrygian => [0, 1, 3, 5, 7, 8, 10, 12],
            Self::Lydian => [0, 2, 4, 6, 7, 9, 11, 12],
            Self::Mixolydian => [0, 2, 4, 5, 7, 9, 10, 12],
            Self::NaturalMinor => [0, 2, 3, 5, 7, 8, 10, 12],
            Self::Locrian => [0, 1, 3, 5, 6, 8, 10, 12],
        }
    }

    pub fn shifted(self, delta: i32) -> Self {
        let index = Self::ALL
            .iter()
            .position(|scale| *scale == self)
            .expect("every scale is listed in Scale::ALL") as i32;
        Self::ALL[(index + delta).clamp(0, Self::ALL.len() as i32 - 1) as usize]
    }
}
impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Major => "Major",
                Self::Dorian => "Dorian",
                Self::Phrygian => "Phrygian",
                Self::Lydian => "Lydian",
                Self::Mixolydian => "Mixolydian",
                Self::NaturalMinor => "Natural minor",
                Self::Locrian => "Locrian",
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FmAlgorithm {
    #[default]
    Cascade,
    SplitStack,
    Converge,
    Pairs,
    FanIn,
    FanOut,
    Mixed,
    Additive,
}

impl FmAlgorithm {
    pub const ALL: [Self; 8] = [
        Self::Cascade,
        Self::SplitStack,
        Self::Converge,
        Self::Pairs,
        Self::FanIn,
        Self::FanOut,
        Self::Mixed,
        Self::Additive,
    ];

    pub const fn number(self) -> u8 {
        self as u8 + 1
    }

    pub const fn diagram(self) -> &'static str {
        match self {
            Self::Cascade => "4>3>2>1 · OUT 1",
            Self::SplitStack => "4>3 · 3>1+2 · OUT 1+2",
            Self::Converge => "4+3>2>1 · OUT 1",
            Self::Pairs => "4>3 · 2>1 · OUT 1+3",
            Self::FanIn => "4+3+2>1 · OUT 1",
            Self::FanOut => "4>1+2+3 · OUT 1+2+3",
            Self::Mixed => "4>3 · OUT 1+2+3",
            Self::Additive => "OUT 1+2+3+4",
        }
    }

    pub const fn routes(self, source: usize, target: usize) -> bool {
        match self {
            Self::Cascade => matches!((source, target), (3, 2) | (2, 1) | (1, 0)),
            Self::SplitStack => matches!((source, target), (3, 2) | (2, 1) | (2, 0)),
            Self::Converge => matches!((source, target), (3, 1) | (2, 1) | (1, 0)),
            Self::Pairs => matches!((source, target), (3, 2) | (1, 0)),
            Self::FanIn => matches!((source, target), (3, 0) | (2, 0) | (1, 0)),
            Self::FanOut => matches!((source, target), (3, 0) | (3, 1) | (3, 2)),
            Self::Mixed => matches!((source, target), (3, 2)),
            Self::Additive => false,
        }
    }

    pub const fn is_carrier(self, operator: usize) -> bool {
        match self {
            Self::Cascade | Self::Converge | Self::FanIn => operator == 0,
            Self::SplitStack => operator <= 1,
            Self::Pairs => matches!(operator, 0 | 2),
            Self::FanOut | Self::Mixed => operator <= 2,
            Self::Additive => true,
        }
    }

    pub const fn role(self, operator: usize) -> &'static str {
        if self.is_carrier(operator) {
            return "OUT";
        }
        match (self, operator) {
            (Self::Cascade, 1) => ">1",
            (Self::Cascade, 2) => ">2",
            (Self::Cascade, 3) => ">3",
            (Self::SplitStack, 2) => ">1+2",
            (Self::SplitStack, 3) => ">3",
            (Self::Converge, 1) => ">1",
            (Self::Converge, 2 | 3) => ">2",
            (Self::Pairs, 1) => ">1",
            (Self::Pairs, 3) => ">3",
            (Self::FanIn, 1..=3) => ">1",
            (Self::FanOut, 3) => ">1+2+3",
            (Self::Mixed, 3) => ">3",
            _ => "---",
        }
    }
}

impl fmt::Display for FmAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "A{}", self.number())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FmRatio {
    #[serde(rename = "0.5")]
    Half,
    #[serde(rename = "1")]
    One,
    #[serde(rename = "1.5")]
    OneAndHalf,
    #[default]
    #[serde(rename = "2")]
    Two,
    #[serde(rename = "3")]
    Three,
    #[serde(rename = "4")]
    Four,
    #[serde(rename = "5")]
    Five,
    #[serde(rename = "6")]
    Six,
    #[serde(rename = "8")]
    Eight,
}

impl FmRatio {
    pub const ALL: [Self; 9] = [
        Self::Half,
        Self::One,
        Self::OneAndHalf,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Eight,
    ];
    pub const fn value(self) -> f32 {
        match self {
            Self::Half => 0.5,
            Self::One => 1.0,
            Self::OneAndHalf => 1.5,
            Self::Two => 2.0,
            Self::Three => 3.0,
            Self::Four => 4.0,
            Self::Five => 5.0,
            Self::Six => 6.0,
            Self::Eight => 8.0,
        }
    }
}

impl fmt::Display for FmRatio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value())
    }
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
    pub reset_on_trigger: bool,
    pub start_phase: Percent,
    pub rate: LfoRate,
    pub depth: Percent,
}

impl Default for LfoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            waveform: LfoWaveform::Sine,
            reset_on_trigger: false,
            start_phase: Percent::ZERO,
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
    #[serde(default = "default_reverb_return")]
    pub reverb_return: Percent,
    pub sidechain: SidechainParameters,
    pub key: PitchClass,
    pub scale: Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidechainParameters {
    pub depth: Percent,
    pub attack: Percent,
    pub release: Percent,
}

impl SidechainParameters {
    pub const fn depth_db(self) -> f32 {
        self.depth.get() as f32 * 18.0 / 100.0
    }

    pub fn attack_ms(self) -> f32 {
        0.5 * 60.0_f32.powf(self.attack.get() as f32 / 100.0)
    }

    pub fn release_ms(self) -> f32 {
        40.0 * 25.0_f32.powf(self.release.get() as f32 / 100.0)
    }
}

impl Default for SidechainParameters {
    fn default() -> Self {
        Self {
            depth: Percent::ZERO,
            attack: Percent(20),
            release: Percent(35),
        }
    }
}

fn default_reverb_tone() -> Percent {
    Percent(40)
}

fn default_reverb_pre_delay_ms() -> u16 {
    20
}

fn default_reverb_return() -> Percent {
    Percent(30)
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
    ReverbReturn,
    Ducking,
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
            reverb_return: default_reverb_return(),
            sidechain: SidechainParameters::default(),
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
    Tom,
    Cymbal,
    Rimshot,
    Bass,
    Chord,
    Lead,
    Fm,
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
    Single,
    DyadThird,
    DyadFifth,
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
    pub const ALL: [Self; 20] = [
        Self::Single,
        Self::DyadThird,
        Self::DyadFifth,
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
            Self::Single => &[1],
            Self::DyadThird => &[1, 3],
            Self::DyadFifth => &[1, 5],
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
    pub open: HatRecipe,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HatRecipe {
    pub tune: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomParameters {
    pub tune: Percent,
    pub tone: Percent,
    pub decay: Percent,
    pub medium: TomRecipe,
    pub high: TomRecipe,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomRecipe {
    pub tune: Percent,
    pub tone: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CymbalParameters {
    pub tune: Percent,
    pub tone: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RimshotParameters {
    pub tune: Percent,
    pub tone: Percent,
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
    pub noise: Percent,
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
    pub noise: Percent,
    pub sub_mode: LeadSubMode,
    pub keyboard_tracking: Percent,
    pub portamento_time: Percent,
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
pub struct FmOperator {
    pub ratio: FmRatio,
    pub level: Percent,
    pub feedback: Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmOperatorField {
    Ratio,
    Level,
    Feedback,
}

impl FmOperatorField {
    pub const ALL: [Self; 3] = [Self::Ratio, Self::Level, Self::Feedback];
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmParameters {
    pub algorithm: FmAlgorithm,
    pub operators: [FmOperator; 4],
    pub brightness: Percent,
    pub attack: Percent,
    pub decay: Percent,
    pub sustain: Percent,
    pub release: Percent,
}

/// The Lead divider is phase locked to the main oscillator. The narrow
/// mode deliberately keeps the two-octave divider's shorter pulse shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeadSubMode {
    OneOctaveSquare,
    #[default]
    TwoOctaveSquare,
    TwoOctaveNarrowPulse,
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlangerParameters {
    pub rate: Percent,
    pub delay: Percent,
    pub depth: Percent,
    pub feedback: Percent,
    pub mix: Percent,
}

impl Default for FlangerParameters {
    fn default() -> Self {
        Self {
            rate: Percent(25),
            delay: Percent(18),
            depth: Percent(50),
            feedback: Percent(20),
            mix: Percent::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitCrusherParameters {
    pub bits: Percent,
    pub rate: Percent,
    pub mix: Percent,
}

impl Default for BitCrusherParameters {
    fn default() -> Self {
        Self {
            bits: Percent(50),
            rate: Percent(50),
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
    #[serde(default)]
    pub flanger: FlangerParameters,
    #[serde(default)]
    pub bit_crusher: BitCrusherParameters,
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
    Tom(TomParameters),
    Cymbal(CymbalParameters),
    Rimshot(RimshotParameters),
    Bass(BassParameters),
    Chord(ChordParameters),
    Lead(LeadParameters),
    Fm(FmParameters),
}

const PARAMETER_COUNT: usize = ParameterId::COUNT;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterLocks {
    values: [Option<ParameterValue>; PARAMETER_COUNT],
}

impl Default for ParameterLocks {
    fn default() -> Self {
        Self {
            values: [None; PARAMETER_COUNT],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LfoAssignments {
    values: [Option<LfoConfig>; PARAMETER_COUNT],
}

impl Default for LfoAssignments {
    fn default() -> Self {
        Self {
            values: [None; PARAMETER_COUNT],
        }
    }
}

impl LfoAssignments {
    pub fn get(&self, parameter: ParameterId) -> Option<LfoConfig> {
        self.values[parameter as usize]
    }

    pub fn set(&mut self, parameter: ParameterId, config: Option<LfoConfig>) -> bool {
        if !parameter.is_lfo_assignable() {
            return false;
        }
        self.values[parameter as usize] = config;
        true
    }
}
impl ParameterLocks {
    pub fn is_empty(&self) -> bool {
        self.values.iter().all(Option::is_none)
    }

    pub fn get(&self, parameter: ParameterId) -> Option<ParameterValue> {
        self.values[parameter as usize]
    }

    pub fn percent(&self, parameter: ParameterId) -> Option<Percent> {
        match self.get(parameter) {
            Some(ParameterValue::Percent(value)) => Some(value),
            _ => None,
        }
    }

    pub fn waveform(&self) -> Option<Waveform> {
        match self.get(ParameterId::Waveform) {
            Some(ParameterValue::Waveform(value)) => Some(value),
            _ => None,
        }
    }

    pub fn chorus(&self) -> Option<ChorusMode> {
        match self.get(ParameterId::Chorus) {
            Some(ParameterValue::Chorus(value)) => Some(value),
            _ => None,
        }
    }

    pub fn spread(&self) -> Option<ChordSpread> {
        match self.get(ParameterId::Spread) {
            Some(ParameterValue::Spread(value)) => Some(value),
            _ => None,
        }
    }

    pub fn lead_sub_mode(&self) -> Option<LeadSubMode> {
        match self.get(ParameterId::LeadSubMode) {
            Some(ParameterValue::LeadSubMode(value)) => Some(value),
            _ => None,
        }
    }

    pub fn fm_algorithm(&self) -> Option<FmAlgorithm> {
        match self.get(ParameterId::FmAlgorithm) {
            Some(ParameterValue::FmAlgorithm(value)) => Some(value),
            _ => None,
        }
    }

    pub fn fm_ratio(&self, parameter: ParameterId) -> Option<FmRatio> {
        match self.get(parameter) {
            Some(ParameterValue::FmRatio(value)) => Some(value),
            _ => None,
        }
    }

    pub fn set(&mut self, parameter: ParameterId, value: ParameterValue) -> bool {
        if !parameter.accepts_lock_value(value) {
            return false;
        }
        self.values[parameter as usize] = Some(value);
        true
    }

    pub fn clear(&mut self, parameter: ParameterId) {
        self.values[parameter as usize] = None;
    }

    pub fn overlay(&mut self, overlay: Self) {
        for parameter in ParameterId::ALL {
            if let Some(value) = overlay.get(parameter) {
                self.values[parameter as usize] = Some(value);
            }
        }
    }

    pub fn from_pairs<const N: usize>(pairs: [(ParameterId, ParameterValue); N]) -> Self {
        let mut locks = Self::default();
        for (parameter, value) in pairs {
            assert!(locks.set(parameter, value), "invalid parameter lock");
        }
        locks
    }
}

impl Serialize for ParameterLocks {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for parameter in ParameterId::ALL {
            if let Some(value) = self.get(parameter) {
                match value {
                    ParameterValue::Percent(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::Waveform(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::Chorus(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::Spread(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::LeadSubMode(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::FmAlgorithm(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                    ParameterValue::FmRatio(value) => {
                        map.serialize_entry(parameter.name(), &value)?
                    }
                }
            }
        }
        map.end()
    }
}

struct ParameterLocksVisitor;

impl<'de> Visitor<'de> for ParameterLocksVisitor {
    type Value = ParameterLocks;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a parameter-lock object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut locks = ParameterLocks::default();
        let mut seen = [false; PARAMETER_COUNT];
        while let Some(name) = map.next_key::<String>()? {
            let parameter = ParameterId::from_name(&name)
                .filter(|parameter| parameter.is_lockable())
                .ok_or_else(|| A::Error::custom(format_args!("unknown parameter lock `{name}`")))?;
            let index = parameter as usize;
            if std::mem::replace(&mut seen[index], true) {
                return Err(A::Error::duplicate_field(parameter.name()));
            }
            let value = match parameter.value_kind() {
                ParameterValueKind::Percent => map
                    .next_value::<Option<Percent>>()?
                    .map(ParameterValue::Percent),
                ParameterValueKind::Waveform => map
                    .next_value::<Option<Waveform>>()?
                    .map(ParameterValue::Waveform),
                ParameterValueKind::Chorus => map
                    .next_value::<Option<ChorusMode>>()?
                    .map(ParameterValue::Chorus),
                ParameterValueKind::Spread => map
                    .next_value::<Option<ChordSpread>>()?
                    .map(ParameterValue::Spread),
                ParameterValueKind::LeadSubMode => map
                    .next_value::<Option<LeadSubMode>>()?
                    .map(ParameterValue::LeadSubMode),
                ParameterValueKind::FmAlgorithm => map
                    .next_value::<Option<FmAlgorithm>>()?
                    .map(ParameterValue::FmAlgorithm),
                ParameterValueKind::FmRatio => map
                    .next_value::<Option<FmRatio>>()?
                    .map(ParameterValue::FmRatio),
            };
            if let Some(value) = value
                && !locks.set(parameter, value)
            {
                return Err(A::Error::custom(format_args!("invalid value for `{name}`")));
            }
        }
        Ok(locks)
    }
}

impl<'de> Deserialize<'de> for ParameterLocks {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ParameterLocksVisitor)
    }
}

impl Serialize for LfoAssignments {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for parameter in ParameterId::ALL {
            if let Some(config) = self.get(parameter) {
                map.serialize_entry(parameter.name(), &config)?;
            }
        }
        map.end()
    }
}

struct LfoAssignmentsVisitor;

impl<'de> Visitor<'de> for LfoAssignmentsVisitor {
    type Value = LfoAssignments;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LFO-assignment object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut assignments = LfoAssignments::default();
        let mut seen = [false; PARAMETER_COUNT];
        while let Some(name) = map.next_key::<String>()? {
            let parameter = ParameterId::from_name(&name)
                .filter(|parameter| parameter.is_lfo_assignable())
                .ok_or_else(|| A::Error::custom(format_args!("unknown LFO assignment `{name}`")))?;
            let index = parameter as usize;
            if std::mem::replace(&mut seen[index], true) {
                return Err(A::Error::duplicate_field(parameter.name()));
            }
            assignments.values[index] = map.next_value()?;
        }
        Ok(assignments)
    }
}

impl<'de> Deserialize<'de> for LfoAssignments {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(LfoAssignmentsVisitor)
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
        #[serde(default, skip_serializing_if = "drum_recipe_is_default")]
        recipe: DrumRecipeSlot,
        #[serde(default, skip_serializing_if = "trigger_condition_is_default")]
        condition: TriggerCondition,
        #[serde(
            default = "default_retrigger_count",
            skip_serializing_if = "retrigger_count_is_default"
        )]
        retrigger_count: u8,
        #[serde(default, skip_serializing_if = "microtiming_is_default")]
        microtiming: Microtiming,
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
        #[serde(default, skip_serializing_if = "microtiming_is_default")]
        microtiming: Microtiming,
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
        #[serde(default, skip_serializing_if = "microtiming_is_default")]
        microtiming: Microtiming,
        locks: ParameterLocks,
    },
    LeadNote {
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
        #[serde(default, skip_serializing_if = "microtiming_is_default")]
        microtiming: Microtiming,
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
fn microtiming_is_default(value: &Microtiming) -> bool {
    *value == Microtiming::ZERO
}
impl StepEvent {
    pub fn drum_recipe(&self) -> Option<DrumRecipeSlot> {
        match self {
            Self::Trigger { recipe, .. } => Some(*recipe),
            _ => None,
        }
    }

    pub fn drum_recipe_mut(&mut self) -> Option<&mut DrumRecipeSlot> {
        match self {
            Self::Trigger { recipe, .. } => Some(recipe),
            _ => None,
        }
    }

    pub fn locks(&self) -> &ParameterLocks {
        match self {
            Self::Trigger { locks, .. }
            | Self::BassNote { locks, .. }
            | Self::Note { locks, .. }
            | Self::LeadNote { locks, .. }
            | Self::Tie { locks } => locks,
        }
    }
    pub fn locks_mut(&mut self) -> &mut ParameterLocks {
        match self {
            Self::Trigger { locks, .. }
            | Self::BassNote { locks, .. }
            | Self::Note { locks, .. }
            | Self::LeadNote { locks, .. }
            | Self::Tie { locks } => locks,
        }
    }

    pub fn accent(&self) -> Option<bool> {
        match self {
            Self::Trigger { accent, .. }
            | Self::BassNote { accent, .. }
            | Self::Note { accent, .. } => Some(*accent),
            Self::LeadNote { accent, .. } => Some(*accent),
            Self::Tie { .. } => None,
        }
    }

    pub fn accent_mut(&mut self) -> Option<&mut bool> {
        match self {
            Self::Trigger { accent, .. }
            | Self::BassNote { accent, .. }
            | Self::Note { accent, .. } => Some(accent),
            Self::LeadNote { accent, .. } => Some(accent),
            Self::Tie { .. } => None,
        }
    }

    pub fn slide_mut(&mut self) -> Option<&mut bool> {
        match self {
            Self::BassNote { slide, .. } | Self::LeadNote { slide, .. } => Some(slide),
            _ => None,
        }
    }
    pub fn condition(&self) -> Option<TriggerCondition> {
        match self {
            Self::Trigger { condition, .. }
            | Self::BassNote { condition, .. }
            | Self::Note { condition, .. } => Some(*condition),
            Self::LeadNote { condition, .. } => Some(*condition),
            Self::Tie { .. } => None,
        }
    }
    pub fn condition_mut(&mut self) -> Option<&mut TriggerCondition> {
        match self {
            Self::Trigger { condition, .. }
            | Self::BassNote { condition, .. }
            | Self::Note { condition, .. } => Some(condition),
            Self::LeadNote { condition, .. } => Some(condition),
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
            }
            | Self::LeadNote {
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
            }
            | Self::LeadNote {
                retrigger_count, ..
            } => Some(retrigger_count),
            Self::Tie { .. } => None,
        }
    }

    pub fn microtiming(&self) -> Option<Microtiming> {
        match self {
            Self::Trigger { microtiming, .. }
            | Self::BassNote { microtiming, .. }
            | Self::Note { microtiming, .. }
            | Self::LeadNote { microtiming, .. } => Some(*microtiming),
            Self::Tie { .. } => None,
        }
    }

    pub fn microtiming_mut(&mut self) -> Option<&mut Microtiming> {
        match self {
            Self::Trigger { microtiming, .. }
            | Self::BassNote { microtiming, .. }
            | Self::Note { microtiming, .. }
            | Self::LeadNote { microtiming, .. } => Some(microtiming),
            Self::Tie { .. } => None,
        }
    }
}
pub type Step = Option<StepEvent>;

#[derive(Clone, Debug, PartialEq, Serialize)]
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
    #[serde(default = "default_probability")]
    pub probability: Percent,
    pub instrument: Instrument,
    #[serde(default)]
    pub effects: TrackEffects,
    pub lfos: LfoAssignments,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackWire {
    kind: TrackKind,
    name: String,
    level: Percent,
    #[serde(default = "default_pan")]
    pan: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    #[serde(default)]
    swing: Percent,
    #[serde(default = "default_probability")]
    probability: Percent,
    instrument: Box<serde_json::value::RawValue>,
    #[serde(default)]
    effects: TrackEffects,
    lfos: LfoAssignments,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_degree: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_octave: Option<u8>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    input_accent: bool,
    #[serde(default, skip_serializing_if = "chord_shape_is_default")]
    input_chord_shape: Option<ChordShape>,
    #[serde(default, skip_serializing_if = "arpeggio_is_default")]
    input_chord_arpeggio: Option<ArpeggioConfig>,
}

impl<'de> Deserialize<'de> for Track {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TrackWire::deserialize(deserializer)?;
        let instrument = match wire.kind {
            TrackKind::Kick => Instrument::Kick(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Snare => Instrument::Snare(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Hat => Instrument::Hat(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Tom => Instrument::Tom(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Cymbal => Instrument::Cymbal(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Rimshot => Instrument::Rimshot(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Bass => Instrument::Bass(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Chord => Instrument::Chord(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Lead => Instrument::Lead(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
            TrackKind::Fm => Instrument::Fm(
                serde_json::from_str(wire.instrument.get()).map_err(serde::de::Error::custom)?,
            ),
        };
        Ok(Self {
            kind: wire.kind,
            name: wire.name,
            level: wire.level,
            pan: wire.pan,
            muted: wire.muted,
            delay_send: wire.delay_send,
            reverb_send: wire.reverb_send,
            swing: wire.swing,
            probability: wire.probability,
            instrument,
            effects: wire.effects,
            lfos: wire.lfos,
            input_degree: wire.input_degree,
            input_octave: wire.input_octave,
            input_accent: wire.input_accent,
            input_chord_shape: wire.input_chord_shape,
            input_chord_arpeggio: wire.input_chord_arpeggio,
        })
    }
}

/// A pattern owns only its ten sequences. Instrument and mixer settings live
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

fn p(n: u8) -> Percent {
    Percent(n)
}
fn default_pan() -> Percent {
    Percent(50)
}
fn default_probability() -> Percent {
    Percent(100)
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
            probability: default_probability(),
            instrument,
            effects: TrackEffects::default(),
            lfos: LfoAssignments::default(),
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
                TrackKind::Chord | TrackKind::Lead | TrackKind::Fm => p(20),
                _ => p(0),
            },
            swing: p(0),
            probability: default_probability(),
            instrument,
            effects: TrackEffects::default(),
            lfos: LfoAssignments::default(),
            input_degree: Some(1),
            input_octave: Some(3),
            input_accent: false,
            input_chord_shape: None,
            input_chord_arpeggio: None,
        };
        Self {
            format_version: 22,
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
                        decay: p(15),
                        open: HatRecipe {
                            tune: p(50),
                            decay: p(85),
                        },
                    }),
                ),
                track(
                    TrackKind::Tom,
                    "Tom",
                    Instrument::Tom(TomParameters {
                        tune: p(15),
                        tone: p(35),
                        decay: p(60),
                        medium: TomRecipe {
                            tune: p(50),
                            tone: p(50),
                            decay: p(45),
                        },
                        high: TomRecipe {
                            tune: p(85),
                            tone: p(65),
                            decay: p(35),
                        },
                    }),
                ),
                track(
                    TrackKind::Cymbal,
                    "Cymbal",
                    Instrument::Cymbal(CymbalParameters {
                        tune: p(50),
                        tone: p(55),
                        decay: p(30),
                    }),
                ),
                track(
                    TrackKind::Rimshot,
                    "Rimshot",
                    Instrument::Rimshot(RimshotParameters {
                        tune: p(50),
                        tone: p(50),
                        decay: p(50),
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
                        noise: p(0),
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
                        noise: p(0),
                        sub_mode: LeadSubMode::TwoOctaveSquare,
                        keyboard_tracking: p(50),
                        portamento_time: p(50),
                        cutoff: p(50),
                        resonance: p(35),
                        filter_envelope: p(55),
                        attack: p(0),
                        decay: p(35),
                        sustain: p(55),
                        release: p(20),
                    }),
                ),
                synth(
                    TrackKind::Fm,
                    "FM",
                    Instrument::Fm(FmParameters {
                        algorithm: FmAlgorithm::Cascade,
                        operators: [
                            FmOperator {
                                ratio: FmRatio::One,
                                level: p(100),
                                feedback: p(0),
                            },
                            FmOperator {
                                ratio: FmRatio::Two,
                                level: p(35),
                                feedback: p(8),
                            },
                            FmOperator {
                                ratio: FmRatio::One,
                                level: p(0),
                                feedback: p(0),
                            },
                            FmOperator {
                                ratio: FmRatio::One,
                                level: p(0),
                                feedback: p(0),
                            },
                        ],
                        brightness: p(72),
                        attack: p(0),
                        decay: p(55),
                        sustain: p(55),
                        release: p(40),
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

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.format_version != 22 {
            return Err(ValidationError::Version(self.format_version));
        }
        if self.tracks.len() != TRACK_COUNT {
            return Err(ValidationError::TrackCount);
        }
        let expected = [
            (TrackKind::Kick, "Kick"),
            (TrackKind::Snare, "Snare"),
            (TrackKind::Hat, "Hi-hat"),
            (TrackKind::Tom, "Tom"),
            (TrackKind::Cymbal, "Cymbal"),
            (TrackKind::Rimshot, "Rimshot"),
            (TrackKind::Bass, "Bass"),
            (TrackKind::Chord, "Chord"),
            (TrackKind::Lead, "Lead"),
            (TrackKind::Fm, "FM"),
        ];
        if !(40..=240).contains(&self.globals.tempo_bpm)
            || !self.globals.reverb_time_seconds.is_finite()
            || !(0.2..=10.0).contains(&self.globals.reverb_time_seconds)
            || self.globals.delay_feedback.get() > 95
            || self.globals.reverb_tone.get() > 100
            || self.globals.reverb_pre_delay_ms > 200
            || self.globals.reverb_return.get() > 100
        {
            return Err(ValidationError::TrackOrder(0, "valid globals"));
        }
        if !(MIN_PATTERN_COUNT..=MAX_PATTERN_COUNT).contains(&self.patterns.len()) {
            return Err(ValidationError::PatternCount(self.patterns.len()));
        }
        if self.song.is_empty() || self.song.len() > MAX_SONG_ENTRY_COUNT {
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
            let pitched = matches!(
                t.kind,
                TrackKind::Bass | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
            );
            let instrument_ok = matches!(
                (t.kind, t.instrument),
                (TrackKind::Kick, Instrument::Kick(_))
                    | (TrackKind::Snare, Instrument::Snare(_))
                    | (TrackKind::Hat, Instrument::Hat(_))
                    | (TrackKind::Tom, Instrument::Tom(_))
                    | (TrackKind::Cymbal, Instrument::Cymbal(_))
                    | (TrackKind::Rimshot, Instrument::Rimshot(_))
                    | (TrackKind::Bass, Instrument::Bass(_))
                    | (TrackKind::Chord, Instrument::Chord(_))
                    | (TrackKind::Lead, Instrument::Lead(_))
                    | (TrackKind::Fm, Instrument::Fm(_))
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
            if t.probability.get() > 100 {
                return Err(ValidationError::Probability(ti));
            }
            if t.effects.phaser.feedback.get() > 90 {
                return Err(ValidationError::Effect(ti, "phaser_feedback"));
            }
            if t.effects.flanger.feedback.get() > 90 {
                return Err(ValidationError::Effect(ti, "flanger_feedback"));
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
                                TrackKind::Kick
                                    | TrackKind::Snare
                                    | TrackKind::Hat
                                    | TrackKind::Tom
                                    | TrackKind::Cymbal
                                    | TrackKind::Rimshot,
                                StepEvent::Trigger { .. }
                            ) | (TrackKind::Bass, StepEvent::BassNote { .. })
                                | (TrackKind::Bass, StepEvent::Tie { .. })
                                | (TrackKind::Chord, StepEvent::Note { .. })
                                | (TrackKind::Lead, StepEvent::LeadNote { .. })
                                | (
                                    TrackKind::Fm,
                                    StepEvent::Note {
                                        chord_shape: None,
                                        arpeggio: ArpeggioConfig { enabled: false, .. },
                                        ..
                                    }
                                )
                                | (
                                    TrackKind::Chord | TrackKind::Lead | TrackKind::Fm,
                                    StepEvent::Tie { .. }
                                )
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
                        if let StepEvent::Trigger { recipe, .. } = event {
                            let maximum = match track.kind {
                                TrackKind::Hat => 2,
                                TrackKind::Tom => 3,
                                _ => 1,
                            };
                            if recipe.get() > maximum {
                                return Err(ValidationError::DrumRecipe(ti, si));
                            }
                        }
                        if let StepEvent::Note {
                            chord_shape,
                            arpeggio,
                            ..
                        } = event
                        {
                            if matches!(track.kind, TrackKind::Lead | TrackKind::Fm)
                                && (chord_shape.is_some() || !arpeggio.is_default())
                            {
                                return Err(ValidationError::EventKind(ti, si));
                            }
                        }
                        if let StepEvent::Note { degree, octave, .. }
                        | StepEvent::LeadNote { degree, octave, .. }
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
                    TrackKind::Bass | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
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
    if l.percent(ParameterId::PhaserFeedback)
        .is_some_and(|value| value.get() > 90)
    {
        return Err(ValidationError::Lock(ti, si, "phaser_feedback"));
    }
    if l.percent(ParameterId::FlangerFeedback)
        .is_some_and(|value| value.get() > 90)
    {
        return Err(ValidationError::Lock(ti, si, "flanger_feedback"));
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
            Some(
                StepEvent::Note { .. } | StepEvent::LeadNote { .. } | StepEvent::BassNote { .. },
            ) => return Some(i),
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
    Noise,
    LeadSubMode,
    KeyboardTracking,
    PortamentoTime,
    Chorus,
    Spread,
    Cutoff,
    Resonance,
    FilterEnvelope,
    Attack,
    Sustain,
    Release,
    Pitch,
    FlangerRate,
    FlangerDelay,
    FlangerDepth,
    FlangerFeedback,
    FlangerMix,
    BitCrusherBits,
    BitCrusherRate,
    BitCrusherMix,
    FmAlgorithm,
    FmOp1Ratio,
    FmOp1Level,
    FmOp1Feedback,
    FmOp2Ratio,
    FmOp2Level,
    FmOp2Feedback,
    FmOp3Ratio,
    FmOp3Level,
    FmOp3Feedback,
    FmOp4Ratio,
    FmOp4Level,
    FmOp4Feedback,
    Brightness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterValue {
    Percent(Percent),
    Waveform(Waveform),
    Chorus(ChorusMode),
    Spread(ChordSpread),
    LeadSubMode(LeadSubMode),
    FmAlgorithm(FmAlgorithm),
    FmRatio(FmRatio),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterValueKind {
    Percent,
    Waveform,
    Chorus,
    Spread,
    LeadSubMode,
    FmAlgorithm,
    FmRatio,
}

impl ParameterId {
    pub const COUNT: usize = 54;
    pub const ALL: [Self; Self::COUNT] = [
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
        Self::Noise,
        Self::LeadSubMode,
        Self::KeyboardTracking,
        Self::PortamentoTime,
        Self::Chorus,
        Self::Spread,
        Self::Cutoff,
        Self::Resonance,
        Self::FilterEnvelope,
        Self::Attack,
        Self::Sustain,
        Self::Release,
        Self::Pitch,
        Self::FlangerRate,
        Self::FlangerDelay,
        Self::FlangerDepth,
        Self::FlangerFeedback,
        Self::FlangerMix,
        Self::BitCrusherBits,
        Self::BitCrusherRate,
        Self::BitCrusherMix,
        Self::FmAlgorithm,
        Self::FmOp1Ratio,
        Self::FmOp1Level,
        Self::FmOp1Feedback,
        Self::FmOp2Ratio,
        Self::FmOp2Level,
        Self::FmOp2Feedback,
        Self::FmOp3Ratio,
        Self::FmOp3Level,
        Self::FmOp3Feedback,
        Self::FmOp4Ratio,
        Self::FmOp4Level,
        Self::FmOp4Feedback,
        Self::Brightness,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|parameter| parameter.name() == name)
    }

    pub const fn value_kind(self) -> ParameterValueKind {
        match self {
            Self::Waveform => ParameterValueKind::Waveform,
            Self::Chorus => ParameterValueKind::Chorus,
            Self::Spread => ParameterValueKind::Spread,
            Self::LeadSubMode => ParameterValueKind::LeadSubMode,
            Self::FmAlgorithm => ParameterValueKind::FmAlgorithm,
            Self::FmOp1Ratio | Self::FmOp2Ratio | Self::FmOp3Ratio | Self::FmOp4Ratio => {
                ParameterValueKind::FmRatio
            }
            _ => ParameterValueKind::Percent,
        }
    }

    pub const fn fm_operator(operator: usize, field: FmOperatorField) -> Option<Self> {
        match (operator, field) {
            (0, FmOperatorField::Ratio) => Some(Self::FmOp1Ratio),
            (0, FmOperatorField::Level) => Some(Self::FmOp1Level),
            (0, FmOperatorField::Feedback) => Some(Self::FmOp1Feedback),
            (1, FmOperatorField::Ratio) => Some(Self::FmOp2Ratio),
            (1, FmOperatorField::Level) => Some(Self::FmOp2Level),
            (1, FmOperatorField::Feedback) => Some(Self::FmOp2Feedback),
            (2, FmOperatorField::Ratio) => Some(Self::FmOp3Ratio),
            (2, FmOperatorField::Level) => Some(Self::FmOp3Level),
            (2, FmOperatorField::Feedback) => Some(Self::FmOp3Feedback),
            (3, FmOperatorField::Ratio) => Some(Self::FmOp4Ratio),
            (3, FmOperatorField::Level) => Some(Self::FmOp4Level),
            (3, FmOperatorField::Feedback) => Some(Self::FmOp4Feedback),
            _ => None,
        }
    }

    pub const fn fm_operator_field(self) -> Option<(usize, FmOperatorField)> {
        match self {
            Self::FmOp1Ratio => Some((0, FmOperatorField::Ratio)),
            Self::FmOp1Level => Some((0, FmOperatorField::Level)),
            Self::FmOp1Feedback => Some((0, FmOperatorField::Feedback)),
            Self::FmOp2Ratio => Some((1, FmOperatorField::Ratio)),
            Self::FmOp2Level => Some((1, FmOperatorField::Level)),
            Self::FmOp2Feedback => Some((1, FmOperatorField::Feedback)),
            Self::FmOp3Ratio => Some((2, FmOperatorField::Ratio)),
            Self::FmOp3Level => Some((2, FmOperatorField::Level)),
            Self::FmOp3Feedback => Some((2, FmOperatorField::Feedback)),
            Self::FmOp4Ratio => Some((3, FmOperatorField::Ratio)),
            Self::FmOp4Level => Some((3, FmOperatorField::Level)),
            Self::FmOp4Feedback => Some((3, FmOperatorField::Feedback)),
            _ => None,
        }
    }

    pub const fn is_percentage(self) -> bool {
        matches!(self.value_kind(), ParameterValueKind::Percent)
    }

    pub const fn upper_bound(self) -> u8 {
        match self {
            Self::PhaserFeedback | Self::FlangerFeedback => 90,
            _ => 100,
        }
    }

    pub const fn is_lockable(self) -> bool {
        !matches!(self, Self::Pitch)
    }

    pub const fn is_lfo_assignable(self) -> bool {
        matches!(
            self,
            Self::Pan
                | Self::Level
                | Self::Tune
                | Self::Tone
                | Self::Snappy
                | Self::Decay
                | Self::OscillatorMix
                | Self::PulseWidth
                | Self::SubOscillator
                | Self::Noise
                | Self::KeyboardTracking
                | Self::Cutoff
                | Self::Resonance
                | Self::FilterEnvelope
                | Self::Attack
                | Self::Sustain
                | Self::Release
                | Self::Pitch
                | Self::FmOp1Level
                | Self::FmOp1Feedback
                | Self::FmOp2Level
                | Self::FmOp2Feedback
                | Self::FmOp3Level
                | Self::FmOp3Feedback
                | Self::FmOp4Level
                | Self::FmOp4Feedback
                | Self::Brightness
        )
    }

    pub const fn accepts_lock_value(self, value: ParameterValue) -> bool {
        if !self.is_lockable() {
            return false;
        }
        match (self.value_kind(), value) {
            (ParameterValueKind::Percent, ParameterValue::Percent(value)) => {
                value.get() <= self.upper_bound()
            }
            (ParameterValueKind::Waveform, ParameterValue::Waveform(_))
            | (ParameterValueKind::Chorus, ParameterValue::Chorus(_))
            | (ParameterValueKind::Spread, ParameterValue::Spread(_))
            | (ParameterValueKind::LeadSubMode, ParameterValue::LeadSubMode(_)) => true,
            (ParameterValueKind::FmAlgorithm, ParameterValue::FmAlgorithm(_))
            | (ParameterValueKind::FmRatio, ParameterValue::FmRatio(_)) => true,
            _ => false,
        }
    }

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
            | Self::PhaserMix
            | Self::FlangerRate
            | Self::FlangerDelay
            | Self::FlangerDepth
            | Self::FlangerFeedback
            | Self::FlangerMix
            | Self::BitCrusherBits
            | Self::BitCrusherRate
            | Self::BitCrusherMix => true,
            Self::Tune => matches!(
                kind,
                TrackKind::Kick
                    | TrackKind::Snare
                    | TrackKind::Hat
                    | TrackKind::Tom
                    | TrackKind::Cymbal
                    | TrackKind::Rimshot
            ),
            Self::Tone => matches!(
                kind,
                TrackKind::Snare | TrackKind::Tom | TrackKind::Cymbal | TrackKind::Rimshot
            ),
            Self::Snappy => matches!(kind, TrackKind::Snare),
            Self::Decay => matches!(
                kind,
                TrackKind::Kick
                    | TrackKind::Hat
                    | TrackKind::Tom
                    | TrackKind::Cymbal
                    | TrackKind::Rimshot
                    | TrackKind::Bass
                    | TrackKind::Chord
                    | TrackKind::Lead
                    | TrackKind::Fm
            ),
            Self::Attack => matches!(
                kind,
                TrackKind::Kick | TrackKind::Chord | TrackKind::Lead | TrackKind::Fm
            ),
            Self::Waveform => matches!(kind, TrackKind::Bass),
            Self::OscillatorMix | Self::PulseWidth | Self::SubOscillator | Self::Noise => {
                matches!(kind, TrackKind::Chord | TrackKind::Lead)
            }
            Self::LeadSubMode | Self::KeyboardTracking | Self::PortamentoTime => {
                matches!(kind, TrackKind::Lead)
            }
            Self::Chorus => matches!(kind, TrackKind::Chord),
            Self::Spread => matches!(kind, TrackKind::Chord),
            Self::Cutoff | Self::Resonance | Self::FilterEnvelope => {
                matches!(kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead)
            }
            Self::Sustain | Self::Release => {
                matches!(kind, TrackKind::Chord | TrackKind::Lead | TrackKind::Fm)
            }
            Self::FmAlgorithm
            | Self::FmOp1Ratio
            | Self::FmOp1Level
            | Self::FmOp1Feedback
            | Self::FmOp2Ratio
            | Self::FmOp2Level
            | Self::FmOp2Feedback
            | Self::FmOp3Ratio
            | Self::FmOp3Level
            | Self::FmOp3Feedback
            | Self::FmOp4Ratio
            | Self::FmOp4Level
            | Self::FmOp4Feedback
            | Self::Brightness => matches!(kind, TrackKind::Fm),
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
        (matches!(self, Self::Pitch)
            && matches!(kind, TrackKind::Chord | TrackKind::Lead | TrackKind::Fm))
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
                        | Self::FlangerRate
                        | Self::FlangerDelay
                        | Self::FlangerDepth
                        | Self::FlangerFeedback
                        | Self::FlangerMix
                        | Self::BitCrusherBits
                        | Self::BitCrusherRate
                        | Self::BitCrusherMix
                        | Self::Waveform
                        | Self::Noise
                        | Self::LeadSubMode
                        | Self::KeyboardTracking
                        | Self::PortamentoTime
                        | Self::Chorus
                        | Self::Spread
                        | Self::FmAlgorithm
                        | Self::FmOp1Ratio
                        | Self::FmOp2Ratio
                        | Self::FmOp3Ratio
                        | Self::FmOp4Ratio
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
            Self::Noise => "noise",
            Self::LeadSubMode => "sub_mode",
            Self::KeyboardTracking => "keyboard_tracking",
            Self::PortamentoTime => "portamento_time",
            Self::Chorus => "chorus",
            Self::Spread => "spread",
            Self::Cutoff => "cutoff",
            Self::Resonance => "resonance",
            Self::FilterEnvelope => "filter_envelope",
            Self::Attack => "attack",
            Self::Sustain => "sustain",
            Self::Release => "release",
            Self::Pitch => "pitch",
            Self::FlangerRate => "flanger_rate",
            Self::FlangerDelay => "flanger_delay",
            Self::FlangerDepth => "flanger_depth",
            Self::FlangerFeedback => "flanger_feedback",
            Self::FlangerMix => "flanger_mix",
            Self::BitCrusherBits => "bit_crusher_bits",
            Self::BitCrusherRate => "bit_crusher_rate",
            Self::BitCrusherMix => "bit_crusher_mix",
            Self::FmAlgorithm => "fm_algorithm",
            Self::FmOp1Ratio => "fm_op1_ratio",
            Self::FmOp1Level => "fm_op1_level",
            Self::FmOp1Feedback => "fm_op1_feedback",
            Self::FmOp2Ratio => "fm_op2_ratio",
            Self::FmOp2Level => "fm_op2_level",
            Self::FmOp2Feedback => "fm_op2_feedback",
            Self::FmOp3Ratio => "fm_op3_ratio",
            Self::FmOp3Level => "fm_op3_level",
            Self::FmOp3Feedback => "fm_op3_feedback",
            Self::FmOp4Ratio => "fm_op4_ratio",
            Self::FmOp4Level => "fm_op4_level",
            Self::FmOp4Feedback => "fm_op4_feedback",
            Self::Brightness => "brightness",
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
            Self::FlangerRate => "flanger rate",
            Self::FlangerDelay => "flanger delay",
            Self::FlangerDepth => "flanger depth",
            Self::FlangerFeedback => "flanger feedback",
            Self::FlangerMix => "flanger mix",
            Self::BitCrusherBits => "bit crusher bits",
            Self::BitCrusherRate => "bit crusher rate",
            Self::BitCrusherMix => "bit crusher mix",
            Self::FmAlgorithm => "FM algorithm",
            Self::FmOp1Ratio => "operator 1 ratio",
            Self::FmOp1Level => "operator 1 level",
            Self::FmOp1Feedback => "operator 1 feedback",
            Self::FmOp2Ratio => "operator 2 ratio",
            Self::FmOp2Level => "operator 2 level",
            Self::FmOp2Feedback => "operator 2 feedback",
            Self::FmOp3Ratio => "operator 3 ratio",
            Self::FmOp3Level => "operator 3 level",
            Self::FmOp3Feedback => "operator 3 feedback",
            Self::FmOp4Ratio => "operator 4 ratio",
            Self::FmOp4Level => "operator 4 level",
            Self::FmOp4Feedback => "operator 4 feedback",
            Self::Brightness => "brightness",
            _ => self.name(),
        }
    }
}

impl Track {
    pub const fn drum_recipe_count(&self) -> u8 {
        match self.kind {
            TrackKind::Hat => 2,
            TrackKind::Tom => 3,
            _ => 1,
        }
    }

    pub fn drum_recipe_parameter(
        &self,
        recipe: DrumRecipeSlot,
        parameter: ParameterId,
    ) -> Option<ParameterValue> {
        if recipe == DrumRecipeSlot::ONE {
            return self.parameter(parameter);
        }
        match (self.instrument, recipe, parameter) {
            (Instrument::Hat(p), DrumRecipeSlot::TWO, ParameterId::Tune) => {
                Some(ParameterValue::Percent(p.open.tune))
            }
            (Instrument::Hat(p), DrumRecipeSlot::TWO, ParameterId::Decay) => {
                Some(ParameterValue::Percent(p.open.decay))
            }
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Tune) => {
                Some(ParameterValue::Percent(p.medium.tune))
            }
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Tone) => {
                Some(ParameterValue::Percent(p.medium.tone))
            }
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Decay) => {
                Some(ParameterValue::Percent(p.medium.decay))
            }
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Tune) => {
                Some(ParameterValue::Percent(p.high.tune))
            }
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Tone) => {
                Some(ParameterValue::Percent(p.high.tone))
            }
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Decay) => {
                Some(ParameterValue::Percent(p.high.decay))
            }
            _ => None,
        }
    }

    pub fn set_drum_recipe_parameter(
        &mut self,
        recipe: DrumRecipeSlot,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> bool {
        if recipe == DrumRecipeSlot::ONE {
            return self.set_parameter(parameter, value);
        }
        let ParameterValue::Percent(value) = value else {
            return false;
        };
        match (&mut self.instrument, recipe, parameter) {
            (Instrument::Hat(p), DrumRecipeSlot::TWO, ParameterId::Tune) => p.open.tune = value,
            (Instrument::Hat(p), DrumRecipeSlot::TWO, ParameterId::Decay) => p.open.decay = value,
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Tune) => p.medium.tune = value,
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Tone) => p.medium.tone = value,
            (Instrument::Tom(p), DrumRecipeSlot::TWO, ParameterId::Decay) => p.medium.decay = value,
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Tune) => p.high.tune = value,
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Tone) => p.high.tone = value,
            (Instrument::Tom(p), DrumRecipeSlot::THREE, ParameterId::Decay) => p.high.decay = value,
            _ => return false,
        }
        true
    }

    pub fn parameter(&self, parameter: ParameterId) -> Option<ParameterValue> {
        if let Some((operator, field)) = parameter.fm_operator_field() {
            let Instrument::Fm(parameters) = self.instrument else {
                return None;
            };
            let operator = parameters.operators[operator];
            return Some(match field {
                FmOperatorField::Ratio => ParameterValue::FmRatio(operator.ratio),
                FmOperatorField::Level => ParameterValue::Percent(operator.level),
                FmOperatorField::Feedback => ParameterValue::Percent(operator.feedback),
            });
        }
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
            ParameterId::FlangerRate => ParameterValue::Percent(self.effects.flanger.rate),
            ParameterId::FlangerDelay => ParameterValue::Percent(self.effects.flanger.delay),
            ParameterId::FlangerDepth => ParameterValue::Percent(self.effects.flanger.depth),
            ParameterId::FlangerFeedback => ParameterValue::Percent(self.effects.flanger.feedback),
            ParameterId::FlangerMix => ParameterValue::Percent(self.effects.flanger.mix),
            ParameterId::BitCrusherBits => ParameterValue::Percent(self.effects.bit_crusher.bits),
            ParameterId::BitCrusherRate => ParameterValue::Percent(self.effects.bit_crusher.rate),
            ParameterId::BitCrusherMix => ParameterValue::Percent(self.effects.bit_crusher.mix),
            ParameterId::Tune => match self.instrument {
                Instrument::Kick(p) => ParameterValue::Percent(p.tune),
                Instrument::Snare(p) => ParameterValue::Percent(p.tune),
                Instrument::Hat(p) => ParameterValue::Percent(p.tune),
                Instrument::Tom(p) => ParameterValue::Percent(p.tune),
                Instrument::Cymbal(p) => ParameterValue::Percent(p.tune),
                Instrument::Rimshot(p) => ParameterValue::Percent(p.tune),
                _ => return None,
            },
            ParameterId::Tone => match self.instrument {
                Instrument::Snare(p) => ParameterValue::Percent(p.tone),
                Instrument::Tom(p) => ParameterValue::Percent(p.tone),
                Instrument::Cymbal(p) => ParameterValue::Percent(p.tone),
                Instrument::Rimshot(p) => ParameterValue::Percent(p.tone),
                _ => return None,
            },
            ParameterId::Snappy => match self.instrument {
                Instrument::Snare(p) => ParameterValue::Percent(p.snappy),
                _ => return None,
            },
            ParameterId::Decay => match self.instrument {
                Instrument::Kick(p) => ParameterValue::Percent(p.decay),
                Instrument::Hat(p) => ParameterValue::Percent(p.decay),
                Instrument::Tom(p) => ParameterValue::Percent(p.decay),
                Instrument::Cymbal(p) => ParameterValue::Percent(p.decay),
                Instrument::Rimshot(p) => ParameterValue::Percent(p.decay),
                Instrument::Bass(p) => ParameterValue::Percent(p.decay),
                Instrument::Chord(p) => ParameterValue::Percent(p.decay),
                Instrument::Lead(p) => ParameterValue::Percent(p.decay),
                Instrument::Fm(p) => ParameterValue::Percent(p.decay),
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
            ParameterId::Noise => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.noise),
                Instrument::Lead(p) => ParameterValue::Percent(p.noise),
                _ => return None,
            },
            ParameterId::LeadSubMode => match self.instrument {
                Instrument::Lead(p) => ParameterValue::LeadSubMode(p.sub_mode),
                _ => return None,
            },
            ParameterId::KeyboardTracking => match self.instrument {
                Instrument::Lead(p) => ParameterValue::Percent(p.keyboard_tracking),
                _ => return None,
            },
            ParameterId::PortamentoTime => match self.instrument {
                Instrument::Lead(p) => ParameterValue::Percent(p.portamento_time),
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
                Instrument::Fm(p) => ParameterValue::Percent(p.attack),
                _ => return None,
            },
            ParameterId::Sustain => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.sustain),
                Instrument::Lead(p) => ParameterValue::Percent(p.sustain),
                Instrument::Fm(p) => ParameterValue::Percent(p.sustain),
                _ => return None,
            },
            ParameterId::Release => match self.instrument {
                Instrument::Chord(p) => ParameterValue::Percent(p.release),
                Instrument::Lead(p) => ParameterValue::Percent(p.release),
                Instrument::Fm(p) => ParameterValue::Percent(p.release),
                _ => return None,
            },
            ParameterId::FmAlgorithm => match self.instrument {
                Instrument::Fm(p) => ParameterValue::FmAlgorithm(p.algorithm),
                _ => return None,
            },
            ParameterId::FmOp1Ratio
            | ParameterId::FmOp1Level
            | ParameterId::FmOp1Feedback
            | ParameterId::FmOp2Ratio
            | ParameterId::FmOp2Level
            | ParameterId::FmOp2Feedback
            | ParameterId::FmOp3Ratio
            | ParameterId::FmOp3Level
            | ParameterId::FmOp3Feedback
            | ParameterId::FmOp4Ratio
            | ParameterId::FmOp4Level
            | ParameterId::FmOp4Feedback => unreachable!("operator parameters returned above"),
            ParameterId::Brightness => match self.instrument {
                Instrument::Fm(p) => ParameterValue::Percent(p.brightness),
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
        if let Some((operator, field)) = parameter.fm_operator_field() {
            let Instrument::Fm(parameters) = &mut self.instrument else {
                return false;
            };
            match (field, value) {
                (FmOperatorField::Ratio, ParameterValue::FmRatio(value)) => {
                    parameters.operators[operator].ratio = value
                }
                (FmOperatorField::Level, ParameterValue::Percent(value)) => {
                    parameters.operators[operator].level = value
                }
                (FmOperatorField::Feedback, ParameterValue::Percent(value)) => {
                    parameters.operators[operator].feedback = value
                }
                _ => return false,
            }
            return true;
        }
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
            (ParameterId::FlangerRate, ParameterValue::Percent(v)) => self.effects.flanger.rate = v,
            (ParameterId::FlangerDelay, ParameterValue::Percent(v)) => {
                self.effects.flanger.delay = v
            }
            (ParameterId::FlangerDepth, ParameterValue::Percent(v)) => {
                self.effects.flanger.depth = v
            }
            (ParameterId::FlangerFeedback, ParameterValue::Percent(v)) => {
                if v.get() > 90 {
                    return false;
                }
                self.effects.flanger.feedback = v
            }
            (ParameterId::FlangerMix, ParameterValue::Percent(v)) => self.effects.flanger.mix = v,
            (ParameterId::BitCrusherBits, ParameterValue::Percent(v)) => {
                self.effects.bit_crusher.bits = v
            }
            (ParameterId::BitCrusherRate, ParameterValue::Percent(v)) => {
                self.effects.bit_crusher.rate = v
            }
            (ParameterId::BitCrusherMix, ParameterValue::Percent(v)) => {
                self.effects.bit_crusher.mix = v
            }
            (ParameterId::Tune, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Kick(p) => p.tune = v,
                Instrument::Snare(p) => p.tune = v,
                Instrument::Hat(p) => p.tune = v,
                Instrument::Tom(p) => p.tune = v,
                Instrument::Cymbal(p) => p.tune = v,
                Instrument::Rimshot(p) => p.tune = v,
                _ => return false,
            },
            (ParameterId::Tone, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Snare(p) => p.tone = v,
                Instrument::Tom(p) => p.tone = v,
                Instrument::Cymbal(p) => p.tone = v,
                Instrument::Rimshot(p) => p.tone = v,
                _ => return false,
            },
            (ParameterId::Snappy, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Snare(p) => p.snappy = v,
                _ => return false,
            },
            (ParameterId::Decay, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Kick(p) => p.decay = v,
                Instrument::Hat(p) => p.decay = v,
                Instrument::Tom(p) => p.decay = v,
                Instrument::Cymbal(p) => p.decay = v,
                Instrument::Rimshot(p) => p.decay = v,
                Instrument::Bass(p) => p.decay = v,
                Instrument::Chord(p) => p.decay = v,
                Instrument::Lead(p) => p.decay = v,
                Instrument::Fm(p) => p.decay = v,
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
            (ParameterId::Noise, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.noise = v,
                Instrument::Lead(p) => p.noise = v,
                _ => return false,
            },
            (ParameterId::LeadSubMode, ParameterValue::LeadSubMode(v)) => {
                match &mut self.instrument {
                    Instrument::Lead(p) => p.sub_mode = v,
                    _ => return false,
                }
            }
            (ParameterId::KeyboardTracking, ParameterValue::Percent(v)) => {
                match &mut self.instrument {
                    Instrument::Lead(p) => p.keyboard_tracking = v,
                    _ => return false,
                }
            }
            (ParameterId::PortamentoTime, ParameterValue::Percent(v)) => match &mut self.instrument
            {
                Instrument::Lead(p) => p.portamento_time = v,
                _ => return false,
            },
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
                Instrument::Fm(p) => p.attack = v,
                _ => return false,
            },
            (ParameterId::Sustain, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.sustain = v,
                Instrument::Lead(p) => p.sustain = v,
                Instrument::Fm(p) => p.sustain = v,
                _ => return false,
            },
            (ParameterId::Release, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Chord(p) => p.release = v,
                Instrument::Lead(p) => p.release = v,
                Instrument::Fm(p) => p.release = v,
                _ => return false,
            },
            (ParameterId::FmAlgorithm, ParameterValue::FmAlgorithm(v)) => {
                match &mut self.instrument {
                    Instrument::Fm(p) => p.algorithm = v,
                    _ => return false,
                }
            }
            (ParameterId::Brightness, ParameterValue::Percent(v)) => match &mut self.instrument {
                Instrument::Fm(p) => p.brightness = v,
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
        assert_eq!(project.tracks.len(), TRACK_COUNT);
        assert_eq!(project.tracks[RIMSHOT_TRACK_INDEX].kind, TrackKind::Rimshot);
        assert_eq!(project.tracks[RIMSHOT_TRACK_INDEX].name, "Rimshot");
        assert!(matches!(
            project.tracks[RIMSHOT_TRACK_INDEX].instrument,
            Instrument::Rimshot(RimshotParameters {
                tune: Percent(50),
                tone: Percent(50),
                decay: Percent(50),
            })
        ));
        project.validate().unwrap();
    }

    #[test]
    fn rimshot_supports_only_its_documented_instrument_parameters() {
        for parameter in [ParameterId::Tune, ParameterId::Tone, ParameterId::Decay] {
            assert!(parameter.is_valid_for(TrackKind::Rimshot));
            assert!(parameter.supports_lfo(TrackKind::Rimshot));
        }
        for parameter in [
            ParameterId::Snappy,
            ParameterId::Attack,
            ParameterId::Cutoff,
        ] {
            assert!(!parameter.is_valid_for(TrackKind::Rimshot));
        }
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
        project.globals.reverb_return = p(100);
        assert!(project.validate().is_ok());

        project.globals.reverb_pre_delay_ms = 201;
        assert!(project.validate().is_err());

        project.globals.reverb_pre_delay_ms = 200;
        project.globals.reverb_return = p(101);
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
    fn scales_have_expected_offsets_and_clamped_selector_order() {
        let cases = [
            (Scale::Major, [0, 2, 4, 5, 7, 9, 11, 12]),
            (Scale::Dorian, [0, 2, 3, 5, 7, 9, 10, 12]),
            (Scale::Phrygian, [0, 1, 3, 5, 7, 8, 10, 12]),
            (Scale::Lydian, [0, 2, 4, 6, 7, 9, 11, 12]),
            (Scale::Mixolydian, [0, 2, 4, 5, 7, 9, 10, 12]),
            (Scale::NaturalMinor, [0, 2, 3, 5, 7, 8, 10, 12]),
            (Scale::Locrian, [0, 1, 3, 5, 6, 8, 10, 12]),
        ];
        assert_eq!(
            Scale::ALL.map(|scale| scale.offsets()),
            cases.map(|(_, offsets)| offsets)
        );
        assert_eq!(Scale::Major.shifted(-1), Scale::Major);
        assert_eq!(Scale::Major.shifted(1), Scale::Dorian);
        assert_eq!(Scale::Locrian.shifted(1), Scale::Locrian);
        assert_eq!(Scale::Locrian.shifted(-1), Scale::NaturalMinor);
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
        assert_eq!(ChordShape::Single.to_string(), "1");
        assert_eq!(ChordShape::DyadThird.to_string(), "1-3");
        assert_eq!(ChordShape::DyadFifth.to_string(), "1-5");
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::Single),
            Some(([48, 0, 0, 0], 1))
        );
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::DyadThird),
            Some(([48, 52, 0, 0], 2))
        );
        assert_eq!(
            project.chord_midis_for(1, 3, ChordShape::DyadFifth),
            Some(([48, 55, 0, 0], 2))
        );
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
    fn microtiming_is_signed_and_bounded() {
        assert_eq!(Microtiming::new(-50).unwrap().get(), -50);
        assert_eq!(Microtiming::new(50).unwrap().get(), 50);
        assert_eq!(Microtiming::new(-51), None);
        assert_eq!(Microtiming::new(51), None);
        assert_eq!(Microtiming::new(49).unwrap().saturating_add(10).get(), 50);
        assert_eq!(
            Microtiming::new(-49).unwrap().saturating_add(-10).get(),
            -50
        );
        assert_eq!(Microtiming::new(25).unwrap().to_string(), "+25%");
        assert_eq!(
            serde_json::to_string(&Microtiming::new(-25).unwrap()).unwrap(),
            "-25"
        );
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
            microtiming: crate::model::Microtiming::ZERO,
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
        project.patterns[0].tracks[SYNTH_TRACK_START].steps = vec![None; 3];
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[2] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        assert_eq!(
            tie_source(&project.patterns[0].tracks[SYNTH_TRACK_START].steps, 0),
            Some(2)
        );
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
    fn track_probability_defaults_to_100_percent() {
        let project = Project::new();
        assert!(
            project
                .tracks
                .iter()
                .all(|track| track.probability == Percent::new(100).unwrap())
        );
        let value = serde_json::to_value(&project).unwrap();
        assert_eq!(value["tracks"][0]["probability"], 100);
    }

    #[test]
    fn effects_have_shared_defaults_and_are_lockable_on_every_track() {
        let project = Project::new();
        assert_eq!(project.format_version, 22);
        assert_eq!(project.globals.sidechain, SidechainParameters::default());
        assert_eq!(project.globals.sidechain.depth_db(), 0.0);
        assert!((project.globals.sidechain.attack_ms() - 1.134).abs() < 0.01);
        assert_eq!(project.tracks[0].effects.distortion.drive, Percent::ZERO);
        assert_eq!(project.tracks[0].effects.distortion.tone, p(50));
        assert_eq!(project.tracks[0].effects.phaser.rate, p(25));
        assert_eq!(project.tracks[0].effects.flanger.delay, p(18));
        assert_eq!(project.tracks[0].effects.flanger.depth, p(50));
        assert_eq!(project.tracks[0].effects.bit_crusher.bits, p(50));
        assert_eq!(project.tracks[0].effects.bit_crusher.rate, p(50));
        assert_eq!(project.tracks[0].effects.bit_crusher.mix, Percent::ZERO);
        assert!(ParameterId::PhaserMix.is_valid_for(TrackKind::Kick));
        assert!(!ParameterId::PhaserMix.supports_lfo(TrackKind::Kick));
        assert!(ParameterId::FlangerMix.is_valid_for(TrackKind::Kick));
        assert!(!ParameterId::FlangerMix.supports_lfo(TrackKind::Kick));
        assert!(ParameterId::BitCrusherMix.is_valid_for(TrackKind::Kick));
        assert!(!ParameterId::BitCrusherMix.supports_lfo(TrackKind::Kick));

        let mut locks = ParameterLocks::default();
        assert!(locks.set(ParameterId::DistortionDrive, ParameterValue::Percent(p(80))));
        assert_eq!(
            locks.get(ParameterId::DistortionDrive),
            Some(ParameterValue::Percent(p(80)))
        );
        assert!(locks.set(ParameterId::PhaserFeedback, ParameterValue::Percent(p(90))));
        assert_eq!(locks.percent(ParameterId::PhaserFeedback), Some(p(90)));
        assert!(!locks.set(ParameterId::PhaserFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(locks.percent(ParameterId::PhaserFeedback), Some(p(90)));
        assert!(locks.set(ParameterId::FlangerFeedback, ParameterValue::Percent(p(90))));
        assert_eq!(locks.percent(ParameterId::FlangerFeedback), Some(p(90)));
        assert!(locks.set(ParameterId::BitCrusherBits, ParameterValue::Percent(p(75))));
        assert_eq!(locks.percent(ParameterId::BitCrusherBits), Some(p(75)));
        assert!(!locks.set(ParameterId::FlangerFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(locks.percent(ParameterId::FlangerFeedback), Some(p(90)));

        let mut track = project.tracks[0].clone();
        assert!(track.set_parameter(ParameterId::PhaserFeedback, ParameterValue::Percent(p(90))));
        assert!(!track.set_parameter(ParameterId::PhaserFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(track.effects.phaser.feedback, p(90));
        assert!(track.set_parameter(ParameterId::FlangerFeedback, ParameterValue::Percent(p(90))));
        assert!(!track.set_parameter(ParameterId::FlangerFeedback, ParameterValue::Percent(p(91))));
        assert_eq!(track.effects.flanger.feedback, p(90));
        assert!(track.set_parameter(ParameterId::BitCrusherRate, ParameterValue::Percent(p(65))));
        assert_eq!(track.effects.bit_crusher.rate, p(65));

        let mut invalid = project;
        invalid.tracks[0].effects.phaser.feedback = p(91);
        assert_eq!(
            invalid.validate(),
            Err(ValidationError::Effect(0, "phaser_feedback"))
        );

        invalid.tracks[0].effects.phaser.feedback = p(20);
        invalid.tracks[0].effects.flanger.feedback = p(91);
        assert_eq!(
            invalid.validate(),
            Err(ValidationError::Effect(0, "flanger_feedback"))
        );
    }
    #[test]
    fn accent_and_slide_event_fields_are_strict() {
        let trigger = StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        };
        assert_eq!(serde_json::to_value(trigger).unwrap()["accent"], false);
        assert_eq!(serde_json::to_value(note).unwrap()["accent"], false);
        assert!(
            serde_json::to_value(trigger)
                .unwrap()
                .get("microtiming")
                .is_none()
        );
        let mut shifted = trigger;
        *shifted.microtiming_mut().unwrap() = Microtiming::new(-25).unwrap();
        assert_eq!(serde_json::to_value(shifted).unwrap()["microtiming"], -25);
        assert!(serde_json::from_str::<Microtiming>("51").is_err());
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

        project.tracks[SYNTH_TRACK_START].input_accent = true;
        let value = serde_json::to_value(&project).unwrap();
        assert_eq!(value["tracks"][SYNTH_TRACK_START]["input_accent"], true);
        assert_eq!(serde_json::from_value::<Project>(value).unwrap(), project);
    }
    #[test]
    fn lock_compatibility_matches_track_kind() {
        let mut drum = Project::new();
        drum.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: ParameterLocks::from_pairs([(
                ParameterId::Cutoff,
                ParameterValue::Percent(Percent::new(50).unwrap()),
            )]),
        });
        assert!(matches!(
            drum.validate(),
            Err(ValidationError::Lock(0, 0, "cutoff"))
        ));

        let mut synth = Project::new();
        synth.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: ParameterLocks::from_pairs([(
                ParameterId::Tone,
                ParameterValue::Percent(Percent::new(50).unwrap()),
            )]),
        });
        assert!(matches!(
            synth.validate(),
            Err(ValidationError::Lock(CHORD_TRACK_INDEX, 0, "tone"))
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
            } else if parameter == ParameterId::LeadSubMode {
                ParameterValue::LeadSubMode(LeadSubMode::TwoOctaveSquare)
            } else if parameter == ParameterId::FmAlgorithm {
                ParameterValue::FmAlgorithm(FmAlgorithm::Pairs)
            } else if matches!(parameter.value_kind(), ParameterValueKind::FmRatio) {
                ParameterValue::FmRatio(FmRatio::Four)
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
            for scale in Scale::ALL {
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
        let default = LfoConfig::default();
        assert!(!default.reset_on_trigger);
        assert_eq!(default.start_phase, Percent::ZERO);
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
        project.tracks[0]
            .lfos
            .set(ParameterId::Cutoff, Some(LfoConfig::default()));
        assert_eq!(project.validate(), Err(ValidationError::Lfo(0, "cutoff")));
        project.tracks[0].lfos.set(ParameterId::Cutoff, None);
        project.tracks[SYNTH_TRACK_START]
            .lfos
            .set(ParameterId::Tone, Some(LfoConfig::default()));
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lfo(SYNTH_TRACK_START, "tone"))
        );
        project.tracks[SYNTH_TRACK_START]
            .lfos
            .set(ParameterId::Tone, None);
        project.tracks[CHORD_TRACK_INDEX]
            .lfos
            .set(ParameterId::Release, Some(LfoConfig::default()));
        project.validate().unwrap();
        assert!(ParameterId::Pitch.supports_lfo(TrackKind::Chord));
        assert!(ParameterId::Pitch.supports_lfo(TrackKind::Lead));
        assert!(!ParameterId::Pitch.supports_lfo(TrackKind::Bass));
        assert!(!ParameterId::Noise.supports_lfo(TrackKind::Chord));
        assert!(!ParameterId::Noise.supports_lfo(TrackKind::Lead));
        assert!(!ParameterId::KeyboardTracking.supports_lfo(TrackKind::Lead));
        project.tracks[CHORD_TRACK_INDEX]
            .lfos
            .set(ParameterId::Noise, Some(LfoConfig::default()));
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lfo(CHORD_TRACK_INDEX, "noise"))
        );
        project.tracks[CHORD_TRACK_INDEX]
            .lfos
            .set(ParameterId::Noise, None);
        project.tracks[LEAD_TRACK_INDEX]
            .lfos
            .set(ParameterId::KeyboardTracking, Some(LfoConfig::default()));
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lfo(LEAD_TRACK_INDEX, "keyboard_tracking"))
        );
        project.tracks[LEAD_TRACK_INDEX]
            .lfos
            .set(ParameterId::KeyboardTracking, None);
        assert!(!ParameterId::Pitch.is_valid_for(TrackKind::Chord));
        assert!(!project.tracks[CHORD_TRACK_INDEX].set_parameter(
            ParameterId::Pitch,
            ParameterValue::Percent(Percent::new(50).unwrap())
        ));
        assert!(!ParameterLocks::default().set(
            ParameterId::Pitch,
            ParameterValue::Percent(Percent::new(50).unwrap())
        ));
        project.tracks[CHORD_TRACK_INDEX]
            .lfos
            .set(ParameterId::Pitch, Some(LfoConfig::default()));
        project.validate().unwrap();
        project.tracks[SYNTH_TRACK_START]
            .lfos
            .set(ParameterId::Pitch, Some(LfoConfig::default()));
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lfo(SYNTH_TRACK_START, "pitch"))
        );
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
        let mut locks = ParameterLocks::from_pairs([(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(20).unwrap()),
        )]);
        let overlay = ParameterLocks::from_pairs([(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(30).unwrap()),
        )]);
        locks.overlay(overlay);
        assert_eq!(locks.percent(ParameterId::Cutoff), Percent::new(30));

        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks,
        });
        assert_eq!(
            project.validate(),
            Err(ValidationError::Lock(0, 0, "cutoff"))
        );
    }

    #[test]
    fn indexed_parameter_storage_preserves_strict_json_schema() {
        let locks = ParameterLocks::from_pairs([
            (ParameterId::Cutoff, ParameterValue::Percent(p(42))),
            (
                ParameterId::Waveform,
                ParameterValue::Waveform(Waveform::Saw),
            ),
            (ParameterId::Chorus, ParameterValue::Chorus(ChorusMode::Ii)),
            (
                ParameterId::Spread,
                ParameterValue::Spread(ChordSpread::Wide),
            ),
            (
                ParameterId::LeadSubMode,
                ParameterValue::LeadSubMode(LeadSubMode::TwoOctaveSquare),
            ),
        ]);
        let json = serde_json::to_string(&locks).unwrap();
        assert_eq!(
            serde_json::from_str::<ParameterLocks>(&json).unwrap(),
            locks
        );
        assert!(serde_json::from_str::<ParameterLocks>(r#"{"cutoff":20,"cutoff":30}"#).is_err());
        assert!(serde_json::from_str::<ParameterLocks>(r#"{"unknown":20}"#).is_err());

        let lfo = serde_json::to_string(&LfoAssignments::default()).unwrap();
        assert_eq!(lfo, "{}");
        assert!(
            serde_json::from_str::<LfoAssignments>(r#"{"cutoff":null,"cutoff":null}"#).is_err()
        );
    }

    #[test]
    fn drum_recipe_defaults_and_track_bounds_are_strict() {
        let mut project = Project::new();
        let Instrument::Hat(hat) = project.tracks[2].instrument else {
            panic!("expected hat")
        };
        assert_eq!(hat.decay, p(15));
        assert_eq!(hat.open.decay, p(85));
        let Instrument::Tom(tom) = project.tracks[3].instrument else {
            panic!("expected tom")
        };
        assert_eq!((tom.tune, tom.tone, tom.decay), (p(15), p(35), p(60)));
        assert_eq!(tom.medium.tune, p(50));
        assert_eq!(tom.high.tune, p(85));

        project.patterns[0].tracks[2].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: DrumRecipeSlot::TWO,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: DrumRecipeSlot::THREE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.validate().unwrap();
        *project.patterns[0].tracks[2].steps[0]
            .as_mut()
            .unwrap()
            .drum_recipe_mut()
            .unwrap() = DrumRecipeSlot::THREE;
        assert_eq!(project.validate(), Err(ValidationError::DrumRecipe(2, 0)));
    }

    #[test]
    fn fm_defaults_selectors_locks_and_event_shape_are_strict() {
        let mut project = Project::new();
        assert_eq!(project.tracks[FM_TRACK_INDEX].kind, TrackKind::Fm);
        let Instrument::Fm(fm) = project.tracks[FM_TRACK_INDEX].instrument else {
            panic!("expected FM")
        };
        assert_eq!(fm.algorithm, FmAlgorithm::Cascade);
        assert_eq!(
            fm.operators,
            [
                FmOperator {
                    ratio: FmRatio::One,
                    level: p(100),
                    feedback: p(0)
                },
                FmOperator {
                    ratio: FmRatio::Two,
                    level: p(35),
                    feedback: p(8)
                },
                FmOperator {
                    ratio: FmRatio::One,
                    level: p(0),
                    feedback: p(0)
                },
                FmOperator {
                    ratio: FmRatio::One,
                    level: p(0),
                    feedback: p(0)
                },
            ]
        );
        assert_eq!(fm.brightness, p(72));
        assert_eq!(
            (fm.attack, fm.decay, fm.sustain, fm.release),
            (p(0), p(55), p(55), p(40))
        );
        assert_eq!(project.tracks[FM_TRACK_INDEX].reverb_send, p(20));
        assert_eq!(serde_json::to_string(&FmRatio::Half).unwrap(), "\"0.5\"");
        assert_eq!(
            serde_json::to_string(&FmRatio::OneAndHalf).unwrap(),
            "\"1.5\""
        );
        assert!(ParameterId::FmOp2Level.supports_lfo(TrackKind::Fm));
        assert!(ParameterId::FmOp2Feedback.supports_lfo(TrackKind::Fm));
        assert!(ParameterId::Pitch.supports_lfo(TrackKind::Fm));
        assert!(!ParameterId::FmOp2Ratio.supports_lfo(TrackKind::Fm));
        assert!(!ParameterId::FmAlgorithm.supports_lfo(TrackKind::Fm));

        project.patterns[0].tracks[FM_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: true,
            chord_shape: Some(ChordShape::Single),
            arpeggio: Default::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: Default::default(),
        });
        assert_eq!(
            project.validate(),
            Err(ValidationError::EventKind(FM_TRACK_INDEX, 0))
        );
    }

    #[test]
    fn fm_algorithms_have_stable_acyclic_routes_and_carriers() {
        let expected_carriers = [1, 2, 1, 2, 1, 3, 3, 4];
        for (algorithm, expected) in FmAlgorithm::ALL.into_iter().zip(expected_carriers) {
            assert_eq!(
                (0..4)
                    .filter(|operator| algorithm.is_carrier(*operator))
                    .count(),
                expected
            );
            for source in 0..4 {
                for target in 0..4 {
                    if algorithm.routes(source, target) {
                        assert!(source > target, "{} contains a cyclic route", algorithm);
                        assert!(!algorithm.is_carrier(source));
                    }
                }
            }
            assert!(!algorithm.diagram().is_empty());
        }
    }
}
