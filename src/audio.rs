use crate::{
    dsp::{Adsr, PolyBlepOsc, safety},
    engine::{GateAction, StepClock, synth_action},
    model::{Instrument, Percent, ProjectV1, StepEvent, Waveform},
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

#[derive(Clone, Debug)]
pub enum AudioCommand {
    PlayPause,
    Stop,
    SetStep {
        track: u8,
        step: u8,
        event: Option<StepEvent>,
    },
    SetMute {
        track: u8,
        muted: bool,
    },
    SetTrackParameters {
        track: u8,
        level: Percent,
        delay_send: Percent,
        reverb_send: Percent,
        instrument: Instrument,
    },
}
pub struct AudioStatus {
    pub running: AtomicBool,
    pub playhead: AtomicU8,
    pub failed: AtomicBool,
}
impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            playhead: AtomicU8::new(u8::MAX),
            failed: AtomicBool::new(false),
        }
    }
}
pub struct Audio {
    pub stream: Stream,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    producer: Producer<AudioCommand>,
}
impl Audio {
    pub fn send(&mut self, command: AudioCommand) -> Result<(), AudioCommand> {
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(command)| command)
    }
    pub fn available_commands(&self) -> usize {
        self.producer.slots()
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
            n => bail!("audio output name `{name}` is ambiguous ({n} matches)"),
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
    let (producer, consumer) = RingBuffer::new(1024);
    let stream=match format{
   SampleFormat::F32=>{let s=status.clone();let mut r=Renderer::new(project.clone(),sr);let mut c=consumer;device.build_output_stream(&config,move|out:&mut[f32],_|render(out,channels,&mut r,&mut c,|x|x),move |_|mark_failed(&s),None)},
   SampleFormat::I16=>{let s=status.clone();let mut r=Renderer::new(project.clone(),sr);let mut c=consumer;device.build_output_stream(&config,move|out:&mut[i16],_|render(out,channels,&mut r,&mut c,|x|(x.clamp(-1.,1.)*32767.) as i16),move |_|mark_failed(&s),None)},
   SampleFormat::U16=>{let s=status.clone();let mut r=Renderer::new(project.clone(),sr);let mut c=consumer;device.build_output_stream(&config,move|out:&mut[u16],_|render(out,channels,&mut r,&mut c,|x|((x.clamp(-1.,1.)*0.5+0.5)*65535.) as u16),move |_|mark_failed(&s),None)},
   other=>bail!("audio output `{name}` uses unsupported sample format {other:?}; supported: f32, i16, u16"),
 }.with_context(||format!("could not build stream for audio output `{name}`"))?;
    stream
        .play()
        .with_context(|| format!("could not start audio output `{name}`"))?;
    status.running.store(true, Ordering::Release);
    Ok(Audio {
        stream,
        device_name: name,
        status,
        producer,
    })
}
fn mark_failed(status: &AudioStatus) {
    status.failed.store(true, Ordering::Release);
    status.running.store(false, Ordering::Release)
}

