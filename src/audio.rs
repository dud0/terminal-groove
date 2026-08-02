use crate::{
    dsp::{
        Adsr, Biquad, DcBlock, Delay, Lfo, PolyBlepOsc, Reverb, Smoother, Svf, exp_map_f32, safety,
    },
    engine::{GateAction, StepClock, synth_action},
    model::{
        Globals, Instrument, LfoAssignments, MAX_STEP_COUNT, ParameterId, ParameterLocks, Percent,
        ProjectV3, StepEvent, TRACK_COUNT, Waveform,
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

#[derive(Clone, Copy, Debug)]
struct AudioTrack {
    level: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    instrument: Instrument,
    lfos: LfoAssignments,
    steps: [Option<StepEvent>; MAX_STEP_COUNT],
    step_count: u8,
    input_degree: u8,
    input_octave: u8,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioProject {
    globals: Globals,
    tracks: [AudioTrack; TRACK_COUNT],
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
    pub fn from_project(project: &ProjectV3) -> Self {
        Self {
            globals: project.globals,
            tracks: std::array::from_fn(|i| {
                let t = &project.tracks[i];
                AudioTrack {
                    level: t.level,
                    muted: t.muted,
                    delay_send: t.delay_send,
                    reverb_send: t.reverb_send,
                    instrument: t.instrument,
                    lfos: t.lfos,
                    steps: std::array::from_fn(|s| t.steps.get(s).copied().flatten()),
                    step_count: t.steps.len() as u8,
                    input_degree: t.input_degree.unwrap_or(1),
                    input_octave: t.input_octave.unwrap_or(3),
                }
            }),
        }
    }
}

#[allow(clippy::large_enum_variant)] // fixed inline snapshot keeps callback messages allocation-free
#[derive(Clone, Copy, Debug)]
pub enum AudioCommand {
    PlayPause,
    Stop,
    ReplaceProject {
        project: AudioProject,
        smoothing: ParameterSmoothing,
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
}
impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            playheads: std::array::from_fn(|_| AtomicU8::new(u8::MAX)),
            failed: AtomicBool::new(false),
            non_finite: AtomicBool::new(false),
        }
    }
}
pub struct Audio {
    pub stream: Stream,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    producer: Producer<AudioCommand>,
}
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("audio command queue full")]
pub struct QueueFull;
impl Audio {
    pub fn send(&mut self, command: AudioCommand) -> Result<(), QueueFull> {
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| QueueFull)
    }
    pub fn available_commands(&self) -> usize {
        self.producer.slots()
    }
    pub fn snapshot(project: &ProjectV3) -> AudioCommand {
        Self::snapshot_with_smoothing(project, ParameterSmoothing::Default)
    }
    pub fn snapshot_with_smoothing(
        project: &ProjectV3,
        smoothing: ParameterSmoothing,
    ) -> AudioCommand {
        AudioCommand::ReplaceProject {
            project: AudioProject::from_project(project),
            smoothing,
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
pub fn open(requested: Option<&str>, project: &ProjectV3) -> Result<Audio> {
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
    let initial = AudioProject::from_project(project);
    let stream = match format {
        SampleFormat::F32 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new(initial,sr,rs); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[f32],_|render(out,channels,&mut r,&mut c,|x|x),move |_|mark_failed(&s),None) }
        SampleFormat::I16 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new(initial,sr,rs); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[i16],_|render(out,channels,&mut r,&mut c,|x|(x.clamp(-1.,1.)*32767.) as i16),move |_|mark_failed(&s),None) }
        SampleFormat::U16 => { let s=status.clone(); let rs=status.clone(); let mut r=Renderer::new(initial,sr,rs); let mut c=consumer; device.build_output_stream(&config,move|out:&mut[u16],_|render(out,channels,&mut r,&mut c,|x|((x.clamp(-1.,1.)*0.5+0.5)*65535.) as u16),move |_|mark_failed(&s),None) }
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
    })
}

