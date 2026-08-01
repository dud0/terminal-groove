use crate::{
    dsp::{Adsr, DcBlock, Delay, PolyBlepOsc, Reverb, Smoother, Svf, exp_map, safety},
    engine::{GateAction, StepClock, synth_action},
    model::{
        Globals, Instrument, ParameterLocks, Percent, ProjectV1, STEP_COUNT, StepEvent,
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
    steps: [Option<StepEvent>; STEP_COUNT],
    input_degree: u8,
    input_octave: u8,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioProject {
    globals: Globals,
    tracks: [AudioTrack; TRACK_COUNT],
}
impl AudioProject {
    pub fn from_project(project: &ProjectV1) -> Self {
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
                    steps: std::array::from_fn(|s| t.steps[s]),
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
    pub playhead: AtomicU8,
    pub failed: AtomicBool,
    pub non_finite: AtomicBool,
}
impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            playhead: AtomicU8::new(u8::MAX),
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
    pub fn snapshot(project: &ProjectV1) -> AudioCommand {
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
        let matches = host
            .output_devices()
            .context("could not enumerate audio outputs")?
            .filter(|d| d.name().ok().as_deref() == Some(name))
            .collect::<Vec<_>>();
        match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => bail!(
                "audio output `{name}` was not found; run `terminal-groove --list-audio-devices`"
            ),
            n => bail!(
                "audio output name `{name}` is ambiguous ({n} matches); use an exact unique name from `terminal-groove --list-audio-devices`"
            ),
        }
    } else {
        host.default_output_device()
            .context("no default audio output; run `terminal-groove --list-audio-devices`")
    }
}

#[allow(deprecated)]
pub fn open(requested: Option<&str>, project: &ProjectV1) -> Result<Audio> {
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
    status.playhead.store(u8::MAX, Ordering::Release);
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
    playing: bool,
    sr: f32,
    status: Arc<AudioStatus>,
    drum_amp: [f32; 3],
    drum_phase: [f32; 3],
    drum_tone: [f32; 3],
    drum_decay: [f32; 3],
    drum_mix: [[f32; 3]; 3],
    preview_drum_amp: [f32; 3],
    preview_drum_phase: [f32; 3],
    preview_drum_tone: [f32; 3],
    preview_drum_decay: [f32; 3],
    preview_drum_mix: [[f32; 3]; 3],
    noise: [u32; 3],
    snare_phase2: f32,
    snare_noise_filter: Svf,
    hat_osc: [PolyBlepOsc; 6],
    synth: [SynthVoice; 3],
    preview: [SynthVoice; 3],
    delay: Delay,
    reverb: Reverb,
    dc: DcBlock,
}
impl Renderer {
    fn new(project: AudioProject, sr: u32, status: Arc<AudioStatus>) -> Self {
        let mut r = Self {
            project,
            clock: StepClock::new(sr, project.globals.tempo_bpm),
            playing: false,
            sr: sr as f32,
            status,
            drum_amp: [0.0; 3],
            drum_phase: [0.0; 3],
            drum_tone: [0.5; 3],
            drum_decay: [0.5; 3],
            drum_mix: [[0.0; 3]; 3],
            noise: [0x1234_abcd, 0x9137_2468, 0xdead_beef],
            preview_drum_amp: [0.0; 3],
            preview_drum_phase: [0.0; 3],
            preview_drum_tone: [0.5; 3],
            preview_drum_decay: [0.5; 3],
            preview_drum_mix: [[0.0; 3]; 3],
            snare_phase2: 0.0,
            snare_noise_filter: Default::default(),
            hat_osc: std::array::from_fn(|_| Default::default()),
            synth: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            preview: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            dc: Default::default(),
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
    fn command(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::PlayPause => {
                self.playing = !self.playing;
                self.status.running.store(self.playing, Ordering::Release);
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
                self.drum_amp = [0.0; 3];
                self.preview_drum_amp = [0.0; 3];
                for v in self.synth.iter_mut().chain(self.preview.iter_mut()) {
                    v.env.gate_off();
                    v.active = false;
                    v.remaining = 0;
                }
                self.delay.clear();
                self.reverb.clear();
                self.status.running.store(false, Ordering::Release);
                self.status.playhead.store(u8::MAX, Ordering::Release);
            }
            AudioCommand::ReplaceProject(project) => {
                self.project = project;
                self.configure_effects();
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
        self.drum_amp[track] = 1.0;
        self.drum_phase[track] = 0.0;
        if track == 1 {
            self.snare_phase2 = 0.0;
        }
        self.drum_tone[track] = locks.tone.unwrap_or(p.tone).normalized();
        self.drum_decay[track] = locks.decay.unwrap_or(p.decay).normalized();
        self.drum_mix[track] = [
            locks.level.unwrap_or(t.level).normalized().powi(2),
            locks.delay_send.unwrap_or(t.delay_send).normalized(),
            locks.reverb_send.unwrap_or(t.reverb_send).normalized(),
        ];
    }
    fn trigger_preview_drum(&mut self, track: usize, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Instrument::Drum(p) = t.instrument else {
            return;
        };
        self.preview_drum_amp[track] = 1.0;
        self.preview_drum_phase[track] = 0.0;
        self.preview_drum_tone[track] = locks.tone.unwrap_or(p.tone).normalized();
        self.preview_drum_decay[track] = locks.decay.unwrap_or(p.decay).normalized();
        self.preview_drum_mix[track] = [
            locks.level.unwrap_or(t.level).normalized().powi(2),
            locks.delay_send.unwrap_or(t.delay_send).normalized(),
            locks.reverb_send.unwrap_or(t.reverb_send).normalized(),
        ];
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
        voice
            .cutoff
            .set(exp_map(cutoff, 40.0, (sr * 0.42).max(41.0)), smoothing);
        voice.resonance.set(
            0.55 + locks.resonance.unwrap_or(p.resonance).normalized() * 11.0,
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
        if track >= TRACK_COUNT || step >= STEP_COUNT {
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
                let Some(source) = crate::model::tie_source(&t.steps, step) else {
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
    fn boundary(&mut self, step: usize) {
        self.status.playhead.store(step as u8, Ordering::Release);
        for track in 0..TRACK_COUNT {
            let t = self.project.tracks[track];
            if track < 3 {
                if let Some(StepEvent::Trigger { locks }) = t.steps[step] {
                    self.trigger_drum(track, locks);
                }
            } else {
                let vi = track - 3;
                match synth_action(&t.steps, step, self.synth[vi].active) {
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
        }
    }
    fn noise(&mut self, i: usize) -> f32 {
        let mut x = self.noise[i];
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise[i] = x;
        x as i32 as f32 / i32::MAX as f32
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
        let cutoff = (v.cutoff.next_value() * (1.0 + env * v.filter_env * 7.0)).min(sr * 0.45);
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
    fn next(&mut self) -> (f32, f32) {
        if self.playing {
            if let Some(step) = self.clock.tick() {
                self.boundary(step)
            }
        }
        let mut dry = 0.0;
        let mut delay_in = 0.0;
        let mut reverb_in = 0.0;
        for i in 0..3 {
            let tone = self.drum_tone[i];
            let hz = match i {
                0 => 35.0 + tone * 45.0 + self.drum_amp[i] * (80.0 + tone * 100.0),
                1 => 120.0 + tone * 180.0,
                _ => 4500.0 + tone * 6500.0,
            };
            self.drum_phase[i] = (self.drum_phase[i] + hz / self.sr).fract();
            let noise = self.noise(i);
            let raw = match i {
                0 => (TAU * self.drum_phase[i]).sin() + noise * 0.12 * self.drum_amp[i].powi(8),
                1 => {
                    self.snare_phase2 = (self.snare_phase2 + (hz * 1.47) / self.sr).fract();
                    let n = self.snare_noise_filter.lowpass(
                        noise,
                        2500.0 + tone * 7000.0,
                        0.8,
                        self.sr,
                    );
                    0.22 * (TAU * self.drum_phase[i]).sin()
                        + 0.18 * (TAU * self.snare_phase2).sin()
                        + 0.60 * n
                }
                _ => {
                    let ratios = [1.0, 1.342, 1.613, 1.952, 2.431, 2.917];
                    ratios
                        .iter()
                        .zip(&mut self.hat_osc)
                        .map(|(ratio, osc)| {
                            osc.next_square((2100.0 + tone * 900.0) * ratio, self.sr)
                        })
                        .sum::<f32>()
                        / 6.0
                }
            };
            let x = raw * self.drum_amp[i] * self.drum_mix[i][0] * 0.42;
            if !self.project.tracks[i].muted {
                dry += x;
                delay_in += x * self.drum_mix[i][1];
                reverb_in += x * self.drum_mix[i][2];
            }
            let seconds = match i {
                0 => 0.05 + self.drum_decay[i] * 1.2,
                1 => 0.04 + self.drum_decay[i] * 0.8,
                _ => 0.015 + self.drum_decay[i] * 0.35,
            };
            self.drum_amp[i] *= (-1.0 / (seconds * self.sr)).exp();
        }
        for i in 0..3 {
            let tone = self.preview_drum_tone[i];
            let hz = [
                55.0 + tone * 60.0,
                180.0 + tone * 180.0,
                5500.0 + tone * 5000.0,
            ][i];
            self.preview_drum_phase[i] = (self.preview_drum_phase[i] + hz / self.sr).fract();
            let noise = self.noise(i);
            let raw = match i {
                0 => (TAU * self.preview_drum_phase[i]).sin(),
                1 => 0.35 * (TAU * self.preview_drum_phase[i]).sin() + 0.65 * noise,
                _ => noise,
            };
            let x = raw * self.preview_drum_amp[i] * self.preview_drum_mix[i][0] * 0.42;
            dry += x;
            delay_in += x * self.preview_drum_mix[i][1];
            reverb_in += x * self.preview_drum_mix[i][2];
            let seconds = [
                0.05 + self.preview_drum_decay[i] * 1.2,
                0.04 + self.preview_drum_decay[i] * 0.8,
                0.015 + self.preview_drum_decay[i] * 0.35,
            ][i];
            self.preview_drum_amp[i] *= (-1.0 / (seconds * self.sr)).exp();
        }
        for i in 0..3 {
            let (x, ds, rs) = Self::render_synth(&mut self.synth[i], self.sr);
            if !self.project.tracks[i + 3].muted {
                dry += x;
                delay_in += x * ds;
                reverb_in += x * rs;
            }
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
pub fn render_offline(project: &ProjectV1, sample_rate: u32, frames: usize) -> Vec<(f32, f32)> {
    let status = Arc::new(AudioStatus::default());
    let mut renderer = Renderer::new(AudioProject::from_project(project), sample_rate, status);
    renderer.command(AudioCommand::PlayPause);
    (0..frames).map(|_| renderer.next()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_contains_all_steps_without_heap_backed_commands() {
        let p = ProjectV1::new();
        let AudioCommand::ReplaceProject(s) = Audio::snapshot(&p) else {
            panic!()
        };
        assert_eq!(s.globals.tempo_bpm, 120);
        assert!(s.tracks[0].steps.iter().all(Option::is_none));
    }
    #[test]
    fn offline_render_is_deterministic_and_finite() {
        let mut p = ProjectV1::new();
        p.tracks[0].steps[0] = Some(StepEvent::Trigger {
            locks: Default::default(),
        });
        let a = render_offline(&p, 8_000, 2_000);
        let b = render_offline(&p, 8_000, 2_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(a.iter().any(|(l, _)| l.abs() > 0.001));
    }
}
