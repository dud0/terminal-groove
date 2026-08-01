use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub const STEP_COUNT: usize = 16;
pub const TRACK_COUNT: usize = 6;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError {
    #[error("unsupported format_version {0}")]
    Version(u32),
    #[error("tracks: expected exactly six tracks")]
    TrackCount,
    #[error("tracks[{0}]: expected {1}")]
    TrackOrder(usize, &'static str),
    #[error("tracks[{0}].steps: expected exactly 16 steps")]
    StepCount(usize),
    #[error("tracks[{0}].steps[{1}]: event is incompatible with track")]
    EventKind(usize, usize),
    #[error("tracks[{0}].steps[{1}].locks: incompatible lock `{2}`")]
    Lock(usize, usize, &'static str),
    #[error("tracks[{0}].steps[{1}]: tie has no source note")]
    Tie(usize, usize),
    #[error("tracks[{0}]: a pattern containing only ties is invalid")]
    AllTies(usize),
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
    pub key: PitchClass,
    pub scale: Scale,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalParameterId {
    Tempo,
    DelayDivision,
    DelayFeedback,
    ReverbTime,
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
    Synth,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrumParameters {
    pub tone: Percent,
    pub decay: Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynthParameters {
    pub waveform: Waveform,
    pub cutoff: Percent,
    pub resonance: Percent,
    pub filter_envelope: Percent,
    pub attack: Percent,
    pub decay: Percent,
    pub sustain: Percent,
    pub release: Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Instrument {
    Drum(DrumParameters),
    Synth(SynthParameters),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterLocks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay_send: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverb_send: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay: Option<Percent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Waveform>,
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
impl ParameterLocks {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepEvent {
    Trigger {
        locks: ParameterLocks,
    },
    Note {
        degree: u8,
        octave: u8,
        locks: ParameterLocks,
    },
    Tie {
        locks: ParameterLocks,
    },
}
impl StepEvent {
    pub fn locks(&self) -> &ParameterLocks {
        match self {
            Self::Trigger { locks } | Self::Note { locks, .. } | Self::Tie { locks } => locks,
        }
    }
    pub fn locks_mut(&mut self) -> &mut ParameterLocks {
        match self {
            Self::Trigger { locks } | Self::Note { locks, .. } | Self::Tie { locks } => locks,
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
    pub muted: bool,
    pub delay_send: Percent,
    pub reverb_send: Percent,
    pub instrument: Instrument,
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_degree: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_octave: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectV1 {
    pub format_version: u32,
    pub globals: Globals,
    pub tracks: Vec<Track>,
}

fn p(n: u8) -> Percent {
    Percent(n)
}
impl ProjectV1 {
    pub fn new() -> Self {
        let drum = |kind: TrackKind, name: &str, tone: u8, decay: u8| Track {
            kind,
            name: name.into(),
            level: p(80),
            muted: false,
            delay_send: p(0),
            reverb_send: p(0),
            instrument: Instrument::Drum(DrumParameters {
                tone: p(tone),
                decay: p(decay),
            }),
            steps: vec![None; STEP_COUNT],
            input_degree: None,
            input_octave: None,
        };
        let synth = |name: &str| Track {
            kind: TrackKind::Synth,
            name: name.into(),
            level: p(80),
            muted: false,
            delay_send: p(0),
            reverb_send: p(0),
            instrument: Instrument::Synth(SynthParameters {
                waveform: Waveform::Saw,
                cutoff: p(65),
                resonance: p(10),
                filter_envelope: p(25),
                attack: p(0),
                decay: p(25),
                sustain: p(70),
                release: p(15),
            }),
            steps: vec![None; STEP_COUNT],
            input_degree: Some(1),
            input_octave: Some(3),
        };
        Self {
            format_version: 1,
            globals: Globals::default(),
            tracks: vec![
                drum(TrackKind::Kick, "Kick", 50, 35),
                drum(TrackKind::Snare, "Snare", 50, 35),
                drum(TrackKind::Hat, "Hi-hat", 60, 20),
                synth("Synth 1"),
                synth("Synth 2"),
                synth("Synth 3"),
            ],
        }
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.format_version != 1 {
            return Err(ValidationError::Version(self.format_version));
        }
        if self.tracks.len() != TRACK_COUNT {
            return Err(ValidationError::TrackCount);
        }
        let expected = [
            (TrackKind::Kick, "Kick"),
            (TrackKind::Snare, "Snare"),
            (TrackKind::Hat, "Hi-hat"),
            (TrackKind::Synth, "Synth 1"),
            (TrackKind::Synth, "Synth 2"),
            (TrackKind::Synth, "Synth 3"),
        ];
        if !(40..=240).contains(&self.globals.tempo_bpm)
            || !self.globals.reverb_time_seconds.is_finite()
            || !(0.2..=10.0).contains(&self.globals.reverb_time_seconds)
            || self.globals.delay_feedback.get() > 95
        {
            return Err(ValidationError::TrackOrder(0, "valid globals"));
        }
        for (ti, t) in self.tracks.iter().enumerate() {
            if t.kind != expected[ti].0 || t.name != expected[ti].1 {
                return Err(ValidationError::TrackOrder(ti, expected[ti].1));
            }
            if t.steps.len() != STEP_COUNT {
                return Err(ValidationError::StepCount(ti));
            }
            let synth = t.kind == TrackKind::Synth;
            if synth != matches!(t.instrument, Instrument::Synth(_))
                || synth != t.input_degree.is_some()
                || synth != t.input_octave.is_some()
            {
                return Err(ValidationError::TrackOrder(ti, expected[ti].1));
            }
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
            for (si, event) in t.steps.iter().enumerate() {
                if let Some(event) = event {
                    let event_ok = matches!(
                        (synth, event),
                        (false, StepEvent::Trigger { .. })
                            | (true, StepEvent::Note { .. })
                            | (true, StepEvent::Tie { .. })
                    );
                    if !event_ok {
                        return Err(ValidationError::EventKind(ti, si));
                    }
                    if let StepEvent::Note { degree, octave, .. } = event {
                        if !(1..=8).contains(degree) || *octave > 7 {
                            return Err(ValidationError::EventKind(ti, si));
                        }
                    }
                    validate_locks(ti, si, synth, event.locks())?;
                }
            }
            if synth {
                validate_ties(ti, &t.steps)?;
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
}
impl Default for ProjectV1 {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_locks(
    ti: usize,
    si: usize,
    synth: bool,
    l: &ParameterLocks,
) -> Result<(), ValidationError> {
    let bad = if synth {
        if l.tone.is_some() { Some("tone") } else { None }
    } else if l.waveform.is_some() {
        Some("waveform")
    } else if l.cutoff.is_some() {
        Some("cutoff")
    } else if l.resonance.is_some() {
        Some("resonance")
    } else if l.filter_envelope.is_some() {
        Some("filter_envelope")
    } else if l.attack.is_some() {
        Some("attack")
    } else if l.sustain.is_some() {
        Some("sustain")
    } else if l.release.is_some() {
        Some("release")
    } else {
        None
    };
    bad.map_or(Ok(()), |name| Err(ValidationError::Lock(ti, si, name)))
}
pub fn tie_source(steps: &[Step], at: usize) -> Option<usize> {
    if steps.len() != STEP_COUNT {
        return None;
    }
    let mut i = (at + STEP_COUNT - 1) % STEP_COUNT;
    for _ in 0..STEP_COUNT {
        match &steps[i] {
            Some(StepEvent::Note { .. }) => return Some(i),
            Some(StepEvent::Tie { .. }) => i = (i + STEP_COUNT - 1) % STEP_COUNT,
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
    if ties == STEP_COUNT {
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
    DelaySend,
    ReverbSend,
    Tone,
    Decay,
    Waveform,
    Cutoff,
    Resonance,
    FilterEnvelope,
    Attack,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterValue {
    Percent(Percent),
    Waveform(Waveform),
}

impl ParameterId {
    pub const ALL: [Self; 12] = [
        Self::Level,
        Self::DelaySend,
        Self::ReverbSend,
        Self::Tone,
        Self::Decay,
        Self::Waveform,
        Self::Cutoff,
        Self::Resonance,
        Self::FilterEnvelope,
        Self::Attack,
        Self::Sustain,
        Self::Release,
    ];

    pub const fn is_valid_for(self, kind: TrackKind) -> bool {
        match self {
            Self::Level | Self::DelaySend | Self::ReverbSend | Self::Decay => true,
            Self::Tone => !matches!(kind, TrackKind::Synth),
            Self::Waveform
            | Self::Cutoff
            | Self::Resonance
            | Self::FilterEnvelope
            | Self::Attack
            | Self::Sustain
            | Self::Release => matches!(kind, TrackKind::Synth),
        }
    }

    pub const fn is_waveform(self) -> bool {
        matches!(self, Self::Waveform)
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
        ProjectV1::new().validate().unwrap();
    }
    #[test]
    fn scale_and_frequency() {
        let mut p = ProjectV1::new();
        assert_eq!(p.note_midi(8, 3), Some(60));
        p.globals.key = PitchClass::A;
        assert!((p.note_frequency(1, 4).unwrap() - 440.0).abs() < 0.001);
        p.globals.scale = Scale::NaturalMinor;
        assert_eq!(p.note_midi(3, 3), Some(60));
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
            locks: Default::default(),
        });
        s[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        assert_eq!(tie_source(&s, 0), Some(15));
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
    fn lock_compatibility_matches_track_kind() {
        let mut drum = ProjectV1::new();
        drum.tracks[0].steps[0] = Some(StepEvent::Trigger {
            locks: ParameterLocks {
                cutoff: Percent::new(50),
                ..Default::default()
            },
        });
        assert!(matches!(
            drum.validate(),
            Err(ValidationError::Lock(0, 0, "cutoff"))
        ));

        let mut synth = ProjectV1::new();
        synth.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            locks: ParameterLocks {
                tone: Percent::new(50),
                ..Default::default()
            },
        });
        assert!(matches!(
            synth.validate(),
            Err(ValidationError::Lock(3, 0, "tone"))
        ));
    }
    #[test]
    fn all_keys_map_degrees() {
        for key in PitchClass::ALL {
            for scale in [Scale::Major, Scale::NaturalMinor] {
                let mut p = ProjectV1::new();
                p.globals.key = key;
                p.globals.scale = scale;
                for degree in 1..=8 {
                    assert!(p.note_frequency(degree, 3).unwrap().is_finite())
                }
                assert_eq!(p.note_midi(8, 3).unwrap() - p.note_midi(1, 3).unwrap(), 12)
            }
        }
    }
}