fn overlay_locks(target: &mut ParameterLocks, overlay: ParameterLocks) {
    if overlay.level.is_some() {
        target.level = overlay.level;
    }
    if overlay.delay_send.is_some() {
        target.delay_send = overlay.delay_send;
    }
    if overlay.reverb_send.is_some() {
        target.reverb_send = overlay.reverb_send;
    }
    if overlay.tone.is_some() {
        target.tone = overlay.tone;
    }
    if overlay.decay.is_some() {
        target.decay = overlay.decay;
    }
    if overlay.waveform.is_some() {
        target.waveform = overlay.waveform;
    }
    if overlay.cutoff.is_some() {
        target.cutoff = overlay.cutoff;
    }
    if overlay.resonance.is_some() {
        target.resonance = overlay.resonance;
    }
    if overlay.filter_envelope.is_some() {
        target.filter_envelope = overlay.filter_envelope;
    }
    if overlay.attack.is_some() {
        target.attack = overlay.attack;
    }
    if overlay.sustain.is_some() {
        target.sustain = overlay.sustain;
    }
    if overlay.release.is_some() {
        target.release = overlay.release;
    }
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
    env: Adsr,
    filter: Svf,
    freq: f32,
    wave: Waveform,
    cutoff_percent: Smoother,
    resonance_percent: Smoother,
    filter_env_percent: Smoother,
    locks: ParameterLocks,
    active: bool,
    remaining: u32,
    level: Smoother,
    delay_send: Smoother,
    reverb_send: Smoother,
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
        self.peak = 75.0 + tone * 145.0;
        self.settled = 38.0 + tone * 20.0;
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
    filter: Biquad,
    noise: u32,
    tone: f32,
    level: Smoother,
    delay_send: Smoother,
    reverb_send: Smoother,
    locks: ParameterLocks,
}
impl DrumVoice {
    fn new(seed: u32) -> Self {
        Self {
            envelope: DrumEnvelope::new(),
            kick_pitch: KickPitchEnvelope::new(),
            phase: 0.0,
            filter: Biquad::new(),
            noise: seed,
            tone: 0.5,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
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
            env: Adsr::new(sr),
            filter: Default::default(),
            freq: 110.0,
            wave: Waveform::Saw,
            cutoff_percent: Smoother::new(65.0),
            resonance_percent: Smoother::new(10.0),
            filter_env_percent: Smoother::new(25.0),
            locks: ParameterLocks::default(),
            active: false,
            remaining: 0,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
        }
    }
}