struct SynthVoice {
    osc: PolyBlepOsc,
    env: Adsr,
    freq: f32,
    wave: Waveform,
    active: bool,
}
struct Renderer {
    project: ProjectV1,
    clock: StepClock,
    playing: bool,
    sr: f32,
    drum_amp: [f32; 3],
    drum_phase: [f32; 3],
    noise: u32,
    synth: [SynthVoice; 3],
}
impl Renderer {
    fn new(project: ProjectV1, sr: u32) -> Self {
        let clock = StepClock::new(sr, project.globals.tempo_bpm);
        Self {
            project,
            clock,
            playing: false,
            sr: sr as f32,
            drum_amp: [0.; 3],
            drum_phase: [0.; 3],
            noise: 0x1234abcd,
            synth: std::array::from_fn(|_| SynthVoice {
                osc: Default::default(),
                env: Adsr::new(sr as f32),
                freq: 110.,
                wave: Waveform::Saw,
                active: false,
            }),
        }
    }
    fn command(&mut self, c: AudioCommand) {
        match c {
            AudioCommand::PlayPause => {
                self.playing = !self.playing;
                if !self.playing {
                    for v in &mut self.synth {
                        v.env.gate_off()
                    }
                }
            }
            AudioCommand::Stop => {
                self.playing = false;
                self.clock.reset();
                self.drum_amp = [0.; 3];
                for v in &mut self.synth {
                    v.env.gate_off();
                    v.active = false
                }
            }
            AudioCommand::SetStep { track, step, event } => {
                if let Some(s) = self
                    .project
                    .tracks
                    .get_mut(track as usize)
                    .and_then(|t| t.steps.get_mut(step as usize))
                {
                    *s = event
                }
            }
            AudioCommand::SetMute { track, muted } => {
                if let Some(t) = self.project.tracks.get_mut(track as usize) {
                    t.muted = muted
                }
            }
            AudioCommand::SetTrackParameters {
                track,
                level,
                delay_send,
                reverb_send,
                instrument,
            } => {
                if let Some(t) = self.project.tracks.get_mut(track as usize) {
                    t.level = level;
                    t.delay_send = delay_send;
                    t.reverb_send = reverb_send;
                    t.instrument = instrument;
                }
            }
        }
    }
    fn boundary(&mut self, step: usize) {
        for ti in 0..6 {
            let t = &self.project.tracks[ti];
            if ti < 3 {
                if matches!(t.steps[step], Some(StepEvent::Trigger { .. })) {
                    self.drum_amp[ti] = 1.0
                }
            } else {
                let vi = ti - 3;
                match synth_action(&t.steps, step, self.synth[vi].active) {
                    GateAction::Trigger { degree, octave } => {
                        self.synth[vi].freq =
                            self.project.note_frequency(degree, octave).unwrap_or(110.);
                        if let crate::model::Instrument::Synth(p) = &t.instrument {
                            self.synth[vi].wave = p.waveform;
                            self.synth[vi].env.configure(
                                if p.attack.get() == 0 {
                                    0.0
                                } else {
                                    crate::dsp::exp_map(p.attack.get(), 0.001, 2.)
                                },
                                crate::dsp::exp_map(p.decay.get(), 0.005, 3.),
                                p.sustain.normalized(),
                                crate::dsp::exp_map(p.release.get(), 0.005, 5.),
                            )
                        }
                        self.synth[vi].env.gate_on();
                        self.synth[vi].active = true
                    }
                    GateAction::Hold => {}
                    GateAction::Release => {
                        self.synth[vi].env.gate_off();
                        self.synth[vi].active = false
                    }
                    GateAction::None => {}
                }
            }
        }
    }
    fn next(&mut self) -> (f32, f32) {
        if self.playing {
            if let Some(step) = self.clock.tick() {
                self.boundary(step)
            }
        }
        let mut sum = 0.;
        for i in 0..3 {
            let t = &self.project.tracks[i];
            let level = if t.muted {
                0.
            } else {
                t.level.normalized().powi(2)
            };
            self.drum_phase[i] = (self.drum_phase[i] + [55., 180., 6000.][i] / self.sr).fract();
            self.noise ^= self.noise << 13;
            self.noise ^= self.noise >> 17;
            self.noise ^= self.noise << 5;
            let noise = self.noise as i32 as f32 / i32::MAX as f32;
            let raw = match i {
                0 => (TAU * self.drum_phase[i]).sin(),
                1 => 0.35 * (TAU * self.drum_phase[i]).sin() + 0.65 * noise,
                _ => noise,
            };
            sum += raw * self.drum_amp[i] * level * 0.45;
            self.drum_amp[i] *= [0.9990, 0.996, 0.97][i]
        }
        for (i, v) in self.synth.iter_mut().enumerate() {
            let t = &self.project.tracks[i + 3];
            let level = if t.muted {
                0.
            } else {
                t.level.normalized().powi(2)
            };
            let osc = match v.wave {
                Waveform::Saw => v.osc.next_saw(v.freq, self.sr),
                Waveform::Square => v.osc.next_square(v.freq, self.sr),
            };
            sum += osc * v.env.next_sample() * level * 0.18
        }
        let x = safety(sum);
        (x, x)
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
            *sample = convert(0.)
        }
    }
}
