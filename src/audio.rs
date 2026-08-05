use crate::{
    dsp::{
        Adsr, Biquad, DcBlock, Delay, EnvStage, EnvelopeProfile, LadderFilter, Lfo, MasterLimiter,
        PolyBlepOsc, Reverb, Smoother, StereoChorus, equal_power_pan, exp_map_f32,
    },
    engine::{GateAction, StepClock, synth_action},
    model::{
        ArpeggioConfig, ArpeggioRate, ArpeggioType, ChordShape, ChorusMode, Globals, Instrument,
        LfoAssignments, MAX_STEP_COUNT, ParameterId, ParameterLocks, Percent, Project, StepEvent,
        TRACK_COUNT, Waveform,
    },
};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    f32::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

/// Deterministic old-pattern-index to new-pattern-index transform. `u8::MAX`
/// represents a removed index and is resolved to the nearest valid index by
/// the receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternIndexMap {
    forward: [u8; 100],
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
    fn rebase(self, index: usize, next_count: usize) -> usize {
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

#[derive(Clone, Copy, Debug)]
struct AudioTrack {
    level: Percent,
    pan: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    instrument: Instrument,
    lfos: LfoAssignments,
    input_degree: u8,
    input_octave: u8,
    input_chord_shape: ChordShape,
    input_chord_arpeggio: ArpeggioConfig,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct AudioSequence {
    steps: [Option<StepEvent>; MAX_STEP_COUNT],
    step_count: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct AudioPattern {
    tracks: [AudioSequence; TRACK_COUNT],
}
#[derive(Clone, Debug)]
pub struct AudioProject {
    globals: Globals,
    tracks: [AudioTrack; TRACK_COUNT],
    patterns: Box<[AudioPattern]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterSmoothing {
    Default,
    Fader,
}
impl ParameterSmoothing {
    fn samples(self, sample_rate: f32) -> u32 {
        let seconds = match self {
            Self::Default => 0.005,
            Self::Fader => 0.030,
        };
        (sample_rate * seconds).round().max(1.0) as u32
    }
}
impl AudioProject {
    pub fn from_project(project: &Project) -> Self {
        Self {
            globals: project.globals,
            tracks: std::array::from_fn(|i| {
                let t = &project.tracks[i];
                AudioTrack {
                    level: t.level,
                    pan: t.pan,
                    muted: t.muted,
                    delay_send: t.delay_send,
                    reverb_send: t.reverb_send,
                    instrument: t.instrument,
                    lfos: t.lfos,
                    input_degree: t.input_degree.unwrap_or(1),
                    input_octave: t.input_octave.unwrap_or(3),
                    input_chord_shape: t.input_chord_shape.unwrap_or_default(),
                    input_chord_arpeggio: t.input_chord_arpeggio.unwrap_or_default(),
                }
            }),
            patterns: project
                .patterns
                .iter()
                .map(|source| AudioPattern {
                    tracks: std::array::from_fn(|track| {
                        let steps = &source.tracks[track].steps;
                        AudioSequence {
                            steps: std::array::from_fn(|s| steps.get(s).copied().flatten()),
                            step_count: steps.len() as u8,
                        }
                    }),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AudioCommand {
    PlayPause,
    Stop,
    SelectPattern {
        pattern: u8,
    },
    ReplaceProject {
        project: Box<AudioProject>,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    },
    Audition {
        track: u8,
        step: u8,
    },
}
pub struct AudioStatus {
    pub running: AtomicBool,
    pub paused: AtomicBool,
    pub playheads: [AtomicU8; TRACK_COUNT],
    pub failed: AtomicBool,
    pub non_finite: AtomicBool,
    pub active_pattern: AtomicU8,
    pub queued_pattern: AtomicU8,
}
impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            playheads: std::array::from_fn(|_| AtomicU8::new(u8::MAX)),
            failed: AtomicBool::new(false),
            non_finite: AtomicBool::new(false),
            active_pattern: AtomicU8::new(0),
            queued_pattern: AtomicU8::new(u8::MAX),
        }
    }
}
pub struct Audio {
    pub stream: Stream,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    producer: Producer<AudioCommand>,
    retired: Consumer<Box<AudioProject>>,
}
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("audio command queue full")]
pub struct QueueFull;
impl Audio {
    pub fn send(&mut self, command: AudioCommand) -> Result<(), QueueFull> {
        self.reap_retired();
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| QueueFull)
    }
    pub fn available_commands(&self) -> usize {
        self.producer.slots()
    }
    /// Destroy snapshots on the UI thread, never from the audio callback.
    pub fn reap_retired(&mut self) {
        while let Ok(snapshot) = self.retired.pop() {
            drop(snapshot);
        }
    }
    pub fn snapshot(project: &Project) -> AudioCommand {
        Self::snapshot_for_pattern(project, 0)
    }
    pub fn snapshot_for_pattern(project: &Project, active_pattern: usize) -> AudioCommand {
        Self::snapshot_with_smoothing_for_pattern(
            project,
            active_pattern,
            ParameterSmoothing::Default,
        )
    }
    pub fn snapshot_with_smoothing(
        project: &Project,
        smoothing: ParameterSmoothing,
    ) -> AudioCommand {
        Self::snapshot_with_smoothing_for_pattern(project, 0, smoothing)
    }
    pub fn snapshot_with_smoothing_for_pattern(
        project: &Project,
        _active_pattern: usize,
        smoothing: ParameterSmoothing,
    ) -> AudioCommand {
        Self::snapshot_with_smoothing_and_map(project, smoothing, PatternIndexMap::identity())
    }
    pub fn snapshot_with_smoothing_and_map(
        project: &Project,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    ) -> AudioCommand {
        AudioCommand::ReplaceProject {
            project: Box::new(AudioProject::from_project(project)),
            smoothing,
            pattern_map,
        }
    }
}

#[allow(deprecated)]
pub fn output_device_names() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut names = host
        .output_devices()
        .context("could not enumerate audio outputs")?
        .filter_map(|d| d.name().ok())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}
#[allow(deprecated)]
fn choose_device(requested: Option<&str>) -> Result<Device> {
    let host = cpal::default_host();
    if let Some(name) = requested {
        let devices = host
            .output_devices()
            .context("could not enumerate audio outputs")?
            .collect::<Vec<_>>();
        let candidates = devices
            .iter()
            .filter_map(|d| d.name().ok())
            .collect::<Vec<_>>()
            .join(", ");
        let matches = devices
            .into_iter()
            .filter(|d| d.name().ok().as_deref() == Some(name))
            .collect::<Vec<_>>();
        match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => bail!(
                "audio output `{name}` was not found; available outputs: {candidates}; run `terminal-groove --list-audio-devices`"
            ),
            n => bail!(
                "audio output name `{name}` is ambiguous ({n} matches); available outputs: {candidates}; use an exact unique name from `terminal-groove --list-audio-devices`"
            ),
        }
    } else {
        host.default_output_device()
            .context("no default audio output; run `terminal-groove --list-audio-devices`")
    }
}

#[allow(deprecated)]
pub fn open(requested: Option<&str>, project: &Project) -> Result<Audio> {
    let device = choose_device(requested)?;
    let name = device.name().unwrap_or_else(|_| "unknown".into());
    let supported = device
        .default_output_config()
        .with_context(|| format!("could not query audio output `{name}`"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = config.channels as usize;
    let sr = config.sample_rate;
    let status = Arc::new(AudioStatus::default());
    let (producer, consumer) = RingBuffer::new(256);
    let (retire_producer, retired) = RingBuffer::new(32);
    let initial = AudioProject::from_project(project);
    let stream = match format {
        SampleFormat::F32 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new_with_retirement(Box::new(initial),sr,rs,retire_producer); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[f32],_|render(out,channels,&mut r,&mut c,|x|x),move |_|mark_failed(&s),None) }
        SampleFormat::I16 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new_with_retirement(Box::new(initial),sr,rs,retire_producer); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[i16],_|render(out,channels,&mut r,&mut c,|x|(x.clamp(-1.,1.)*32767.) as i16),move |_|mark_failed(&s),None) }
        SampleFormat::U16 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new_with_retirement(Box::new(initial),sr,rs,retire_producer); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[u16],_|render(out,channels,&mut r,&mut c,|x|((x.clamp(-1.,1.)*0.5+0.5)*65535.) as u16),move |_|mark_failed(&s),None) }
        other => bail!("audio output `{name}` uses unsupported sample format {other:?}; supported: f32, i16, u16"),
    }.with_context(|| format!("could not build stream for audio output `{name}`"))?;
    stream
        .play()
        .with_context(|| format!("could not start audio output `{name}`"))?;
    Ok(Audio {
        stream,
        device_name: name,
        status,
        producer,
        retired,
    })
}

fn mark_failed(status: &AudioStatus) {
    status.failed.store(true, Ordering::Release);
    status.running.store(false, Ordering::Release);
    status.paused.store(false, Ordering::Release);
    for playhead in &status.playheads {
        playhead.store(u8::MAX, Ordering::Release);
    }
}

struct SynthVoice {
    osc: PolyBlepOsc,
    sub_osc: PolyBlepOsc,
    env: Adsr,
    bass_filter: LadderFilter,
    roland_filter: LadderFilter,
    freq: Smoother,
    wave: Waveform,
    oscillator_mix: Smoother,
    pulse_width: Smoother,
    sub_oscillator: Smoother,
    cutoff_percent: Smoother,
    resonance_percent: Smoother,
    filter_env_percent: Smoother,
    locks: ParameterLocks,
    active: bool,
    remaining: u32,
    accent_gain: Smoother,
    accent_filter: Smoother,
    slide_armed: bool,
    bass: bool,
    chord: bool,
    level: Smoother,
    delay_send: Smoother,
    reverb_send: Smoother,
    pan: Smoother,
}

#[derive(Clone, Copy)]
struct SynthTrigger {
    degree: u8,
    octave: u8,
    accent: bool,
    slide: bool,
    chord_shape: Option<ChordShape>,
    arpeggio: ArpeggioConfig,
}

const CHORD_GROUP_SIZE: usize = 4;

#[derive(Clone, Copy, Debug)]
struct ArpeggioState {
    enabled: bool,
    kind: ArpeggioType,
    rate: ArpeggioRate,
    shape: ChordShape,
    order: [u8; 8],
    order_len: u8,
    position: u8,
    phase: f64,
    random: u32,
}

impl Default for ArpeggioState {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: ArpeggioType::default(),
            rate: ArpeggioRate::default(),
            shape: ChordShape::default(),
            order: [0; 8],
            order_len: 0,
            position: 0,
            phase: 0.0,
            random: 0x6d2b_79f5,
        }
    }
}

impl ArpeggioState {
    fn reset(
        &mut self,
        shape: ChordShape,
        kind: ArpeggioType,
        rate: ArpeggioRate,
        sr: f32,
        bpm: u16,
    ) {
        self.enabled = true;
        self.kind = kind;
        self.rate = rate;
        self.shape = shape;
        self.position = 0;
        self.phase = sr as f64 * 60.0 / bpm as f64 * rate.beats();
        self.rebuild_order();
    }

