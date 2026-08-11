pub use crate::model::{PatternIndexMap, SongIndexMap};
use crate::{
    dsp::{
        DcBlock, Delay, Lfo, MasterLimiter, Reverb, SidechainCompressor, Smoother, TrackEffectChain,
    },
    engine::StepClock,
    model::{
        ArpeggioConfig, CHORD_TRACK_INDEX, ChordShape, DRUM_TRACK_COUNT, Globals, Instrument,
        LfoAssignments, MAX_STEP_COUNT, ParameterId, Percent, Project, SYNTH_TRACK_START,
        SongEntry, StepEvent, TRACK_COUNT, TrackEffects,
    },
};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, StreamConfig, SupportedBufferSize};
use rtrb::{Producer, RingBuffer};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Copy, Debug)]
struct AudioTrack {
    level: Percent,
    pan: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    swing: Percent,
    probability: Percent,
    instrument: Instrument,
    effects: TrackEffects,
    lfos: LfoAssignments,
    input_degree: u8,
    input_octave: u8,
    input_accent: bool,
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
    song: Box<[SongEntry]>,
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
                    swing: t.swing,
                    probability: t.probability,
                    instrument: t.instrument,
                    effects: t.effects,
                    lfos: t.lfos,
                    input_degree: t.input_degree.unwrap_or(1),
                    input_octave: t.input_octave.unwrap_or(3),
                    input_accent: t.input_accent,
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
            song: project.song.clone().into_boxed_slice(),
        }
    }
}

struct Renderer {
    project: Box<AudioProject>,
    retire: Producer<Box<AudioProject>>,
    pending: Option<(
        Box<AudioProject>,
        ParameterSmoothing,
        PatternIndexMap,
        SongIndexMap,
    )>,
    clock: StepClock,
    next_steps: [usize; TRACK_COUNT],
    active_pattern: usize,
    queued_pattern: Option<usize>,
    song_mode: bool,
    active_song: usize,
    queued_song: Option<usize>,
    song_bar: u8,
    playing: bool,
    sr: f32,
    status: Arc<AudioStatus>,
    drums: [DrumVoice; DRUM_TRACK_COUNT],
    preview_drums: [DrumVoice; DRUM_TRACK_COUNT],
    synth: [SynthVoice; 3],
    preview: [SynthVoice; 3],
    chord: ChordVoicePool,
    preview_chord: ChordVoicePool,
    effects: [TrackEffectChain; NON_CHORD_TRACK_COUNT],
    preview_effects: [TrackEffectChain; NON_CHORD_TRACK_COUNT],
    chord_effects: [TrackEffectChain; 2],
    preview_chord_effects: [TrackEffectChain; 2],
    sidechain: SidechainCompressor,
    delay: Delay,
    reverb: Reverb,
    reverb_return: Smoother,
    dc: DcBlock,
    limiter: MasterLimiter,
    mute: [Smoother; TRACK_COUNT],
    lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfos: [[Lfo; ParameterId::ALL.len()]; TRACK_COUNT],
    lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
    preview_lfo_offsets: [[f32; ParameterId::ALL.len()]; TRACK_COUNT],
    lfo_destinations: [[ParameterId; ParameterId::ALL.len()]; TRACK_COUNT],
    lfo_destination_count: [u8; TRACK_COUNT],
    preview_activity: [bool; TRACK_COUNT],
    scheduled: [Option<ScheduledTrackAction>; SCHEDULED_ACTION_COUNT],
    early_armed: [Option<u8>; TRACK_COUNT],
    cycle_counts: [[u32; MAX_STEP_COUNT]; TRACK_COUNT],
    condition_rng: [u32; TRACK_COUNT],
    probability_rng: [u32; TRACK_COUNT],
    preview_scheduled: [Option<PreviewAction>; 24],
}

const NON_CHORD_TRACK_COUNT: usize = TRACK_COUNT - 1;
const SCHEDULED_ACTION_COUNT: usize = 96;