struct Renderer {
    project: AudioProject,
    clock: StepClock,
    next_steps: [usize; TRACK_COUNT],
    playing: bool,
    sr: f32,
    status: Arc<AudioStatus>,
    drums: [DrumVoice; 3],
    preview_drums: [DrumVoice; 3],
    synth: [SynthVoice; 3],
    preview: [SynthVoice; 3],
    delay: Delay,
    reverb: Reverb,
    dc: DcBlock,
    mute: [Smoother; TRACK_COUNT],
    lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
}
impl Renderer {
    fn new(project: AudioProject, sr: u32, status: Arc<AudioStatus>) -> Self {
        let mut r = Self {
            project,
            clock: StepClock::new(sr, project.globals.tempo_bpm),
            next_steps: [0; TRACK_COUNT],
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
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            dc: Default::default(),
            mute: std::array::from_fn(|i| Smoother::new((!project.tracks[i].muted) as u8 as f32)),
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
                self.delay.clear();
                self.reverb.clear();
                self.reset_lfos();
                self.status.running.store(false, Ordering::Release);
                self.status.paused.store(false, Ordering::Release);
                for playhead in &self.status.playheads {
                    playhead.store(u8::MAX, Ordering::Release);
                }
            }
            AudioCommand::ReplaceProject { project, smoothing } => {
                self.reconcile_lfos(&project);
                self.project = project;
                let smoothing_samples = smoothing.samples(self.sr);
                for (track, next) in self.next_steps.iter_mut().enumerate() {
                    if *next >= self.project.tracks[track].step_count as usize {
                        *next = 0;
                    }
                    let playhead = &self.status.playheads[track];
                    if playhead.load(Ordering::Acquire) >= self.project.tracks[track].step_count {
                        playhead.store(u8::MAX, Ordering::Release);
                    }
                }
                self.configure_effects(smoothing_samples);
                self.update_mutes(false);
                self.refresh_active_parameters(smoothing_samples);
            }
            AudioCommand::Audition { track, step } => self.audition(track as usize, step as usize),
        }
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
        let t = self.project.tracks[track];
        let Some(event) = t.steps[step] else {
            return ParameterLocks::default();
        };
        let mut locks = *event.locks();
        if let StepEvent::Tie { .. } = event {
            if let Some(source) = crate::model::tie_source(&t.steps[..t.step_count as usize], step)
            {
                if let Some(StepEvent::Note {
                    locks: source_locks,
                    ..
                }) = t.steps[source]
                {
                    locks = source_locks;
                    let mut i = (source + 1) % t.step_count as usize;
                    while i != step {
                        if let Some(StepEvent::Tie { locks: tie_locks }) = t.steps[i] {
                            overlay_locks(&mut locks, tie_locks);
                        }
                        i = (i + 1) % t.step_count as usize;
                    }
                    overlay_locks(&mut locks, *event.locks());
                }
            }
        }
        locks
    }
    fn trigger_drum(&mut self, track: usize, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Instrument::Drum(p) = t.instrument else {
            return;
        };
        let tone = modulated_percent(
            locks.tone.unwrap_or(p.tone).get() as f32,
            self.lfo_offsets[track][ParameterId::Tone as usize],
        ) / 100.0;
        let decay = modulated_percent(
            locks.decay.unwrap_or(p.decay).get() as f32,
            self.lfo_offsets[track][ParameterId::Decay as usize],
        ) / 100.0;
        Self::start_drum_voice(&mut self.drums[track], track, tone, decay, self.sr);
    }
    fn trigger_preview_drum(&mut self, track: usize, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Instrument::Drum(p) = t.instrument else {
            return;
        };
        let tone = modulated_percent(
            locks.tone.unwrap_or(p.tone).get() as f32,
            self.preview_lfo_offsets[track][ParameterId::Tone as usize],
        ) / 100.0;
        let decay = modulated_percent(
            locks.decay.unwrap_or(p.decay).get() as f32,
            self.preview_lfo_offsets[track][ParameterId::Decay as usize],
        ) / 100.0;
        Self::start_drum_voice(&mut self.preview_drums[track], track, tone, decay, self.sr);
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
        tone: f32,
        decay_control: f32,
        sr: f32,
    ) {
        let decay = match track {
            0 => 0.08 + decay_control * 0.75,
            1 => 0.05 + decay_control * 0.5,
            _ => 0.025 + decay_control * 0.32,
        };
        let (attack, peak) = match track {
            0 => (0.004, 1.2),
            1 => (0.001, 0.85),
            _ => (0.001, 0.55),
        };
        voice.tone = tone;
        voice.envelope.trigger(peak, attack, decay, sr);
        match track {
            0 => voice.kick_pitch.trigger(tone, decay, sr),
            1 => voice
                .filter
                .set_bandpass(800.0 + tone * 5200.0, 0.6 + tone * 5.0, sr),
            _ => voice.filter.set_highpass(2800.0 + tone * 9000.0, 1.2, sr),
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
        let Instrument::Synth(p) = t.instrument else {
            return;
        };
        voice.wave = locks.waveform.unwrap_or(p.waveform);
        voice
            .cutoff_percent
            .set(locks.cutoff.unwrap_or(p.cutoff).get() as f32, smoothing);
        voice.resonance_percent.set(
            locks.resonance.unwrap_or(p.resonance).get() as f32,
            smoothing,
        );
        voice.filter_env_percent.set(
            locks.filter_envelope.unwrap_or(p.filter_envelope).get() as f32,
            smoothing,
        );
        voice.env.configure_percent(
            locks.attack.unwrap_or(p.attack).get(),
            locks.decay.unwrap_or(p.decay).get(),
            locks.sustain.unwrap_or(p.sustain).get(),
            locks.release.unwrap_or(p.release).get(),
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
        voice.locks = locks;
    }
    fn configure_synth_voice(
        project: &AudioProject,
        sr: f32,
        track: usize,
        degree: u8,
        octave: u8,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
    ) {
        let midi = 12 * (octave as i32 + 1)
            + project.globals.key.semitone()
            + project.globals.scale.offsets()[degree as usize - 1];
        voice.freq = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
        Self::apply_synth_params(
            project,
            sr,
            track,
            locks,
            voice,
            ParameterSmoothing::Default.samples(sr),
        );
        voice.env.gate_on();
        voice.active = true;
    }
    fn active_step_locks(&self, track: usize) -> Option<ParameterLocks> {
        if !self.playing {
            return None;
        }
        let count = self.project.tracks[track].step_count as usize;
        let step = (self.next_steps[track] + count - 1) % count;
        Some(self.locks_at(track, step))
    }
    fn refresh_active_parameters(&mut self, smoothing: u32) {
        for track in 0..3 {
            let params = self.project.tracks[track];
            let locks = self
                .active_step_locks(track)
                .unwrap_or(self.drums[track].locks);
            Self::apply_drum_mix(&mut self.drums[track], params, locks, smoothing);
            let locks = self.preview_drums[track].locks;
            Self::apply_drum_mix(&mut self.preview_drums[track], params, locks, smoothing);
        }
        for track in 3..TRACK_COUNT {
            let index = track - 3;
            if self.synth[index].active {
                let locks = self
                    .active_step_locks(track)
                    .unwrap_or(self.synth[index].locks);
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
    }
    fn audition(&mut self, track: usize, step: usize) {
        if track >= TRACK_COUNT || step >= self.project.tracks[track].step_count as usize {
            return;
        }
        self.reset_preview_lfos(track);
        if track < 3 {
            self.trigger_preview_drum(track, self.locks_at(track, step));
            return;
        }
        let t = self.project.tracks[track];
        let (degree, octave, locks) = match t.steps[step] {
            Some(StepEvent::Note {
                degree,
                octave,
                locks,
            }) => (degree, octave, locks),
            Some(StepEvent::Tie { .. }) => {
                let Some(source) =
                    crate::model::tie_source(&t.steps[..t.step_count as usize], step)
                else {
                    return;
                };
                let Some(StepEvent::Note { degree, octave, .. }) = t.steps[source] else {
                    return;
                };
                (degree, octave, self.locks_at(track, step))
            }
            _ => (t.input_degree, t.input_octave, ParameterLocks::default()),
        };
        let v = &mut self.preview[track - 3];
        Self::configure_synth_voice(&self.project, self.sr, track, degree, octave, locks, v);
        v.remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
    }
    fn boundary(&mut self) {
        for track in 0..TRACK_COUNT {
            let step = self.next_steps[track];
            self.status.playheads[track].store(step as u8, Ordering::Release);
            let t = self.project.tracks[track];
            if track < 3 {
                self.update_drum_mix(track, step, ParameterSmoothing::Default.samples(self.sr));
                if let Some(StepEvent::Trigger { locks }) = t.steps[step] {
                    self.trigger_drum(track, locks);
                }
            } else {
                let vi = track - 3;
                match synth_action(
                    &t.steps[..t.step_count as usize],
                    step,
                    self.synth[vi].active,
                ) {
                    GateAction::Trigger { degree, octave } => {
                        let locks = self.locks_at(track, step);
                        let v = &mut self.synth[vi];
                        Self::configure_synth_voice(
                            &self.project,
                            self.sr,
                            track,
                            degree,
                            octave,
                            locks,
                            v,
                        );
                    }
                    GateAction::Hold => {
                        if matches!(t.steps[step], Some(StepEvent::Tie { .. })) {
                            let locks = self.locks_at(track, step);
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
                    GateAction::Release => {
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
                    }
                    GateAction::None => {}
                }
            }
            self.next_steps[track] = (step + 1) % t.step_count as usize;
        }
    }
    fn render_synth(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        if v.remaining > 0 {
            v.remaining -= 1;
            if v.remaining == 0 {
                v.env.gate_off();
                v.active = false;
            }
        }
        let osc = match v.wave {
            Waveform::Saw => v.osc.next_saw(v.freq, sr),
            Waveform::Square => v.osc.next_square(v.freq, sr),
        };
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
        let cutoff = (exp_map_f32(cutoff_percent, 20.0, 20_000.0_f32.min(sr * 0.45))
            * 2.0_f32.powf(env * filter_env * 6.0))
        .min(20_000.0_f32.min(sr * 0.45));
        let resonance_percent = modulated_percent(
            v.resonance_percent.next_value(),
            offsets[ParameterId::Resonance as usize],
        );
        let resonance = 0.707 + resonance_percent / 100.0 * (10.0 - 0.707);
        let level_percent =
            modulated_percent(v.level.next_value(), offsets[ParameterId::Level as usize]);
        let level = (level_percent / 100.0).powi(2);
        let delay_send = v.delay_send.next_value();
        let reverb_send = v.reverb_send.next_value();
        (
            v.filter.lowpass(osc, cutoff, resonance, sr) * env * level * 0.22,
            delay_send,
            reverb_send,
        )
    }
    fn render_drum(
        voice: &mut DrumVoice,
        track: usize,
        sr: f32,
        level_offset: f32,
    ) -> (f32, f32, f32) {
        let raw = match track {
            0 => {
                let hz = voice.kick_pitch.next_value();
                let sample = (TAU * voice.phase).sin();
                voice.phase = (voice.phase + hz / sr).fract();
                sample
            }
            1 => {
                let hz = 145.0 + voice.tone * 170.0;
                let triangle = 1.0 - 4.0 * (voice.phase - 0.5).abs();
                voice.phase = (voice.phase + hz / sr).fract();
                let noise = voice.noise();
                voice.filter.process(noise) * 0.75 + triangle * 0.35
            }
            _ => {
                let noise = voice.noise();
                voice.filter.process(noise)
            }
        };
        let level = modulated_percent(voice.level.next_value(), level_offset) / 100.0;
        let sample = raw * voice.envelope.next_value() * level.powi(2) * 0.42;
        (
            sample,
            voice.delay_send.next_value(),
            voice.reverb_send.next_value(),
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
        let mut dry = 0.0;
        let mut delay_in = 0.0;
        let mut reverb_in = 0.0;
        for i in 0..3 {
            let (x, delay_send, reverb_send) = Self::render_drum(
                &mut self.drums[i],
                i,
                self.sr,
                self.lfo_offsets[i][ParameterId::Level as usize],
            );
            let gain = self.mute[i].next_value();
            dry += x * gain;
            delay_in += x * delay_send * gain;
            reverb_in += x * reverb_send * gain;
        }
        for i in 0..3 {
            let (x, delay_send, reverb_send) = Self::render_drum(
                &mut self.preview_drums[i],
                i,
                self.sr,
                self.preview_lfo_offsets[i][ParameterId::Level as usize],
            );
            dry += x;
            delay_in += x * delay_send;
            reverb_in += x * reverb_send;
        }
        for i in 0..3 {
            let (x, ds, rs) =
                Self::render_synth(&mut self.synth[i], self.sr, &self.lfo_offsets[i + 3]);
            let gain = self.mute[i + 3].next_value();
            dry += x * gain;
            delay_in += x * ds * gain;
            reverb_in += x * rs * gain;
            let (x, ds, rs) = Self::render_synth(
                &mut self.preview[i],
                self.sr,
                &self.preview_lfo_offsets[i + 3],
            );
            dry += x;
            delay_in += x * ds;
            reverb_in += x * rs;
        }
        let (dl, dr) = self.delay.process(delay_in, delay_in);
        let (rl, rr) = self
            .reverb
            .process(reverb_in + dl * 0.25, reverb_in + dr * 0.25);
        let (l, r) = self.dc.process(
            dry + dl * 0.45 + rl * REVERB_RETURN_GAIN,
            dry + dr * 0.45 + rr * REVERB_RETURN_GAIN,
        );
        if !(l.is_finite() && r.is_finite()) {
            self.status.non_finite.store(true, Ordering::Release);
            return (0.0, 0.0);
        }
        (safety(l), safety(r))
    }
}

fn render<T: Copy, F: Fn(f32) -> T>(
    out: &mut [T],
    channels: usize,
    renderer: &mut Renderer,
    commands: &mut Consumer<AudioCommand>,
    convert: F,
) {
    while let Ok(c) = commands.pop() {
        renderer.command(c)
    }
    for frame in out.chunks_mut(channels) {
        let (l, r) = renderer.next();
        if !frame.is_empty() {
            frame[0] = convert(if channels == 1 {
                (l + r) * std::f32::consts::FRAC_1_SQRT_2
            } else {
                l
            })
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

/// Deterministic, device-independent rendering used by tests and diagnostics.
pub fn render_offline(project: &ProjectV3, sample_rate: u32, frames: usize) -> Vec<(f32, f32)> {
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
    fn kick_pitch_uses_browser_peak_and_settled_mappings() {
        let mut pitch = KickPitchEnvelope::new();
        pitch.trigger(0.5, 0.455, 10_000.0);
        let at_peak = (0..=15).map(|_| pitch.next_value()).last().unwrap();
        assert!((at_peak - 147.5).abs() < 0.001);
        let settled = (16..=1_300).map(|_| pitch.next_value()).last().unwrap();
        assert!((settled - 48.0).abs() < 0.001);
    }
    #[test]
    fn snapshot_contains_all_steps_without_heap_backed_commands() {
        let mut p = ProjectV3::new();
        p.tracks[0].steps.resize(MAX_STEP_COUNT, None);
        let AudioCommand::ReplaceProject {
            project: s,
            smoothing,
        } = Audio::snapshot(&p)
        else {
            panic!()
        };
        assert_eq!(smoothing, ParameterSmoothing::Default);
        assert_eq!(s.globals.tempo_bpm, 120);
        assert_eq!(s.tracks[0].step_count as usize, MAX_STEP_COUNT);
        assert_eq!(s.tracks[1].step_count, 16);
        assert!(s.tracks[0].steps.iter().all(Option::is_none));
    }

    #[test]
    fn renderer_reports_independent_track_playheads() {
        let mut project = ProjectV3::new();
        project.tracks[0].steps.resize(3, None);
        project.tracks[1].steps.resize(5, None);
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
        let mut p = ProjectV3::new();
        p.tracks[0].steps[0] = Some(StepEvent::Trigger {
            locks: Default::default(),
        });
        let a = render_offline(&p, 8_000, 2_000);
        let b = render_offline(&p, 8_000, 2_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(a.iter().any(|(l, _)| l.abs() > 0.001));
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

        let mut dry_project = ProjectV3::new();
        dry_project.tracks[0].steps[0] = Some(StepEvent::Trigger {
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
        let mut renderer = Renderer::new(
            AudioProject::from_project(&ProjectV3::new()),
            48_000,
            status,
        );
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
        let mut project = ProjectV3::new();
        project.tracks[0].steps[0] = Some(StepEvent::Trigger {
            locks: ParameterLocks {
                level: Some(Percent::ZERO),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary();
        let locked = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        renderer.boundary();
        let restored = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(locked, 0.0);
        assert!(restored > 0.0001);
    }

    #[test]
    fn synth_tie_locks_inherit_source_note_and_allow_tie_overrides() {
        let mut project = ProjectV3::new();
        project.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            locks: ParameterLocks {
                level: Percent::new(30),
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        project.tracks[3].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                resonance: Percent::new(70),
                ..Default::default()
            },
        });
        project.tracks[3].steps[2] = Some(StepEvent::Tie {
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
    fn wrapped_synth_tie_locks_inherit_from_wrapped_source_note() {
        let mut project = ProjectV3::new();
        project.tracks[3].steps.resize(3, None);
        project.tracks[3].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        project.tracks[3].steps[2] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
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
        let mut project = ProjectV3::new();
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
        let project = AudioProject::from_project(&ProjectV3::new());
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
    fn sequence_lfo_freezes_on_pause_and_resets_on_stop() {
        let mut project = ProjectV3::new();
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
}