    fn rebuild_order(&mut self) {
        let n = self.shape.degrees().len().min(4);
        self.order_len = match self.kind {
            ArpeggioType::Up | ArpeggioType::Down | ArpeggioType::Random => n as u8,
            ArpeggioType::UpDown | ArpeggioType::DownUp => {
                if n > 1 {
                    (n * 2 - 2) as u8
                } else {
                    n as u8
                }
            }
        };
        let length = self.order_len as usize;
        match self.kind {
            ArpeggioType::Up | ArpeggioType::Random => {
                for (i, slot) in self.order[..length].iter_mut().enumerate() {
                    *slot = i as u8;
                }
            }
            ArpeggioType::Down => {
                for (i, slot) in self.order[..length].iter_mut().enumerate() {
                    *slot = (n - i - 1) as u8;
                }
            }
            ArpeggioType::UpDown => {
                for i in 0..length {
                    self.order[i] = if i < n { i } else { length - i } as u8;
                }
            }
            ArpeggioType::DownUp => {
                for i in 0..length {
                    self.order[i] = if i < n { n - i - 1 } else { i - n + 1 } as u8;
                }
            }
        }
        if self.kind == ArpeggioType::Random {
            self.shuffle();
        }
    }

    fn shuffle(&mut self) {
        for i in (1..self.order_len as usize).rev() {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            let j = (self.random as usize) % (i + 1);
            self.order.swap(i, j);
        }
    }

    fn current_voice(&self) -> usize {
        self.order[self.position as usize] as usize
    }

    fn tick(&mut self, sr: f32, bpm: u16) -> bool {
        if !self.enabled {
            return false;
        }
        self.phase -= 1.0;
        if self.phase <= 0.0 {
            self.phase += sr as f64 * 60.0 / bpm as f64 * self.rate.beats();
            self.position += 1;
            if self.position >= self.order_len {
                self.position = 0;
                if self.kind == ArpeggioType::Random {
                    self.shuffle();
                }
            }
            true
        } else {
            false
        }
    }
}

struct ChordVoicePool {
    voices: [SynthVoice; CHORD_GROUP_SIZE * 2],
    group: usize,
    voice_count: usize,
    active: bool,
    chorus: StereoChorus,
    arpeggiated: bool,
    arpeggio: ArpeggioState,
    arpeggio_trigger: SynthTrigger,
    arpeggio_locks: ParameterLocks,
    preview_remaining: u64,
}

impl ChordVoicePool {
    fn new(sample_rate: u32) -> Self {
        Self {
            voices: std::array::from_fn(|_| SynthVoice::new(sample_rate as f32)),
            group: 1,
            voice_count: 0,
            active: false,
            chorus: StereoChorus::new(sample_rate),
            arpeggiated: false,
            arpeggio: ArpeggioState::default(),
            arpeggio_trigger: SynthTrigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
            },
            arpeggio_locks: ParameterLocks::default(),
            preview_remaining: 0,
        }
    }
}

const DRUM_SILENCE: f32 = 0.0001;
const REVERB_RETURN_GAIN: f32 = 0.5;

struct DrumEnvelope {
    value: f32,
    start: f32,
    peak: f32,
    attack_samples: u32,
    decay_samples: u32,
    elapsed: u32,
}
impl DrumEnvelope {
    fn new() -> Self {
        Self {
            value: DRUM_SILENCE,
            start: DRUM_SILENCE,
            peak: DRUM_SILENCE,
            attack_samples: 1,
            decay_samples: 1,
            elapsed: 1,
        }
    }
    fn trigger(&mut self, peak: f32, attack: f32, decay: f32, sr: f32) {
        self.start = self.value.max(DRUM_SILENCE);
        self.peak = peak;
        self.attack_samples = (attack * sr).round().max(1.0) as u32;
        self.decay_samples = (decay * sr).round().max(self.attack_samples as f32 + 1.0) as u32;
        self.elapsed = 0;
    }
    fn next_value(&mut self) -> f32 {
        if self.elapsed < self.attack_samples {
            let t = self.elapsed as f32 / self.attack_samples as f32;
            self.value = self.start * (self.peak / self.start).powf(t);
        } else if self.elapsed < self.decay_samples {
            let t = (self.elapsed - self.attack_samples) as f32
                / (self.decay_samples - self.attack_samples) as f32;
            self.value = self.peak * (DRUM_SILENCE / self.peak).powf(t);
        } else {
            self.value = DRUM_SILENCE;
        }
        self.elapsed = self.elapsed.saturating_add(1);
        self.value
    }
}

struct KickPitchEnvelope {
    value: f32,
    start: f32,
    peak: f32,
    settled: f32,
    rise_samples: u32,
    fall_samples: u32,
    elapsed: u32,
}
impl KickPitchEnvelope {
    fn new() -> Self {
        Self {
            value: 75.0,
            start: 75.0,
            peak: 75.0,
            settled: 48.0,
            rise_samples: 1,
            fall_samples: 1,
            elapsed: 1,
        }
    }
    fn trigger(&mut self, tone: f32, decay: f32, sr: f32) {
        self.start = self.value.max(20.0);
        self.peak = 110.0 + tone * 170.0;
        self.settled = 45.0 + tone * 25.0;
        self.rise_samples = (0.0015 * sr).round().max(1.0) as u32;
        self.fall_samples = (decay.min(0.13) * sr)
            .round()
            .max(self.rise_samples as f32 + 1.0) as u32;
        self.elapsed = 0;
    }
    fn next_value(&mut self) -> f32 {
        if self.elapsed < self.rise_samples {
            let t = self.elapsed as f32 / self.rise_samples as f32;
            self.value = self.start + (self.peak - self.start) * t;
        } else if self.elapsed < self.fall_samples {
            let t = (self.elapsed - self.rise_samples) as f32
                / (self.fall_samples - self.rise_samples) as f32;
            self.value = self.peak * (self.settled / self.peak).powf(t);
        } else {
            self.value = self.settled;
        }
        self.elapsed = self.elapsed.saturating_add(1);
        self.value
    }
}

struct DrumVoice {
    envelope: DrumEnvelope,
    kick_pitch: KickPitchEnvelope,
    phase: f32,
    phase2: f32,
    metallic: [PolyBlepOsc; 6],
    filter: Biquad,
    filter2: Biquad,
    noise: u32,
    tune: f32,
    tone: f32,
    snappy: f32,
    attack: f32,
    accent: bool,
    level: Smoother,
    delay_send: Smoother,
    reverb_send: Smoother,
    pan: Smoother,
    locks: ParameterLocks,
}

#[derive(Clone, Copy)]
struct DrumControls {
    tune: f32,
    tone: f32,
    snappy: f32,
    decay: f32,
    attack: f32,
}

impl DrumVoice {
    fn new(seed: u32) -> Self {
        Self {
            envelope: DrumEnvelope::new(),
            kick_pitch: KickPitchEnvelope::new(),
            phase: 0.0,
            phase2: 0.0,
            metallic: std::array::from_fn(|_| PolyBlepOsc::default()),
            filter: Biquad::new(),
            filter2: Biquad::new(),
            noise: seed,
            tune: 0.5,
            tone: 0.5,
            snappy: 0.55,
            attack: 0.35,
            accent: false,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
            pan: Smoother::new(50.0),
            locks: ParameterLocks::default(),
        }
    }
    fn noise(&mut self) -> f32 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        x as i32 as f32 / i32::MAX as f32
    }
}
impl SynthVoice {
    fn new(sr: f32) -> Self {
        Self {
            osc: Default::default(),
            sub_osc: Default::default(),
            env: Adsr::new(sr),
            bass_filter: Default::default(),
            roland_filter: Default::default(),
            freq: Smoother::new(110.0),
            wave: Waveform::Saw,
            oscillator_mix: Smoother::new(70.0),
            pulse_width: Smoother::new(50.0),
            sub_oscillator: Smoother::new(0.0),
            cutoff_percent: Smoother::new(65.0),
            resonance_percent: Smoother::new(10.0),
            filter_env_percent: Smoother::new(25.0),
            locks: ParameterLocks::default(),
            active: false,
            remaining: 0,
            accent_gain: Smoother::new(1.0),
            accent_filter: Smoother::new(0.0),
            slide_armed: false,
            bass: false,
            chord: false,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
            pan: Smoother::new(50.0),
        }
    }
}

struct Renderer {
    project: Box<AudioProject>,
    retire: Producer<Box<AudioProject>>,
    pending: Option<(Box<AudioProject>, ParameterSmoothing, PatternIndexMap)>,
    clock: StepClock,
    next_steps: [usize; TRACK_COUNT],
    active_pattern: usize,
    queued_pattern: Option<usize>,
    playing: bool,
    sr: f32,
    status: Arc<AudioStatus>,
    drums: [DrumVoice; 3],
    preview_drums: [DrumVoice; 3],
    synth: [SynthVoice; 3],
    preview: [SynthVoice; 3],
    chord: ChordVoicePool,
    preview_chord: ChordVoicePool,
    delay: Delay,
    reverb: Reverb,
    dc: DcBlock,
    limiter: MasterLimiter,
    mute: [Smoother; TRACK_COUNT],
    lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
}
impl Renderer {
    fn new(project: AudioProject, sr: u32, status: Arc<AudioStatus>) -> Self {
        let (retire, _discarded) = RingBuffer::new(32);
        Self::new_with_retirement(Box::new(project), sr, status, retire)
    }

