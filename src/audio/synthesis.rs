use super::effects::modulated_percent;
use super::voices::{
    CHORD_GROUP_SIZE, ChordVoicePool, DrumControls, DrumVoice, SynthTrigger, SynthVoiceKind,
};
use super::{
    AudioProject, AudioTrack, ParameterSmoothing, PreviewAction, Renderer, ScheduledTrackAction,
    SynthVoice, TRACK_COUNT,
};
use crate::dsp::{EnvelopeProfile, StereoChorus};
use crate::engine::{GateAction, synth_action};
use crate::model::{
    ArpeggioConfig, CHORD_TRACK_INDEX, ChordShape, ChorusMode, DRUM_TRACK_COUNT, Instrument,
    LEAD_TRACK_INDEX, ParameterId, ParameterLocks, Percent, SYNTH_TRACK_START, StepEvent,
};
use std::sync::atomic::Ordering;

impl Renderer {
    pub(super) fn drum_controls(
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
            Instrument::Tom(p) => Some(DrumControls {
                tune: value(p.tune, locks.tune, ParameterId::Tune),
                tone: value(p.tone, locks.tone, ParameterId::Tone),
                snappy: 0.0,
                decay: value(p.decay, locks.decay, ParameterId::Decay),
                attack: 0.0,
            }),
            Instrument::Cymbal(p) => Some(DrumControls {
                tune: value(p.tune, locks.tune, ParameterId::Tune),
                tone: value(p.tone, locks.tone, ParameterId::Tone),
                snappy: 0.0,
                decay: value(p.decay, locks.decay, ParameterId::Decay),
                attack: 0.0,
            }),
            _ => None,
        }
    }

    pub(super) fn trigger_drum(&mut self, track: usize, accent: bool, locks: ParameterLocks) {
        let t = self.project.tracks[track];
        let Some(controls) = Self::drum_controls(t, locks, &self.lfo_offsets[track]) else {
            return;
        };
        Self::start_drum_voice(&mut self.drums[track], track, controls, accent, self.sr);
    }
    pub(super) fn trigger_preview_drum(
        &mut self,
        track: usize,
        accent: bool,
        locks: ParameterLocks,
    ) {
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
        voice.pan.set(locks.pan.unwrap_or(t.pan).get() as f32, 0);
        voice.locks = locks;
    }
    pub(super) fn start_drum_voice(
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
            2 => 0.025 * (32.0_f32).powf(decay_control),
            3 => 0.09 * (8.888_889_f32).powf(decay_control),
            _ => 0.08 * (22.5_f32).powf(decay_control),
        };
        let (attack, peak) = match track {
            0 => (
                0.0015 + attack_control * 0.0025,
                if accent { 1.22 } else { 0.78 },
            ),
            1 => (0.001, if accent { 1.08 } else { 0.68 }),
            2 => (0.0007, if accent { 0.9 } else { 0.64 }),
            3 => (0.0007, if accent { 1.05 } else { 0.72 }),
            _ => (0.0007, if accent { 0.86 } else { 0.60 }),
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
            2 => {
                voice.filter.set_highpass(4_000.0 + tune * 5_500.0, 0.9, sr);
                voice
                    .filter2
                    .set_bandpass(6_000.0 + tune * 5_000.0, 1.2, sr);
            }
            3 => {
                voice.tom_pitch.trigger_tom(tune, tone, decay, sr);
                voice
                    .filter
                    .set_bandpass(350.0 + tone * 1_900.0, 0.7 + tone * 1.4, sr);
                voice.filter2.set_highpass(90.0 + tone * 260.0, 0.8, sr);
            }
            _ => {
                voice.filter.set_highpass(3_200.0 + tone * 4_800.0, 0.8, sr);
                voice
                    .filter2
                    .set_bandpass(5_000.0 + tune * 5_500.0, 1.0 + tone * 1.2, sr);
            }
        }
    }
    pub(super) fn update_drum_mix(&mut self, track: usize, step: usize, smoothing: u32) {
        let t = self.project.tracks[track];
        let locks = self.locks_at(track, step);
        let voice = &mut self.drums[track];
        Self::apply_drum_mix(voice, t, locks, smoothing);
    }
    pub(super) fn apply_drum_mix(
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
    pub(super) fn apply_synth_params_core(
        project: &AudioProject,
        track: usize,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
        smoothing: u32,
    ) {
        let t = project.tracks[track];
        let (cutoff, resonance, filter_envelope, attack, decay, sustain, release) =
            match t.instrument {
                Instrument::Bass(p) => {
                    voice.kind = SynthVoiceKind::Bass;
                    voice.sub_mode = crate::dsp::SubOscillatorMode::OneOctave;
                    voice.noise_level.set(0.0, smoothing);
                    voice.wave = locks.waveform.unwrap_or(p.waveform);
                    voice
                        .bass_decay_percent
                        .set(locks.decay.unwrap_or(p.decay).get() as f32, smoothing);
                    (
                        p.cutoff,
                        p.resonance,
                        p.filter_envelope,
                        Percent::ZERO,
                        Percent::ZERO,
                        Percent::ZERO,
                        Percent::ZERO,
                    )
                }
                Instrument::Chord(p) => {
                    voice.kind = SynthVoiceKind::Juno;
                    voice.sub_mode = crate::dsp::SubOscillatorMode::OneOctave;
                    voice.noise_level.set(
                        locks.noise.unwrap_or(p.noise).normalized() * 0.35,
                        smoothing,
                    );
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
                    voice.kind = SynthVoiceKind::Sh101;
                    voice.sub_mode = match locks.sub_mode.unwrap_or(p.sub_mode) {
                        crate::model::LeadSubMode::OneOctaveSquare => {
                            crate::dsp::SubOscillatorMode::OneOctave
                        }
                        crate::model::LeadSubMode::TwoOctaveSquare => {
                            crate::dsp::SubOscillatorMode::TwoOctaves
                        }
                        crate::model::LeadSubMode::TwoOctaveNarrowPulse => {
                            crate::dsp::SubOscillatorMode::TwoOctavesNarrowPulse
                        }
                    };
                    voice
                        .noise_level
                        .set(locks.noise.unwrap_or(p.noise).normalized(), smoothing);
                    voice.keyboard_tracking =
                        locks.keyboard_tracking.unwrap_or(p.keyboard_tracking).get() as f32;
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
        if voice.kind != SynthVoiceKind::Bass {
            voice.env.configure_percent(
                locks.attack.unwrap_or(attack).get(),
                locks.decay.unwrap_or(decay).get(),
                locks.sustain.unwrap_or(sustain).get(),
                locks.release.unwrap_or(release).get(),
                smoothing,
            );
        }
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
        if !(voice.kind == SynthVoiceKind::Juno && voice.active) {
            voice
                .pan
                .set(locks.pan.unwrap_or(t.pan).get() as f32, smoothing);
        }
        voice.locks = locks;
    }

    pub(super) fn configure_chorus(
        chorus: &mut StereoChorus,
        track: AudioTrack,
        locks: ParameterLocks,
    ) {
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
    pub(super) fn configure_synth_voice(
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

    pub(super) fn configure_synth_voice_frequency(
        project: &AudioProject,
        sr: f32,
        track: usize,
        frequency: f32,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
    ) {
        let bass_track = matches!(project.tracks[track].instrument, Instrument::Bass(_));
        let legato_slide = bass_track && voice.active && voice.slide_armed;
        let lead_track = matches!(project.tracks[track].instrument, Instrument::Lead(_));
        let lead_legato = lead_track && voice.active && voice.slide_armed;
        let lead_time = match project.tracks[track].instrument {
            Instrument::Lead(p) => voice
                .locks
                .portamento_time
                .unwrap_or(p.portamento_time)
                .get(),
            _ => 0,
        };
        voice.freq.set(
            frequency,
            if legato_slide {
                (sr * 0.060).round() as u32
            } else if lead_legato && lead_time > 0 {
                // 1–100% maps exponentially from 1 ms to 5 s.
                (sr * (0.001 * 5000.0_f32.powf((lead_time - 1) as f32 / 99.0))).round() as u32
            } else {
                0
            },
        );
        Self::apply_synth_params_core(
            project,
            track,
            locks,
            voice,
            ParameterSmoothing::Default.samples(sr),
        );
        let gain = if voice.kind == SynthVoiceKind::Bass {
            1.0
        } else if trigger.accent {
            1.413
        } else {
            1.0
        };
        voice
            .accent_gain
            .set(gain, ParameterSmoothing::Default.samples(sr));
        voice.accent_filter.set(
            if trigger.accent {
                if voice.kind == SynthVoiceKind::Bass {
                    1.0
                } else {
                    0.2
                }
            } else {
                0.0
            },
            ParameterSmoothing::Default.samples(sr),
        );
        if voice.kind == SynthVoiceKind::Bass {
            voice.bass_accent_envelope.trigger(trigger.accent);
            if !legato_slide {
                voice.bass_vca.gate_on();
                voice
                    .bass_filter_envelope
                    .trigger(voice.bass_decay_percent.value());
            }
        } else if !legato_slide && !lead_legato {
            voice.env.gate_on();
        }
        voice.slide_armed =
            matches!(voice.kind, SynthVoiceKind::Bass | SynthVoiceKind::Sh101) && trigger.slide;
        voice.active = true;
    }

    pub(super) fn chord_midis(
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

    pub(super) fn trigger_chord(
        project: &AudioProject,
        sr: f32,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        pool: &mut ChordVoicePool,
    ) {
        let Instrument::Chord(_) = project.tracks[CHORD_TRACK_INDEX].instrument else {
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
            let start = pool.group * CHORD_GROUP_SIZE;
            for voice in &mut pool.voices[start + 1..start + CHORD_GROUP_SIZE] {
                voice.reset_to_idle();
            }
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
            pool.group_voice_counts[pool.group] = 1;
            Self::trigger_arpeggio_tone(project, sr, pool);
            for chorus in &mut pool.choruses {
                Self::configure_chorus(chorus, project.tracks[CHORD_TRACK_INDEX], locks);
            }
            pool.active = true;
            return;
        }
        pool.arpeggiated = false;
        if pool.active {
            for voice in &mut pool.voices
                [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
            {
                voice.gate_off();
                voice.active = false;
            }
        }
        pool.group = 1 - pool.group;
        let (midis, voice_count) =
            Self::chord_midis(project, trigger.degree, trigger.octave, shape);
        let start = pool.group * CHORD_GROUP_SIZE;
        for voice in &mut pool.voices[start + voice_count..start + CHORD_GROUP_SIZE] {
            voice.reset_to_idle();
        }
        for (voice, midi) in pool.voices
            [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + voice_count]
            .iter_mut()
            .zip(midis.into_iter().take(voice_count))
        {
            let frequency = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            Self::configure_synth_voice_frequency(
                project,
                sr,
                CHORD_TRACK_INDEX,
                frequency,
                trigger,
                locks,
                voice,
            );
        }
        pool.voice_count = voice_count;
        pool.group_voice_counts[pool.group] = voice_count;
        let spread =
            locks
                .spread
                .unwrap_or_else(|| match project.tracks[CHORD_TRACK_INDEX].instrument {
                    Instrument::Chord(p) => p.spread,
                    _ => crate::model::ChordSpread::Off,
                });
        let offsets = match voice_count {
            2 => [-50.0, 50.0, 0.0, 0.0],
            3 => [-50.0, 0.0, 50.0, 0.0],
            4 => [-50.0, -16.6667, 16.6667, 50.0],
            _ => [0.0; 4],
        };
        for (index, offset) in offsets.into_iter().take(voice_count).enumerate() {
            let target = locks
                .pan
                .unwrap_or(project.tracks[CHORD_TRACK_INDEX].pan)
                .get() as f32
                + offset * spread.percent().normalized();
            pool.voices[pool.group * CHORD_GROUP_SIZE + index]
                .pan
                .set(target.clamp(0.0, 100.0), 0);
        }
        for chorus in &mut pool.choruses {
            Self::configure_chorus(chorus, project.tracks[CHORD_TRACK_INDEX], locks);
        }
        pool.active = true;
    }

    pub(super) fn trigger_arpeggio_tone(
        project: &AudioProject,
        sr: f32,
        pool: &mut ChordVoicePool,
    ) {
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
            CHORD_TRACK_INDEX,
            frequency,
            pool.arpeggio_trigger,
            pool.arpeggio_locks,
            voice,
        );
        let spread = pool.arpeggio_locks.spread.unwrap_or_else(|| {
            match project.tracks[CHORD_TRACK_INDEX].instrument {
                Instrument::Chord(p) => p.spread,
                _ => crate::model::ChordSpread::Off,
            }
        });
        let offsets = match count {
            2 => [-50.0, 50.0, 0.0, 0.0],
            3 => [-50.0, 0.0, 50.0, 0.0],
            4 => [-50.0, -16.6667, 16.6667, 50.0],
            _ => [0.0; 4],
        };
        voice.pan.set(
            (pool
                .arpeggio_locks
                .pan
                .unwrap_or(project.tracks[CHORD_TRACK_INDEX].pan)
                .get() as f32
                + offsets[index.min(3)] * spread.percent().normalized())
            .clamp(0.0, 100.0),
            0,
        );
    }

    pub(super) fn release_chord(pool: &mut ChordVoicePool) {
        if pool.active {
            for voice in &mut pool.voices
                [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
            {
                voice.gate_off();
                voice.active = false;
            }
            pool.active = false;
            pool.voice_count = 0;
        }
        pool.arpeggiated = false;
        pool.arpeggio.enabled = false;
        pool.preview_remaining = 0;
    }
    pub(super) fn refresh_active_parameters(&mut self, smoothing: u32) {
        for track in 0..TRACK_COUNT {
            let live_locks = match track {
                0..DRUM_TRACK_COUNT => self.drums[track].locks,
                SYNTH_TRACK_START | LEAD_TRACK_INDEX => self.synth[track - SYNTH_TRACK_START].locks,
                CHORD_TRACK_INDEX => self
                    .chord
                    .voices
                    .get(self.chord.group * CHORD_GROUP_SIZE)
                    .map_or(ParameterLocks::default(), |voice| voice.locks),
                _ => ParameterLocks::default(),
            };
            let preview_locks = match track {
                0..DRUM_TRACK_COUNT => self.preview_drums[track].locks,
                SYNTH_TRACK_START | LEAD_TRACK_INDEX => {
                    self.preview[track - SYNTH_TRACK_START].locks
                }
                CHORD_TRACK_INDEX => self
                    .preview_chord
                    .voices
                    .get(self.preview_chord.group * CHORD_GROUP_SIZE)
                    .map_or(ParameterLocks::default(), |voice| voice.locks),
                _ => ParameterLocks::default(),
            };
            self.configure_track_effects(track, live_locks, smoothing, false);
            self.configure_track_effects(track, preview_locks, smoothing, true);
        }
        for track in 0..DRUM_TRACK_COUNT {
            let params = self.project.tracks[track];
            // Locks are captured at the step boundary. A live project snapshot may
            // change the current step, but that edit must not affect its active hit.
            let locks = self.drums[track].locks;
            Self::apply_drum_mix(&mut self.drums[track], params, locks, smoothing);
            let locks = self.preview_drums[track].locks;
            Self::apply_drum_mix(&mut self.preview_drums[track], params, locks, smoothing);
        }
        for track in [SYNTH_TRACK_START, LEAD_TRACK_INDEX] {
            let index = track - SYNTH_TRACK_START;
            if self.synth[index].active {
                // Keep the effective lock chain latched until the next boundary.
                let locks = self.synth[index].locks;
                Self::apply_synth_params_core(
                    &self.project,
                    track,
                    locks,
                    &mut self.synth[index],
                    smoothing,
                );
            }
            if self.preview[index].active {
                let locks = self.preview[index].locks;
                Self::apply_synth_params_core(
                    &self.project,
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
                Self::apply_synth_params_core(
                    &self.project,
                    CHORD_TRACK_INDEX,
                    locks,
                    voice,
                    smoothing,
                );
            }
            for chorus in &mut self.chord.choruses {
                Self::configure_chorus(
                    chorus,
                    self.project.tracks[CHORD_TRACK_INDEX],
                    chorus_locks,
                );
            }
        }
        if self.preview_chord.active {
            let chorus_locks =
                self.preview_chord.voices[self.preview_chord.group * CHORD_GROUP_SIZE].locks;
            for voice in &mut self.preview_chord.voices[self.preview_chord.group * CHORD_GROUP_SIZE
                ..self.preview_chord.group * CHORD_GROUP_SIZE + self.preview_chord.voice_count]
            {
                let locks = voice.locks;
                Self::apply_synth_params_core(
                    &self.project,
                    CHORD_TRACK_INDEX,
                    locks,
                    voice,
                    smoothing,
                );
            }
            for chorus in &mut self.preview_chord.choruses {
                Self::configure_chorus(
                    chorus,
                    self.project.tracks[CHORD_TRACK_INDEX],
                    chorus_locks,
                );
            }
        }
    }
    pub(super) fn audition(&mut self, track: usize, step: usize) {
        if track >= TRACK_COUNT
            || step >= self.project.patterns[self.active_pattern].tracks[track].step_count as usize
        {
            return;
        }
        self.preview_scheduled = [None; 24];
        let count = self.project.patterns[self.active_pattern].tracks[track].steps[step]
            .and_then(|event| event.retrigger_count())
            .unwrap_or(1);
        self.audition_once(track, step);
        let step_samples = self.clock.step_samples().round() as u32;
        for hit in 1..count {
            if let Some(slot) = self
                .preview_scheduled
                .iter_mut()
                .find(|slot| slot.is_none())
            {
                *slot = Some(PreviewAction {
                    remaining: step_samples * u32::from(hit) / u32::from(count),
                    track: track as u8,
                    step: step as u8,
                });
            }
        }
    }
    pub(super) fn audition_once(&mut self, track: usize, step: usize) {
        if track >= TRACK_COUNT
            || step >= self.project.patterns[self.active_pattern].tracks[track].step_count as usize
        {
            return;
        }
        self.reset_preview_lfos(track);
        self.configure_track_effects(
            track,
            self.locks_at(track, step),
            ParameterSmoothing::Default.samples(self.sr),
            true,
        );
        if track < DRUM_TRACK_COUNT {
            let accent = match self.project.patterns[self.active_pattern].tracks[track].steps[step]
            {
                Some(StepEvent::Trigger { accent, .. }) => accent,
                _ => self.project.tracks[track].input_accent,
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
                ..
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
                self.project.tracks[track].input_accent,
                false,
                (track == CHORD_TRACK_INDEX)
                    .then_some(self.project.tracks[track].input_chord_shape),
                if track == CHORD_TRACK_INDEX {
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
        if track == CHORD_TRACK_INDEX {
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
        let v = &mut self.preview[track - SYNTH_TRACK_START];
        Self::configure_synth_voice(&self.project, self.sr, track, trigger, locks, v);
        v.remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
    }
    pub(super) fn boundary(&mut self, global_step: usize) {
        if global_step % 16 == 0
            && let Some(pattern) = self.queued_pattern
        {
            self.activate_pattern(pattern);
            for voice in &mut self.synth {
                voice.gate_off();
                voice.active = false;
            }
            Self::release_chord(&mut self.chord);
        }
        for track in 0..TRACK_COUNT {
            let step = self.next_steps[track];
            self.status.playheads[track].store(step as u8, Ordering::Release);
            let sequence = self.project.patterns[self.active_pattern].tracks[track];
            let event = sequence.steps[step];
            let condition_allowed = event
                .and_then(|event| event.condition())
                .map(|condition| self.condition_passes(track, step, condition))
                .unwrap_or(true);
            let trigger_allowed = match event.and_then(|event| event.condition()) {
                Some(_) if condition_allowed => self.probability_passes(track),
                Some(_) => false,
                None => condition_allowed,
            };
            let step_samples = self.clock.step_samples().round() as u32;
            let delay = if global_step % 2 == 1 {
                step_samples * u32::from(self.project.tracks[track].swing.get()) / 100
            } else {
                0
            };
            self.enqueue(ScheduledTrackAction {
                remaining: delay,
                track: track as u8,
                step: step as u8,
                retrigger: false,
                trigger_allowed,
            });
            if trigger_allowed {
                if let Some(count) = event.and_then(|event| event.retrigger_count()) {
                    let next_delay = if (global_step + 1) % 2 == 1 {
                        step_samples * u32::from(self.project.tracks[track].swing.get()) / 100
                    } else {
                        0
                    };
                    let horizon = (step_samples + next_delay).saturating_sub(delay);
                    for hit in 1..count {
                        self.enqueue(ScheduledTrackAction {
                            remaining: delay + horizon * u32::from(hit) / u32::from(count),
                            track: track as u8,
                            step: step as u8,
                            retrigger: true,
                            trigger_allowed: true,
                        });
                    }
                }
            }
            self.next_steps[track] = (step + 1) % sequence.step_count as usize;
        }
        // Execute straight-grid actions at the boundary itself. Delayed swing
        // actions remain in the fixed queue for subsequent callback samples.
        self.advance_scheduled();
    }

    pub(super) fn process_track_action(
        &mut self,
        track: usize,
        step: usize,
        retrigger: bool,
        trigger_allowed: bool,
    ) {
        let t = self.project.tracks[track];
        let sequence = self.project.patterns[self.active_pattern].tracks[track];
        let locks = self.locks_at(track, step);
        if !retrigger {
            self.configure_track_effects(
                track,
                locks,
                ParameterSmoothing::Default.samples(self.sr),
                false,
            );
        }
        if track < DRUM_TRACK_COUNT {
            if !retrigger {
                self.update_drum_mix(track, step, ParameterSmoothing::Default.samples(self.sr));
            }
            if trigger_allowed
                && let Some(StepEvent::Trigger { accent, locks, .. }) = sequence.steps[step]
            {
                self.trigger_drum(track, accent, locks);
            }
            return;
        }
        let vi = track - SYNTH_TRACK_START;
        let active = if track == CHORD_TRACK_INDEX {
            self.chord.active
        } else {
            self.synth[vi].active
        };
        let action = if !trigger_allowed
            && !retrigger
            && sequence.steps[step]
                .and_then(|event| event.condition())
                .is_some()
        {
            GateAction::Release
        } else {
            synth_action(
                &sequence.steps[..sequence.step_count as usize],
                step,
                active,
            )
        };
        match action {
            GateAction::Trigger {
                degree,
                octave,
                accent,
                slide,
                chord_shape,
                arpeggio,
            } => {
                let trigger = SynthTrigger {
                    degree,
                    octave,
                    accent,
                    slide,
                    chord_shape,
                    arpeggio,
                };
                if track == CHORD_TRACK_INDEX {
                    Self::trigger_chord(&self.project, self.sr, trigger, locks, &mut self.chord);
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
            GateAction::Hold if !retrigger => {
                if matches!(sequence.steps[step], Some(StepEvent::Tie { .. })) {
                    let locks = self.locks_at(track, step);
                    if track == CHORD_TRACK_INDEX {
                        for voice in &mut self.chord.voices[self.chord.group * CHORD_GROUP_SIZE
                            ..self.chord.group * CHORD_GROUP_SIZE + self.chord.voice_count]
                        {
                            Self::apply_synth_params_core(
                                &self.project,
                                track,
                                locks,
                                voice,
                                ParameterSmoothing::Default.samples(self.sr),
                            );
                        }
                        for chorus in &mut self.chord.choruses {
                            Self::configure_chorus(chorus, t, locks);
                        }
                    } else {
                        Self::apply_synth_params_core(
                            &self.project,
                            track,
                            locks,
                            &mut self.synth[vi],
                            ParameterSmoothing::Default.samples(self.sr),
                        );
                    }
                }
            }
            GateAction::Release if !retrigger => {
                self.configure_track_effects(
                    track,
                    ParameterLocks::default(),
                    ParameterSmoothing::Default.samples(self.sr),
                    false,
                );
                if track == CHORD_TRACK_INDEX {
                    // The group has finished its step but may still be in
                    // release.  Keep its latched mixer and voice controls;
                    // only the shared track effect controls return to base.
                    Self::release_chord(&mut self.chord);
                    for chorus in &mut self.chord.choruses {
                        Self::configure_chorus(chorus, t, ParameterLocks::default());
                    }
                } else {
                    Self::apply_synth_params_core(
                        &self.project,
                        track,
                        ParameterLocks::default(),
                        &mut self.synth[vi],
                        ParameterSmoothing::Default.samples(self.sr),
                    );
                    self.synth[vi].gate_off();
                    self.synth[vi].active = false;
                    self.synth[vi].slide_armed = false;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
impl Renderer {
    pub(super) fn apply_synth_params(
        project: &AudioProject,
        _sr: f32,
        track: usize,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
        smoothing: u32,
    ) {
        Self::apply_synth_params_core(project, track, locks, voice, smoothing);
    }
}
