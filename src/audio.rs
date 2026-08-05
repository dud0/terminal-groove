pub use crate::model::PatternIndexMap;
use crate::{
    dsp::{
        Adsr, Biquad, DcBlock, Delay, EnvStage, EnvelopeProfile, LadderFilter, Lfo, MasterLimiter,
        PolyBlepOsc, Reverb, Smoother, StereoChorus, equal_power_pan, exp_map_f32,
    },
    engine::{GateAction, StepClock, synth_action},
    model::{
        ArpeggioConfig, ArpeggioRate, ArpeggioType, ChordShape, ChorusMode, Globals, Instrument,
        LfoAssignments, MAX_STEP_COUNT, ParameterId, ParameterLocks, Percent, Project, StepEvent,
        TRACK_COUNT, TriggerCondition, Waveform,
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
    pan: Percent,
    muted: bool,
    delay_send: Percent,
    reverb_send: Percent,
    swing: Percent,
    instrument: Instrument,
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
                    instrument: t.instrument,
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

mod command;
mod effects;
mod queue;
mod renderer;
mod scheduler;
mod synthesis;
mod voices;

pub use command::AudioCommand;
pub(crate) use effects::{modulated_percent, pitch_modulated_frequency, render};
pub use queue::{Audio, QueueFull};
pub(crate) use renderer::{PreviewAction, Renderer, ScheduledTrackAction};
pub(crate) use voices::*;

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
    fn scheduled_actions_freeze_while_paused() {
        let mut project = Project::new();
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 2,
            locks: Default::default(),
        });
        project.tracks[3].swing = Percent::new(75).unwrap();

        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(1);
        let remaining_before = renderer
            .scheduled
            .iter()
            .find(|action| matches!(action, Some(action) if action.track == 3 && !action.retrigger))
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
            .find(|action| matches!(action, Some(action) if action.track == 3 && !action.retrigger))
            .and_then(|action| action.as_ref())
            .unwrap()
            .remaining;
        assert_eq!(remaining_after, remaining_before);
        assert!(!renderer.synth[0].active);
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
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            project.patterns[0].tracks[4].steps[step] = Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
            project.patterns[0].tracks[5].steps[step] = Some(StepEvent::Note {
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
            project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
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
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: true,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        for _ in 0..40 {
            renderer.synth[0].accent_gain.next_value();
        }
        assert!(renderer.synth[0].accent_gain.next_value() > 1.3);

        renderer.boundary(0);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
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
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::BassNote {
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
        let stage_before_slide = renderer.synth[0].env.stage;
        let starting_frequency = renderer.synth[0].freq.next_value();

        renderer.boundary(0);
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
            project.patterns[0].tracks[3].steps[step] = Some(StepEvent::BassNote {
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
        project.patterns[0].tracks[3].steps.resize(1, None);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
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
            edited.patterns[0].tracks[3].steps[0].as_mut()
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
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::BassNote {
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
        project.patterns[0].tracks[3].steps.resize(3, None);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[2] = Some(StepEvent::BassNote {
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

        assert_eq!(renderer.locks_at(3, 0).cutoff, Percent::new(25));
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
            condition: Default::default(),
            retrigger_count: 1,
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
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
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
            condition: Default::default(),
            retrigger_count: 1,
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
            condition: Default::default(),
            retrigger_count: 1,
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
