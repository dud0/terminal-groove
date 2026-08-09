pub use crate::model::PatternIndexMap;
use crate::{
    dsp::{
        DcBlock, Delay, Lfo, MasterLimiter, Reverb, SidechainCompressor, Smoother, TrackEffectChain,
    },
    engine::StepClock,
    model::{
        ArpeggioConfig, CHORD_TRACK_INDEX, ChordShape, DRUM_TRACK_COUNT, Globals, Instrument,
        LfoAssignments, MAX_STEP_COUNT, ParameterId, Percent, Project, SYNTH_TRACK_START,
        StepEvent, TRACK_COUNT, TrackEffects,
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
    drums: [DrumVoice; DRUM_TRACK_COUNT],
    preview_drums: [DrumVoice; DRUM_TRACK_COUNT],
    synth: [SynthVoice; 3],
    preview: [SynthVoice; 3],
    chord: ChordVoicePool,
    preview_chord: ChordVoicePool,
    effects: [TrackEffectChain; TRACK_COUNT],
    preview_effects: [TrackEffectChain; TRACK_COUNT],
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
    scheduled: [Option<ScheduledTrackAction>; 32],
    cycle_counts: [[u32; MAX_STEP_COUNT]; TRACK_COUNT],
    condition_rng: [u32; TRACK_COUNT],
    probability_rng: [u32; TRACK_COUNT],
    preview_scheduled: [Option<PreviewAction>; 24],
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
mod tests {
    use super::voices::SynthTrigger;
    use super::*;
    use crate::model::{
        ArpeggioRate, ArpeggioType, ChordShape, DistortionParameters, FlangerParameters,
        LEAD_TRACK_INDEX, LfoConfig, LfoDivision, LfoRate, LfoWaveform, PhaserParameters,
        TrackEffects,
    };
    use std::{fs, time::Instant};

    fn performance_project() -> Project {
        let mut project = Project::new();
        let saturated_effects = TrackEffects {
            distortion: DistortionParameters {
                drive: Percent::new(85).unwrap(),
                tone: Percent::new(65).unwrap(),
                mix: Percent::new(75).unwrap(),
            },
            phaser: PhaserParameters {
                rate: Percent::new(60).unwrap(),
                depth: Percent::new(80).unwrap(),
                feedback: Percent::new(70).unwrap(),
                mix: Percent::new(65).unwrap(),
            },
            flanger: FlangerParameters {
                rate: Percent::new(55).unwrap(),
                delay: Percent::new(70).unwrap(),
                depth: Percent::new(80).unwrap(),
                feedback: Percent::new(65).unwrap(),
                mix: Percent::new(60).unwrap(),
            },
        };
        let lfo = Some(LfoConfig {
            enabled: true,
            waveform: LfoWaveform::Sine,
            rate: LfoRate::Synced {
                division: LfoDivision::Sixteenth,
            },
            depth: Percent::new(35).unwrap(),
        });
        for track in &mut project.tracks {
            track.delay_send = Percent::new(100).unwrap();
            track.reverb_send = Percent::new(100).unwrap();
            track.effects = saturated_effects;
            track.lfos.level = lfo;
            track.lfos.pan = lfo;
        }
        project.globals.delay_feedback = Percent::new(85).unwrap();
        project.globals.reverb_return = Percent::new(75).unwrap();
        project.globals.reverb_time_seconds = 10.0;
        if let Instrument::Chord(parameters) = &mut project.tracks[CHORD_TRACK_INDEX].instrument {
            parameters.release = Percent::new(100).unwrap();
        }
        for step in 0..16 {
            for track in 0..DRUM_TRACK_COUNT {
                project.patterns[0].tracks[track].steps[step] = Some(StepEvent::Trigger {
                    accent: step % 4 == 0,
                    condition: Default::default(),
                    retrigger_count: 4,
                    locks: Default::default(),
                });
            }
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[step] = Some(StepEvent::BassNote {
                degree: (step % 7 + 1) as u8,
                octave: 2,
                accent: step % 4 == 0,
                slide: step % 3 == 0,
                condition: Default::default(),
                retrigger_count: 4,
                locks: Default::default(),
            });
            project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[step] = Some(StepEvent::Note {
                degree: (step % 7 + 1) as u8,
                octave: 3,
                accent: step % 4 == 0,
                chord_shape: Some(ChordShape::SeventhRoot),
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 4,
                locks: Default::default(),
            });
            project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[step] = Some(StepEvent::LeadNote {
                degree: (step % 7 + 1) as u8,
                octave: 4,
                accent: step % 4 == 0,
                slide: step % 3 == 0,
                condition: Default::default(),
                retrigger_count: 4,
                locks: Default::default(),
            });
        }
        project
    }

    fn arm_second_worst_case_chord_group(renderer: &mut Renderer) {
        Renderer::trigger_chord(
            &renderer.project,
            renderer.sr,
            SynthTrigger {
                degree: 5,
                octave: 3,
                accent: true,
                slide: false,
                chord_shape: Some(ChordShape::SeventhRoot),
                arpeggio: ArpeggioConfig::default(),
            },
            ParameterLocks::default(),
            &mut renderer.chord,
        );
        assert_eq!(renderer.chord.group_voice_counts, [4, 4]);
        assert!(
            renderer
                .chord
                .voices
                .iter()
                .all(|voice| voice.env.stage != crate::dsp::EnvStage::Idle)
        );
    }

    fn allocator_counts() -> (usize, usize) {
        crate::test_allocator::counts()
    }

    #[test]
    fn stream_failure_marks_audio_stopped_and_queues_error() {
        let status = AudioStatus::default();
        let (mut producer, mut consumer) = RingBuffer::new(2);

        mark_failed(
            &status,
            &mut producer,
            cpal::StreamError::DeviceNotAvailable,
        );

        assert!(status.failed.load(Ordering::Acquire));
        assert!(!status.running.load(Ordering::Acquire));
        assert!(!status.paused.load(Ordering::Acquire));
        assert_eq!(
            consumer.pop().unwrap(),
            cpal::StreamError::DeviceNotAvailable
        );
    }

    #[test]
    fn audio_log_contains_failure_context() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.log");

        append_audio_log(
            &path,
            "Null Output",
            "runtime stream failure",
            "device disappeared",
        )
        .unwrap();

        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("timestamp_unix_ms="));
        assert!(contents.contains("device=Null Output"));
        assert!(contents.contains("kind=runtime stream failure"));
        assert!(contents.contains("message=device disappeared"));
    }

    #[test]
    fn automatic_audio_buffer_is_clamped_to_device_limits() {
        assert_eq!(
            select_buffer_size(&SupportedBufferSize::Range { min: 128, max: 256 }, None).unwrap(),
            BufferSize::Fixed(256)
        );
        assert_eq!(
            select_buffer_size(
                &SupportedBufferSize::Range {
                    min: 1024,
                    max: 2048
                },
                None
            )
            .unwrap(),
            BufferSize::Fixed(1024)
        );
    }

    #[test]
    fn explicit_audio_buffer_must_be_supported() {
        assert_eq!(
            select_buffer_size(
                &SupportedBufferSize::Range {
                    min: 128,
                    max: 1024
                },
                Some(512)
            )
            .unwrap(),
            BufferSize::Fixed(512)
        );
        assert!(
            select_buffer_size(
                &SupportedBufferSize::Range { min: 128, max: 256 },
                Some(512)
            )
            .is_err()
        );
        assert_eq!(
            select_buffer_size(&SupportedBufferSize::Unknown, None).unwrap(),
            BufferSize::Default
        );
    }

    #[test]
    fn zero_audio_buffer_is_rejected() {
        assert!(select_buffer_size(&SupportedBufferSize::Unknown, Some(0)).is_err());
    }

    #[test]
    fn callback_paths_do_not_allocate_or_deallocate() {
        let project = performance_project();
        let status = Arc::new(AudioStatus::default());
        let (retire, _retired) = RingBuffer::new(32);
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        arm_second_worst_case_chord_group(&mut renderer);
        let (mut producer, mut commands) = RingBuffer::new(16);
        let mut output = vec![0.0_f32; 256 * 2];

        crate::test_allocator::reset();
        let before = allocator_counts();
        render(
            &mut output,
            2,
            48_000,
            &status,
            &mut renderer,
            &mut commands,
            |sample| sample,
        );
        assert_eq!(allocator_counts(), before);

        let callback_commands = [
            AudioCommand::Audition { track: 6, step: 0 },
            Audio::snapshot(&project),
            AudioCommand::Stop,
            AudioCommand::PlayPause,
        ];
        for command in callback_commands {
            producer.push(command).unwrap();
            crate::test_allocator::reset();
            let before = allocator_counts();
            render(
                &mut output,
                2,
                48_000,
                &status,
                &mut renderer,
                &mut commands,
                |sample| sample,
            );
            assert_eq!(allocator_counts(), before);
        }
    }

    #[test]
    fn pending_snapshot_path_is_allocation_free_when_retirement_is_full() {
        let project = performance_project();
        let status = Arc::new(AudioStatus::default());
        let (mut retire, _retired) = RingBuffer::new(1);
        retire
            .push(Box::new(AudioProject::from_project(&project)))
            .unwrap();
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        let (mut producer, mut commands) = RingBuffer::new(2);
        producer.push(Audio::snapshot(&project)).unwrap();
        let mut output = vec![0.0_f32; 128 * 2];

        crate::test_allocator::reset();
        let before = allocator_counts();
        render(
            &mut output,
            2,
            48_000,
            &status,
            &mut renderer,
            &mut commands,
            |sample| sample,
        );
        assert_eq!(allocator_counts(), before);
    }

    #[test]
    fn idle_preview_does_not_advance_unselected_lfos_or_effects() {
        let project = performance_project();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
        for _ in 0..1_000 {
            renderer.next();
        }
        assert!(!renderer.preview_activity.iter().any(|active| *active));
        assert!(
            renderer
                .preview_lfo_offsets
                .iter()
                .flatten()
                .all(|offset| *offset == 0.0)
        );
        assert!(
            !renderer
                .preview_effects
                .iter()
                .any(TrackEffectChain::is_active)
        );

        renderer.command(AudioCommand::Audition { track: 0, step: 0 });
        renderer.next();
        renderer.next();
        assert!(renderer.preview_activity[0]);
        assert!(!renderer.preview_activity[1]);
        assert!(renderer.preview_lfo_offsets[0][ParameterId::Level as usize] != 0.0);
        assert!(
            renderer.preview_lfo_offsets[1]
                .iter()
                .all(|offset| *offset == 0.0)
        );
    }

    #[test]
    fn worst_case_fixture_reports_callback_cost_without_a_brittle_limit() {
        const WARMUP_CALLBACKS: usize = 64;
        const MEASURED_CALLBACKS: usize = 64;
        let project = performance_project();
        eprintln!(
            "audio fixture: os={} arch={} rustc={} profile={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            option_env!("RUSTC_VERSION").unwrap_or("unknown"),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        for sample_rate in [44_100, 48_000, 96_000] {
            for buffer_frames in [128, 256, 512] {
                let status = Arc::new(AudioStatus::default());
                let (retire, _retired) = RingBuffer::new(32);
                let mut renderer = Renderer::new_with_retirement(
                    Box::new(AudioProject::from_project(&project)),
                    sample_rate,
                    status.clone(),
                    retire,
                );
                renderer.command(AudioCommand::PlayPause);
                renderer.boundary(0);
                arm_second_worst_case_chord_group(&mut renderer);
                let (producer, mut commands) = RingBuffer::new(2);
                drop(producer);
                let mut output = vec![0.0_f32; buffer_frames * 2];
                for _ in 0..WARMUP_CALLBACKS {
                    render(
                        &mut output,
                        2,
                        sample_rate,
                        &status,
                        &mut renderer,
                        &mut commands,
                        |sample| sample,
                    );
                }
                status.max_callback_duration_ns.store(0, Ordering::Relaxed);
                status
                    .max_callback_load_per_mille
                    .store(0, Ordering::Relaxed);
                status.callback_overruns.store(0, Ordering::Relaxed);
                let mut durations = Vec::with_capacity(MEASURED_CALLBACKS);
                for _ in 0..MEASURED_CALLBACKS {
                    let start = Instant::now();
                    render(
                        &mut output,
                        2,
                        sample_rate,
                        &status,
                        &mut renderer,
                        &mut commands,
                        |sample| sample,
                    );
                    durations.push(start.elapsed().as_nanos());
                }
                durations.sort_unstable();
                let median = durations[MEASURED_CALLBACKS / 2];
                let p95 = durations[(MEASURED_CALLBACKS * 95 / 100).min(MEASURED_CALLBACKS - 1)];
                let budget_ns =
                    buffer_frames as u128 * 1_000_000_000_u128 / u128::from(sample_rate);
                eprintln!(
                    "audio fixture: sr={} buffer={} median_ns/frame={:.1} median_load={}‰ p95_load={}‰ max_load={}‰",
                    sample_rate,
                    buffer_frames,
                    median as f64 / buffer_frames as f64,
                    median * 1_000 / budget_ns,
                    p95 * 1_000 / budget_ns,
                    status.max_callback_load_per_mille.load(Ordering::Relaxed),
                );
                assert!(output.iter().all(|sample| sample.is_finite()));
            }
        }
    }

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
            condition: Default::default(),
            retrigger_count: 1,
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
            renderer.boundary(0);
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
    fn scheduled_actions_for_removed_steps_are_discarded_after_resize() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(4, None);
        project.patterns[0].tracks[0].steps[3] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 4,
            locks: Default::default(),
        });
        project.tracks[0].swing = Percent::new(75).unwrap();

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.next_steps[0] = 3;
        renderer.boundary(1);
        assert!(renderer.scheduled.iter().any(|action| {
            matches!(action, Some(action) if action.track == 0 && action.step == 3)
        }));

        project.patterns[0].tracks[0].steps.resize(3, None);
        renderer.command(Audio::snapshot(&project));
        assert!(!renderer.scheduled.iter().any(|action| {
            matches!(action, Some(action) if action.track == 0 && action.step == 3)
        }));

        for _ in 0..1_000 {
            renderer.next();
        }
    }

    #[test]
    fn scheduled_actions_for_replaced_events_are_discarded() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 4,
            locks: Default::default(),
        });
        project.tracks[0].swing = Percent::new(75).unwrap();

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(1);
        assert!(renderer.scheduled.iter().any(|action| {
            matches!(action, Some(action) if action.track == 0 && action.step == 0 && action.retrigger)
        }));

        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        renderer.command(Audio::snapshot(&project));

        assert!(!renderer.scheduled.iter().any(|action| {
            matches!(action, Some(action) if action.track == 0 && action.step == 0)
        }));
    }

    #[test]
    fn swung_retriggers_are_offset_from_the_swung_start() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 2,
            locks: Default::default(),
        });
        project.tracks[0].swing = Percent::new(75).unwrap();

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(1);

        let track_actions: Vec<_> = renderer
            .scheduled
            .iter()
            .flatten()
            .filter(|action| action.track == 0)
            .collect();
        assert_eq!(track_actions.len(), 2);
        let initial = track_actions
            .iter()
            .find(|action| !action.retrigger)
            .unwrap();
        let retrigger = track_actions
            .iter()
            .find(|action| action.retrigger)
            .unwrap();
        assert_eq!(initial.remaining, 749);
        assert_eq!(retrigger.remaining, 874);
        assert!(retrigger.remaining > initial.remaining);
    }

    #[test]
    fn track_probability_gates_the_base_action_and_entire_retrigger_burst() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 4,
            locks: Default::default(),
        });
        project.tracks[0].probability = Percent::ZERO;
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        assert!(renderer.drums[0].envelope.is_idle());
        assert!(
            !renderer
                .scheduled
                .iter()
                .flatten()
                .any(|action| action.track == 0)
        );

        renderer.project.tracks[0].probability = Percent::new(100).unwrap();
        renderer.next_steps[0] = 0;
        renderer.boundary(1);
        assert!(!renderer.drums[0].envelope.is_idle());
        assert_eq!(
            renderer
                .scheduled
                .iter()
                .flatten()
                .filter(|action| action.track == 0 && action.retrigger)
                .count(),
            3
        );
    }

    #[test]
    fn probability_is_evaluated_after_conditions_without_sharing_rng() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: crate::model::TriggerCondition::Chance {
                probability: Percent::ZERO,
            },
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.tracks[0].probability = Percent::new(50).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let condition_before = renderer.condition_rng;
        let probability_before = renderer.probability_rng;
        renderer.boundary(0);
        assert_ne!(renderer.condition_rng, condition_before);
        assert_eq!(renderer.probability_rng, probability_before);
        assert!(renderer.drums[0].envelope.is_idle());
    }

    #[test]
    fn rejected_pitched_note_releases_but_tie_is_not_probability_gated() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::BassNote {
            degree: 2,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        assert!(renderer.synth[0].active);
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::ZERO;
        renderer.boundary(1);
        assert!(!renderer.synth[0].active);

        renderer.project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::new(100).unwrap();
        renderer.next_steps[SYNTH_TRACK_START] = 0;
        renderer.boundary(2);
        assert!(renderer.synth[0].active);
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::ZERO;
        renderer.next_steps[SYNTH_TRACK_START] = 1;
        renderer.boundary(3);
        assert!(renderer.synth[0].active);
    }

    #[test]
    fn probability_rng_resets_on_stop() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.tracks[0].probability = Percent::new(50).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let initial = renderer.probability_rng;
        renderer.boundary(0);
        assert_ne!(renderer.probability_rng, initial);
        renderer.command(AudioCommand::Stop);
        assert_eq!(renderer.probability_rng, initial);
    }

    #[test]
    fn scheduled_actions_freeze_while_paused() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 2,
            locks: Default::default(),
        });
        project.tracks[SYNTH_TRACK_START].swing = Percent::new(75).unwrap();

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(1);
        let remaining_before = renderer
            .scheduled
            .iter()
            .find(|action| matches!(action, Some(action) if action.track == SYNTH_TRACK_START as u8 && !action.retrigger))
            .and_then(|action| action.as_ref())
            .unwrap()
            .remaining;

        renderer.command(AudioCommand::PlayPause);
        for _ in 0..1_000 {
            renderer.next();
        }

        let remaining_after = renderer
            .scheduled
            .iter()
            .find(|action| matches!(action, Some(action) if action.track == SYNTH_TRACK_START as u8 && !action.retrigger))
            .and_then(|action| action.as_ref())
            .unwrap()
            .remaining;
        assert_eq!(remaining_after, remaining_before);
        assert!(!renderer.synth[0].active);
    }

    #[test]
    fn idle_drums_stop_advancing_while_paused() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.command(AudioCommand::PlayPause);

        let live_before = (
            renderer.drums[0].phase,
            renderer.drums[0].phase2,
            renderer.drums[0].noise,
        );
        let preview_before = (
            renderer.preview_drums[0].phase,
            renderer.preview_drums[0].phase2,
            renderer.preview_drums[0].noise,
        );

        for _ in 0..128 {
            renderer.next();
        }

        assert_eq!(
            (
                renderer.drums[0].phase,
                renderer.drums[0].phase2,
                renderer.drums[0].noise,
            ),
            live_before
        );
        assert_eq!(
            (
                renderer.preview_drums[0].phase,
                renderer.preview_drums[0].phase2,
                renderer.preview_drums[0].noise,
            ),
            preview_before
        );
    }

    #[test]
    fn idle_drums_stop_advancing_while_playing() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        let before = (
            renderer.drums[0].phase,
            renderer.drums[0].phase2,
            renderer.drums[0].noise,
        );

        for _ in 0..128 {
            renderer.next();
        }

        assert_eq!(
            (
                renderer.drums[0].phase,
                renderer.drums[0].phase2,
                renderer.drums[0].noise,
            ),
            before
        );
    }

    #[test]
    fn stopping_and_restarting_preserves_configured_track_effects() {
        let mut project = Project::new();
        project.tracks[0].effects.distortion.drive = crate::model::Percent::new(100).unwrap();
        project.tracks[0].effects.distortion.mix = crate::model::Percent::new(100).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        let before_stop = renderer.effects[0].process(0.25).0;
        assert!((before_stop - 0.25).abs() > 0.01);

        renderer.command(AudioCommand::Stop);
        renderer.command(AudioCommand::PlayPause);

        let after_restart = renderer.effects[0].process(0.25).0;
        assert!((after_restart - 0.25).abs() > 0.01);
    }

    #[test]
    fn active_drum_tail_still_advances_while_paused() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        renderer.trigger_drum(0, false, ParameterLocks::default());
        renderer.command(AudioCommand::PlayPause);
        renderer.command(AudioCommand::PlayPause);
        let elapsed_before = renderer.drums[0].envelope.elapsed;

        renderer.next();

        assert_eq!(renderer.drums[0].envelope.elapsed, elapsed_before + 1);
        for _ in 1..renderer.drums[0].envelope.decay_samples {
            renderer.next();
        }
        assert!(renderer.drums[0].envelope.is_idle());
    }

    #[test]
    fn automatic_audition_is_allowed_when_stopped_or_paused() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });

        let status = Arc::new(AudioStatus::default());
        let mut stopped = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        stopped.command(AudioCommand::AutoAudition { track: 0, step: 0 });
        assert!(!stopped.preview_drums[0].envelope.is_idle());

        let status = Arc::new(AudioStatus::default());
        let mut paused = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        paused.command(AudioCommand::PlayPause);
        paused.command(AudioCommand::PlayPause);
        paused.command(AudioCommand::AutoAudition { track: 0, step: 0 });
        assert!(!paused.preview_drums[0].envelope.is_idle());
    }

    #[test]
    fn queued_automatic_audition_is_ignored_after_playback_starts() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);

        renderer.command(AudioCommand::PlayPause);
        renderer.command(Audio::snapshot(&project));
        renderer.command(AudioCommand::AutoAudition { track: 0, step: 0 });

        assert!(renderer.playing);
        assert!(renderer.preview_drums[0].envelope.is_idle());
    }

    #[test]
    fn explicit_audition_remains_available_while_playing() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.command(AudioCommand::PlayPause);
        renderer.command(AudioCommand::Audition { track: 0, step: 0 });

        assert!(!renderer.preview_drums[0].envelope.is_idle());
    }

    #[test]
    fn empty_step_audition_uses_the_track_input_accent_default() {
        let mut project = Project::new();
        project.tracks[0].input_accent = true;
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.audition_once(0, 0);

        assert!(renderer.preview_drums[0].accent);
    }

    #[test]
    fn preview_drum_audition_uses_effective_pan() {
        let mut project = Project::new();
        project.tracks[0].pan = Percent::new(100).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.audition_once(0, 0);
        assert_eq!(renderer.preview_drums[0].pan.next_value(), 100.0);

        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                pan: Percent::new(25),
                ..Default::default()
            },
        });
        renderer.command(Audio::snapshot(&project));
        renderer.audition_once(0, 0);
        assert_eq!(renderer.preview_drums[0].pan.next_value(), 25.0);
    }
    #[test]
    fn offline_render_is_deterministic_and_finite() {
        let mut p = Project::new();
        p.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let a = render_offline(&p, 8_000, 2_000);
        let b = render_offline(&p, 8_000, 2_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(a.iter().any(|(l, _)| l.abs() > 0.001));
    }

    #[test]
    fn tom_and_cymbal_render_deterministically_and_finitely() {
        fn render_drum(track_index: usize) -> Vec<(f32, f32)> {
            let mut project = Project::new();
            for track in &mut project.tracks {
                track.muted = true;
            }
            project.tracks[track_index].muted = false;
            project.patterns[0].tracks[track_index].steps[0] = Some(StepEvent::Trigger {
                accent: true,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            render_offline(&project, 8_000, 2_000)
        }

        let tom = render_drum(3);
        let cymbal = render_drum(4);
        assert_eq!(tom, render_drum(3));
        assert_eq!(cymbal, render_drum(4));
        assert!(tom.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(cymbal.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(tom.iter().any(|(l, _)| l.abs() > 0.001));
        assert!(cymbal.iter().any(|(l, _)| l.abs() > 0.001));
        assert_ne!(tom, cymbal);
    }

    #[test]
    fn active_chord_and_lead_render_is_finite_at_44100_hz() {
        let mut project = Project::new();
        for track in &mut project.tracks[..3] {
            track.muted = true;
        }
        for step in [0, 4, 8, 12] {
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[step] = Some(StepEvent::BassNote {
                degree: 1,
                octave: 2,
                accent: false,
                slide: false,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[step] = Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[step] = Some(StepEvent::Note {
                degree: 5,
                octave: 4,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
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
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.boundary(0);
            (0..400)
                .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        }

        fn bass_peak(accent: bool) -> f32 {
            let mut project = Project::new();
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
                degree: 1,
                octave: 3,
                accent,
                slide: false,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.boundary(0);
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
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: true,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        for _ in 0..40 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[0].bass_accent_envelope.value() > 0.7);

        renderer.boundary(0);
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        renderer.command(Audio::snapshot(&project));
        for _ in 0..40 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[0].bass_accent_envelope.value() > 0.5);
    }

    #[test]
    fn bass_slide_is_legato_and_reaches_pitch_in_sixty_milliseconds() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::BassNote {
            degree: 8,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        for _ in 0..80 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        let vca_before_slide = renderer.synth[0].bass_vca.value();
        let contour_before_slide = renderer.synth[0].bass_filter_envelope.value();
        let starting_frequency = renderer.synth[0].freq.next_value();

        renderer.boundary(0);
        assert_eq!(renderer.synth[0].bass_vca.value(), vca_before_slide);
        assert_eq!(
            renderer.synth[0].bass_filter_envelope.value(),
            contour_before_slide
        );
        let first_frequency = renderer.synth[0].freq.next_value();
        assert!(first_frequency > starting_frequency);
        assert!(first_frequency < starting_frequency * 2.0);
        Renderer::render_synth(
            &mut renderer.synth[0],
            renderer.sr,
            &[0.0; ParameterId::ALL.len()],
        );
        assert!(renderer.synth[0].bass_vca.value() >= vca_before_slide);
        assert!(renderer.synth[0].bass_filter_envelope.value() < contour_before_slide);
        for _ in 1..480 {
            renderer.synth[0].freq.next_value();
        }
        let final_frequency = renderer.synth[0].freq.next_value();
        assert!((final_frequency - starting_frequency * 2.0).abs() < 0.01);
    }

    #[test]
    fn lead_slide_uses_portamento_without_retriggering_the_adsr() {
        let mut project = Project::new();
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[0] = Some(StepEvent::LeadNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(StepEvent::LeadNote {
            degree: 8,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let lead = LEAD_TRACK_INDEX - SYNTH_TRACK_START;

        renderer.boundary(0);
        for _ in 0..1_000 {
            Renderer::render_synth(
                &mut renderer.synth[lead],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert_eq!(
            renderer.synth[lead].env.stage,
            crate::dsp::EnvStage::Sustain
        );
        let starting_frequency = renderer.synth[lead].freq.next_value();

        renderer.boundary(0);
        assert_eq!(
            renderer.synth[lead].env.stage,
            crate::dsp::EnvStage::Sustain
        );
        let first_frequency = renderer.synth[lead].freq.next_value();
        assert!(first_frequency > starting_frequency);
        assert!(first_frequency < starting_frequency * 2.0);
        for _ in 0..600 {
            renderer.synth[lead].freq.next_value();
        }
        let final_frequency = renderer.synth[lead].freq.next_value();
        assert!((final_frequency - starting_frequency * 2.0).abs() < 0.01);
    }

    #[test]
    fn lead_slide_uses_source_portamento_lock_and_tie_override() {
        fn note(degree: u8, slide: bool, portamento: u8) -> StepEvent {
            StepEvent::LeadNote {
                degree,
                octave: 3,
                accent: false,
                slide,
                condition: Default::default(),
                retrigger_count: 1,
                locks: ParameterLocks {
                    portamento_time: Percent::new(portamento),
                    ..Default::default()
                },
            }
        }

        let mut project = Project::new();
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[0] = Some(note(1, true, 100));
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(note(8, false, 0));
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let lead = LEAD_TRACK_INDEX - SYNTH_TRACK_START;

        renderer.boundary(0);
        let source = renderer.synth[lead].freq.value();
        renderer.boundary(0);
        let first = renderer.synth[lead].freq.next_value();
        assert!(first > source && first < source * 1.01);

        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                portamento_time: Percent::new(0),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[2] = Some(note(8, false, 100));
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        renderer.boundary(0);
        renderer.boundary(0);
        assert!((renderer.synth[lead].freq.value() - source * 2.0).abs() < 0.01);
    }

    #[test]
    fn bass_idle_path_clears_accent_before_an_unaccented_note() {
        let mut project = Project::new();
        for (step, accent) in [true, false].into_iter().enumerate() {
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[step] = Some(StepEvent::BassNote {
                degree: step as u8 + 1,
                octave: 3,
                accent,
                slide: false,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
        }
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        for _ in 0..40 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[0].bass_accent_envelope.value() > 0.7);
        renderer.synth[0].gate_off();
        renderer.synth[0].active = false;
        for _ in 0..1_000 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert_eq!(renderer.synth[0].bass_accent_envelope.value(), 0.0);
        assert_eq!(renderer.synth[0].bass_filter_envelope.value(), 0.0);

        renderer.boundary(0);
        Renderer::render_synth(
            &mut renderer.synth[0],
            renderer.sr,
            &[0.0; ParameterId::ALL.len()],
        );
        assert_eq!(renderer.synth[0].bass_accent_envelope.value(), 0.0);
    }

    #[test]
    fn empty_bass_step_releases_the_fixed_vca_gate() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        for _ in 0..80 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[0].bass_vca.value() > 0.99);

        renderer.boundary(1);
        for _ in 0..500 {
            Renderer::render_synth(
                &mut renderer.synth[0],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[0].is_idle());
    }

    #[test]
    fn representative_groove_has_calibrated_rms_and_safe_peak() {
        let mut project = Project::new();
        for step in [0, 4, 8, 12] {
            project.patterns[0].tracks[0].steps[step] = Some(StepEvent::Trigger {
                accent: step == 0,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
        }
        for step in [4, 12] {
            project.patterns[0].tracks[1].steps[step] = Some(StepEvent::Trigger {
                accent: true,
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
        }
        for step in 0..16 {
            project.patterns[0].tracks[2].steps[step] = Some(StepEvent::Trigger {
                accent: step == 6 || step == 14,
                condition: Default::default(),
                retrigger_count: 1,
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
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[step] = Some(StepEvent::BassNote {
                degree,
                octave: 2,
                accent: step == 8,
                slide: step == 4,
                condition: Default::default(),
                retrigger_count: 1,
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
            condition: Default::default(),
            retrigger_count: 1,
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
    fn zero_reverb_return_suppresses_the_wet_signal() {
        let mut dry_project = Project::new();
        dry_project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let mut wet_project = dry_project.clone();
        wet_project.tracks[0].reverb_send = Percent::new(100).unwrap();
        wet_project.globals.reverb_return = Percent::ZERO;

        assert_eq!(
            render_offline(&dry_project, 8_000, 10_000),
            render_offline(&wet_project, 8_000, 10_000)
        );
    }

    #[test]
    fn delay_return_is_not_routed_into_reverb() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.tracks[0].delay_send = Percent::new(100).unwrap();
        project.globals.delay_feedback = Percent::ZERO;
        project.globals.reverb_return = Percent::new(100).unwrap();

        let mut without_reverb_return = project.clone();
        without_reverb_return.globals.reverb_return = Percent::ZERO;
        assert_eq!(
            render_offline(&project, 8_000, 10_000),
            render_offline(&without_reverb_return, 8_000, 10_000)
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
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                level: Some(Percent::ZERO),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        let locked = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        renderer.boundary(0);
        let restored = (0..40)
            .map(|_| Renderer::render_drum(&mut renderer.drums[0], 0, renderer.sr, 0.0, 0.0).0)
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(locked, 0.0);
        assert!(restored > 0.0001);
    }

    #[test]
    fn current_step_locks_are_latched_until_the_next_boundary() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START]
            .steps
            .resize(1, None);
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));

        let mut edited = project.clone();
        let Some(StepEvent::BassNote { locks, .. }) =
            edited.patterns[0].tracks[SYNTH_TRACK_START].steps[0].as_mut()
        else {
            panic!("expected Bass note")
        };
        locks.cutoff = Percent::new(80);
        renderer.command(Audio::snapshot(&edited));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));

        renderer.boundary(0);
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(80));
    }

    #[test]
    fn bass_tie_locks_inherit_source_note_and_allow_tie_overrides() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                level: Percent::new(30),
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                resonance: Percent::new(70),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[2] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, None);

        renderer.boundary(0);
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, Percent::new(70));

        renderer.boundary(0);
        assert_eq!(renderer.synth[0].locks.level, Percent::new(30));
        assert_eq!(renderer.synth[0].locks.cutoff, Percent::new(20));
        assert_eq!(renderer.synth[0].locks.resonance, Percent::new(70));
    }

    #[test]
    fn wrapped_bass_tie_locks_inherit_from_wrapped_source_note() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START]
            .steps
            .resize(3, None);
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[2] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                cutoff: Percent::new(25),
                ..Default::default()
            },
        });
        let status = Arc::new(AudioStatus::default());
        let renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        assert_eq!(
            renderer.locks_at(SYNTH_TRACK_START, 0).cutoff,
            Percent::new(25)
        );
    }

    #[test]
    fn fader_snapshot_ramps_an_active_drum_mixer_value_over_thirty_ms() {
        let mut project = Project::new();
        project.tracks[0].level = Percent::new(100).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
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
        Renderer::apply_synth_params(
            &project,
            48_000.0,
            SYNTH_TRACK_START,
            locks,
            &mut voice,
            240,
        );
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
        Renderer::apply_synth_params(
            &project,
            48_000.0,
            SYNTH_TRACK_START,
            locks,
            &mut voice,
            240,
        );
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
    fn juno_and_sh101_noise_controls_use_instrument_specific_source_ranges() {
        let project = AudioProject::from_project(&Project::new());
        let locks = ParameterLocks {
            noise: Percent::new(100),
            ..Default::default()
        };
        let mut juno = SynthVoice::new(48_000.0);
        Renderer::apply_synth_params(&project, 48_000.0, CHORD_TRACK_INDEX, locks, &mut juno, 0);
        let mut sh101 = SynthVoice::new(48_000.0);
        Renderer::apply_synth_params(&project, 48_000.0, LEAD_TRACK_INDEX, locks, &mut sh101, 0);

        assert!((juno.noise_level.value() - 0.35).abs() < f32::EPSILON);
        assert!((sh101.noise_level.value() - 1.0).abs() < f32::EPSILON);
        assert!(sh101.noise_level.value() > juno.noise_level.value() * 2.0);
    }

    #[test]
    fn chord_track_triggers_close_position_triads_and_alternates_voice_groups() {
        let mut project = Project::new();
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: ParameterLocks {
                cutoff: Percent::new(20),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[1] = Some(StepEvent::Tie {
            locks: ParameterLocks {
                resonance: Percent::new(70),
                ..Default::default()
            },
        });
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[2] = Some(StepEvent::Note {
            degree: 4,
            octave: 3,
            accent: true,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);

        renderer.boundary(0);
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

        renderer.boundary(0);
        assert_eq!(renderer.chord.group, first_group);
        for voice in &renderer.chord.voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.locks.cutoff, Percent::new(20));
            assert_eq!(voice.locks.resonance, Percent::new(70));
        }
        renderer.boundary(0);
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

        renderer.boundary(0);
        assert!(!renderer.chord.active);
    }

    #[test]
    fn chord_track_renders_four_note_shapes_with_overlap_capacity() {
        let mut project = Project::new();
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::SeventhRoot),
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
        renderer.boundary(0);
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
    fn reused_chord_group_clears_slots_not_used_by_the_new_shape() {
        fn trigger(shape: ChordShape, arpeggio: ArpeggioConfig) -> SynthTrigger {
            SynthTrigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                chord_shape: Some(shape),
                arpeggio,
            }
        }

        let project = AudioProject::from_project(&Project::new());
        let mut pool = ChordVoicePool::new(48_000);
        Renderer::trigger_chord(
            &project,
            48_000.0,
            trigger(ChordShape::SeventhRoot, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );
        let first_group = pool.group;
        Renderer::trigger_chord(
            &project,
            48_000.0,
            trigger(ChordShape::TriadRoot, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );
        Renderer::trigger_chord(
            &project,
            48_000.0,
            trigger(ChordShape::TriadRoot, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );

        assert_eq!(pool.group, first_group);
        assert_eq!(pool.group_voice_counts[first_group], 3);
        let unused = &pool.voices[first_group * CHORD_GROUP_SIZE + 3];
        assert_eq!(unused.env.stage, crate::dsp::EnvStage::Idle);
        assert!(!unused.active);

        Renderer::trigger_chord(
            &project,
            48_000.0,
            trigger(
                ChordShape::SeventhRoot,
                ArpeggioConfig {
                    enabled: true,
                    ..Default::default()
                },
            ),
            Default::default(),
            &mut pool,
        );
        assert_eq!(pool.group_voice_counts[pool.group], 1);
        assert!(
            pool.voices[pool.group * CHORD_GROUP_SIZE + 1..(pool.group + 1) * CHORD_GROUP_SIZE]
                .iter()
                .all(|voice| voice.env.stage == crate::dsp::EnvStage::Idle)
        );
    }

    #[test]
    fn releasing_chord_group_keeps_its_latched_level_after_a_new_lock() {
        fn note(locks: ParameterLocks) -> StepEvent {
            StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
                locks,
            }
        }

        let mut project = Project::new();
        for (index, track) in project.tracks.iter_mut().enumerate() {
            track.muted = index != CHORD_TRACK_INDEX;
        }
        let mut old_locks = ParameterLocks::default();
        assert!(old_locks.set(
            ParameterId::Level,
            crate::model::ParameterValue::Percent(Percent::new(100).unwrap()),
        ));
        let mut new_locks = ParameterLocks::default();
        assert!(new_locks.set(
            ParameterId::Level,
            crate::model::ParameterValue::Percent(Percent::ZERO),
        ));
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(note(old_locks));
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[1] = Some(note(new_locks));

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.process_track_action(CHORD_TRACK_INDEX, 0, false, true);
        for _ in 0..500 {
            renderer.next();
        }
        renderer.process_track_action(CHORD_TRACK_INDEX, 1, false, true);
        let releasing_group = 1 - renderer.chord.group;
        assert_eq!(
            renderer.chord.voices[releasing_group * CHORD_GROUP_SIZE]
                .level
                .value(),
            100.0
        );

        // Wait for the new silent group to finish its fader ramp, then prove
        // that the old release is still audible.
        for _ in 0..500 {
            renderer.next();
        }
        let energy: f32 = (0..500)
            .map(|_| {
                let (left, right) = renderer.next();
                (left * left + right * right) * 0.5
            })
            .sum();
        assert!(energy > 0.000_1, "latched tail energy {energy}");
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
            track.muted = index != CHORD_TRACK_INDEX;
        }
        dry_project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let mut wet_project = dry_project.clone();
        wet_project.tracks[CHORD_TRACK_INDEX].reverb_send = Percent::new(100).unwrap();

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
            track.muted = index != CHORD_TRACK_INDEX;
        }
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: ArpeggioConfig {
                enabled: true,
                r#type: ArpeggioType::UpDown,
                rate: ArpeggioRate::ThirtySecond,
            },
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[1] = None;
        let output = render_offline(&project, 8_000, 8_000);
        assert!(
            output
                .iter()
                .all(|(left, right)| left.is_finite() && right.is_finite())
        );
    }
}
