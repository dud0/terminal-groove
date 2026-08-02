use crate::{
    dsp::{Adsr, Biquad, DcBlock, Delay, PolyBlepOsc, Reverb, Smoother, Svf, exp_map, safety},
    engine::{GateAction, StepClock, synth_action},
    model::{
        Globals, Instrument, MAX_STEP_COUNT, ParameterLocks, Percent, ProjectV2, StepEvent,
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

#[derive(Clone, Copy, Debug)]
struct AudioTrack {
    level: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    instrument: Instrument,
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
impl AudioProject {
    pub fn from_project(project: &ProjectV2) -> Self {
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
    ReplaceProject(AudioProject),
    Audition { track: u8, step: u8 },
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
    pub fn snapshot(project: &ProjectV2) -> AudioCommand {
        AudioCommand::ReplaceProject(AudioProject::from_project(project))
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
pub fn open(requested: Option<&str>, project: &ProjectV2) -> Result<Audio> {
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
    cutoff: Smoother,
    resonance: Smoother,
    filter_env: f32,
    active: bool,
    remaining: u32,
    level: Smoother,
    delay_send: Smoother,
    reverb_send: Smoother,
}

const DRUM_SILENCE: f32 = 0.0001;

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
            cutoff: Smoother::new(4000.0),
            resonance: Smoother::new(0.7),
            filter_env: 0.0,
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
        };
        r.configure_effects();
        r
    }
    fn configure_effects(&mut self) {
        self.clock.set_bpm(self.project.globals.tempo_bpm);
        self.delay.configure(
            self.project
                .globals
                .delay_division
                .samples(self.project.globals.tempo_bpm, self.sr as u32) as usize,
            self.project.globals.delay_feedback.normalized(),
        );
        self.reverb
            .set_time(self.project.globals.reverb_time_seconds);
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
                self.status.running.store(false, Ordering::Release);
                self.status.paused.store(false, Ordering::Release);
                for playhead in &self.status.playheads {
                    playhead.store(u8::MAX, Ordering::Release);
                }
            }
            AudioCommand::ReplaceProject(project) => {
                self.project = project;
                for (track, next) in self.next_steps.iter_mut().enumerate() {
                    if *next >= self.project.tracks[track].step_count as usize {
                        *next = 0;
                    }
                    let playhead = &self.status.playheads[track];
                    if playhead.load(Ordering::Acquire) >= self.project.tracks[track].step_count {
                        playhead.store(u8::MAX, Ordering::Release);
                    }
                }
                self.configure_effects();
                self.update_mutes(false);
            }
            AudioCommand::Audition { track, step } => self.audition(track as usize, step as usize),
        }
    }
    fn locks_at(&self, track: usize, step: usize) -> ParameterLocks {
        self.project.tracks[track].steps[step]
            .map(|e| *e.locks())
            .unwrap_or_default()
    }
    fn trigger_drum(&mut self, track: usize, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Instrument::Drum(p) = t.instrument else {
            return;
        };
        let tone = locks.tone.unwrap_or(p.tone).normalized();
        let decay = locks.decay.unwrap_or(p.decay).normalized();
        Self::start_drum_voice(&mut self.drums[track], track, tone, decay, self.sr);
    }
    fn trigger_preview_drum(&mut self, track: usize, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Instrument::Drum(p) = t.instrument else {
            return;
        };
        let tone = locks.tone.unwrap_or(p.tone).normalized();
        let decay = locks.decay.unwrap_or(p.decay).normalized();
        Self::start_drum_voice(&mut self.preview_drums[track], track, tone, decay, self.sr);
        let voice = &mut self.preview_drums[track];
        voice
            .level
            .set(locks.level.unwrap_or(t.level).normalized().powi(2), 0);
        voice
            .delay_send
            .set(locks.delay_send.unwrap_or(t.delay_send).normalized(), 0);
        voice
            .reverb_send
            .set(locks.reverb_send.unwrap_or(t.reverb_send).normalized(), 0);
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
    fn update_drum_mix(&mut self, track: usize, step: usize) {
        let t = self.project.tracks[track];
        let locks = self.locks_at(track, step);
        let smoothing = (self.sr * 0.005) as u32;
        let voice = &mut self.drums[track];
        voice.level.set(
            locks.level.unwrap_or(t.level).normalized().powi(2),
            smoothing,
        );
        voice.delay_send.set(
            locks.delay_send.unwrap_or(t.delay_send).normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks.reverb_send.unwrap_or(t.reverb_send).normalized(),
            smoothing,
        );
    }
    fn apply_synth_params(
        project: &AudioProject,
        sr: f32,
        track: usize,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
    ) {
        let t = project.tracks[track];
        let Instrument::Synth(p) = t.instrument else {
            return;
        };
        voice.wave = locks.waveform.unwrap_or(p.waveform);
        let cutoff = locks.cutoff.unwrap_or(p.cutoff).get();
        let smoothing = (sr * 0.005) as u32;
        voice.cutoff.set(
            exp_map(cutoff, 20.0, 20_000.0_f32.min(sr * 0.45)),
            smoothing,
        );
        voice.resonance.set(
            0.707 + locks.resonance.unwrap_or(p.resonance).normalized() * (10.0 - 0.707),
            smoothing,
        );
        voice.filter_env = locks
            .filter_envelope
            .unwrap_or(p.filter_envelope)
            .normalized();
        voice.env.configure(
            if locks.attack.unwrap_or(p.attack).get() == 0 {
                0.0
            } else {
                exp_map(locks.attack.unwrap_or(p.attack).get(), 0.001, 2.0)
            },
            exp_map(locks.decay.unwrap_or(p.decay).get(), 0.005, 3.0),
            locks.sustain.unwrap_or(p.sustain).normalized(),
            exp_map(locks.release.unwrap_or(p.release).get(), 0.005, 5.0),
        );
        voice.level.set(
            locks.level.unwrap_or(t.level).normalized().powi(2),
            smoothing,
        );
        voice.delay_send.set(
            locks.delay_send.unwrap_or(t.delay_send).normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks.reverb_send.unwrap_or(t.reverb_send).normalized(),
            smoothing,
        );
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
        Self::apply_synth_params(project, sr, track, locks, voice);
        voice.env.gate_on();
        voice.active = true;
    }
    fn audition(&mut self, track: usize, step: usize) {
        if track >= TRACK_COUNT || step >= self.project.tracks[track].step_count as usize {
            return;
        }
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
            Some(StepEvent::Tie { locks }) => {
                let Some(source) =
                    crate::model::tie_source(&t.steps[..t.step_count as usize], step)
                else {
                    return;
                };
                let Some(StepEvent::Note { degree, octave, .. }) = t.steps[source] else {
                    return;
                };
                (degree, octave, locks)
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
                self.update_drum_mix(track, step);
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
                        if let Some(StepEvent::Tie { locks }) = t.steps[step] {
                            Self::apply_synth_params(
                                &self.project,
                                self.sr,
                                track,
                                locks,
                                &mut self.synth[vi],
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
    fn render_synth(v: &mut SynthVoice, sr: f32) -> (f32, f32, f32) {
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
        let env = v.env.next_sample();
        let cutoff = (v.cutoff.next_value() * 2.0_f32.powf(env * v.filter_env * 6.0))
            .min(20_000.0_f32.min(sr * 0.45));
        let resonance = v.resonance.next_value();
        let level = v.level.next_value();
        let delay_send = v.delay_send.next_value();
        let reverb_send = v.reverb_send.next_value();
        (
            v.filter.lowpass(osc, cutoff, resonance, sr) * env * level * 0.22,
            delay_send,
            reverb_send,
        )
    }
    fn render_drum(voice: &mut DrumVoice, track: usize, sr: f32) -> (f32, f32, f32) {
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
        let sample = raw * voice.envelope.next_value() * voice.level.next_value() * 0.42;
        (
            sample,
            voice.delay_send.next_value(),
            voice.reverb_send.next_value(),
        )
    }
    fn next(&mut self) -> (f32, f32) {
        if self.playing && self.clock.tick().is_some() {
            self.boundary()
        }
        let mut dry = 0.0;
        let mut delay_in = 0.0;
        let mut reverb_in = 0.0;
        for i in 0..3 {
            let (x, delay_send, reverb_send) = Self::render_drum(&mut self.drums[i], i, self.sr);
            let gain = self.mute[i].next_value();
            dry += x * gain;
            delay_in += x * delay_send * gain;
            reverb_in += x * reverb_send * gain;
        }
        for i in 0..3 {
            let (x, delay_send, reverb_send) =
                Self::render_drum(&mut self.preview_drums[i], i, self.sr);
            dry += x;
            delay_in += x * delay_send;
            reverb_in += x * reverb_send;
        }
        for i in 0..3 {
            let (x, ds, rs) = Self::render_synth(&mut self.synth[i], self.sr);
            let gain = self.mute[i + 3].next_value();
            dry += x * gain;
            delay_in += x * ds * gain;
            reverb_in += x * rs * gain;
            let (x, ds, rs) = Self::render_synth(&mut self.preview[i], self.sr);
            dry += x;
            delay_in += x * ds;
            reverb_in += x * rs;
        }
        let (dl, dr) = self.delay.process(delay_in, delay_in);
        let (rl, rr) = self
            .reverb
            .process(reverb_in + dl * 0.25, reverb_in + dr * 0.25);
        let (l, r) = self
            .dc
            .process(dry + dl * 0.45 + rl * 0.35, dry + dr * 0.45 + rr * 0.35);
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

/// Deterministic, device-independent rendering used by tests and diagnostics.
pub fn render_offline(project: &ProjectV2, sample_rate: u32, frames: usize) -> Vec<(f32, f32)> {
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
        let mut p = ProjectV2::new();
        p.tracks[0].steps.resize(MAX_STEP_COUNT, None);
        let AudioCommand::ReplaceProject(s) = Audio::snapshot(&p) else {
            panic!()
        };
        assert_eq!(s.globals.tempo_bpm, 120);
        assert_eq!(s.tracks[0].step_count as usize, MAX_STEP_COUNT);
        assert_eq!(s.tracks[1].step_count, 16);
        assert!(s.tracks[0].steps.iter().all(Option::is_none));
    }

    #[test]
    fn renderer_reports_independent_track_playheads() {
        let mut project = ProjectV2::new();
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
        let mut p = ProjectV2::new();
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
    fn resume_triggers_the_next_step_immediately() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(
            AudioProject::from_project(&ProjectV2::new()),
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
        let mut project = ProjectV2::new();
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
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        renderer.boundary();
        let restored = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(locked, 0.0);
        assert!(restored > 0.0001);
    }

    #[test]
    fn synth_filter_mappings_match_the_specified_limits() {
        let project = AudioProject::from_project(&ProjectV2::new());
        let mut voice = SynthVoice::new(48_000.0);
        let mut locks = ParameterLocks {
            cutoff: Percent::new(0),
            resonance: Percent::new(0),
            ..Default::default()
        };
        Renderer::apply_synth_params(&project, 48_000.0, 3, locks, &mut voice);
        for _ in 0..240 {
            voice.cutoff.next_value();
            voice.resonance.next_value();
        }
        assert!((voice.cutoff.next_value() - 20.0).abs() < 0.001);
        assert!((voice.resonance.next_value() - 0.707).abs() < 0.001);
        locks.cutoff = Percent::new(100);
        locks.resonance = Percent::new(100);
        Renderer::apply_synth_params(&project, 48_000.0, 3, locks, &mut voice);
        for _ in 0..240 {
            voice.cutoff.next_value();
            voice.resonance.next_value();
        }
        assert!((voice.cutoff.next_value() - 20_000.0).abs() < 0.01);
        assert!((voice.resonance.next_value() - 10.0).abs() < 0.001);
    }
}