/// Maps a logical track to its ordinary effect-chain slot. Chord is handled by
/// its two independent group chains and intentionally has no ordinary slot.
const fn effect_slot(track: usize) -> Option<usize> {
    if track == CHORD_TRACK_INDEX {
        None
    } else if track < CHORD_TRACK_INDEX {
        Some(track)
    } else if track < TRACK_COUNT {
        Some(track - 1)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct ScheduledTrackAction {
    remaining: u32,
    track: u8,
    step: u8,
    retrigger: bool,
    trigger_allowed: bool,
}

#[derive(Clone, Copy)]
struct PreviewAction {
    remaining: u32,
    track: u8,
    step: u8,
}

mod command;
mod effects;
mod queue;
mod renderer;
mod scheduler;
mod synthesis;
mod voices;

#[cfg(test)]
use crate::dsp::exp_map_f32;
#[cfg(test)]
use crate::model::ParameterLocks;
pub use command::AudioCommand;
use effects::render;
#[cfg(test)]
use effects::{modulated_percent, pitch_modulated_frequency};
pub use queue::{Audio, QueueFull};
#[cfg(test)]
use voices::{ArpeggioState, CHORD_GROUP_SIZE, DRUM_SILENCE, DrumEnvelope, KickPitchEnvelope};
use voices::{ChordVoicePool, DrumVoice, SynthVoice};

pub struct AudioStatus {
    pub running: AtomicBool,
    pub paused: AtomicBool,
    pub playheads: [AtomicU8; TRACK_COUNT],
    pub failed: AtomicBool,
    pub non_finite: AtomicBool,
    pub active_pattern: AtomicU8,
    pub queued_pattern: AtomicU8,
    pub song_mode: AtomicBool,
    pub active_song: AtomicU8,
    pub queued_song: std::sync::atomic::AtomicU16,
    pub song_bar: AtomicU8,
    pub callback_overruns: AtomicU64,
    pub max_callback_duration_ns: AtomicU64,
    pub max_callback_load_per_mille: AtomicU64,
}

const AUDIO_LOG_FILE_NAME: &str = "terminal-groove-audio.log";
const AUDIO_ERROR_QUEUE_SIZE: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioDiagnostics {
    pub stream_failures: usize,
    pub non_finite: bool,
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
            song_mode: AtomicBool::new(false),
            active_song: AtomicU8::new(0),
            queued_song: std::sync::atomic::AtomicU16::new(u16::MAX),
            song_bar: AtomicU8::new(0),
            callback_overruns: AtomicU64::new(0),
            max_callback_duration_ns: AtomicU64::new(0),
            max_callback_load_per_mille: AtomicU64::new(0),
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

const AUTO_BUFFER_FRAMES: u32 = 512;

fn select_buffer_size(
    supported: &SupportedBufferSize,
    requested: Option<u32>,
) -> Result<BufferSize> {
    let frames = requested.unwrap_or(AUTO_BUFFER_FRAMES);
    if frames == 0 {
        bail!("audio buffer must contain at least one frame")
    }
    match supported {
        SupportedBufferSize::Range { min, max } => {
            if let Some(requested) = requested {
                if requested < *min || requested > *max {
                    bail!(
                        "audio buffer {requested} frames is unsupported; device accepts {min}–{max} frames"
                    )
                }
                Ok(BufferSize::Fixed(requested))
            } else {
                Ok(BufferSize::Fixed(frames.clamp(*min, *max)))
            }
        }
        SupportedBufferSize::Unknown => Ok(if requested.is_some() {
            BufferSize::Fixed(frames)
        } else {
            BufferSize::Default
        }),
    }
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
pub fn open(
    requested: Option<&str>,
    project: &Project,
    audio_buffer: Option<u32>,
) -> Result<Audio> {
    let result = open_inner(requested, project, audio_buffer);
    if let Err(error) = &result {
        let device = requested.unwrap_or("<default>");
        let path = default_audio_log_path();
        if let Err(log_error) =
            append_audio_log(&path, device, "initialization", &error.to_string())
        {
            eprintln!(
                "could not write audio diagnostic log {}: {log_error}",
                path.display()
            );
        }
    }
    result
}

#[allow(deprecated)]
fn open_inner(
    requested: Option<&str>,
    project: &Project,
    audio_buffer: Option<u32>,
) -> Result<Audio> {
    let device = choose_device(requested)?;
    let name = device.name().unwrap_or_else(|_| "unknown".into());
    let supported = device
        .default_output_config()
        .with_context(|| format!("could not query audio output `{name}`"))?;
    let format = supported.sample_format();
    let mut config: StreamConfig = supported.clone().into();
    config.buffer_size = select_buffer_size(supported.buffer_size(), audio_buffer)
        .with_context(|| format!("invalid audio buffer for output `{name}`"))?;
    let channels = config.channels as usize;
    let sr = config.sample_rate;
    let status = Arc::new(AudioStatus::default());
    let (producer, consumer) = RingBuffer::new(256);
    let (retire_producer, retired) = RingBuffer::new(32);
    let (error_producer, error_consumer) = RingBuffer::new(AUDIO_ERROR_QUEUE_SIZE);
    let initial = AudioProject::from_project(project);
    let stream = match format {
        SampleFormat::F32 => {
            let error_status = status.clone();
            let render_status = status.clone();
            let timing_status = status.clone();
            let mut errors = error_producer;
            let mut renderer = Renderer::new_with_retirement(
                Box::new(initial),
                sr,
                render_status,
                retire_producer,
            );
            let mut commands = consumer;
            device.build_output_stream(
                &config,
                move |out: &mut [f32], _| {
                    render(
                        out,
                        channels,
                        sr,
                        &timing_status,
                        &mut renderer,
                        &mut commands,
                        |x| x,
                    )
                },
                move |error| mark_failed(&error_status, &mut errors, error),
                None,
            )
        }
        SampleFormat::I16 => {
            let error_status = status.clone();
            let render_status = status.clone();
            let timing_status = status.clone();
            let mut errors = error_producer;
            let mut renderer = Renderer::new_with_retirement(
                Box::new(initial),
                sr,
                render_status,
                retire_producer,
            );
            let mut commands = consumer;
            device.build_output_stream(
                &config,
                move |out: &mut [i16], _| {
                    render(
                        out,
                        channels,
                        sr,
                        &timing_status,
                        &mut renderer,
                        &mut commands,
                        |x| (x.clamp(-1.0, 1.0) * 32767.0) as i16,
                    )
                },
                move |error| mark_failed(&error_status, &mut errors, error),
                None,
            )
        }
        SampleFormat::U16 => {
            let error_status = status.clone();
            let render_status = status.clone();
            let timing_status = status.clone();
            let mut errors = error_producer;
            let mut renderer = Renderer::new_with_retirement(
                Box::new(initial),
                sr,
                render_status,
                retire_producer,
            );
            let mut commands = consumer;
            device.build_output_stream(
                &config,
                move |out: &mut [u16], _| {
                    render(
                        out,
                        channels,
                        sr,
                        &timing_status,
                        &mut renderer,
                        &mut commands,
                        |x| ((x.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16,
                    )
                },
                move |error| mark_failed(&error_status, &mut errors, error),
                None,
            )
        }
        other => bail!("audio output `{name}` uses unsupported sample format {other:?}; supported: f32, i16, u16"),
    }.with_context(|| format!("could not build stream for audio output `{name}`"))?;
    stream
        .play()
        .with_context(|| format!("could not start audio output `{name}`"))?;
    Ok(Audio::new(
        stream,
        name,
        status,
        producer,
        retired,
        error_consumer,
        default_audio_log_path(),
    ))
}

fn mark_failed(
    status: &AudioStatus,
    errors: &mut Producer<cpal::StreamError>,
    error: cpal::StreamError,
) {
    status.failed.store(true, Ordering::Release);
    status.running.store(false, Ordering::Release);
    status.paused.store(false, Ordering::Release);
    for playhead in &status.playheads {
        playhead.store(u8::MAX, Ordering::Release);
    }
    let _ = errors.push(error);
}

pub fn default_audio_log_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(AUDIO_LOG_FILE_NAME)
}

fn append_audio_log(path: &Path, device: &str, kind: &str, message: &str) -> std::io::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "timestamp_unix_ms={timestamp}")?;
    writeln!(file, "device={device}")?;
    writeln!(file, "kind={kind}")?;
    writeln!(file, "message={message}")?;
    writeln!(file)?;
    Ok(())
}

/// Deterministic, device-independent rendering used by tests and diagnostics.
pub fn render_offline(project: &Project, sample_rate: u32, frames: usize) -> Vec<(f32, f32)> {
    let status = Arc::new(AudioStatus::default());
    let mut renderer = Renderer::new(AudioProject::from_project(project), sample_rate, status);
    renderer.command(AudioCommand::PlayPause);
    (0..frames).map(|_| renderer.next()).collect()
}

#[cfg(test)]
include!("audio/tests.rs");
