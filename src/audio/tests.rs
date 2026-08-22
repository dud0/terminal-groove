#[cfg(test)]
mod tests {
    use super::voices::{SynthTrigger, SynthVoiceKind};
    use super::*;
    use crate::model::{
        ArpeggioRate, ArpeggioType, ChordShape, DistortionParameters,
        FlangerParameters, LEAD_TRACK_INDEX, FM_TRACK_INDEX, FmAlgorithm, LfoConfig, LfoDivision, LfoRate, LfoWaveform,
        ParameterValue, PhaserParameters, RIMSHOT_TRACK_INDEX, TrackEffects, TrackKind,
        Microtiming,
    };
    use std::{fs, time::Instant};

    fn parameter_locks<const N: usize>(pairs: [(ParameterId, u8); N]) -> ParameterLocks {
        ParameterLocks::from_pairs(pairs.map(|(parameter, value)| {
            (
                parameter,
                ParameterValue::Percent(Percent::new(value).unwrap()),
            )
        }))
    }

    fn performance_project() -> Project {
        let mut project = Project::new();
        let saturated_effects = TrackEffects {
            chorus: Default::default(),
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
            bit_crusher: Default::default(),
        };
        let lfo = Some(LfoConfig {
            enabled: true,
            waveform: LfoWaveform::Sine,
            reset_on_trigger: false,
            start_phase: Percent::ZERO,
            rate: LfoRate::Synced {
                division: LfoDivision::Sixteenth,
            },
            depth: Percent::new(35).unwrap(),
        });
        for track in &mut project.tracks {
            track.delay_send = Percent::new(100).unwrap();
            track.reverb_send = Percent::new(100).unwrap();
            track.effects = saturated_effects;
            for parameter in ParameterId::ALL {
                if track.supports_lfo(parameter) {
                    assert!(track.set_lfo(parameter, lfo));
                }
            }
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
                    recipe: crate::model::DrumRecipeSlot::ONE,
                    condition: Default::default(),
                    retrigger_count: 4,
                    microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            project.patterns[0].tracks[FM_TRACK_INDEX].steps[step] = Some(StepEvent::Note {
                degree: (step % 7 + 1) as u8,
                octave: 3,
                accent: step % 4 == 0,
                chord_shape: Some(ChordShape::SeventhRoot),
                arpeggio: ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 4,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[step] = Some(StepEvent::LeadNote {
                degree: (step % 7 + 1) as u8,
                octave: 4,
                accent: step % 4 == 0,
                slide: step % 3 == 0,
                condition: Default::default(),
                retrigger_count: 4,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        project
    }

    fn assignable_worst_case_project() -> Project {
        let mut project = performance_project();
        let chord = project.tracks[CHORD_TRACK_INDEX].clone();
        for track in 0..TRACK_COUNT {
            project.tracks[track] = chord.clone();
            for step in 0..16 {
                project.patterns[0].tracks[track].steps[step] = Some(StepEvent::Note {
                    degree: (step % 7 + 1) as u8,
                    octave: 3,
                    accent: step % 4 == 0,
                    chord_shape: Some(ChordShape::SeventhRoot),
                    arpeggio: ArpeggioConfig::default(),
                    condition: Default::default(),
                    retrigger_count: 4,
                    microtiming: Microtiming::ZERO,
                    locks: Default::default(),
                });
            }
        }
        project
    }

    fn arm_second_worst_case_voicing_groups(renderer: &mut Renderer) {
        let trigger = SynthTrigger {
            degree: 5,
            octave: 3,
            accent: true,
            slide: false,
            chord_shape: Some(ChordShape::SeventhRoot),
            arpeggio: ArpeggioConfig::default(),
        };
        for track in 0..TRACK_COUNT {
            if !renderer.project.tracks[track]
                .instrument_kind()
                .supports_voicing()
            {
                continue;
            }
            let pool = &mut renderer.voicings[track];
            Renderer::trigger_chord(
                &renderer.project,
                renderer.sr,
                track,
                trigger,
                ParameterLocks::default(),
                pool,
            );
            assert_eq!(pool.group_voice_counts, [4, 4]);
            assert!(
                pool.voices
                    .iter()
                    .all(|voice| voice.env.stage != crate::dsp::EnvStage::Idle)
            );
        }
    }

    fn allocator_counts() -> (usize, usize) {
        crate::test_allocator::counts()
    }

    #[test]
    fn effect_slots_cover_every_track() {
        let mut slots = [false; TRACK_COUNT];
        for track in 0..TRACK_COUNT {
            let slot = effect_slot(track).unwrap();
            assert!(!slots[slot], "slot {slot} is shared by logical tracks");
            slots[slot] = true;
        }
        assert!(slots.into_iter().all(|used| used));
        assert_eq!(effect_slot(TRACK_COUNT), None);
    }

    #[test]
    fn chord_highpass_reset_preserves_constructor_coefficients() {
        let mut voice = SynthVoice::new(48_000.0);
        let coefficients = voice.chord_highpass.coefficients();
        assert_ne!(coefficients, [1.0, 0.0, 0.0, 0.0, 0.0]);
        voice.chord_highpass.process(1.0);
        voice.reset_to_idle();
        assert_eq!(voice.chord_highpass.coefficients(), coefficients);

        let mut fresh = SynthVoice::new(48_000.0);
        for input in [1.0, 0.0, -0.25, 0.5] {
            assert_eq!(
                voice.chord_highpass.process(input),
                fresh.chord_highpass.process(input)
            );
        }
    }

    #[test]
    fn zero_attack_synth_starts_from_silence() {
        for track in [
            SYNTH_TRACK_START,
            CHORD_TRACK_INDEX,
            LEAD_TRACK_INDEX,
            FM_TRACK_INDEX,
        ] {
            let mut project = Project::new();
            match &mut project.tracks[track].instrument {
                Instrument::Bass(_) => {}
                Instrument::Chord(parameters) => parameters.attack = Percent::ZERO,
                Instrument::Lead(parameters) => parameters.attack = Percent::ZERO,
                Instrument::Fm(parameters) => parameters.attack = Percent::ZERO,
                _ => unreachable!(),
            }
            let audio = AudioProject::from_project(&project);
            let mut voice = SynthVoice::new(8_000.0);
            Renderer::configure_synth_voice(
                &audio,
                8_000.0,
                track,
                SynthTrigger {
                    degree: 1,
                    octave: 3,
                    accent: false,
                    slide: false,
                    chord_shape: None,
                    arpeggio: Default::default(),
                },
                ParameterLocks::default(),
                &mut voice,
            );

            let first = Renderer::render_synth(
                &mut voice,
                8_000.0,
                &[0.0; ParameterId::ALL.len()],
            )
            .0;
            assert_eq!(first, 0.0, "track {track} did not start at silence");

            let mut peak: f32 = 0.0;
            for _ in 0..256 {
                peak = peak.max(
                    Renderer::render_synth(
                        &mut voice,
                        8_000.0,
                        &[0.0; ParameterId::ALL.len()],
                    )
                    .0
                    .abs(),
                );
            }
            assert!(peak > 0.001, "track {track} never became audible");
        }
    }

    #[test]
    fn hard_mono_retrigger_is_continuous() {
        let mut project = Project::new();
        if let Instrument::Lead(parameters) = &mut project.tracks[LEAD_TRACK_INDEX].instrument {
            parameters.attack = Percent::new(50).unwrap();
        }
        let audio = AudioProject::from_project(&project);
        let mut voice = SynthVoice::new(8_000.0);
        let first_trigger = SynthTrigger {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            chord_shape: None,
            arpeggio: Default::default(),
        };
        Renderer::configure_synth_voice(
            &audio,
            8_000.0,
            LEAD_TRACK_INDEX,
            first_trigger,
            ParameterLocks::default(),
            &mut voice,
        );
        let offsets = [0.0; ParameterId::ALL.len()];
        let mut previous = 0.0;
        for _ in 0..256 {
            previous = Renderer::render_synth(&mut voice, 8_000.0, &offsets).0;
        }

        Renderer::configure_synth_voice(
            &audio,
            8_000.0,
            LEAD_TRACK_INDEX,
            SynthTrigger {
                degree: 5,
                ..first_trigger
            },
            ParameterLocks::default(),
            &mut voice,
        );
        let first_after_retrigger = Renderer::render_synth(&mut voice, 8_000.0, &offsets).0;
        assert_eq!(first_after_retrigger, previous);
        assert_eq!(voice.env.stage, crate::dsp::EnvStage::Attack);
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
        let project = assignable_worst_case_project();
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
        arm_second_worst_case_voicing_groups(&mut renderer);
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
            AudioCommand::StartRecording,
            AudioCommand::StopRecording,
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

        let mut reassigned = project.clone();
        reassigned.tracks[0] = Project::new().tracks[1].clone();
        reassigned.patterns[0].tracks[0].steps.fill(None);
        producer.push(Audio::snapshot(&reassigned)).unwrap();
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
    fn stopped_mono_callback_records_the_internal_stereo_audition() {
        let mut project = performance_project();
        project.tracks[0].pan = Percent::new(0).unwrap();
        let status = Arc::new(AudioStatus::default());
        let (retire, _retired) = RingBuffer::new(32);
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        let (recording, mut captured) = RingBuffer::new(600);
        renderer.recording = recording::RecordingProducer::new(recording, status);
        renderer.command(AudioCommand::StartRecording);
        renderer.command(AudioCommand::Audition { track: 0, step: 0 });
        let (command_producer, mut commands) = RingBuffer::new(1);
        drop(command_producer);
        let mut mono = vec![0.0_f32; 512];
        render(
            &mut mono,
            1,
            48_000,
            &renderer.status.clone(),
            &mut renderer,
            &mut commands,
            |sample| sample,
        );
        renderer.command(AudioCommand::StopRecording);

        let mut frames = Vec::new();
        while let Ok(item) = captured.pop() {
            match item {
                recording::RecordingItem::Frame(left, right) => frames.push((left, right)),
                recording::RecordingItem::End { overflowed } => {
                    assert!(!overflowed);
                    break;
                }
            }
        }
        assert_eq!(frames.len(), 512);
        assert!(frames.iter().any(|(left, right)| left != right));
        assert!(!renderer.playing);
    }

    #[test]
    fn pending_snapshot_path_is_allocation_free_when_retirement_is_full() {
        let project = assignable_worst_case_project();
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
    fn pending_snapshot_applies_after_ui_reaps_without_another_command() {
        let project = Project::new();
        let status = Arc::new(AudioStatus::default());
        let (mut retire, mut retired) = RingBuffer::new(1);
        retire
            .push(Box::new(AudioProject::from_project(&project)))
            .unwrap();
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        let mut edited = project.clone();
        edited.globals.tempo_bpm = 137;
        renderer.command(Audio::snapshot(&edited));
        assert!(renderer.pending.is_some());
        let (producer, mut commands) = RingBuffer::new(1);
        drop(producer);
        let mut output = [];

        retired.pop().unwrap();
        render(
            &mut output,
            2,
            48_000,
            &status,
            &mut renderer,
            &mut commands,
            |sample| sample,
        );

        assert!(renderer.pending.is_none());
        assert_eq!(renderer.project.globals.tempo_bpm, 137);
    }

    #[test]
    fn callback_processes_only_the_fixed_command_budget() {
        let project = Project::new();
        let status = Arc::new(AudioStatus::default());
        let (retire, _retired) = RingBuffer::new(16);
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        let (mut producer, mut commands) = RingBuffer::new(16);
        for _ in 0..10 {
            producer.push(AudioCommand::PlayPause).unwrap();
        }
        let mut output = [];

        render(
            &mut output,
            2,
            48_000,
            &status,
            &mut renderer,
            &mut commands,
            |sample| sample,
        );

        assert_eq!(producer.slots(), 14);
        assert!(!renderer.playing);
    }

    #[test]
    fn callback_coalesces_identity_snapshots_and_retires_every_box() {
        let project = Project::new();
        let status = Arc::new(AudioStatus::default());
        let (retire, mut retired) = RingBuffer::new(8);
        let mut renderer = Renderer::new_with_retirement(
            Box::new(AudioProject::from_project(&project)),
            48_000,
            status.clone(),
            retire,
        );
        let (mut producer, mut commands) = RingBuffer::new(8);
        for tempo in [121, 122, 123] {
            let mut edited = project.clone();
            edited.globals.tempo_bpm = tempo;
            producer.push(Audio::snapshot(&edited)).unwrap();
        }
        let mut output = [];

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
        assert_eq!(renderer.project.globals.tempo_bpm, 123);
        assert_eq!(producer.slots(), 8);
        let mut retired_count = 0;
        while retired.pop().is_ok() {
            retired_count += 1;
        }
        assert_eq!(retired_count, 3);
    }

    #[test]
    fn incremental_snapshots_reuse_unchanged_patterns_and_match_full_rebuilds() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        let base = AudioProject::from_project(&project);

        project.tracks[0].muted = true;
        let track_update = base.updated(
            &project,
            &crate::reducer::EditImpact {
                tracks: 1,
                ..Default::default()
            },
            PatternIndexMap::identity(),
        );
        assert!(Arc::ptr_eq(&base.patterns.0[0], &track_update.patterns.0[0]));
        assert!(Arc::ptr_eq(&base.patterns.0[1], &track_update.patterns.0[1]));
        assert!(track_update.tracks[0].muted);

        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            recipe: Default::default(),
            locks: Default::default(),
        });
        let sequence_update = track_update.updated(
            &project,
            &crate::reducer::EditImpact {
                sequences: vec![(0, 0)],
                ..Default::default()
            },
            PatternIndexMap::identity(),
        );
        let full = AudioProject::from_project(&project);
        assert!(!Arc::ptr_eq(
            &track_update.patterns.0[0],
            &sequence_update.patterns.0[0]
        ));
        assert!(Arc::ptr_eq(
            &track_update.patterns.0[1],
            &sequence_update.patterns.0[1]
        ));
        assert_eq!(sequence_update.patterns[0], full.patterns[0]);
        assert_eq!(sequence_update.patterns[1], full.patterns[1]);
    }

    #[test]
    fn structural_snapshots_reuse_only_patterns_that_survive_rebasing() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns.push(project.patterns[0].clone());
        let base = AudioProject::from_project(&project);

        let mut inserted = project.clone();
        inserted.patterns.insert(1, inserted.patterns[0].clone());
        let inserted_update = base.updated(
            &inserted,
            &crate::reducer::EditImpact {
                patterns_structural: true,
                ..Default::default()
            },
            PatternIndexMap::insert(0),
        );
        let inserted_full = AudioProject::from_project(&inserted);
        assert!(Arc::ptr_eq(&base.patterns.0[0], &inserted_update.patterns.0[0]));
        assert!(!Arc::ptr_eq(&base.patterns.0[0], &inserted_update.patterns.0[1]));
        assert!(Arc::ptr_eq(&base.patterns.0[1], &inserted_update.patterns.0[2]));
        assert!(Arc::ptr_eq(&base.patterns.0[2], &inserted_update.patterns.0[3]));
        assert_eq!(inserted_update.patterns[1], inserted_full.patterns[1]);

        let mut deleted = project.clone();
        deleted.patterns.remove(1);
        let deleted_update = base.updated(
            &deleted,
            &crate::reducer::EditImpact {
                patterns_structural: true,
                ..Default::default()
            },
            PatternIndexMap::delete(1),
        );
        assert!(Arc::ptr_eq(&base.patterns.0[0], &deleted_update.patterns.0[0]));
        assert!(Arc::ptr_eq(&base.patterns.0[2], &deleted_update.patterns.0[1]));
        assert_eq!(
            deleted_update.patterns[0],
            AudioProject::from_project(&deleted).patterns[0]
        );
        assert_eq!(
            deleted_update.patterns[1],
            AudioProject::from_project(&deleted).patterns[1]
        );
    }

    #[test]
    fn lead_audition_uses_direct_and_tied_note_data_and_locks() {
        let mut project = Project::new();
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[0] = Some(StepEvent::LeadNote {
            degree: 7,
            octave: 5,
            accent: true,
            slide: true,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Level, 20)]),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let lead = LEAD_TRACK_INDEX;
        renderer.audition_once(LEAD_TRACK_INDEX, 0);
        assert_eq!(renderer.preview[lead].locks.percent(ParameterId::Level), Percent::new(20));
        assert!(renderer.preview[lead].active);
        assert!(renderer.preview[lead].slide_armed);
        let direct_frequency = renderer.preview[lead].freq.value();

        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(StepEvent::Tie {
            locks: parameter_locks([(ParameterId::Level, 30)]),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.audition_once(LEAD_TRACK_INDEX, 1);
        assert!((renderer.preview[lead].freq.value() - direct_frequency).abs() < 0.01);
        assert_eq!(renderer.preview[lead].locks.percent(ParameterId::Level), Percent::new(30));
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
    #[ignore = "controlled local release benchmark; timing is host-dependent"]
    fn saturated_callback_benchmark() {
        const TRIALS: usize = 5;
        const WARMUP_CALLBACKS: usize = 128;
        const MEASURED_CALLBACKS: usize = 512;
        if cfg!(debug_assertions) {
            eprintln!("audio fixture: skipped; rerun with cargo test --release");
            return;
        }
        let project = assignable_worst_case_project();
        eprintln!(
            "audio fixture: os={} arch={} profile={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        for sample_rate in [44_100, 48_000, 96_000] {
            for buffer_frames in [128, 256, 512] {
                let mut durations = Vec::with_capacity(TRIALS * MEASURED_CALLBACKS);
                for _ in 0..TRIALS {
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
                    arm_second_worst_case_voicing_groups(&mut renderer);
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
                    assert!(output.iter().all(|sample| sample.is_finite()));
                }
                durations.sort_unstable();
                let count = durations.len();
                let median = durations[count / 2];
                let p95 = durations[(count * 95 / 100).min(count - 1)];
                let p99 = durations[(count * 99 / 100).min(count - 1)];
                let maximum = durations[count - 1];
                let budget_ns =
                    buffer_frames as u128 * 1_000_000_000_u128 / u128::from(sample_rate);
                eprintln!(
                    "audio fixture: sr={} buffer={} trials={} samples={} median={:.1}% p95={:.1}% p99={:.1}% max={:.1}% median_ns/frame={:.1}",
                    sample_rate,
                    buffer_frames,
                    TRIALS,
                    count,
                    median as f64 * 100.0 / budget_ns as f64,
                    p95 as f64 * 100.0 / budget_ns as f64,
                    p99 as f64 * 100.0 / budget_ns as f64,
                    maximum as f64 * 100.0 / budget_ns as f64,
                    median as f64 / buffer_frames as f64,
                );
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
    fn kick_pitch_uses_documented_peak_and_settled_mappings() {
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 4,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 4,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 2,
            microtiming: crate::model::Microtiming::ZERO,
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
    fn microtiming_delays_an_event_inside_its_slot() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(2, None);
        project.patterns[0].tracks[0].steps[1] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::new(25).unwrap(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        renderer.boundary(1);
        let action = renderer
            .scheduled
            .iter()
            .flatten()
            .find(|action| action.track == 0 && action.step == 1)
            .unwrap();
        assert_eq!(action.remaining, 249);
    }

    #[test]
    fn negative_microtiming_is_armed_before_the_nominal_boundary() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(2, None);
        project.patterns[0].tracks[0].steps[1] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::new(-25).unwrap(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        let action = renderer
            .scheduled
            .iter()
            .flatten()
            .find(|action| action.track == 0 && action.step == 1)
            .unwrap();
        assert_eq!(action.remaining, 749);
        assert_eq!(renderer.early_armed[0], Some(1));
        renderer.boundary(1);
        assert_eq!(renderer.early_armed[0], None);
        assert_eq!(renderer
            .scheduled
            .iter()
            .flatten()
            .filter(|action| action.track == 0 && action.step == 1 && !action.retrigger)
            .count(), 1);
    }

    #[test]
    fn retriggers_finish_before_a_following_early_event() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(2, None);
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 4,
            microtiming: Microtiming::new(50).unwrap(),
            locks: Default::default(),
        });
        project.patterns[0].tracks[0].steps[1] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::new(-50).unwrap(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);

        let next_start = renderer
            .scheduled
            .iter()
            .flatten()
            .find(|action| action.track == 0 && action.step == 1 && !action.retrigger)
            .unwrap()
            .remaining;
        let previous_retriggers: Vec<_> = renderer
            .scheduled
            .iter()
            .flatten()
            .filter(|action| action.track == 0 && action.step == 0 && action.retrigger)
            .map(|action| action.remaining)
            .collect();
        assert_eq!(previous_retriggers, vec![124, 249, 374]);
        assert!(previous_retriggers
            .iter()
            .all(|remaining| *remaining < next_start));
    }

    #[test]
    fn negative_microtiming_looks_ahead_across_an_ordinary_bar_boundary() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::new(-50).unwrap(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.next_steps[0] = 15;
        renderer.boundary(15);

        assert_eq!(renderer.early_armed[0], Some(0));
        assert_eq!(
            renderer
                .scheduled
                .iter()
                .flatten()
                .filter(|action| action.track == 0 && action.step == 0 && !action.retrigger)
                .count(),
            1
        );
        renderer.boundary(16);
        assert_eq!(renderer.early_armed[0], None);
        assert_eq!(
            renderer
                .scheduled
                .iter()
                .flatten()
                .filter(|action| action.track == 0 && action.step == 0 && !action.retrigger)
                .count(),
            1
        );
    }

    #[test]
    fn negative_microtiming_does_not_look_ahead_across_a_queued_transition() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::new(-50).unwrap(),
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.next_steps[0] = 15;
        renderer.queued_pattern = Some(1);
        renderer.boundary(15);

        assert_eq!(renderer.early_armed[0], None);
        assert!(renderer
            .scheduled
            .iter()
            .flatten()
            .all(|action| action.track != 0 || action.step != 0));
    }

    #[test]
    fn late_microtiming_and_retriggers_are_clamped_before_the_next_slot() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 4,
            microtiming: Microtiming::new(50).unwrap(),
            locks: Default::default(),
        });
        project.tracks[0].swing = Percent::new(75).unwrap();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(1);
        let actions: Vec<_> = renderer
            .scheduled
            .iter()
            .flatten()
            .filter(|action| action.track == 0 && action.step == 0)
            .collect();
        assert_eq!(actions.len(), 4);
        assert!(actions.iter().all(|action| action.remaining < 1_000));
        assert_eq!(
            actions.iter().find(|action| !action.retrigger).unwrap().remaining,
            811
        );
    }

    #[test]
    fn track_probability_gates_the_base_action_and_entire_retrigger_burst() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 4,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: crate::model::TriggerCondition::Chance {
                probability: Percent::ZERO,
            },
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::BassNote {
            degree: 2,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        assert!(renderer.synth[SYNTH_TRACK_START].active);
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::ZERO;
        renderer.boundary(1);
        assert!(!renderer.synth[SYNTH_TRACK_START].active);

        renderer.project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::new(100).unwrap();
        renderer.next_steps[SYNTH_TRACK_START] = 0;
        renderer.boundary(2);
        assert!(renderer.synth[SYNTH_TRACK_START].active);
        renderer.project.tracks[SYNTH_TRACK_START].probability = Percent::ZERO;
        renderer.next_steps[SYNTH_TRACK_START] = 1;
        renderer.boundary(3);
        assert!(renderer.synth[SYNTH_TRACK_START].active);
    }

    #[test]
    fn probability_rng_resets_on_stop() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            microtiming: crate::model::Microtiming::ZERO,
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
        assert!(!renderer.synth[SYNTH_TRACK_START].active);
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
    fn stop_releases_bass_vcas_and_resets_fm_state() {
        let mut project = Project::new();
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[FM_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: Default::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        renderer.audition_once(SYNTH_TRACK_START, 0);
        renderer.audition_once(FM_TRACK_INDEX, 0);
        let fm_live = renderer.voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        let fm_preview = renderer.preview_voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        for _ in 0..64 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            Renderer::render_synth(
                &mut renderer.preview[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            Renderer::render_synth(
                &mut renderer.voicings[FM_TRACK_INDEX].voices[fm_live],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            Renderer::render_synth(
                &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[fm_preview],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].bass_vca.value() > 0.0);
        assert!(renderer.preview[SYNTH_TRACK_START].bass_vca.value() > 0.0);
        assert!(renderer.voicings[FM_TRACK_INDEX].voices[fm_live]
            .fm_phases
            .iter()
            .any(|phase| *phase != 0.0));
        assert!(renderer.preview_voicings[FM_TRACK_INDEX].voices[fm_preview]
            .fm_phases
            .iter()
            .any(|phase| *phase != 0.0));

        renderer.command(AudioCommand::Stop);

        assert!(!renderer.synth[SYNTH_TRACK_START].is_idle());
        assert!(!renderer.preview[SYNTH_TRACK_START].is_idle());
        assert_eq!(renderer.voicings[FM_TRACK_INDEX].group_voice_counts, [0; 2]);
        assert_eq!(renderer.preview_voicings[FM_TRACK_INDEX].group_voice_counts, [0; 2]);
        for voice in renderer.voicings[FM_TRACK_INDEX]
            .voices
            .iter_mut()
            .chain(renderer.preview_voicings[FM_TRACK_INDEX].voices.iter_mut())
        {
            assert_eq!(voice.fm_phases, [0.0; 4]);
            assert_eq!(voice.fm_previous, [0.0; 4]);
            assert_eq!(voice.fm_filter.process(0.0), 0.0);
        }
        for _ in 0..1_000 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            Renderer::render_synth(
                &mut renderer.preview[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].is_idle());
        assert!(renderer.preview[SYNTH_TRACK_START].is_idle());
    }

    #[test]
    fn fully_idle_voicing_groups_are_retired_after_their_tail() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        let trigger = SynthTrigger {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            chord_shape: Some(ChordShape::SeventhRoot),
            arpeggio: ArpeggioConfig::default(),
        };
        Renderer::trigger_chord(
            &renderer.project,
            renderer.sr,
            FM_TRACK_INDEX,
            trigger,
            ParameterLocks::default(),
            &mut renderer.voicings[FM_TRACK_INDEX],
        );
        Renderer::trigger_chord(
            &renderer.project,
            renderer.sr,
            FM_TRACK_INDEX,
            trigger,
            ParameterLocks::default(),
            &mut renderer.voicings[FM_TRACK_INDEX],
        );
        assert_eq!(renderer.voicings[FM_TRACK_INDEX].group_voice_counts, [4; 2]);
        for voice in &mut renderer.voicings[FM_TRACK_INDEX].voices {
            voice.reset_to_idle();
        }

        renderer.next();

        assert_eq!(renderer.voicings[FM_TRACK_INDEX].group_voice_counts, [0; 2]);
    }

    #[test]
    fn silent_voicing_groups_finish_pending_effect_smoothing() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        let mut enabled = TrackEffects::default();
        enabled.distortion.mix = Percent::new(100).unwrap();
        renderer.voicing_effects[CHORD_TRACK_INDEX][0].configure(enabled, ParameterLocks::default(), 0);
        renderer.voicing_effects[CHORD_TRACK_INDEX][0].configure(TrackEffects::default(), ParameterLocks::default(), 8);

        assert!(!renderer.voicing_effects[CHORD_TRACK_INDEX][0].is_active());
        assert!(renderer.voicing_effects[CHORD_TRACK_INDEX][0].needs_processing());
        for _ in 0..8 {
            renderer.next();
        }
        assert!(!renderer.voicing_effects[CHORD_TRACK_INDEX][0].needs_processing());
    }

    #[test]
    fn active_drum_tail_still_advances_while_paused() {
        let status = Arc::new(AudioStatus::default());
        let mut renderer =
            Renderer::new(AudioProject::from_project(&Project::new()), 8_000, status);
        renderer.trigger_drum(
            0,
            false,
            crate::model::DrumRecipeSlot::ONE,
            ParameterLocks::default(),
        );
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Pan, 25)]),
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let a = render_offline(&p, 8_000, 2_000);
        let b = render_offline(&p, 8_000, 2_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(a.iter().any(|(l, _)| l.abs() > 0.001));
    }

    #[test]
    fn tom_cymbal_and_rimshot_render_deterministically_and_finitely() {
        fn render_drum(track_index: usize) -> Vec<(f32, f32)> {
            let mut project = Project::new();
            for track in &mut project.tracks {
                track.muted = true;
            }
            project.tracks[track_index].muted = false;
            project.patterns[0].tracks[track_index].steps[0] = Some(StepEvent::Trigger {
                accent: true,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            render_offline(&project, 8_000, 2_000)
        }

        let tom = render_drum(3);
        let cymbal = render_drum(4);
        let rimshot = render_drum(RIMSHOT_TRACK_INDEX);
        assert_eq!(tom, render_drum(3));
        assert_eq!(cymbal, render_drum(4));
        assert_eq!(rimshot, render_drum(RIMSHOT_TRACK_INDEX));
        assert!(tom.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(cymbal.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(rimshot.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
        assert!(tom.iter().any(|(l, _)| l.abs() > 0.001));
        assert!(cymbal.iter().any(|(l, _)| l.abs() > 0.001));
        assert!(rimshot.iter().any(|(l, _)| l.abs() > 0.001));
        assert_ne!(tom, cymbal);
        assert_ne!(cymbal, rimshot);
    }

    #[test]
    fn rimshot_controls_scale_modes_and_accent_emphasizes_the_crack() {
        fn triggered_voice(tune: u8, tone: u8, decay: u8, accent: bool) -> DrumVoice {
            let mut project = Project::new();
            let Instrument::Rimshot(parameters) =
                &mut project.tracks[RIMSHOT_TRACK_INDEX].instrument
            else {
                panic!("expected rimshot")
            };
            parameters.tune = Percent::new(tune).unwrap();
            parameters.tone = Percent::new(tone).unwrap();
            parameters.decay = Percent::new(decay).unwrap();
            project.patterns[0].tracks[RIMSHOT_TRACK_INDEX].steps[0] = Some(StepEvent::Trigger {
                accent,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
            renderer.boundary(0);
            renderer.drums.into_iter().nth(RIMSHOT_TRACK_INDEX).unwrap()
        }

        let default = triggered_voice(50, 50, 50, false);
        assert_eq!(default.rimshot_frequencies, [222.0, 500.0, 1_000.0]);

        let high_tune = triggered_voice(100, 50, 50, false);
        assert_eq!(high_tune.rimshot_frequencies, [444.0, 1_000.0, 2_000.0]);

        let short = triggered_voice(50, 50, 0, false);
        let long = triggered_voice(50, 50, 100, false);
        assert!(long.rimshot_decay_coefficients[0] > short.rimshot_decay_coefficients[0]);

        let dark = triggered_voice(50, 0, 50, false);
        let bright = triggered_voice(50, 100, 50, false);
        assert!(dark.rimshot_amplitudes[1] > bright.rimshot_amplitudes[1]);
        assert!(bright.rimshot_amplitudes[2] > dark.rimshot_amplitudes[2]);

        let accent = triggered_voice(50, 50, 50, true);
        assert!(accent.rimshot_amplitudes[2] > default.rimshot_amplitudes[2]);
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
                microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
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
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.boundary(0);
            (0..400)
                .map(|_| {
                    Renderer::render_synth(
                        &mut renderer.synth[SYNTH_TRACK_START],
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
    fn rendered_bass_resonance_emphasizes_cutoff_relative_to_the_fundamental() {
        fn cutoff_to_fundamental_ratio(resonance: u8) -> f32 {
            let mut project = Project::new();
            if let Instrument::Bass(parameters) = &mut project.tracks[SYNTH_TRACK_START].instrument
            {
                parameters.cutoff = Percent::new(25).unwrap();
                parameters.resonance = Percent::new(resonance).unwrap();
                parameters.filter_envelope = Percent::ZERO;
            } else {
                panic!("expected the first synth track to be Bass");
            }
            project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
            renderer.boundary(0);
            let sample_rate = 48_000.0;
            let fundamental = 130.8128;
            let cutoff_harmonic = fundamental * 2.0;
            let mut fundamental_sine = 0.0;
            let mut fundamental_cosine = 0.0;
            let mut cutoff_sine = 0.0;
            let mut cutoff_cosine = 0.0;
            let mut count = 0;
            for sample in 0..4_096 {
                let output = Renderer::render_synth(
                    &mut renderer.synth[SYNTH_TRACK_START],
                    renderer.sr,
                    &[0.0; ParameterId::ALL.len()],
                )
                .0;
                if sample >= 1_024 {
                    let fundamental_phase =
                        std::f32::consts::TAU * fundamental * sample as f32 / sample_rate;
                    fundamental_sine += output * fundamental_phase.sin();
                    fundamental_cosine += output * fundamental_phase.cos();
                    let cutoff_phase =
                        std::f32::consts::TAU * cutoff_harmonic * sample as f32 / sample_rate;
                    cutoff_sine += output * cutoff_phase.sin();
                    cutoff_cosine += output * cutoff_phase.cos();
                    count += 1;
                }
            }
            let amplitude = |sine: f32, cosine: f32| {
                2.0 * (sine * sine + cosine * cosine).sqrt() / count as f32
            };
            amplitude(cutoff_sine, cutoff_cosine) / amplitude(fundamental_sine, fundamental_cosine)
        }

        let without_resonance = cutoff_to_fundamental_ratio(0);
        let with_resonance = cutoff_to_fundamental_ratio(100);
        assert!(
            with_resonance > without_resonance,
            "Bass resonance did not emphasize cutoff relative to the fundamental: \
             {without_resonance:.3} -> {with_resonance:.3}"
        );
    }

    #[test]
    fn rendered_chord_and_lead_resonance_emphasize_the_cutoff_region() {
        fn cutoff_region_energy(track: usize, resonance: u8) -> f32 {
            let mut project = Project::new();
            for current in &mut project.tracks {
                current.muted = true;
            }
            project.tracks[track].muted = false;
            match &mut project.tracks[track].instrument {
                Instrument::Chord(parameters) => {
                    parameters.cutoff = Percent::new(27).unwrap();
                    parameters.resonance = Percent::new(resonance).unwrap();
                    parameters.filter_envelope = Percent::ZERO;
                }
                Instrument::Lead(parameters) => {
                    parameters.cutoff = Percent::new(27).unwrap();
                    parameters.resonance = Percent::new(resonance).unwrap();
                    parameters.filter_envelope = Percent::ZERO;
                    parameters.keyboard_tracking = Percent::ZERO;
                }
                _ => panic!("expected a pitched polyphonic instrument"),
            }
            project.patterns[0].tracks[track].steps[0] = Some(StepEvent::Note {
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

            let sample_rate = 48_000.0;
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(
                AudioProject::from_project(&project),
                sample_rate as u32,
                status,
            );
            renderer.boundary(0);
            let mut sine = 0.0;
            let mut cosine = 0.0;
            let mut count = 0;
            for sample in 0..12_000 {
                let output = if track == CHORD_TRACK_INDEX {
                    let index = renderer.voicings[CHORD_TRACK_INDEX].group * CHORD_GROUP_SIZE;
                    Renderer::render_synth(
                        &mut renderer.voicings[CHORD_TRACK_INDEX].voices[index],
                        renderer.sr,
                        &[0.0; ParameterId::ALL.len()],
                    )
                    .0
                } else {
                    Renderer::render_synth(
                        &mut renderer.synth[track],
                        renderer.sr,
                        &[0.0; ParameterId::ALL.len()],
                    )
                    .0
                };
                if sample >= 4_000 {
                    let phase = std::f32::consts::TAU * 130.8128 * sample as f32 / sample_rate;
                    sine += output * phase.sin();
                    cosine += output * phase.cos();
                    count += 1;
                }
            }
            2.0 * (sine * sine + cosine * cosine).sqrt() / count as f32
        }

        for (name, track) in [("Chord", CHORD_TRACK_INDEX), ("Lead", LEAD_TRACK_INDEX)] {
            let without_resonance = cutoff_region_energy(track, 0);
            let with_resonance = cutoff_region_energy(track, 100);
            assert!(
                with_resonance > without_resonance,
                "{name} resonance did not add cutoff-region energy: {without_resonance:.4} -> {with_resonance:.4}"
            );
        }
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
            microtiming: crate::model::Microtiming::ZERO,
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
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].bass_accent_envelope.value() > 0.7);

        renderer.boundary(0);
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        renderer.command(Audio::snapshot(&project));
        for _ in 0..40 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].bass_accent_envelope.value() > 0.5);
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::BassNote {
            degree: 8,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        for _ in 0..80 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        let vca_before_slide = renderer.synth[SYNTH_TRACK_START].bass_vca.value();
        let contour_before_slide = renderer.synth[SYNTH_TRACK_START].bass_filter_envelope.value();
        let starting_frequency = renderer.synth[SYNTH_TRACK_START].freq.next_value();

        renderer.boundary(0);
        assert_eq!(renderer.synth[SYNTH_TRACK_START].bass_vca.value(), vca_before_slide);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].bass_filter_envelope.value(),
            contour_before_slide
        );
        let first_frequency = renderer.synth[SYNTH_TRACK_START].freq.next_value();
        assert!(first_frequency > starting_frequency);
        assert!(first_frequency < starting_frequency * 2.0);
        Renderer::render_synth(
            &mut renderer.synth[SYNTH_TRACK_START],
            renderer.sr,
            &[0.0; ParameterId::ALL.len()],
        );
        assert!(renderer.synth[SYNTH_TRACK_START].bass_vca.value() >= vca_before_slide);
        assert!(renderer.synth[SYNTH_TRACK_START].bass_filter_envelope.value() < contour_before_slide);
        for _ in 1..480 {
            renderer.synth[SYNTH_TRACK_START].freq.next_value();
        }
        let final_frequency = renderer.synth[SYNTH_TRACK_START].freq.next_value();
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(StepEvent::LeadNote {
            degree: 8,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let lead = LEAD_TRACK_INDEX;

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
                microtiming: crate::model::Microtiming::ZERO,
                locks: parameter_locks([(ParameterId::PortamentoTime, portamento)]),
            }
        }

        let mut project = Project::new();
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[0] = Some(note(1, true, 100));
        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(note(8, false, 0));
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        let lead = LEAD_TRACK_INDEX;

        renderer.boundary(0);
        let source = renderer.synth[lead].freq.value();
        renderer.boundary(0);
        let first = renderer.synth[lead].freq.next_value();
        assert!(first > source && first < source * 1.01);

        project.patterns[0].tracks[LEAD_TRACK_INDEX].steps[1] = Some(StepEvent::Tie {
            locks: parameter_locks([(ParameterId::PortamentoTime, 0)]),
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
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        for _ in 0..40 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].bass_accent_envelope.value() > 0.7);
        renderer.synth[SYNTH_TRACK_START].gate_off();
        renderer.synth[SYNTH_TRACK_START].active = false;
        for _ in 0..1_000 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert_eq!(renderer.synth[SYNTH_TRACK_START].bass_accent_envelope.value(), 0.0);
        assert_eq!(renderer.synth[SYNTH_TRACK_START].bass_filter_envelope.value(), 0.0);

        renderer.boundary(0);
        Renderer::render_synth(
            &mut renderer.synth[SYNTH_TRACK_START],
            renderer.sr,
            &[0.0; ParameterId::ALL.len()],
        );
        assert_eq!(renderer.synth[SYNTH_TRACK_START].bass_accent_envelope.value(), 0.0);
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        for _ in 0..80 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].bass_vca.value() > 0.99);

        renderer.boundary(1);
        for _ in 0..500 {
            Renderer::render_synth(
                &mut renderer.synth[SYNTH_TRACK_START],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        assert!(renderer.synth[SYNTH_TRACK_START].is_idle());
    }

    #[test]
    fn representative_groove_has_calibrated_rms_and_safe_peak() {
        let mut project = Project::new();
        for step in [0, 4, 8, 12] {
            project.patterns[0].tracks[0].steps[step] = Some(StepEvent::Trigger {
                accent: step == 0,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        for step in [4, 12] {
            project.patterns[0].tracks[1].steps[step] = Some(StepEvent::Trigger {
                accent: true,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        for step in 0..16 {
            project.patterns[0].tracks[2].steps[step] = Some(StepEvent::Trigger {
                accent: step == 6 || step == 14,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
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
                microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
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
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Level, 0)]),
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Cutoff, 20)]),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(20)
        );

        let mut edited = project.clone();
        let Some(StepEvent::BassNote { locks, .. }) =
            edited.patterns[0].tracks[SYNTH_TRACK_START].steps[0].as_mut()
        else {
            panic!("expected Bass note")
        };
        locks.set(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(80).unwrap()),
        );
        renderer.command(Audio::snapshot(&edited));
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(20)
        );

        renderer.boundary(0);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(80)
        );
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Level, 30), (ParameterId::Cutoff, 20)]),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Tie {
            locks: parameter_locks([(ParameterId::Resonance, 70)]),
        });
        project.patterns[0].tracks[SYNTH_TRACK_START].steps[2] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        renderer.boundary(0);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Level),
            Percent::new(30)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(20)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Resonance),
            None
        );

        renderer.boundary(0);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Level),
            Percent::new(30)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(20)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Resonance),
            Percent::new(70)
        );

        renderer.boundary(0);
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Level),
            Percent::new(30)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Cutoff),
            Percent::new(20)
        );
        assert_eq!(
            renderer.synth[SYNTH_TRACK_START].locks.percent(ParameterId::Resonance),
            Percent::new(70)
        );
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Cutoff, 25)]),
        });
        let status = Arc::new(AudioStatus::default());
        let renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);

        assert_eq!(
            renderer
                .locks_at(SYNTH_TRACK_START, 0)
                .percent(ParameterId::Cutoff),
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
        let mut locks = parameter_locks([(ParameterId::Cutoff, 0), (ParameterId::Resonance, 0)]);
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
        locks.set(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(100).unwrap()),
        );
        locks.set(
            ParameterId::Resonance,
            ParameterValue::Percent(Percent::new(100).unwrap()),
        );
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
    fn chord_and_lead_noise_controls_use_instrument_specific_source_ranges() {
        let project = AudioProject::from_project(&Project::new());
        let locks = parameter_locks([(ParameterId::Noise, 100)]);
        let mut chord = SynthVoice::new(48_000.0);
        Renderer::apply_synth_params(&project, 48_000.0, CHORD_TRACK_INDEX, locks, &mut chord, 0);
        let mut lead = SynthVoice::new(48_000.0);
        Renderer::apply_synth_params(&project, 48_000.0, LEAD_TRACK_INDEX, locks, &mut lead, 0);

        assert!((chord.noise_level.value() - 0.35).abs() < f32::EPSILON);
        assert!((lead.noise_level.value() - 1.0).abs() < f32::EPSILON);
        assert!(lead.noise_level.value() > chord.noise_level.value() * 2.0);
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: parameter_locks([(ParameterId::Cutoff, 20)]),
        });
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[1] = Some(StepEvent::Tie {
            locks: parameter_locks([(ParameterId::Resonance, 70)]),
        });
        project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[2] = Some(StepEvent::Note {
            degree: 4,
            octave: 3,
            accent: true,
            chord_shape: None,
            arpeggio: ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);

        renderer.boundary(0);
        let first_group = renderer.voicings[CHORD_TRACK_INDEX].group;
        let frequencies = std::array::from_fn::<_, 3, _>(|voice| {
            renderer.voicings[CHORD_TRACK_INDEX].voices[first_group * CHORD_GROUP_SIZE + voice]
                .freq
                .next_value()
        });
        let expected = [48, 52, 55].map(|midi| 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0));
        for (actual, expected) in frequencies.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.001);
        }
        for voice in &renderer.voicings[CHORD_TRACK_INDEX].voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.locks.percent(ParameterId::Cutoff), Percent::new(20));
        }

        renderer.boundary(0);
        assert_eq!(renderer.voicings[CHORD_TRACK_INDEX].group, first_group);
        for voice in &renderer.voicings[CHORD_TRACK_INDEX].voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.locks.percent(ParameterId::Cutoff), Percent::new(20));
            assert_eq!(
                voice.locks.percent(ParameterId::Resonance),
                Percent::new(70)
            );
        }
        renderer.boundary(0);
        assert_ne!(renderer.voicings[CHORD_TRACK_INDEX].group, first_group);
        for voice in &renderer.voicings[CHORD_TRACK_INDEX].voices
            [first_group * CHORD_GROUP_SIZE..first_group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.env.stage, crate::dsp::EnvStage::Release);
        }
        for voice in &renderer.voicings[CHORD_TRACK_INDEX].voices
            [renderer.voicings[CHORD_TRACK_INDEX].group * CHORD_GROUP_SIZE..renderer.voicings[CHORD_TRACK_INDEX].group * CHORD_GROUP_SIZE + 3]
        {
            assert_eq!(voice.env.stage, crate::dsp::EnvStage::Attack);
        }

        renderer.boundary(0);
        assert!(!renderer.voicings[CHORD_TRACK_INDEX].active);
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
        renderer.boundary(0);
        assert_eq!(renderer.voicings[CHORD_TRACK_INDEX].voice_count, 4);
        let group = renderer.voicings[CHORD_TRACK_INDEX].group;
        let expected = [48, 52, 55, 59];
        for (voice, midi) in expected.into_iter().enumerate() {
            let frequency = renderer.voicings[CHORD_TRACK_INDEX].voices[group * CHORD_GROUP_SIZE + voice]
                .freq
                .next_value();
            let expected = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            assert!((frequency - expected).abs() < 0.001);
        }
    }

    #[test]
    fn chord_track_renders_single_and_dyad_shapes_with_expected_spread() {
        fn trigger(shape: ChordShape) -> SynthTrigger {
            SynthTrigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                chord_shape: Some(shape),
                arpeggio: ArpeggioConfig::default(),
        }
    }

        let project = Project::new();
        let project = AudioProject::from_project(&project);
        let mut pool = ChordVoicePool::new(48_000);

        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
            trigger(ChordShape::Single),
            Default::default(),
            &mut pool,
        );
        let start = pool.group * CHORD_GROUP_SIZE;
        assert_eq!(pool.group_voice_counts[pool.group], 1);
        assert_eq!(pool.voices[start].pan.next_value(), 50.0);
        assert!((pool.voices[start].freq.next_value() - 130.81279).abs() < 0.001);

        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
            trigger(ChordShape::DyadThird),
            Default::default(),
            &mut pool,
        );
        let start = pool.group * CHORD_GROUP_SIZE;
        assert_eq!(pool.group_voice_counts[pool.group], 2);
        assert_eq!(pool.voices[start].pan.next_value(), 0.0);
        assert_eq!(pool.voices[start + 1].pan.next_value(), 100.0);
        let expected = [48, 52].map(|midi| 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0));
        for (voice, expected) in pool.voices[start..start + 2].iter_mut().zip(expected) {
            assert!((voice.freq.next_value() - expected).abs() < 0.001);
        }

        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
            trigger(ChordShape::DyadFifth),
            Default::default(),
            &mut pool,
        );
        let start = pool.group * CHORD_GROUP_SIZE;
        let expected = [48, 55].map(|midi| 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0));
        for (voice, expected) in pool.voices[start..start + 2].iter_mut().zip(expected) {
            assert!((voice.freq.next_value() - expected).abs() < 0.001);
        }
        assert!(
            pool.voices[start + 2..start + CHORD_GROUP_SIZE]
                .iter()
                .all(|voice| voice.env.stage == crate::dsp::EnvStage::Idle && !voice.active)
        );
    }

    #[test]
    fn fm_track_renders_four_voice_shapes_with_fixed_wide_distribution() {
        let project = AudioProject::from_project(&Project::new());
        let mut pool = ChordVoicePool::new(48_000);
        Renderer::trigger_chord(
            &project,
            48_000.0,
            FM_TRACK_INDEX,
            SynthTrigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                chord_shape: Some(ChordShape::SeventhRoot),
                arpeggio: ArpeggioConfig::default(),
            },
            ParameterLocks::default(),
            &mut pool,
        );

        let start = pool.group * CHORD_GROUP_SIZE;
        assert_eq!(pool.group_voice_counts[pool.group], 4);
        let expected_pans = [0.0, 33.3333, 66.6667, 100.0];
        let expected_midis = [48, 52, 55, 59];
        for (index, (pan, midi)) in expected_pans.into_iter().zip(expected_midis).enumerate() {
            let voice = &mut pool.voices[start + index];
            assert!((voice.pan.next_value() - pan).abs() < 0.001);
            assert_eq!(voice.kind, SynthVoiceKind::Fm);
            let expected = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            assert!((voice.freq.next_value() - expected).abs() < 0.001);
        }
    }

    #[test]
    fn fixed_voicing_spread_survives_ties_refreshes_previews_and_release_tails() {
        fn pans(pool: &mut ChordVoicePool) -> [f32; 4] {
            let start = pool.group * CHORD_GROUP_SIZE;
            std::array::from_fn(|index| pool.voices[start + index].pan.value())
        }

        fn assert_pans(actual: [f32; 4], expected: [f32; 4]) {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
            }
        }

        for track in [CHORD_TRACK_INDEX, FM_TRACK_INDEX] {
            let mut project = Project::new();
            project.patterns[0].tracks[track].steps[0] = Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: Some(ChordShape::SeventhRoot),
                arpeggio: Default::default(),
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: Microtiming::ZERO,
                locks: Default::default(),
            });
            project.patterns[0].tracks[track].steps[1] = Some(StepEvent::Tie {
                locks: Default::default(),
            });
            let status = Arc::new(AudioStatus::default());
            let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
            renderer.process_track_action(track, 0, false, true);
            renderer.audition_once(track, 0);

            let initial = [0.0, 33.3333, 66.6667, 100.0];
            if track == CHORD_TRACK_INDEX {
                assert_pans(pans(&mut renderer.voicings[CHORD_TRACK_INDEX]), initial);
                assert_pans(pans(&mut renderer.preview_voicings[CHORD_TRACK_INDEX]), initial);
            } else {
                assert_pans(pans(&mut renderer.voicings[FM_TRACK_INDEX]), initial);
                assert_pans(pans(&mut renderer.preview_voicings[FM_TRACK_INDEX]), initial);
            }

            renderer.process_track_action(track, 1, false, true);
            let smoothing = ParameterSmoothing::Default.samples(renderer.sr);
            {
                let (live, preview) = if track == CHORD_TRACK_INDEX {
                    (&mut renderer.voicings[CHORD_TRACK_INDEX], &mut renderer.preview_voicings[CHORD_TRACK_INDEX])
                } else {
                    (&mut renderer.voicings[FM_TRACK_INDEX], &mut renderer.preview_voicings[FM_TRACK_INDEX])
                };
                for _ in 0..smoothing {
                    for pool in [&mut *live, &mut *preview] {
                        let start = pool.group * CHORD_GROUP_SIZE;
                        for voice in &mut pool.voices[start..start + 4] {
                            voice.pan.next_value();
                        }
                    }
                }
                assert_pans(pans(live), initial);
            }

            renderer.project.tracks[track].pan = Percent::new(70).unwrap();
            renderer.refresh_active_parameters(0);
            let shifted = [20.0, 53.3333, 86.6667, 100.0];
            {
                let (live, preview) = if track == CHORD_TRACK_INDEX {
                    (&mut renderer.voicings[CHORD_TRACK_INDEX], &mut renderer.preview_voicings[CHORD_TRACK_INDEX])
                } else {
                    (&mut renderer.voicings[FM_TRACK_INDEX], &mut renderer.preview_voicings[FM_TRACK_INDEX])
                };
                assert_pans(pans(live), shifted);
                assert_pans(pans(preview), shifted);
                for pool in [live, preview] {
                    let start = pool.group * CHORD_GROUP_SIZE;
                    for voice in &mut pool.voices[start..start + 4] {
                        voice.gate_off();
                        voice.active = false;
                    }
                }
            }
            renderer.project.tracks[track].pan = Percent::new(40).unwrap();
            renderer.refresh_active_parameters(0);
            let released = [0.0, 23.3333, 56.6667, 90.0];
            if track == CHORD_TRACK_INDEX {
                assert_pans(pans(&mut renderer.voicings[CHORD_TRACK_INDEX]), released);
                assert_pans(pans(&mut renderer.preview_voicings[CHORD_TRACK_INDEX]), released);
            } else {
                assert_pans(pans(&mut renderer.voicings[FM_TRACK_INDEX]), released);
                assert_pans(pans(&mut renderer.preview_voicings[FM_TRACK_INDEX]), released);
            }
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
            CHORD_TRACK_INDEX,
            trigger(ChordShape::SeventhRoot, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );
        let first_group = pool.group;
        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
            trigger(ChordShape::TriadRoot, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );
        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
            trigger(ChordShape::Single, ArpeggioConfig::default()),
            Default::default(),
            &mut pool,
        );

        assert_eq!(pool.group, first_group);
        assert_eq!(pool.group_voice_counts[first_group], 1);
        assert!(
            pool.voices[first_group * CHORD_GROUP_SIZE + 1..(first_group + 1) * CHORD_GROUP_SIZE]
                .iter()
                .all(|voice| voice.env.stage == crate::dsp::EnvStage::Idle && !voice.active)
        );

        Renderer::trigger_chord(
            &project,
            48_000.0,
            CHORD_TRACK_INDEX,
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
                microtiming: crate::model::Microtiming::ZERO,
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
        let releasing_group = 1 - renderer.voicings[CHORD_TRACK_INDEX].group;
        assert_eq!(
            renderer.voicings[CHORD_TRACK_INDEX].voices[releasing_group * CHORD_GROUP_SIZE]
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
            microtiming: crate::model::Microtiming::ZERO,
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
        project.tracks[0].lfos.set(
            ParameterId::Level,
            Some(crate::model::LfoConfig {
                rate: crate::model::LfoRate::Free {
                    rate_percent: Percent::new(100).unwrap(),
                },
                depth: Percent::new(50).unwrap(),
                ..Default::default()
            }),
        );
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
    fn trigger_reset_applies_exact_phase_before_drum_sampling() {
        let mut project = Project::new();
        project.tracks[0].lfos.set(
            ParameterId::Tune,
            Some(LfoConfig {
                waveform: LfoWaveform::Square,
                reset_on_trigger: true,
                start_phase: Percent::new(75).unwrap(),
                rate: LfoRate::Free {
                    rate_percent: Percent::new(100).unwrap(),
                },
                depth: Percent::new(50).unwrap(),
                ..Default::default()
            }),
        );
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 2,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 1_000, status);

        for _ in 0..20 {
            renderer.advance_lfos();
        }
        let moving = renderer.lfo_offsets[0][ParameterId::Tune as usize];
        assert!(moving > -49.0);
        renderer.process_track_action(0, 0, false, false);
        assert_eq!(renderer.lfo_offsets[0][ParameterId::Tune as usize], moving);

        renderer.process_track_action(0, 0, false, true);
        assert_eq!(renderer.lfo_offsets[0][ParameterId::Tune as usize], -50.0);
        assert_eq!(renderer.drums[0].tune, 0.0);

        renderer.advance_lfos();
        renderer.process_track_action(0, 0, true, true);
        assert_eq!(renderer.lfo_offsets[0][ParameterId::Tune as usize], -50.0);

        renderer.audition(0, 0);
        assert_eq!(
            renderer.preview_lfo_offsets[0][ParameterId::Tune as usize],
            -50.0
        );
        assert_eq!(renderer.preview_drums[0].tune, 0.0);
    }

    #[test]
    fn held_ties_do_not_reset_lfos_but_cold_ties_do() {
        let track = SYNTH_TRACK_START;
        let mut project = Project::new();
        project.tracks[track].lfos.set(
            ParameterId::Level,
            Some(LfoConfig {
                waveform: LfoWaveform::Sine,
                reset_on_trigger: true,
                start_phase: Percent::new(25).unwrap(),
                rate: LfoRate::Free {
                    rate_percent: Percent::new(100).unwrap(),
                },
                depth: Percent::new(40).unwrap(),
                ..Default::default()
            }),
        );
        project.patterns[0].tracks[track].steps[0] = Some(StepEvent::BassNote {
            degree: 1,
            octave: 3,
            accent: false,
            slide: false,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        project.patterns[0].tracks[track].steps[1] = Some(StepEvent::Tie {
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 1_000, status);

        renderer.process_track_action(track, 0, false, true);
        assert!((renderer.lfo_offsets[track][ParameterId::Level as usize] - 40.0).abs() < 0.000_1);
        for _ in 0..20 {
            renderer.advance_lfos();
        }
        let moving = renderer.lfo_offsets[track][ParameterId::Level as usize];
        renderer.process_track_action(track, 1, false, true);
        assert_eq!(
            renderer.lfo_offsets[track][ParameterId::Level as usize],
            moving
        );

        renderer.synth[SYNTH_TRACK_START].active = false;
        renderer.process_track_action(track, 1, false, true);
        assert!((renderer.lfo_offsets[track][ParameterId::Level as usize] - 40.0).abs() < 0.000_1);
    }

    #[test]
    fn chord_arpeggio_substeps_do_not_reset_lfos() {
        let track = CHORD_TRACK_INDEX;
        let mut project = Project::new();
        project.tracks[track].lfos.set(
            ParameterId::Level,
            Some(LfoConfig {
                waveform: LfoWaveform::Sine,
                reset_on_trigger: true,
                start_phase: Percent::new(25).unwrap(),
                rate: LfoRate::Free {
                    rate_percent: Percent::new(100).unwrap(),
                },
                depth: Percent::new(40).unwrap(),
                ..Default::default()
            }),
        );
        project.patterns[0].tracks[track].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig {
                enabled: true,
                r#type: ArpeggioType::Up,
                rate: ArpeggioRate::Sixteenth,
            },
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 1_000, status);

        renderer.process_track_action(track, 0, false, true);
        for _ in 0..20 {
            renderer.advance_lfos();
        }
        let moving = renderer.lfo_offsets[track][ParameterId::Level as usize];
        let position = renderer.voicings[CHORD_TRACK_INDEX].arpeggio.position;
        renderer.voicings[CHORD_TRACK_INDEX].arpeggio.phase = 0.0;
        renderer.advance_chord_arpeggios();
        assert_ne!(renderer.voicings[CHORD_TRACK_INDEX].arpeggio.position, position);
        assert_eq!(
            renderer.lfo_offsets[track][ParameterId::Level as usize],
            moving
        );
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
        for (shape, expected_up_down, expected_length) in [
            (ChordShape::Single, [0, 0, 0, 0, 0, 0, 0, 0], 1),
            (ChordShape::DyadThird, [0, 1, 0, 0, 0, 0, 0, 0], 2),
        ] {
            let mut state = ArpeggioState::default();
            state.reset(
                shape,
                ArpeggioType::UpDown,
                ArpeggioRate::Sixteenth,
                44_100.0,
                123,
            );
            assert_eq!(state.order, expected_up_down);
            assert_eq!(state.order_len as usize, expected_length);
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
            microtiming: crate::model::Microtiming::ZERO,
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

    #[test]
    fn drum_recipe_resolution_precedes_locks() {
        let project = AudioProject::from_project(&Project::new());
        let offsets = [0.0; ParameterId::ALL.len()];
        let hat = Renderer::drum_controls(
            project.tracks[2],
            crate::model::DrumRecipeSlot::TWO,
            ParameterLocks::default(),
            &offsets,
        )
        .unwrap();
        assert!((hat.tune - 0.50).abs() < f32::EPSILON);
        assert!((hat.decay - 0.85).abs() < f32::EPSILON);

        let tom = Renderer::drum_controls(
            project.tracks[3],
            crate::model::DrumRecipeSlot::THREE,
            parameter_locks([(ParameterId::Decay, 12)]),
            &offsets,
        )
        .unwrap();
        assert!((tom.tune - 0.85).abs() < f32::EPSILON);
        assert!((tom.tone - 0.65).abs() < f32::EPSILON);
        assert!((tom.decay - 0.12).abs() < f32::EPSILON);
    }

    #[test]
    fn song_transport_repeats_entries_then_stops_at_the_end() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.song = vec![
            SongEntry { pattern: 1, bars: 2 },
            SongEntry { pattern: 2, bars: 1 },
        ];
        let status = std::sync::Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status.clone());
        renderer.command(AudioCommand::SelectSong { entry: 0 });
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        assert_eq!(renderer.active_pattern, 0);
        renderer.boundary(16);
        assert_eq!(renderer.active_pattern, 0);
        assert_eq!(renderer.song_bar, 1);
        renderer.boundary(32);
        assert_eq!(renderer.active_pattern, 1);
        assert_eq!(renderer.active_song, 1);
        renderer.boundary(48);
        assert!(!renderer.playing);
        assert_eq!(renderer.active_song, 0);
        assert_eq!(renderer.active_pattern, 0);
        assert!(!status.running.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn queued_direct_switch_keeps_song_status_until_the_boundary() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.song = vec![SongEntry {
            pattern: 1,
            bars: 2,
        }];
        let status = std::sync::Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status.clone());
        renderer.command(AudioCommand::SelectSong { entry: 0 });
        renderer.command(AudioCommand::PlayPause);
        renderer.boundary(0);
        renderer.command(AudioCommand::SelectPattern { pattern: 1 });

        assert!(renderer.song_mode);
        assert!(status.song_mode.load(std::sync::atomic::Ordering::Acquire));
        renderer.boundary(16);
        assert!(!renderer.song_mode);
        assert!(!status.song_mode.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(renderer.active_pattern, 1);
    }

    #[test]
    fn deleting_the_active_song_entry_restarts_its_replacement_at_bar_one() {
        let mut project = Project::new();
        project.song = vec![
            SongEntry {
                pattern: 1,
                bars: 4,
            },
            SongEntry {
                pattern: 1,
                bars: 4,
            },
        ];
        let status = std::sync::Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.active_song = 0;
        renderer.song_bar = 2;
        project.song.remove(0);

        renderer.replace_project(
            Box::new(AudioProject::from_project(&project)),
            ParameterSmoothing::Default,
            PatternIndexMap::identity(),
            SongIndexMap::delete(0),
        );

        assert_eq!(renderer.active_song, 0);
        assert_eq!(renderer.song_bar, 0);
    }

    #[test]
    fn duplicate_drum_and_voicing_instruments_use_independent_slot_runtime() {
        let mut project = Project::new();
        project.tracks[1] = project.tracks[0].clone();
        project.tracks[2] = project.tracks[CHORD_TRACK_INDEX].clone();
        for track in [0, 1] {
            project.patterns[0].tracks[track].steps[0] = Some(StepEvent::Trigger {
                accent: track == 1,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        for track in [2, CHORD_TRACK_INDEX] {
            project.patterns[0].tracks[track].steps[0] = Some(StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: Some(ChordShape::TriadRoot),
                arpeggio: Default::default(),
                condition: Default::default(),
                retrigger_count: 1,
                microtiming: Microtiming::ZERO,
                locks: Default::default(),
            });
        }
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);

        renderer.process_track_action(0, 0, false, true);
        renderer.process_track_action(1, 0, false, true);
        renderer.process_track_action(2, 0, false, true);
        renderer.process_track_action(CHORD_TRACK_INDEX, 0, false, true);

        assert!(!renderer.drums[0].envelope.is_idle());
        assert!(!renderer.drums[1].envelope.is_idle());
        assert!(renderer.voicings[2].active);
        assert!(renderer.voicings[CHORD_TRACK_INDEX].active);
        assert_ne!(
            renderer.voicings[2].voices.as_ptr(),
            renderer.voicings[CHORD_TRACK_INDEX].voices.as_ptr()
        );
    }

    #[test]
    fn a_kick_assigned_to_another_slot_drives_the_shared_sidechain() {
        let mut project = Project::new();
        project.tracks[1] = project.tracks[0].clone();
        project.globals.sidechain.depth = Percent::new(100).unwrap();
        project.globals.sidechain.attack = Percent::ZERO;
        project.patterns[0].tracks[1].steps[0] = Some(StepEvent::Trigger {
            accent: true,
            recipe: crate::model::DrumRecipeSlot::ONE,
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: Default::default(),
        });
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);

        renderer.process_track_action(1, 0, false, true);
        for _ in 0..256 {
            renderer.next();
        }

        assert!(renderer.sidechain.current_gain() < 0.99);
    }

    #[test]
    fn replacing_an_instrument_resets_only_that_slots_runtime() {
        let project = Project::new();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 48_000, status);
        renderer.synth[SYNTH_TRACK_START].active = true;
        renderer.synth[LEAD_TRACK_INDEX].active = true;
        renderer.preview_activity[SYNTH_TRACK_START] = true;
        renderer.next_steps[SYNTH_TRACK_START] = 7;

        let hat_controls = Renderer::drum_controls(
            renderer.project.tracks[2],
            crate::model::DrumRecipeSlot::ONE,
            ParameterLocks::default(),
            &[0.0; ParameterId::ALL.len()],
        )
        .unwrap();
        Renderer::start_drum_voice(&mut renderer.drums[0], hat_controls, true, 48_000.0);
        Renderer::start_drum_voice(
            &mut renderer.preview_drums[0],
            hat_controls,
            true,
            48_000.0,
        );
        for _ in 0..32 {
            let _ = Renderer::render_drum_raw(&mut renderer.drums[0], 0, 48_000.0, 0.0, 0.0);
            let _ = Renderer::render_drum_raw(
                &mut renderer.preview_drums[0],
                0,
                48_000.0,
                0.0,
                0.0,
            );
        }

        let mut changed = project;
        changed.tracks[0] = changed.tracks[1].clone();
        changed.patterns[0].tracks[0].steps.fill(None);
        changed.tracks[SYNTH_TRACK_START] = Project::new().tracks[0].clone();
        changed.patterns[0].tracks[SYNTH_TRACK_START].steps.fill(None);
        renderer.replace_project(
            Box::new(AudioProject::from_project(&changed)),
            ParameterSmoothing::Default,
            PatternIndexMap::identity(),
            SongIndexMap::identity(),
        );

        assert_eq!(
            renderer.project.tracks[SYNTH_TRACK_START].instrument_kind(),
            TrackKind::Kick
        );
        assert!(!renderer.synth[SYNTH_TRACK_START].active);
        assert!(!renderer.preview_activity[SYNTH_TRACK_START]);
        assert_eq!(renderer.next_steps[SYNTH_TRACK_START], 0);
        assert!(renderer.synth[LEAD_TRACK_INDEX].active);

        let snare_controls = Renderer::drum_controls(
            renderer.project.tracks[0],
            crate::model::DrumRecipeSlot::ONE,
            ParameterLocks::default(),
            &[0.0; ParameterId::ALL.len()],
        )
        .unwrap();
        let mut expected_live = DrumVoice::new(0x1234_abcd);
        let mut expected_preview = DrumVoice::new(0x4a31_27dd);
        Renderer::start_drum_voice(&mut expected_live, snare_controls, true, 48_000.0);
        Renderer::start_drum_voice(&mut expected_preview, snare_controls, true, 48_000.0);
        renderer.trigger_drum(
            0,
            true,
            crate::model::DrumRecipeSlot::ONE,
            ParameterLocks::default(),
        );
        renderer.trigger_preview_drum(
            0,
            true,
            crate::model::DrumRecipeSlot::ONE,
            ParameterLocks::default(),
        );
        for _ in 0..32 {
            assert_eq!(
                Renderer::render_drum_raw(&mut renderer.drums[0], 0, 48_000.0, 0.0, 0.0),
                Renderer::render_drum_raw(&mut expected_live, 0, 48_000.0, 0.0, 0.0),
            );
            assert_eq!(
                Renderer::render_drum_raw(
                    &mut renderer.preview_drums[0],
                    0,
                    48_000.0,
                    0.0,
                    0.0,
                ),
                Renderer::render_drum_raw(&mut expected_preview, 0, 48_000.0, 0.0, 0.0),
            );
        }
    }

    fn project_with_fm_note() -> Project {
        let mut project = Project::new();
        project.patterns[0].tracks[FM_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: Default::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: Microtiming::ZERO,
            locks: Default::default(),
        });
        project
    }

    fn edit_fm_parameters(project: &mut Project) {
        let track = &mut project.tracks[FM_TRACK_INDEX];
        track.level = Percent::new(31).unwrap();
        track.delay_send = Percent::new(32).unwrap();
        track.reverb_send = Percent::new(33).unwrap();
        track.pan = Percent::new(34).unwrap();
        let Instrument::Fm(parameters) = &mut track.instrument else {
            unreachable!()
        };
        parameters.algorithm = FmAlgorithm::Pairs;
        parameters.operators[1].ratio = crate::model::FmRatio::Four;
        parameters.operators[1].level = Percent::new(81).unwrap();
        parameters.operators[1].feedback = Percent::new(67).unwrap();
        parameters.brightness = Percent::new(23).unwrap();
        parameters.attack = Percent::new(12).unwrap();
        parameters.decay = Percent::new(34).unwrap();
        parameters.sustain = Percent::new(56).unwrap();
        parameters.release = Percent::new(78).unwrap();
    }

    fn assert_edited_fm_parameters(voice: &SynthVoice) {
        assert_eq!(voice.fm_algorithm, FmAlgorithm::Pairs);
        assert_eq!(voice.fm_ratios[1].value(), 4.0);
        assert_eq!(voice.fm_levels[1].value(), 81.0);
        assert_eq!(voice.fm_feedback[1].value(), 67.0);
        assert_eq!(voice.fm_brightness.value(), 23.0);
        assert_eq!(voice.env.parameter_values(), (12.0, 34.0, 56.0, 78.0));
        assert_eq!(voice.level.value(), 31.0);
        assert_eq!(voice.delay_send.value(), 0.32);
        assert_eq!(voice.reverb_send.value(), 0.33);
        assert_eq!(voice.pan.value(), 34.0);
    }

    #[test]
    fn live_fm_parameter_edits_refresh_sustaining_live_and_preview_voices() {
        let mut project = project_with_fm_note();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        renderer.audition_once(FM_TRACK_INDEX, 0);
        let live = renderer.voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        let preview = renderer.preview_voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        assert!(renderer.voicings[FM_TRACK_INDEX].voices[live].active);
        assert!(renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].active);

        edit_fm_parameters(&mut project);
        renderer.command(Audio::snapshot_with_smoothing(
            &project,
            ParameterSmoothing::Fader,
        ));
        assert_eq!(renderer.voicings[FM_TRACK_INDEX].voices[live].fm_algorithm, FmAlgorithm::Pairs);
        assert_eq!(renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].fm_algorithm, FmAlgorithm::Pairs);
        for _ in 0..ParameterSmoothing::Fader.samples(renderer.sr) {
            Renderer::render_synth(
                &mut renderer.voicings[FM_TRACK_INDEX].voices[live],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            renderer.voicings[FM_TRACK_INDEX].voices[live].level.next_value();
            renderer.voicings[FM_TRACK_INDEX].voices[live].pan.next_value();
            Renderer::render_synth(
                &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[preview],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].level.next_value();
            renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].pan.next_value();
        }
        assert_edited_fm_parameters(&renderer.voicings[FM_TRACK_INDEX].voices[live]);
        assert_edited_fm_parameters(&renderer.preview_voicings[FM_TRACK_INDEX].voices[preview]);

        let mut locks = ParameterLocks::default();
        assert!(locks.set(
            ParameterId::FmAlgorithm,
            ParameterValue::FmAlgorithm(FmAlgorithm::Additive),
        ));
        assert!(locks.set(
            ParameterId::FmOp2Level,
            ParameterValue::Percent(Percent::new(17).unwrap()),
        ));
        for voice in [
            &mut renderer.voicings[FM_TRACK_INDEX].voices[live],
            &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[preview],
        ] {
            Renderer::configure_synth_voice(
                &renderer.project,
                renderer.sr,
                FM_TRACK_INDEX,
                SynthTrigger {
                    degree: 1,
                    octave: 3,
                    accent: false,
                    slide: false,
                    chord_shape: None,
                    arpeggio: Default::default(),
                },
                locks,
                voice,
            );
        }
        let mut lock_edit = project.clone();
        let Instrument::Fm(parameters) = &mut lock_edit.tracks[FM_TRACK_INDEX].instrument else {
            unreachable!()
        };
        parameters.algorithm = FmAlgorithm::Cascade;
        parameters.operators[1].level = Percent::new(99).unwrap();
        renderer.command(Audio::snapshot(&lock_edit));
        assert_eq!(renderer.voicings[FM_TRACK_INDEX].voices[live].fm_algorithm, FmAlgorithm::Additive);
        assert_eq!(renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].fm_algorithm, FmAlgorithm::Additive);
        for _ in 0..ParameterSmoothing::Default.samples(renderer.sr) {
            renderer.voicings[FM_TRACK_INDEX].voices[live].fm_levels[1].next_value();
            renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].fm_levels[1].next_value();
        }
        assert_eq!(renderer.voicings[FM_TRACK_INDEX].voices[live].fm_levels[1].value(), 17.0);
        assert_eq!(renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].fm_levels[1].value(), 17.0);
    }

    #[test]
    fn live_fm_parameter_edits_refresh_live_and_preview_release_tails() {
        let mut project = project_with_fm_note();
        let status = Arc::new(AudioStatus::default());
        let mut renderer = Renderer::new(AudioProject::from_project(&project), 8_000, status);
        renderer.boundary(0);
        renderer.audition_once(FM_TRACK_INDEX, 0);
        let live = renderer.voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        let preview = renderer.preview_voicings[FM_TRACK_INDEX].group * CHORD_GROUP_SIZE;
        for _ in 0..64 {
            Renderer::render_synth(
                &mut renderer.voicings[FM_TRACK_INDEX].voices[live],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            Renderer::render_synth(
                &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[preview],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
        }
        for voice in [
            &mut renderer.voicings[FM_TRACK_INDEX].voices[live],
            &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[preview],
        ] {
            voice.gate_off();
            voice.active = false;
            assert_eq!(voice.env.stage, crate::dsp::EnvStage::Release);
            assert!(!voice.is_idle());
        }

        edit_fm_parameters(&mut project);
        renderer.command(Audio::snapshot_with_smoothing(
            &project,
            ParameterSmoothing::Fader,
        ));
        for _ in 0..ParameterSmoothing::Fader.samples(renderer.sr) {
            Renderer::render_synth(
                &mut renderer.voicings[FM_TRACK_INDEX].voices[live],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            renderer.voicings[FM_TRACK_INDEX].voices[live].level.next_value();
            renderer.voicings[FM_TRACK_INDEX].voices[live].pan.next_value();
            Renderer::render_synth(
                &mut renderer.preview_voicings[FM_TRACK_INDEX].voices[preview],
                renderer.sr,
                &[0.0; ParameterId::ALL.len()],
            );
            renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].level.next_value();
            renderer.preview_voicings[FM_TRACK_INDEX].voices[preview].pan.next_value();
        }
        assert_edited_fm_parameters(&renderer.voicings[FM_TRACK_INDEX].voices[live]);
        assert_edited_fm_parameters(&renderer.preview_voicings[FM_TRACK_INDEX].voices[preview]);
    }

    #[test]
    fn fm_voice_is_deterministic_finite_and_algorithms_are_distinct() {
        fn render(algorithm: FmAlgorithm) -> Vec<f32> {
            let mut project = Project::new();
            let Instrument::Fm(parameters) = &mut project.tracks[FM_TRACK_INDEX].instrument else { unreachable!() };
            parameters.algorithm = algorithm;
            for operator in &mut parameters.operators {
                operator.level = Percent::new(100).unwrap();
                operator.feedback = Percent::new(100).unwrap();
            }
            parameters.brightness = Percent::new(100).unwrap();
            let audio = AudioProject::from_project(&project);
            let mut voice = SynthVoice::new(44_100.0);
            Renderer::configure_synth_voice(
                &audio, 44_100.0, FM_TRACK_INDEX,
                SynthTrigger { degree: 1, octave: 3, accent: true, slide: false, chord_shape: None, arpeggio: Default::default() },
                ParameterLocks::default(), &mut voice,
            );
            let offsets = [0.0; ParameterId::ALL.len()];
            (0..2048).map(|_| Renderer::render_synth(&mut voice, 44_100.0, &offsets).0).collect()
        }
        let rendered = FmAlgorithm::ALL.map(render);
        assert_eq!(rendered[0], render(FmAlgorithm::Cascade));
        assert!(rendered
            .iter()
            .flatten()
            .all(|sample| sample.is_finite() && sample.abs() <= 2.0));
        assert!(rendered[0]
            .iter()
            .zip(&rendered[7])
            .any(|(left, right)| (left - right).abs() > 1.0e-4));
    }
}