    fn new_with_retirement(
        project: Box<AudioProject>,
        sr: u32,
        status: Arc<AudioStatus>,
        retire: Producer<Box<AudioProject>>,
    ) -> Self {
        let tempo_bpm = project.globals.tempo_bpm;
        let muted: [bool; TRACK_COUNT] = std::array::from_fn(|i| project.tracks[i].muted);
        let mut r = Self {
            project,
            retire,
            pending: None,
            clock: StepClock::new(sr, tempo_bpm),
            next_steps: [0; TRACK_COUNT],
            active_pattern: 0,
            queued_pattern: None,
            playing: false,
            sr: sr as f32,
            status,
            drums: std::array::from_fn(|i| {
                DrumVoice::new([0x1234_abcd, 0x9137_2468, 0xdead_beef][i])
            }),
            preview_drums: std::array::from_fn(|i| {
                DrumVoice::new([0x4a31_27dd, 0xa187_4c29, 0x6d2b_f193][i])
            }),
            synth: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            preview: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            chord: ChordVoicePool::new(sr),
            preview_chord: ChordVoicePool::new(sr),
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            dc: Default::default(),
            limiter: MasterLimiter::new(sr),
            mute: std::array::from_fn(|i| Smoother::new((!muted[i]) as u8 as f32)),
            lfos: std::array::from_fn(|track| {
                std::array::from_fn(|parameter| {
                    Lfo::new(0x51f0_0001 ^ ((track as u32) << 12) ^ parameter as u32)
                })
            }),
            preview_lfos: std::array::from_fn(|track| {
                std::array::from_fn(|parameter| {
                    Lfo::new(0xa7d1_0001 ^ ((track as u32) << 12) ^ parameter as u32)
                })
            }),
            lfo_offsets: [[0.0; ParameterId::ALL.len()]; TRACK_COUNT],
            preview_lfo_offsets: [[0.0; ParameterId::ALL.len()]; TRACK_COUNT],
        };
        r.configure_effects(0);
        r
    }
    fn configure_effects(&mut self, smoothing_samples: u32) {
        self.clock.set_bpm(self.project.globals.tempo_bpm);
        self.delay.configure(
            self.project
                .globals
                .delay_division
                .samples(self.project.globals.tempo_bpm, self.sr as u32) as usize,
            self.project.globals.delay_feedback.normalized(),
        );
        self.reverb
            .set_time_smoothed(self.project.globals.reverb_time_seconds, smoothing_samples);
        self.reverb.set_tone_smoothed(
            self.project.globals.reverb_tone.normalized(),
            smoothing_samples,
        );
        self.reverb
            .set_pre_delay_smoothed(self.project.globals.reverb_pre_delay_ms, smoothing_samples);
    }
    fn update_mutes(&mut self, immediate: bool) {
        let smoothing = if immediate {
            0
        } else {
            (self.sr * 0.005) as u32
        };
        for (i, mute) in self.mute.iter_mut().enumerate() {
            mute.set((!self.project.tracks[i].muted) as u8 as f32, smoothing);
        }
    }
    fn command(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::PlayPause => {
                if !self.playing && self.status.paused.load(Ordering::Acquire) {
                    self.clock.restart_timing();
                }
                self.playing = !self.playing;
                self.status.running.store(self.playing, Ordering::Release);
                self.status.paused.store(!self.playing, Ordering::Release);
                if !self.playing {
                    for v in &mut self.synth {
                        v.env.gate_off();
                        v.active = false;
                    }
                    Self::release_chord(&mut self.chord);
                    self.chord.arpeggio = ArpeggioState::default();
                }
            }
            AudioCommand::Stop => {
                self.playing = false;
                self.clock.reset();
                self.next_steps = [0; TRACK_COUNT];
                for voice in self.drums.iter_mut().chain(self.preview_drums.iter_mut()) {
                    voice.envelope.value = DRUM_SILENCE;
                    voice.envelope.elapsed = voice.envelope.decay_samples;
                }
                for v in self.synth.iter_mut().chain(self.preview.iter_mut()) {
                    v.env.gate_off();
                    v.active = false;
                    v.remaining = 0;
                }
                for v in self
                    .chord
                    .voices
                    .iter_mut()
                    .chain(self.preview_chord.voices.iter_mut())
                {
                    v.env.gate_off();
                    v.active = false;
                    v.remaining = 0;
                }
                self.chord.active = false;
                self.preview_chord.active = false;
                self.chord.arpeggio = ArpeggioState::default();
                self.preview_chord.arpeggio = ArpeggioState::default();
                self.chord.chorus.clear();
                self.preview_chord.chorus.clear();
                self.delay.clear();
                self.reverb.clear();
                self.limiter.clear();
                self.reset_lfos();
                self.status.running.store(false, Ordering::Release);
                self.status.paused.store(false, Ordering::Release);
                for playhead in &self.status.playheads {
                    playhead.store(u8::MAX, Ordering::Release);
                }
                self.active_pattern = 0;
                self.queued_pattern = None;
                self.status.active_pattern.store(0, Ordering::Release);
                self.status.queued_pattern.store(u8::MAX, Ordering::Release);
            }
            AudioCommand::SelectPattern { pattern }
                if (pattern as usize) < self.project.patterns.len() =>
            {
                if self.playing {
                    self.queued_pattern = Some(pattern as usize);
                    self.status.queued_pattern.store(pattern, Ordering::Release);
                } else {
                    self.activate_pattern(pattern as usize);
                }
            }
            AudioCommand::SelectPattern { .. } => {}
            AudioCommand::ReplaceProject {
                project,
                smoothing,
                pattern_map,
            } => {
                self.replace_project(project, smoothing, pattern_map);
            }
            AudioCommand::Audition { track, step } => self.audition(track as usize, step as usize),
        }
    }

    fn replace_project(
        &mut self,
        project: Box<AudioProject>,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    ) {
        if self.retire.slots() == 0 {
            self.pending = Some((project, smoothing, pattern_map));
            return;
        }
        let active_pattern = pattern_map.rebase(self.active_pattern, project.patterns.len());
        let queued_pattern = self
            .queued_pattern
            .map(|index| pattern_map.rebase(index, project.patterns.len()));
        self.reconcile_lfos(&project);
        let old = std::mem::replace(&mut self.project, project);
        debug_assert!(self.retire.push(old).is_ok());
        self.active_pattern = active_pattern;
        self.queued_pattern = queued_pattern;
        self.status
            .active_pattern
            .store(active_pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(
            queued_pattern.map_or(u8::MAX, |pattern| pattern as u8),
            Ordering::Release,
        );
        let smoothing_samples = smoothing.samples(self.sr);
        for (track, next) in self.next_steps.iter_mut().enumerate() {
            if *next >= self.project.patterns[self.active_pattern].tracks[track].step_count as usize
            {
                *next = 0;
            }
            let playhead = &self.status.playheads[track];
            if playhead.load(Ordering::Acquire)
                >= self.project.patterns[self.active_pattern].tracks[track].step_count
            {
                playhead.store(u8::MAX, Ordering::Release);
            }
        }
        self.configure_effects(smoothing_samples);
        self.update_mutes(false);
        self.refresh_active_parameters(smoothing_samples);
    }

    fn apply_pending(&mut self) -> bool {
        let Some((project, smoothing, map)) = self.pending.take() else {
            return true;
        };
        self.replace_project(project, smoothing, map);
        self.pending.is_none()
    }

    fn activate_pattern(&mut self, pattern: usize) {
        self.active_pattern = pattern;
        self.queued_pattern = None;
        self.next_steps = [0; TRACK_COUNT];
        self.status
            .active_pattern
            .store(pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(u8::MAX, Ordering::Release);
    }

    fn reset_lfos(&mut self) {
        for lfo in self.lfos.iter_mut().flatten() {
            lfo.reset();
        }
        for lfo in self.preview_lfos.iter_mut().flatten() {
            lfo.reset();
        }
        self.lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
        self.preview_lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
    }

    fn reconcile_lfos(&mut self, next: &AudioProject) {
        for track in 0..TRACK_COUNT {
            for parameter in ParameterId::ALL {
                let old_enabled = self.project.tracks[track]
                    .lfos
                    .get(parameter)
                    .is_some_and(|config| config.enabled);
                let new_enabled = next.tracks[track]
                    .lfos
                    .get(parameter)
                    .is_some_and(|config| config.enabled);
                if !old_enabled && new_enabled {
                    self.lfos[track][parameter as usize].reset();
                    self.preview_lfos[track][parameter as usize].reset();
                } else if !new_enabled {
                    self.lfos[track][parameter as usize].disable();
                    self.preview_lfos[track][parameter as usize].disable();
                    self.lfo_offsets[track][parameter as usize] = 0.0;
                    self.preview_lfo_offsets[track][parameter as usize] = 0.0;
                }
            }
        }
    }

    fn advance_lfo_bank(
        states: &mut [Lfo; ParameterId::ALL.len()],
        offsets: &mut [f32; ParameterId::ALL.len()],
        track: AudioTrack,
        tempo_bpm: u16,
        sample_rate: f32,
    ) {
        for parameter in ParameterId::ALL {
            let config = track.lfos.get(parameter);
            let value = states[parameter as usize].next(config, tempo_bpm, sample_rate);
            offsets[parameter as usize] =
                value * config.map_or(0.0, |config| config.depth.get() as f32);
        }
    }

    fn advance_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            Self::advance_lfo_bank(
                &mut self.lfos[track],
                &mut self.lfo_offsets[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    fn advance_preview_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            Self::advance_lfo_bank(
                &mut self.preview_lfos[track],
                &mut self.preview_lfo_offsets[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    fn reset_preview_lfos(&mut self, track: usize) {
        for lfo in &mut self.preview_lfos[track] {
            lfo.reset();
        }
        Self::advance_lfo_bank(
            &mut self.preview_lfos[track],
            &mut self.preview_lfo_offsets[track],
            self.project.tracks[track],
            self.project.globals.tempo_bpm,
            self.sr,
        );
    }
    fn locks_at(&self, track: usize, step: usize) -> ParameterLocks {
        let t = self.project.patterns[self.active_pattern].tracks[track];
        let Some(event) = t.steps[step] else {
            return ParameterLocks::default();
        };
        let mut locks = *event.locks();
        if let StepEvent::Tie { .. } = event {
            if let Some(source) = crate::model::tie_source(&t.steps[..t.step_count as usize], step)
            {
                if let Some(
                    StepEvent::Note {
                        locks: source_locks,
                        ..
                    }
                    | StepEvent::BassNote {
                        locks: source_locks,
                        ..
                    },
                ) = t.steps[source]
                {
                    locks = source_locks;
                    let mut i = (source + 1) % t.step_count as usize;
                    while i != step {
                        if let Some(StepEvent::Tie { locks: tie_locks }) = t.steps[i] {
                            locks.overlay(tie_locks);
                        }
                        i = (i + 1) % t.step_count as usize;
                    }
                    locks.overlay(*event.locks());
                }
            }
        }
        locks
    }
    fn drum_controls(
        track: AudioTrack,
        locks: ParameterLocks,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> Option<DrumControls> {
        let value = |base: Percent, lock: Option<Percent>, id: ParameterId| {
            modulated_percent(lock.unwrap_or(base).get() as f32, offsets[id as usize]) / 100.0
        };
        match track.instrument {
            Instrument::Kick(p) => Some(DrumControls {
                tune: value(p.tune, locks.tune, ParameterId::Tune),
                tone: 0.5,
                snappy: 0.0,
                decay: value(p.decay, locks.decay, ParameterId::Decay),
                attack: value(p.attack, locks.attack, ParameterId::Attack),
            }),
            Instrument::Snare(p) => Some(DrumControls {
                tune: value(p.tune, locks.tune, ParameterId::Tune),
                tone: value(p.tone, locks.tone, ParameterId::Tone),
                snappy: value(p.snappy, locks.snappy, ParameterId::Snappy),
                decay: 0.0,
                attack: 0.0,
            }),
            Instrument::Hat(p) => Some(DrumControls {
                tune: value(p.tune, locks.tune, ParameterId::Tune),
                tone: 0.5,
                snappy: 0.0,
                decay: value(p.decay, locks.decay, ParameterId::Decay),
                attack: 0.0,
            }),
            _ => None,
        }
    }

    fn trigger_drum(&mut self, track: usize, accent: bool, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Some(controls) = Self::drum_controls(t, locks, &self.lfo_offsets[track]) else {
            return;
        };
        Self::start_drum_voice(&mut self.drums[track], track, controls, accent, self.sr);
    }
    fn trigger_preview_drum(&mut self, track: usize, accent: bool, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Some(controls) = Self::drum_controls(t, locks, &self.preview_lfo_offsets[track]) else {
            return;
        };
        Self::start_drum_voice(
            &mut self.preview_drums[track],
            track,
            controls,
            accent,
            self.sr,
        );
        let voice = &mut self.preview_drums[track];
        voice
            .level
            .set(locks.level.unwrap_or(t.level).get() as f32, 0);
        voice
            .delay_send
            .set(locks.delay_send.unwrap_or(t.delay_send).normalized(), 0);
        voice
            .reverb_send
            .set(locks.reverb_send.unwrap_or(t.reverb_send).normalized(), 0);
        voice.locks = locks;
    }
    fn start_drum_voice(
        voice: &mut DrumVoice,
        track: usize,
        controls: DrumControls,
        accent: bool,
        sr: f32,
    ) {
        let DrumControls {
            tune,
            tone,
            snappy,
            decay: decay_control,
            attack: attack_control,
        } = controls;
        let decay = match track {
            0 => 0.08 * (15.0_f32).powf(decay_control),
            1 => 0.08 + snappy * 0.34,
            _ => 0.025 * (32.0_f32).powf(decay_control),
        };
        let (attack, peak) = match track {
            0 => (
                0.0015 + attack_control * 0.0025,
                if accent { 1.22 } else { 0.78 },
            ),
            1 => (0.001, if accent { 1.08 } else { 0.68 }),
            _ => (0.0007, if accent { 0.9 } else { 0.64 }),
        };
        voice.tune = tune;
        voice.tone = tone;
        voice.snappy = (snappy + if accent && track == 1 { 0.2 } else { 0.0 }).min(1.0);
        voice.attack = attack_control;
        voice.accent = accent;
        voice.envelope.trigger(peak, attack, decay, sr);
        match track {
            0 => voice
                .kick_pitch
                .trigger((tune + attack_control * 0.15).min(1.0), decay, sr),
            1 => {
                voice
                    .filter
                    .set_bandpass(900.0 + tone * 4_500.0, 0.8 + tone * 2.5, sr);
                voice.filter2.set_highpass(450.0 + tone * 900.0, 0.8, sr);
            }
            _ => {
                voice.filter.set_highpass(4_000.0 + tune * 5_500.0, 0.9, sr);
                voice
                    .filter2
                    .set_bandpass(6_000.0 + tune * 5_000.0, 1.2, sr);
            }
        }
    }
    fn update_drum_mix(&mut self, track: usize, step: usize, smoothing: u32) {
        let t = self.project.tracks[track];
        let locks = self.locks_at(track, step);
        let voice = &mut self.drums[track];
        Self::apply_drum_mix(voice, t, locks, smoothing);
    }
    fn apply_drum_mix(
        voice: &mut DrumVoice,
        track: AudioTrack,
        locks: ParameterLocks,
        smoothing: u32,
    ) {
        voice
            .level
            .set(locks.level.unwrap_or(track.level).get() as f32, smoothing);
        voice.delay_send.set(
            locks.delay_send.unwrap_or(track.delay_send).normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks.reverb_send.unwrap_or(track.reverb_send).normalized(),
            smoothing,
        );
        voice
            .pan
            .set(locks.pan.unwrap_or(track.pan).get() as f32, smoothing);
        voice.locks = locks;
    }
    fn apply_synth_params(
        project: &AudioProject,
        _sr: f32,
        track: usize,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
        smoothing: u32,
    ) {
        let t = project.tracks[track];
        let (cutoff, resonance, filter_envelope, attack, decay, sustain, release) =
            match t.instrument {
                Instrument::Bass(p) => {
                    voice.bass = true;
                    voice.chord = false;
                    voice.env.set_profile(EnvelopeProfile::Generic);
                    voice.wave = locks.waveform.unwrap_or(p.waveform);
                    (
                        p.cutoff,
                        p.resonance,
                        p.filter_envelope,
                        Percent::ZERO,
                        Percent::new((43.3 + p.decay.get() as f32 * 0.504).round() as u8).unwrap(),
                        Percent::ZERO,
                        Percent::new(4).unwrap(),
                    )
                }
                Instrument::Chord(p) => {
                    voice.bass = false;
                    voice.chord = true;
                    voice.env.set_profile(EnvelopeProfile::Juno);
                    voice.oscillator_mix.set(
                        locks.oscillator_mix.unwrap_or(p.oscillator_mix).get() as f32,
                        smoothing,
                    );
                    voice.pulse_width.set(
                        locks.pulse_width.unwrap_or(p.pulse_width).get() as f32,
                        smoothing,
                    );
                    voice.sub_oscillator.set(
                        locks.sub_oscillator.unwrap_or(p.sub_oscillator).get() as f32,
                        smoothing,
                    );
                    (
                        p.cutoff,
                        p.resonance,
                        p.filter_envelope,
                        p.attack,
                        p.decay,
                        p.sustain,
                        p.release,
                    )
                }
                Instrument::Lead(p) => {
                    voice.bass = false;
                    voice.chord = false;
                    voice.env.set_profile(EnvelopeProfile::Sh101);
                    voice.oscillator_mix.set(
                        locks.oscillator_mix.unwrap_or(p.oscillator_mix).get() as f32,
                        smoothing,
                    );
                    voice.pulse_width.set(
                        locks.pulse_width.unwrap_or(p.pulse_width).get() as f32,
                        smoothing,
                    );
                    voice.sub_oscillator.set(
                        locks.sub_oscillator.unwrap_or(p.sub_oscillator).get() as f32,
                        smoothing,
                    );
                    (
                        p.cutoff,
                        p.resonance,
                        p.filter_envelope,
                        p.attack,
                        p.decay,
                        p.sustain,
                        p.release,
                    )
                }
                _ => return,
            };
        voice
            .cutoff_percent
            .set(locks.cutoff.unwrap_or(cutoff).get() as f32, smoothing);
        voice
            .resonance_percent
            .set(locks.resonance.unwrap_or(resonance).get() as f32, smoothing);
        voice.filter_env_percent.set(
            locks.filter_envelope.unwrap_or(filter_envelope).get() as f32,
            smoothing,
        );
        voice.env.configure_percent(
            locks.attack.unwrap_or(attack).get(),
            locks.decay.unwrap_or(decay).get(),
            locks.sustain.unwrap_or(sustain).get(),
            locks.release.unwrap_or(release).get(),
            smoothing,
        );
        voice
            .level
            .set(locks.level.unwrap_or(t.level).get() as f32, smoothing);
        voice.delay_send.set(
            locks.delay_send.unwrap_or(t.delay_send).normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks.reverb_send.unwrap_or(t.reverb_send).normalized(),
            smoothing,
        );
        if !(voice.chord && voice.active) {
            voice
                .pan
                .set(locks.pan.unwrap_or(t.pan).get() as f32, smoothing);
        }
        voice.locks = locks;
    }

    fn configure_chorus(chorus: &mut StereoChorus, track: AudioTrack, locks: ParameterLocks) {
        let Instrument::Chord(parameters) = track.instrument else {
            return;
        };
        let mode = locks.chorus.unwrap_or(parameters.chorus);
        chorus.configure(match mode {
            ChorusMode::Off => 0,
            ChorusMode::I => 1,
            ChorusMode::Ii => 2,
        });
    }
    fn configure_synth_voice(
        project: &AudioProject,
        sr: f32,
        track: usize,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
    ) {
        let midi = 12 * (trigger.octave as i32 + 1)
            + project.globals.key.semitone()
            + project.globals.scale.offsets()[trigger.degree as usize - 1];
        let frequency = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
        Self::configure_synth_voice_frequency(project, sr, track, frequency, trigger, locks, voice);
    }

    fn configure_synth_voice_frequency(
        project: &AudioProject,
        sr: f32,
        track: usize,
        frequency: f32,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
    ) {
        let legato_slide = voice.bass && voice.active && voice.slide_armed;
        voice.freq.set(
            frequency,
            if legato_slide {
                (sr * 0.060).round() as u32
            } else {
                0
            },
        );
        Self::apply_synth_params(
            project,
            sr,
            track,
            locks,
            voice,
            ParameterSmoothing::Default.samples(sr),
        );
        let gain = if trigger.accent { 1.413 } else { 1.0 };
        voice
            .accent_gain
            .set(gain, ParameterSmoothing::Default.samples(sr));
        voice.accent_filter.set(
            if trigger.accent {
                if voice.bass { 1.0 } else { 0.2 }
            } else {
                0.0
            },
            ParameterSmoothing::Default.samples(sr),
        );
        if !legato_slide {
            voice.env.gate_on();
        }
        voice.slide_armed = voice.bass && trigger.slide;
        voice.active = true;
    }

    fn chord_midis(
        project: &AudioProject,
        degree: u8,
        octave: u8,
        shape: ChordShape,
    ) -> ([i32; 4], usize) {
        let scale = project.globals.scale.offsets();
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
                + project.globals.key.semitone()
                + scale[scale_degree % 7];
        }
        (midis, shape.degrees().len())
    }

    fn trigger_chord(
        project: &AudioProject,
        sr: f32,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        pool: &mut ChordVoicePool,
    ) {
        let Instrument::Chord(_) = project.tracks[4].instrument else {
            return;
        };
        let shape = trigger.chord_shape.unwrap_or_default();
        let arpeggio = trigger.arpeggio;
        pool.arpeggio_trigger = SynthTrigger {
            chord_shape: Some(shape),
            ..trigger
        };
        pool.arpeggio_locks = locks;
        if arpeggio.enabled {
            Self::release_chord(pool);
            pool.group = 1 - pool.group;
            pool.arpeggiated = true;
            pool.arpeggio_trigger = SynthTrigger {
                chord_shape: Some(shape),
                ..trigger
            };
            pool.arpeggio_locks = locks;
            pool.arpeggio.reset(
                shape,
                arpeggio.r#type,
                arpeggio.rate,
                sr,
                project.globals.tempo_bpm,
            );
            pool.voice_count = 1;
            Self::trigger_arpeggio_tone(project, sr, pool);
            Self::configure_chorus(&mut pool.chorus, project.tracks[4], locks);
            pool.active = true;
            return;
        }
        pool.arpeggiated = false;
        if pool.active {
            for voice in &mut pool.voices
                [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
            {
                Self::apply_synth_params(
                    project,
                    sr,
                    4,
                    locks,
                    voice,
                    ParameterSmoothing::Default.samples(sr),
                );
                voice.env.gate_off();
                voice.active = false;
            }
        }
        pool.group = 1 - pool.group;
        let (midis, voice_count) =
            Self::chord_midis(project, trigger.degree, trigger.octave, shape);
        for (voice, midi) in pool.voices
            [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + voice_count]
            .iter_mut()
            .zip(midis.into_iter().take(voice_count))
        {
            let frequency = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            Self::configure_synth_voice_frequency(project, sr, 4, frequency, trigger, locks, voice);
        }
        pool.voice_count = voice_count;
        let spread = locks
            .spread
            .unwrap_or_else(|| match project.tracks[4].instrument {
                Instrument::Chord(p) => p.spread,
                _ => crate::model::ChordSpread::Off,
            });
        let offsets = match voice_count {
            3 => [-50.0, 0.0, 50.0, 0.0],
            4 => [-50.0, -16.6667, 16.6667, 50.0],
            _ => [0.0; 4],
        };
        for (index, offset) in offsets.into_iter().take(voice_count).enumerate() {
            let target = locks.pan.unwrap_or(project.tracks[4].pan).get() as f32
                + offset * spread.percent().normalized();
            pool.voices[pool.group * CHORD_GROUP_SIZE + index]
                .pan
                .set(target.clamp(0.0, 100.0), 0);
        }
        Self::configure_chorus(&mut pool.chorus, project.tracks[4], locks);
        pool.active = true;
    }

    fn trigger_arpeggio_tone(project: &AudioProject, sr: f32, pool: &mut ChordVoicePool) {
        let index = pool.arpeggio.current_voice();
        let (midis, count) = Self::chord_midis(
            project,
            pool.arpeggio_trigger.degree,
            pool.arpeggio_trigger.octave,
            pool.arpeggio.shape,
        );
        let voice = &mut pool.voices[pool.group * CHORD_GROUP_SIZE];
        voice.remaining = 0;
        let frequency = 440.0 * 2.0_f32.powf((midis[index.min(count - 1)] as f32 - 69.0) / 12.0);
        Self::configure_synth_voice_frequency(
            project,
            sr,
            4,
            frequency,
            pool.arpeggio_trigger,
            pool.arpeggio_locks,
            voice,
        );
        let spread =
            pool.arpeggio_locks
                .spread
                .unwrap_or_else(|| match project.tracks[4].instrument {
                    Instrument::Chord(p) => p.spread,
                    _ => crate::model::ChordSpread::Off,
                });
        let offsets = match count {
            3 => [-50.0, 0.0, 50.0, 0.0],
            4 => [-50.0, -16.6667, 16.6667, 50.0],
            _ => [0.0; 4],
        };
        voice.pan.set(
            (pool
                .arpeggio_locks
                .pan
                .unwrap_or(project.tracks[4].pan)
                .get() as f32
                + offsets[index.min(3)] * spread.percent().normalized())
            .clamp(0.0, 100.0),
            0,
        );
    }

    fn release_chord(pool: &mut ChordVoicePool) {
        if pool.active {
            for voice in &mut pool.voices
                [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
            {
                voice.env.gate_off();
                voice.active = false;
            }
            pool.active = false;
            pool.voice_count = 0;
        }
        pool.arpeggiated = false;
        pool.arpeggio.enabled = false;
        pool.preview_remaining = 0;
    }
    fn refresh_active_parameters(&mut self, smoothing: u32) {
        for track in 0..3 {
            let params = self.project.tracks[track];
            // Locks are captured at the step boundary. A live project snapshot may
            // change the current step, but that edit must not affect its active hit.
            let locks = self.drums[track].locks;
            Self::apply_drum_mix(&mut self.drums[track], params, locks, smoothing);
            let locks = self.preview_drums[track].locks;
            Self::apply_drum_mix(&mut self.preview_drums[track], params, locks, smoothing);
        }
        for track in [3, 5] {
            let index = track - 3;
            if self.synth[index].active {
                // Keep the effective lock chain latched until the next boundary.
                let locks = self.synth[index].locks;
                Self::apply_synth_params(
                    &self.project,
                    self.sr,
                    track,
                    locks,
                    &mut self.synth[index],
                    smoothing,
                );
            }
            if self.preview[index].active {
                let locks = self.preview[index].locks;
                Self::apply_synth_params(
                    &self.project,
                    self.sr,
                    track,
                    locks,
                    &mut self.preview[index],
                    smoothing,
                );
            }
        }
        if self.chord.active {
            let chorus_locks = self.chord.voices[self.chord.group * CHORD_GROUP_SIZE].locks;
            for voice in &mut self.chord.voices[self.chord.group * CHORD_GROUP_SIZE
                ..self.chord.group * CHORD_GROUP_SIZE + self.chord.voice_count]
            {
                let locks = voice.locks;
                Self::apply_synth_params(&self.project, self.sr, 4, locks, voice, smoothing);
            }
            Self::configure_chorus(&mut self.chord.chorus, self.project.tracks[4], chorus_locks);
        }
        if self.preview_chord.active {
            let chorus_locks =
                self.preview_chord.voices[self.preview_chord.group * CHORD_GROUP_SIZE].locks;
            for voice in &mut self.preview_chord.voices[self.preview_chord.group * CHORD_GROUP_SIZE
                ..self.preview_chord.group * CHORD_GROUP_SIZE + self.preview_chord.voice_count]
            {
                let locks = voice.locks;
                Self::apply_synth_params(&self.project, self.sr, 4, locks, voice, smoothing);
            }
            Self::configure_chorus(
                &mut self.preview_chord.chorus,
                self.project.tracks[4],
                chorus_locks,
            );
        }
    }
    fn audition(&mut self, track: usize, step: usize) {
        if track >= TRACK_COUNT
            || step >= self.project.patterns[self.active_pattern].tracks[track].step_count as usize
        {
            return;
        }
        self.reset_preview_lfos(track);
        if track < 3 {
            let accent = match self.project.patterns[self.active_pattern].tracks[track].steps[step]
            {
                Some(StepEvent::Trigger { accent, .. }) => accent,
                _ => false,
            };
            self.trigger_preview_drum(track, accent, self.locks_at(track, step));
            return;
        }
        let t = self.project.patterns[self.active_pattern].tracks[track];
        let (degree, octave, accent, slide, chord_shape, arpeggio, locks) = match t.steps[step] {
            Some(StepEvent::BassNote {
                degree,
                octave,
                accent,
                slide,
                locks,
            }) => (
                degree,
                octave,
                accent,
                slide,
                None,
                ArpeggioConfig::default(),
                locks,
            ),
            Some(StepEvent::Note {
                degree,
                octave,
                accent,
                chord_shape,
                arpeggio,
                locks: _,
            }) => (
                degree,
                octave,
                accent,
                false,
                chord_shape,
                arpeggio,
                self.locks_at(track, step),
            ),
            Some(StepEvent::Tie { .. }) => {
                let Some(source) =
                    crate::model::tie_source(&t.steps[..t.step_count as usize], step)
                else {
                    return;
                };
                match t.steps[source] {
                    Some(StepEvent::BassNote {
                        degree,
                        octave,
                        accent,
                        slide,
                        ..
                    }) => (
                        degree,
                        octave,
                        accent,
                        slide,
                        None,
                        ArpeggioConfig::default(),
                        self.locks_at(track, step),
                    ),
                    Some(StepEvent::Note {
                        degree,
                        octave,
                        accent,
                        chord_shape,
                        arpeggio,
                        ..
                    }) => (
                        degree,
                        octave,
                        accent,
                        false,
                        chord_shape,
                        arpeggio,
                        self.locks_at(track, step),
                    ),
                    _ => return,
                }
            }
            _ => (
                self.project.tracks[track].input_degree,
                self.project.tracks[track].input_octave,
                false,
                false,
                (track == 4).then_some(self.project.tracks[track].input_chord_shape),
                if track == 4 {
                    self.project.tracks[track].input_chord_arpeggio
                } else {
                    ArpeggioConfig::default()
                },
                ParameterLocks::default(),
            ),
        };
        let trigger = SynthTrigger {
            degree,
            octave,
            accent,
            slide,
            chord_shape,
            arpeggio,
        };
        if track == 4 {
            Self::trigger_chord(
                &self.project,
                self.sr,
                trigger,
                locks,
                &mut self.preview_chord,
            );
            let remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
            for voice in &mut self.preview_chord.voices[self.preview_chord.group * CHORD_GROUP_SIZE
                ..self.preview_chord.group * CHORD_GROUP_SIZE + self.preview_chord.voice_count]
            {
                voice.remaining = remaining;
            }
            if self.preview_chord.arpeggiated {
                let interval = self.sr as f64 * 60.0 / self.project.globals.tempo_bpm as f64
                    * self.preview_chord.arpeggio.rate.beats();
                self.preview_chord.preview_remaining =
                    (interval * self.preview_chord.arpeggio.order_len as f64).ceil() as u64;
            }
            return;
        }
        let v = &mut self.preview[track - 3];
        Self::configure_synth_voice(&self.project, self.sr, track, trigger, locks, v);
        v.remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
    }
    fn boundary(&mut self) {
        if self.clock.next_step % 16 == 0
            && let Some(pattern) = self.queued_pattern
        {
            self.activate_pattern(pattern);
            for voice in &mut self.synth {
                voice.env.gate_off();
                voice.active = false;
            }
            Self::release_chord(&mut self.chord);
        }
        for track in 0..TRACK_COUNT {
            let step = self.next_steps[track];
            self.status.playheads[track].store(step as u8, Ordering::Release);
            let t = self.project.tracks[track];
            let sequence = self.project.patterns[self.active_pattern].tracks[track];
            if track < 3 {
                self.update_drum_mix(track, step, ParameterSmoothing::Default.samples(self.sr));
                if let Some(StepEvent::Trigger { accent, locks }) = sequence.steps[step] {
                    self.trigger_drum(track, accent, locks);
                }
            } else {
                let vi = track - 3;
                let active = if track == 4 {
                    self.chord.active
                } else {
                    self.synth[vi].active
                };
                match synth_action(
                    &sequence.steps[..sequence.step_count as usize],
                    step,
                    active,
                ) {
                    GateAction::Trigger {
                        degree,
                        octave,
                        accent,
                        slide,
                        chord_shape,
                        arpeggio,
                    } => {
                        let locks = self.locks_at(track, step);
                        let trigger = SynthTrigger {
                            degree,
                            octave,
                            accent,
                            slide,
                            chord_shape,
                            arpeggio,
                        };
                        if track == 4 {
                            Self::trigger_chord(
                                &self.project,
                                self.sr,
                                trigger,
                                locks,
                                &mut self.chord,
                            );
                        } else {
                            Self::configure_synth_voice(
                                &self.project,
                                self.sr,
                                track,
                                trigger,
                                locks,
                                &mut self.synth[vi],
                            );
                        }
                    }
                    GateAction::Hold => {
                        if matches!(sequence.steps[step], Some(StepEvent::Tie { .. })) {
                            let locks = self.locks_at(track, step);
                            if track == 4 {
                                for voice in &mut self.chord.voices[self.chord.group
                                    * CHORD_GROUP_SIZE
                                    ..self.chord.group * CHORD_GROUP_SIZE + self.chord.voice_count]
                                {
                                    Self::apply_synth_params(
                                        &self.project,
                                        self.sr,
                                        track,
                                        locks,
                                        voice,
                                        ParameterSmoothing::Default.samples(self.sr),
                                    );
                                }
                                Self::configure_chorus(&mut self.chord.chorus, t, locks);
                            } else {
                                Self::apply_synth_params(
                                    &self.project,
                                    self.sr,
                                    track,
                                    locks,
                                    &mut self.synth[vi],
                                    ParameterSmoothing::Default.samples(self.sr),
                                );
                            }
                        }
                    }
                    GateAction::Release => {
                        if track == 4 {
                            for voice in &mut self.chord.voices[self.chord.group * CHORD_GROUP_SIZE
                                ..self.chord.group * CHORD_GROUP_SIZE + self.chord.voice_count]
                            {
                                Self::apply_synth_params(
                                    &self.project,
                                    self.sr,
                                    track,
                                    ParameterLocks::default(),
                                    voice,
                                    ParameterSmoothing::Default.samples(self.sr),
                                );
                            }
                            Self::release_chord(&mut self.chord);
                            Self::configure_chorus(
                                &mut self.chord.chorus,
                                t,
                                ParameterLocks::default(),
                            );
                        } else {
                            Self::apply_synth_params(
                                &self.project,
                                self.sr,
                                track,
                                ParameterLocks::default(),
                                &mut self.synth[vi],
                                ParameterSmoothing::Default.samples(self.sr),
                            );
                            self.synth[vi].env.gate_off();
                            self.synth[vi].active = false;
                            self.synth[vi].slide_armed = false;
                        }
                    }
                    GateAction::None => {}
                }
            }
            self.next_steps[track] = (step + 1) % sequence.step_count as usize;
        }
    }
    fn render_synth(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        // Chord pools keep a spare group for released notes.  Once an
        // envelope is idle there is no signal to render, so avoid running
        // oscillators and filters for those voices on every callback sample.
        if v.env.stage == EnvStage::Idle {
            return (0.0, 0.0, 0.0);
        }
        if v.remaining > 0 {
            v.remaining -= 1;
            if v.remaining == 0 {
                v.env.gate_off();
                v.active = false;
            }
        }
        let mut frequency = v.freq.next_value();
        if !v.bass {
            frequency = pitch_modulated_frequency(frequency, offsets[ParameterId::Pitch as usize]);
        }
        let env = v.env.next_sample_modulated(
            offsets[ParameterId::Attack as usize],
            offsets[ParameterId::Decay as usize],
            offsets[ParameterId::Sustain as usize],
            offsets[ParameterId::Release as usize],
        );
        let cutoff_percent = modulated_percent(
            v.cutoff_percent.next_value(),
            offsets[ParameterId::Cutoff as usize],
        );
        let filter_env = modulated_percent(
            v.filter_env_percent.next_value(),
            offsets[ParameterId::FilterEnvelope as usize],
        ) / 100.0;
        let accent_filter = v.accent_filter.next_value();
        let (minimum_cutoff, maximum_cutoff, envelope_octaves) = if v.bass {
            (80.0, 8_000.0_f32.min(sr * 0.45), 5.0)
        } else {
            (20.0, 20_000.0_f32.min(sr * 0.45), 6.0)
        };
        let cutoff = (exp_map_f32(cutoff_percent, minimum_cutoff, maximum_cutoff)
            * 2.0_f32.powf(env * (filter_env * envelope_octaves + accent_filter)))
        .min(maximum_cutoff);
        let resonance_percent = modulated_percent(
            v.resonance_percent.next_value(),
            offsets[ParameterId::Resonance as usize],
        );
        let level_percent =
            modulated_percent(v.level.next_value(), offsets[ParameterId::Level as usize]);
        let level = (level_percent / 100.0).powi(2);
        let accent_gain = v.accent_gain.next_value();
        let delay_send = v.delay_send.next_value();
        let reverb_send = v.reverb_send.next_value();
        let oversampled_rate = sr * 2.0;
        let mut filtered = 0.0;
        for _ in 0..2 {
            let osc = if v.bass {
                match v.wave {
                    Waveform::Saw => v.osc.next_saw(frequency, oversampled_rate),
                    Waveform::Square => v.osc.next_square(frequency, oversampled_rate),
                }
            } else {
                let mix = modulated_percent(
                    v.oscillator_mix.next_value(),
                    offsets[ParameterId::OscillatorMix as usize],
                ) / 100.0;
                let width = 0.05
                    + modulated_percent(
                        v.pulse_width.next_value(),
                        offsets[ParameterId::PulseWidth as usize],
                    ) / 100.0
                        * 0.90;
                let sub = modulated_percent(
                    v.sub_oscillator.next_value(),
                    offsets[ParameterId::SubOscillator as usize],
                ) / 100.0;
                let (saw, pulse) = v.osc.next_saw_pulse(frequency, width, oversampled_rate);
                let angle = mix * std::f32::consts::FRAC_PI_2;
                pulse * angle.cos()
                    + saw * angle.sin()
                    + v.sub_osc.next_square(frequency * 0.5, oversampled_rate) * sub
            };
            filtered += if v.bass {
                let driven = (osc * 1.35).tanh();
                v.bass_filter
                    .lowpass(driven, cutoff, resonance_percent / 100.0, oversampled_rate)
            } else {
                let driven = (osc * if v.chord { 1.10 } else { 1.35 }).tanh();
                v.roland_filter.lowpass(
                    driven,
                    cutoff,
                    (resonance_percent / 100.0) * if v.chord { 0.95 } else { 1.0 },
                    oversampled_rate,
                )
            };
        }
        filtered *= 0.5 / (1.0 + resonance_percent * 0.0035);
        (
            filtered
                * env
                * accent_gain
                * level
                * if v.bass {
                    5.0
                } else if v.chord {
                    1.15 * std::f32::consts::FRAC_1_SQRT_2
                } else {
                    2.0
                },
            delay_send,
            reverb_send,
        )
    }
    fn render_drum(
        voice: &mut DrumVoice,
        track: usize,
        sr: f32,
        level_offset: f32,
        pan_offset: f32,
    ) -> (f32, f32, f32, f32) {
        let raw = match track {
            0 => {
                let hz = voice.kick_pitch.next_value();
                let body = (TAU * voice.phase).sin();
                voice.phase = (voice.phase + hz / sr).fract();
                let click_env = (-(voice.envelope.elapsed as f32) / (sr * 0.003)).exp();
                body + voice.noise() * click_env * voice.attack * 0.35
            }
            1 => {
                let hz = 150.0 + voice.tune * 150.0;
                let lower = 1.0 - 4.0 * (voice.phase - 0.5).abs();
                let upper = 1.0 - 4.0 * (voice.phase2 - 0.5).abs();
                voice.phase = (voice.phase + hz / sr).fract();
                voice.phase2 = (voice.phase2 + hz * 1.72 / sr).fract();
                let noise = voice.noise();
                let noise = voice.filter.process(voice.filter2.process(noise));
                let body = lower * (0.42 - voice.tone * 0.12) + upper * (0.12 + voice.tone * 0.18);
                body + noise * (0.25 + voice.snappy * 0.72)
            }
            _ => {
                const RATIOS: [f32; 6] = [1.0, 1.447, 1.617, 1.926, 2.502, 2.663];
                let base = 310.0 + voice.tune * 360.0;
                let mut metal = 0.0;
                for (osc, ratio) in voice.metallic.iter_mut().zip(RATIOS) {
                    metal += osc.next_square(base * ratio, sr);
                }
                metal /= RATIOS.len() as f32;
                let source = metal * 0.82 + voice.noise() * 0.18;
                let bright = voice.filter.process(source);
                bright * 0.75 + voice.filter2.process(source) * 0.25
            }
        };
        let level = modulated_percent(voice.level.next_value(), level_offset) / 100.0;
        let sample = (raw * 1.15).tanh() * voice.envelope.next_value() * level.powi(2) * 1.40;
        (
            sample,
            voice.delay_send.next_value(),
            voice.reverb_send.next_value(),
            modulated_percent(voice.pan.next_value(), pan_offset),
        )
    }
    fn next(&mut self) -> (f32, f32) {
        if self.playing {
            self.advance_lfos();
        }
        self.advance_preview_lfos();
        if self.playing && self.clock.tick().is_some() {
            self.boundary()
        }
        self.advance_chord_arpeggios();
        let mut dry_l = 0.0;
        let mut dry_r = 0.0;
        let mut delay_l = 0.0;
        let mut delay_r = 0.0;
        let mut reverb_l = 0.0;
        let mut reverb_r = 0.0;
        for i in 0..3 {
            let (x, delay_send, reverb_send, pan) = Self::render_drum(
                &mut self.drums[i],
                i,
                self.sr,
                self.lfo_offsets[i][ParameterId::Level as usize],
                self.lfo_offsets[i][ParameterId::Pan as usize],
            );
            let (pl, pr) = equal_power_pan(pan);
            let gain = self.mute[i].next_value();
            dry_l += x * pl * gain;
            dry_r += x * pr * gain;
            delay_l += x * pl * delay_send * gain;
            delay_r += x * pr * delay_send * gain;
            reverb_l += x * pl * reverb_send * gain;
            reverb_r += x * pr * reverb_send * gain;
        }
        for i in 0..3 {
            let (x, delay_send, reverb_send, pan) = Self::render_drum(
                &mut self.preview_drums[i],
                i,
                self.sr,
                self.preview_lfo_offsets[i][ParameterId::Level as usize],
                self.preview_lfo_offsets[i][ParameterId::Pan as usize],
            );
            let (pl, pr) = equal_power_pan(pan);
            dry_l += x * pl;
            dry_r += x * pr;
            delay_l += x * pl * delay_send;
            delay_r += x * pr * delay_send;
            reverb_l += x * pl * reverb_send;
            reverb_r += x * pr * reverb_send;
        }
        for i in [0, 2] {
            let (x, ds, rs) =
                Self::render_synth(&mut self.synth[i], self.sr, &self.lfo_offsets[i + 3]);
            let (pl, pr) = equal_power_pan(modulated_percent(
                self.synth[i].pan.next_value(),
                self.lfo_offsets[i + 3][ParameterId::Pan as usize],
            ));
            let gain = self.mute[i + 3].next_value();
            dry_l += x * pl * gain;
            dry_r += x * pr * gain;
            delay_l += x * pl * ds * gain;
            delay_r += x * pr * ds * gain;
            reverb_l += x * pl * rs * gain;
            reverb_r += x * pr * rs * gain;
            let (x, ds, rs) = Self::render_synth(
                &mut self.preview[i],
                self.sr,
                &self.preview_lfo_offsets[i + 3],
            );
            let (pl, pr) = equal_power_pan(modulated_percent(
                self.preview[i].pan.next_value(),
                self.preview_lfo_offsets[i + 3][ParameterId::Pan as usize],
            ));
            dry_l += x * pl;
            dry_r += x * pr;
            delay_l += x * pl * ds;
            delay_r += x * pr * ds;
            reverb_l += x * pl * rs;
            reverb_r += x * pr * rs;
        }
        let mut chord_left = 0.0;
        let mut chord_right = 0.0;
        let mut chord_ds = 0.0;
        let mut chord_rs = 0.0;
        let chord_start = self.chord.group * CHORD_GROUP_SIZE;
        let chord_end = chord_start + self.chord.voice_count;
        let mut chord_has_active_send = false;
        let mut chord_tail_send = None;
        for (index, voice) in self.chord.voices.iter_mut().enumerate() {
            let (x, ds, rs) = Self::render_synth(voice, self.sr, &self.lfo_offsets[4]);
            let (pl, pr) = equal_power_pan(modulated_percent(
                voice.pan.next_value(),
                self.lfo_offsets[4][ParameterId::Pan as usize],
            ));
            chord_left += x * pl;
            chord_right += x * pr;
            if voice.env.stage != EnvStage::Idle {
                if self.chord.active && (chord_start..chord_end).contains(&index) {
                    chord_ds = ds;
                    chord_rs = rs;
                    chord_has_active_send = true;
                } else if chord_tail_send.is_none() {
                    chord_tail_send = Some((ds, rs));
                }
            }
        }
        if !chord_has_active_send {
            (chord_ds, chord_rs) = chord_tail_send.unwrap_or((0.0, 0.0));
        }
        let (chord_l, chord_r) = self.chord.chorus.process_stereo(chord_left, chord_right);
        let chord_gain = self.mute[4].next_value();
        dry_l += chord_l * chord_gain;
        dry_r += chord_r * chord_gain;
        delay_l += chord_l * chord_ds * chord_gain;
        delay_r += chord_r * chord_ds * chord_gain;
        reverb_l += chord_l * chord_rs * chord_gain;
        reverb_r += chord_r * chord_rs * chord_gain;

        let mut preview_left = 0.0;
        let mut preview_right = 0.0;
        let mut preview_ds = 0.0;
        let mut preview_rs = 0.0;
        let preview_start = self.preview_chord.group * CHORD_GROUP_SIZE;
        let preview_end = preview_start + self.preview_chord.voice_count;
        let mut preview_has_active_send = false;
        let mut preview_tail_send = None;
        for (index, voice) in self.preview_chord.voices.iter_mut().enumerate() {
            let (x, ds, rs) = Self::render_synth(voice, self.sr, &self.preview_lfo_offsets[4]);
            let (pl, pr) = equal_power_pan(modulated_percent(
                voice.pan.next_value(),
                self.preview_lfo_offsets[4][ParameterId::Pan as usize],
            ));
            preview_left += x * pl;
            preview_right += x * pr;
            if voice.env.stage != EnvStage::Idle {
                if self.preview_chord.active && (preview_start..preview_end).contains(&index) {
                    preview_ds = ds;
                    preview_rs = rs;
                    preview_has_active_send = true;
                } else if preview_tail_send.is_none() {
                    preview_tail_send = Some((ds, rs));
                }
            }
        }
        if !preview_has_active_send {
            (preview_ds, preview_rs) = preview_tail_send.unwrap_or((0.0, 0.0));
        }
        let (preview_l, preview_r) = self
            .preview_chord
            .chorus
            .process_stereo(preview_left, preview_right);
        dry_l += preview_l;
        dry_r += preview_r;
        delay_l += preview_l * preview_ds;
        delay_r += preview_r * preview_ds;
        reverb_l += preview_l * preview_rs;
        reverb_r += preview_r * preview_rs;

        let (dl, dr) = self.delay.process(delay_l, delay_r);
        let (rl, rr) = self
            .reverb
            .process(reverb_l + dl * 0.25, reverb_r + dr * 0.25);
        let (l, r) = self.dc.process(
            dry_l + dl * 0.45 + rl * REVERB_RETURN_GAIN,
            dry_r + dr * 0.45 + rr * REVERB_RETURN_GAIN,
        );
        if !(l.is_finite() && r.is_finite()) {
            self.status.non_finite.store(true, Ordering::Release);
            return (0.0, 0.0);
        }
        self.limiter.process(l, r)
    }

    fn advance_chord_arpeggios(&mut self) {
        let bpm = self.project.globals.tempo_bpm;
        if self.chord.arpeggiated && self.chord.active && self.chord.arpeggio.tick(self.sr, bpm) {
            Self::trigger_arpeggio_tone(&self.project, self.sr, &mut self.chord);
        }
        if self.preview_chord.arpeggiated && self.preview_chord.active {
            if self.preview_chord.preview_remaining > 0 {
                self.preview_chord.preview_remaining -= 1;
            }
            if self.preview_chord.preview_remaining == 0 {
                Self::release_chord(&mut self.preview_chord);
            } else if self.preview_chord.arpeggio.tick(self.sr, bpm) {
                Self::trigger_arpeggio_tone(&self.project, self.sr, &mut self.preview_chord);
            }
        }
    }
}

fn render<T: Copy, F: Fn(f32) -> T>(
    out: &mut [T],
    channels: usize,
    renderer: &mut Renderer,
    commands: &mut Consumer<AudioCommand>,
    convert: F,
) {
    if renderer.apply_pending() {
        while let Ok(c) = commands.pop() {
            let is_replace = matches!(c, AudioCommand::ReplaceProject { .. });
            renderer.command(c);
            if is_replace && renderer.pending.is_some() {
                break;
            }
        }
    }
    for frame in out.chunks_mut(channels) {
        let (l, r) = renderer.next();
        if !frame.is_empty() {
            frame[0] = convert(if channels == 1 { (l + r) * 0.5 } else { l })
        }
        if channels > 1 {
            frame[1] = convert(r)
        }
        for sample in frame.iter_mut().skip(2) {
            *sample = convert(0.0)
        }
    }
}

fn modulated_percent(center: f32, offset: f32) -> f32 {
    (center + offset).clamp(0.0, 100.0)
}

fn pitch_modulated_frequency(base_frequency: f32, offset_percent: f32) -> f32 {
    base_frequency * 2.0_f32.powf((offset_percent / 100.0 * 2.0) / 12.0)
}

/// Deterministic, device-independent rendering used by tests and diagnostics.
pub fn render_offline(project: &Project, sample_rate: u32, frames: usize) -> Vec<(f32, f32)> {
    let status = Arc::new(AudioStatus::default());
    let mut renderer = Renderer::new(AudioProject::from_project(project), sample_rate, status);
    renderer.command(AudioCommand::PlayPause);
    (0..frames).map(|_| renderer.next()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drum_envelope_reaches_peak_and_silence_at_programmed_times() {
        let mut envelope = DrumEnvelope::new();
        envelope.trigger(1.2, 0.004, 0.08, 1_000.0);
        let at_peak = (0..=4).map(|_| envelope.next_value()).last().unwrap();
        assert!((at_peak - 1.2).abs() < 0.0001);
        let at_end = (5..=80).map(|_| envelope.next_value()).last().unwrap();
        assert!((at_end - DRUM_SILENCE).abs() < 0.000001);
    }
    #[test]
    fn kick_pitch_uses_909_inspired_peak_and_settled_mappings() {
        let mut pitch = KickPitchEnvelope::new();
        pitch.trigger(0.5, 0.455, 10_000.0);
        let at_peak = (0..=15).map(|_| pitch.next_value()).last().unwrap();
        assert!((at_peak - 195.0).abs() < 0.001);
        let settled = (16..=1_300).map(|_| pitch.next_value()).last().unwrap();
        assert!((settled - 57.5).abs() < 0.001);
    }
    #[test]
    fn snapshot_contains_all_steps_without_heap_backed_commands() {
        let mut p = Project::new();
        p.patterns[0].tracks[0].steps.resize(MAX_STEP_COUNT, None);
        let AudioCommand::ReplaceProject {
            project: s,
            smoothing,
            ..
        } = Audio::snapshot(&p)
        else {
            panic!()
        };
        assert_eq!(smoothing, ParameterSmoothing::Default);
        assert_eq!(s.globals.tempo_bpm, 120);
        assert_eq!(s.patterns[0].tracks[0].step_count as usize, MAX_STEP_COUNT);
        assert_eq!(s.patterns[0].tracks[1].step_count, 16);
        assert!(s.patterns[0].tracks[0].steps.iter().all(Option::is_none));
    }

    #[test]
    fn snapshot_preserves_dynamic_pattern_count_and_indexes() {
        let mut project = Project::new();
        let extra = project.patterns[0].clone();
        project.patterns.push(extra.clone());
        project.patterns.push(extra);
        project.patterns[2].tracks[1].steps.resize(24, None);
        let AudioCommand::ReplaceProject {
            project: snapshot, ..
        } = Audio::snapshot(&project)
        else {
            panic!()
        };
        assert_eq!(snapshot.patterns.len(), 3);
        assert_eq!(snapshot.patterns[2].tracks[1].step_count, 24);
    }

    #[test]
    fn snapshot_uses_the_editor_workspace_only_for_the_committed_pattern() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns[1].tracks[0].steps[1] = Some(StepEvent::Trigger {
            accent: true,
            locks: Default::default(),
        });
        project.activate_pattern(1);

        let AudioCommand::ReplaceProject {
            project: snapshot, ..
        } = Audio::snapshot_for_pattern(&project, 1)
        else {
            panic!()
        };
        assert!(
            snapshot.patterns[0].tracks[0]
                .steps
                .iter()
                .all(Option::is_none)
        );
        assert!(matches!(
            snapshot.patterns[1].tracks[0].steps[1],
            Some(StepEvent::Trigger { accent: true, .. })
        ));
    }

    #[test]
    fn renderer_reports_independent_track_playheads() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(3, None);
        project.patterns[0].tracks[1].steps.resize(5, None);
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&project), 8_000, status.clone());
        for expected in [[0, 0], [1, 1], [2, 2], [0, 3], [1, 4], [2, 0]] {
            renderer.boundary();
            assert_eq!(
                [
                    status.playheads[0].load(Ordering::Acquire),
                    status.playheads[1].load(Ordering::Acquire),
                ],
                expected
            );
        }
    }
    #[test]
    fn offline_render_is_deterministic_and_finite() {
        let mut p = Project::new();
        p.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: Default::default(),
        });
        let a = render_offline(&p, 8_000, 2_000);
        let b = render_offline(&p, 8_000, 2_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(a.iter().any(|(l, _)| l.abs() > 0.001));
    }

    #[test]
    fn active_chord_and_lead_render_is_finite_at_44100_hz() {
        let mut project = Project::new();
        for track in &mut project.tracks[..3] {
            track.muted = true;
        }
        for step in [0, 4, 8, 12] {
            project.patterns[0].tracks[3].steps[step] = Some(StepEvent::BassNote {
                degree: 1,
                octave: 2,
                accent: false,
                slide: false,
                locks: Default::default(),
            });
            project.patterns[0].tracks[4].steps[step] = Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                locks: Default::default(),
            });
            project.patterns[0].tracks[5].steps[step] = Some(StepEvent::Note {
                degree: 5,
                octave: 4,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                locks: Default::default(),
            });
        }
        let rendered = render_offline(&project, 44_100, 44_100 * 2);
        assert!(
            rendered
                .iter()
                .all(|(left, right)| left.is_finite() && right.is_finite())
        );
    }

    #[test]
    fn accent_increases_drum_and_bass_peaks() {
        fn drum_peak(accent: bool) -> f32 {
            let mut project = Project::new();
            project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
                accent,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.boundary();
            (0..400)
                .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        }

        fn bass_peak(accent: bool) -> f32 {
            let mut project = Project::new();
            project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
                degree: 1,
                octave: 3,
                accent,
                slide: false,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.boundary();
            (0..400)
                .map(|_| {
                    Renderer::render_synth(
                        &mut renderer.synth[0],
                        renderer.sr,
                        &[0.0; ParameterId::ALL.len()],
                    )
                    .0
                })
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        }

        assert!(drum_peak(true) > drum_peak(false));
        assert!(bass_peak(true) > bass_peak(false));
    }

    #[test]
    fn active_bass_keeps_latched_accent_through_ties_and_project_edits() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: true,
            slide: false,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary();
        for _ in 0..40 {
            renderer.synth[0].accent_gain.next_value();
        }
        assert!(renderer.synth[0].accent_gain.next_value() > 1.3);

        renderer.boundary();
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            locks: Default::default(),
        });
        renderer.command(Audio::snapshot(&project));
        for _ in 0..40 {
            renderer.synth[0].accent_gain.next_value();
        }
        assert!(renderer.synth[0].accent_gain.next_value() > 1.3);
    }

    #[test]
    fn bass_slide_is_legato_and_reaches_pitch_in_sixty_milliseconds() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: true,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::BassNote {
            degree: 8,
            octave: 3,
            accent: false,
            slide: false,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary();
        for _ in 0..80 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        let stage_before_slide = renderer.synth[0].env.stage;
        let starting_frequency = renderer.synth[0].freq.next_value();

        renderer.boundary();
        assert_eq!(renderer.synth[0].env.stage, stage_before_slide);
        let first_frequency = renderer.synth[0].freq.next_value();
        assert!(first_frequency > starting_frequency);
        assert!(first_frequency < starting_frequency * 2.0);
        for _ in 1..480 {
            renderer.synth[0].freq.next_value();
        }
        let final_frequency = renderer.synth[0].freq.next_value();
        assert!((final_frequency - starting_frequency * 2.0).abs() < 0.01);
    }

    #[test]
    fn representative_groove_has_calibrated_rms_and_safe_peak() {
        let mut project = Project::new();
        for step in [0, 4, 8, 12] {
            project.patterns[0].tracks[0].steps[step] = Some(StepEvent::Trigger {
                accent: step == 0,
                locks: Default::default(),
            });
        }
        for step in [4, 12] {
            project.patterns[0].tracks[1].steps[step] = Some(StepEvent::Trigger {
                accent: true,
                locks: Default::default(),
            });
        }
        for step in 0..16 {
            project.patterns[0].tracks[2].steps[step] = Some(StepEvent::Trigger {
                accent: step == 6 || step == 14,
                locks: Default::default(),
            });
        }
        for (step, degree) in [
            (0, 1),
            (2, 1),
            (4, 1),
            (6, 3),
            (8, 5),
            (10, 5),
            (12, 4),
            (14, 3),
        ] {
            project.patterns[0].tracks[3].steps[step] = Some(StepEvent::BassNote {
                degree,
                octave: 2,
                accent: step == 8,
                slide: step == 4,
                locks: Default::default(),
            });
        }

        let rendered = render_offline(&project, 8_000, 32_000);
        let settled = &rendered[1_000..];
        let rms = (settled
            .iter()
            .map(|(left, right)| (left * left + right * right) * 0.5)
            .sum::<f32>()
            / settled.len() as f32)
            .sqrt();
        let rms_dbfs = 20.0 * rms.log10();
        let peak = settled.iter().fold(0.0_f32, |peak, (left, right)| {
            peak.max(left.abs()).max(right.abs())
        });
        assert!(
            (-19.0..=-12.0).contains(&rms_dbfs),
            "representative groove RMS was {rms_dbfs:.2} dBFS"
        );
        assert!(peak <= 10.0_f32.powf(-1.0 / 20.0) + 0.000_01);
    }

    #[test]
    fn full_reverb_send_preserves_the_attack_and_produces_an_audible_tail() {
        fn rms(samples: &[(f32, f32)]) -> f32 {
            (samples
                .iter()
                .map(|(l, r)| (l * l + r * r) * 0.5)
                .sum::<f32>()
                / samples.len() as f32)
                .sqrt()
        }

        let mut dry_project = Project::new();
        dry_project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: Default::default(),
        });
        let mut wet_project = dry_project.clone();
        wet_project.tracks[0].reverb_send = Percent::new(100).unwrap();

        let dry = render_offline(&dry_project, 8_000, 10_000);
        let wet = render_offline(&wet_project, 8_000, 10_000);
        assert_eq!(&dry[..160], &wet[..160]);

        let dry_tail = rms(&dry[6_000..10_000]);
        let wet_tail = rms(&wet[6_000..10_000]);
        assert!(
            wet_tail >= dry_tail * 4.0,
            "dry tail RMS {dry_tail}, wet tail RMS {wet_tail}"
        );
    }

    #[test]
    fn resume_triggers_the_next_step_immediately() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 48_000, status);
        renderer.command(AudioCommand::PlayPause);
        assert_eq!(renderer.clock.next_step, 0);
        renderer.next();
        assert_eq!(renderer.clock.next_step, 1);
        renderer.command(AudioCommand::PlayPause);
        for _ in 0..100 {
            renderer.next();
        }
        renderer.command(AudioCommand::PlayPause);
        renderer.next();
        assert_eq!(renderer.clock.next_step, 2);
    }

    #[test]
    fn drum_mixer_lock_reverts_on_the_following_boundary() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: ParameterLocks {
                level: Some(Percent::ZERO),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary();
        let locked = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        renderer.boundary();
        let restored = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(locked, 0.0);
        assert!(restored > 0.0001);
    }

    #[test]
    fn current_step_locks_are_latched_until_the_next_boundary() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps.resize(1, None);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            locks: ParameterLocks {
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary();
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));

        let mut edited = project.clone();
        let Some(StepEvent::BassNote { locks, .. }) =
            edited.patterns[0].tracks[3].steps[0].as_mut()
        else {
            panic!("expected Bass note")
        };
        locks.cutoff = Percent::new(80);
        renderer.command(Audio::snapshot(&edited));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));

        renderer.boundary();
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(80));
    }

    #[test]
    fn bass_tie_locks_inherit_source_note_and_allow_tie_overrides() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            locks: ParameterLocks {
                level: Percent::new(30),
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                resonance: Percent::new(70),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[3].steps[2] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary();
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, None);

        renderer.boundary();
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, Percent::new(70));

        renderer.boundary();
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, Percent::new(70));
    }

    #[test]
    fn wrapped_bass_tie_locks_inherit_from_wrapped_source_note() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps.resize(3, None);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[2] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            locks: ParameterLocks {
                cutoff: Percent::new(25),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        assert_eq!(renderer.locks_at(3, 0).cutoff, Percent::new(25));
    }

    #[test]
    fn fader_snapshot_ramps_an_active_drum_mixer_value_over_thirty_ms() {
        let mut project = Project::new();
        project.tracks[0].level = Percent::new(100).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary();
        for _ in 0..40 {
            renderer.drums[0].level.next_value();
        }

        project.tracks[0].level = Percent::ZERO;
        renderer.command(Audio::snapshot_with_smoothing(
            &project,
            ParameterSmoothing::Fader,
        ));
        let first = renderer.drums[0].level.next_value();
        assert!(first > 0.0 && first < 100.0);
        for _ in 1..240 {
            renderer.drums[0].level.next_value();
        }
        assert_eq!(renderer.drums[0].level.next_value(), 0.0);
    }

    #[test]
    fn synth_filter_mappings_match_the_specified_limits() {
        let project = AudioProject::from_project(&Project::new());
        let mut voice = SynthVoice::new(48_000.0);
        let mut locks = ParameterLocks {
            cutoff: Percent::new(0),
            resonance: Percent::new(0),
            ..Default::default()
        };
        Renderer::apply_synth_params(&project, 48_000.0, 3, locks, &mut voice, 240);
        for _ in 0..240 {
            voice.cutoff_percent.next_value();
            voice.resonance_percent.next_value();
        }
        assert!(
            (exp_map_f32(voice.cutoff_percent.next_value(), 20.0, 20_000.0) - 20.0).abs() < 0.001
        );
        assert!(
            (0.707 + voice.resonance_percent.next_value() / 100.0 * (10.0 - 0.707) - 0.707).abs()
                < 0.001
        );
        locks.cutoff = Percent::new(100);
        locks.resonance = Percent::new(100);
        Renderer::apply_synth_params(&project, 48_000.0, 3, locks, &mut voice, 240);
        for _ in 0..240 {
            voice.cutoff_percent.next_value();
            voice.resonance_percent.next_value();
        }
        assert!(
            (exp_map_f32(voice.cutoff_percent.next_value(), 20.0, 20_000.0) - 20_000.0).abs()
                < 0.01
        );
        assert!(
            (0.707 + voice.resonance_percent.next_value() / 100.0 * (10.0 - 0.707) - 10.0).abs()
                < 0.001
        );
    }

    #[test]
    fn chord_track_triggers_close_position_triads_and_alternates_voice_groups() {
        let mut project = Project::new();
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            locks: ParameterLocks {
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[4].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                resonance: Percent::new(70),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[4].steps[2] = Some(StepEvent::Note {
            degree: 4,
            octave: 3,
            accent: true,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);

        renderer.boundary();
        let first_group = renderer.chord.group;
        let frequencies = std::array::from_fn::<_, 3, _>(|voice| {
            renderer.chord.voices[first_group * CHORD_GROUP_SIZE + voice]
                .freq
                .next_value()
        });
        let expected = [48, 52, 55].map(|midi| 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0));
        for (actual, expected) in frequencies.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.001);
        }
        for voice in &renderer.chord.voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.locks.cutoff, Percent::new(20));
        }

        renderer.boundary();
        assert_eq!(renderer.chord.group, first_group);
        for voice in &renderer.chord.voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.locks.cutoff, Percent::new(20));
            assert_eq!(voice.locks.resonance, Percent::new(70));
        }
        renderer.boundary();
        assert_ne!(renderer.chord.group, first_group);
        for voice in &renderer.chord.voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.env.stage, crate::dsp::EnvStage::Release);
        }
        for voice in &renderer.chord.voices
            [renderer.chord.group * CHORD_GROUP_SIZE..renderer.chord.group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.env.stage, crate::dsp::EnvStage::Attack);
        }

        renderer.boundary();
        assert!(!renderer.chord.active);
    }

    #[test]
    fn chord_track_renders_four_note_shapes_with_overlap_capacity() {
        let mut project = Project::new();
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::SeventhRoot),
            arpeggio: ArpeggioConfig::default(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
        renderer.boundary();
        assert_eq!(renderer.chord.voice_count, 4);
        let group = renderer.chord.group;
        let expected = [48, 52, 55, 59];
        for (voice, midi) in expected.into_iter().enumerate() {
            let frequency = renderer.chord.voices[group * CHORD_GROUP_SIZE + voice]
                .freq
                .next_value();
            let expected = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            assert!((frequency - expected).abs() < 0.001);
        }
    }

    #[test]
    fn chord_reverb_send_survives_the_first_voice_group() {
        fn rms(samples: &[(f32, f32)]) -> f32 {
            (samples
                .iter()
                .map(|(l, r)| (l * l + r * r) * 0.5)
                .sum::<f32>()
                / samples.len() as f32)
                .sqrt()
        }

        let mut dry_project = Project::new();
        for (index, track) in dry_project.tracks.iter_mut().enumerate() {
            track.muted = index != 4;
        }
        dry_project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            locks: Default::default(),
        });
        let mut wet_project = dry_project.clone();
        wet_project.tracks[4].reverb_send = Percent::new(100).unwrap();

        let dry = render_offline(&dry_project, 8_000, 12_000);
        let wet = render_offline(&wet_project, 8_000, 12_000);
        let dry_tail = rms(&dry[8_000..12_000]);
        let wet_tail = rms(&wet[8_000..12_000]);
        assert!(
            wet_tail > dry_tail * 2.0,
            "dry chord tail RMS {dry_tail}, wet chord tail RMS {wet_tail}"
        );
    }

    #[test]
    fn sequence_lfo_freezes_on_pause_and_resets_on_stop() {
        let mut project = Project::new();
        project.tracks[0].lfos.level = Some(crate::model::LfoConfig {
            rate: crate::model::LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            },
            depth: Percent::new(50).unwrap(),
            ..Default::default()
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 1_000, status);
        renderer.command(AudioCommand::PlayPause);
        for _ in 0..20 {
            renderer.next();
        }
        let moving = renderer.lfo_offsets[0][ParameterId::Level as usize];
        assert!(moving.abs() > 0.01);
        renderer.command(AudioCommand::PlayPause);
        for _ in 0..20 {
            renderer.next();
        }
        assert_eq!(renderer.lfo_offsets[0][ParameterId::Level as usize], moving);
        renderer.command(AudioCommand::Stop);
        assert_eq!(renderer.lfo_offsets[0][ParameterId::Level as usize], 0.0);
    }

    #[test]
    fn lfo_offsets_clamp_around_effective_values() {
        assert_eq!(modulated_percent(90.0, 25.0), 100.0);
        assert_eq!(modulated_percent(10.0, -25.0), 0.0);
        assert_eq!(modulated_percent(40.0, 15.0), 55.0);
    }

    #[test]
    fn pitch_lfo_offset_maps_to_two_bipolar_semitones() {
        let base = 440.0;
        assert!((pitch_modulated_frequency(base, 0.0) - base).abs() < 0.0001);
        let up = pitch_modulated_frequency(base, 100.0);
        let down = pitch_modulated_frequency(base, -100.0);
        assert!((up - base * 2.0_f32.powf(2.0 / 12.0)).abs() < 0.0001);
        assert!((down - base * 2.0_f32.powf(-2.0 / 12.0)).abs() < 0.0001);
        assert!(up.is_finite() && down.is_finite());
    }

    #[test]
    fn arpeggio_orders_and_fractional_timing_are_fixed_width() {
        let expected = [
            (ArpeggioType::Up, [0, 1, 2, 0, 0, 0, 0, 0], 3),
            (ArpeggioType::Down, [2, 1, 0, 0, 0, 0, 0, 0], 3),
            (ArpeggioType::UpDown, [0, 1, 2, 1, 0, 0, 0, 0], 4),
            (ArpeggioType::DownUp, [2, 1, 0, 1, 0, 0, 0, 0], 4),
        ];
        for (kind, order, length) in expected {
            let mut state = ArpeggioState::default();
            state.reset(
                ChordShape::TriadRoot,
                kind,
                ArpeggioRate::Sixteenth,
                44_100.0,
                123,
            );
            assert_eq!(state.order, order);
            assert_eq!(state.order_len as usize, length);
        }
        let mut state = ArpeggioState::default();
        state.reset(
            ChordShape::TriadRoot,
            ArpeggioType::Up,
            ArpeggioRate::Sixteenth,
            44_100.0,
            123,
        );
        let interval = 44_100.0_f64 * 60.0 / 123.0 * 0.25;
        let mut elapsed = 0.0;
        let mut ticks = 0;
        for _ in 0..(interval.ceil() as usize * 3) {
            elapsed += 1.0;
            if state.tick(44_100.0, 123) {
                assert!((elapsed - interval * (ticks + 1) as f64).abs() <= 1.0);
                ticks += 1;
            }
        }
        assert_eq!(ticks, 3);
    }

    #[test]
    fn arpeggiated_chord_renders_finite_and_restarts_after_empty_step() {
        let mut project = Project::new();
        for (index, track) in project.tracks.iter_mut().enumerate() {
            track.muted = index != 4;
        }
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig {
                enabled: true,
                r#type: ArpeggioType::UpDown,
                rate: ArpeggioRate::ThirtySecond,
            },
            locks: Default::default(),
        });
        project.patterns[0].tracks[4].steps[1] = None;
        let output = render_offline(&project, 8_000, 8_000);
        assert!(
            output
                .iter()
                .all(|(left, right)| left.is_finite() && right.is_finite())
        );
    }
}
