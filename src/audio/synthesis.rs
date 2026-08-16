use super::effects::modulated_percent;
use super::voices::{
    CHORD_GROUP_SIZE, ChordVoicePool, DRUM_SILENCE, DrumControls, DrumVoice, DrumVoiceKind,
    SynthTrigger, SynthVoiceKind,
};
use super::{
    AudioProject, AudioTrack, ParameterSmoothing, PreviewAction, Renderer, ScheduledTrackAction,
    SynthVoice, TRACK_COUNT,
};
use crate::dsp::EnvelopeProfile;
use crate::engine::{GateAction, synth_action};
use crate::model::{
    ArpeggioConfig, CHORD_TRACK_INDEX, ChordShape, DRUM_TRACK_COUNT, DrumRecipeSlot,
    FM_TRACK_INDEX, FmOperatorField, Instrument, LEAD_TRACK_INDEX, ParameterId, ParameterLocks,
    Percent, SYNTH_TRACK_START, StepEvent,
};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
struct TrackActionSchedule {
    lookahead: bool,
    next_early_start: Option<u32>,
}

impl Renderer {
    fn voicing_pan_offset(voice_count: usize, voice: usize) -> f32 {
        match (voice_count, voice) {
            (2, 0) | (3 | 4, 0) => -50.0,
            (2, 1) | (3, 2) | (4, 3) => 50.0,
            (4, 1) => -16.6667,
            (4, 2) => 16.6667,
            _ => 0.0,
        }
    }

    fn apply_synth_pan(
        track: AudioTrack,
        locks: ParameterLocks,
        voice: &mut SynthVoice,
        smoothing: u32,
    ) {
        let center = locks.percent(ParameterId::Pan).unwrap_or(track.pan).get() as f32;
        let offset = if matches!(voice.kind, SynthVoiceKind::Chord | SynthVoiceKind::Fm) {
            voice.voicing_pan_offset
        } else {
            0.0
        };
        voice
            .pan
            .set((center + offset).clamp(0.0, 100.0), smoothing);
    }

    pub(super) fn drum_controls(
        track: AudioTrack,
        recipe: DrumRecipeSlot,
        locks: ParameterLocks,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> Option<DrumControls> {
        let value = |base: Percent, lock: Option<Percent>, id: ParameterId| {
            modulated_percent(lock.unwrap_or(base).get() as f32, offsets[id as usize]) / 100.0
        };
        match track.instrument {
            Instrument::Kick(p) => Some(DrumControls {
                kind: DrumVoiceKind::Kick,
                tune: value(p.tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                tone: 0.5,
                snappy: 0.0,
                decay: value(
                    p.decay,
                    locks.percent(ParameterId::Decay),
                    ParameterId::Decay,
                ),
                attack: value(
                    p.attack,
                    locks.percent(ParameterId::Attack),
                    ParameterId::Attack,
                ),
            }),
            Instrument::Snare(p) => Some(DrumControls {
                kind: DrumVoiceKind::Snare,
                tune: value(p.tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                tone: value(p.tone, locks.percent(ParameterId::Tone), ParameterId::Tone),
                snappy: value(
                    p.snappy,
                    locks.percent(ParameterId::Snappy),
                    ParameterId::Snappy,
                ),
                decay: 0.0,
                attack: 0.0,
            }),
            Instrument::Hat(p) => {
                let (tune, decay) = if recipe == DrumRecipeSlot::TWO {
                    (p.open.tune, p.open.decay)
                } else {
                    (p.tune, p.decay)
                };
                Some(DrumControls {
                    kind: DrumVoiceKind::Hat,
                    tune: value(tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                    tone: 0.5,
                    snappy: 0.0,
                    decay: value(decay, locks.percent(ParameterId::Decay), ParameterId::Decay),
                    attack: 0.0,
                })
            }
            Instrument::Tom(p) => {
                let (tune, tone, decay) = match recipe {
                    DrumRecipeSlot::TWO => (p.medium.tune, p.medium.tone, p.medium.decay),
                    DrumRecipeSlot::THREE => (p.high.tune, p.high.tone, p.high.decay),
                    _ => (p.tune, p.tone, p.decay),
                };
                Some(DrumControls {
                    kind: DrumVoiceKind::Tom,
                    tune: value(tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                    tone: value(tone, locks.percent(ParameterId::Tone), ParameterId::Tone),
                    snappy: 0.0,
                    decay: value(decay, locks.percent(ParameterId::Decay), ParameterId::Decay),
                    attack: 0.0,
                })
            }
            Instrument::Cymbal(p) => Some(DrumControls {
                kind: DrumVoiceKind::Cymbal,
                tune: value(p.tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                tone: value(p.tone, locks.percent(ParameterId::Tone), ParameterId::Tone),
                snappy: 0.0,
                decay: value(
                    p.decay,
                    locks.percent(ParameterId::Decay),
                    ParameterId::Decay,
                ),
                attack: 0.0,
            }),
            Instrument::Rimshot(p) => Some(DrumControls {
                kind: DrumVoiceKind::Rimshot,
                tune: value(p.tune, locks.percent(ParameterId::Tune), ParameterId::Tune),
                tone: value(p.tone, locks.percent(ParameterId::Tone), ParameterId::Tone),
                snappy: 0.0,
                decay: value(
                    p.decay,
                    locks.percent(ParameterId::Decay),
                    ParameterId::Decay,
                ),
                attack: 0.0,
            }),
            _ => None,
        }
    }

    pub(super) fn trigger_drum(
        &mut self,
        track: usize,
        accent: bool,
        recipe: DrumRecipeSlot,
        locks: ParameterLocks,
    ) {
        let t = self.project.tracks[track];
        let Some(controls) = Self::drum_controls(t, recipe, locks, &self.lfo_offsets[track]) else {
            return;
        };
        Self::start_drum_voice(&mut self.drums[track], controls, accent, self.sr);
    }
    pub(super) fn trigger_preview_drum(
        &mut self,
        track: usize,
        accent: bool,
        recipe: DrumRecipeSlot,
        locks: ParameterLocks,
    ) {
        let t = self.project.tracks[track];
        let Some(controls) =
            Self::drum_controls(t, recipe, locks, &self.preview_lfo_offsets[track])
        else {
            return;
        };
        Self::start_drum_voice(&mut self.preview_drums[track], controls, accent, self.sr);
        let voice = &mut self.preview_drums[track];
        voice.level.set(
            locks.percent(ParameterId::Level).unwrap_or(t.level).get() as f32,
            0,
        );
        voice.delay_send.set(
            locks
                .percent(ParameterId::DelaySend)
                .unwrap_or(t.delay_send)
                .normalized(),
            0,
        );
        voice.reverb_send.set(
            locks
                .percent(ParameterId::ReverbSend)
                .unwrap_or(t.reverb_send)
                .normalized(),
            0,
        );
        voice.pan.set(
            locks.percent(ParameterId::Pan).unwrap_or(t.pan).get() as f32,
            0,
        );
        voice.locks = locks;
    }
    pub(super) fn start_drum_voice(
        voice: &mut DrumVoice,
        controls: DrumControls,
        accent: bool,
        sr: f32,
    ) {
        let DrumControls {
            kind,
            tune,
            tone,
            snappy,
            decay: decay_control,
            attack: attack_control,
        } = controls;
        let decay = match kind {
            DrumVoiceKind::Kick => 0.08 * (15.0_f32).powf(decay_control),
            DrumVoiceKind::Snare => 0.08 + snappy * 0.34,
            DrumVoiceKind::Hat => 0.025 * (32.0_f32).powf(decay_control),
            DrumVoiceKind::Tom => 0.09 * (8.888_889_f32).powf(decay_control),
            DrumVoiceKind::Cymbal => 0.08 * (22.5_f32).powf(decay_control),
            DrumVoiceKind::Rimshot => 0.09 * 16.0_f32.powf(decay_control - 0.5),
        };
        let (attack, peak) = match kind {
            DrumVoiceKind::Kick => (
                0.0015 + attack_control * 0.0025,
                if accent { 1.22 } else { 0.78 },
            ),
            DrumVoiceKind::Snare => (0.001, if accent { 1.08 } else { 0.68 }),
            DrumVoiceKind::Hat => (0.0007, if accent { 0.9 } else { 0.64 }),
            DrumVoiceKind::Tom => (0.0007, if accent { 1.05 } else { 0.72 }),
            DrumVoiceKind::Cymbal => (0.0007, if accent { 0.86 } else { 0.60 }),
            DrumVoiceKind::Rimshot => (0.0005, if accent { 1.413 } else { 1.0 }),
        };
        voice.kind = kind;
        voice.tune = tune;
        voice.tone = tone;
        voice.snappy = (snappy
            + if accent && kind == DrumVoiceKind::Snare {
                0.2
            } else {
                0.0
            })
        .min(1.0);
        voice.attack = attack_control;
        voice.accent = accent;
        voice.envelope.trigger(peak, attack, decay, sr);
        match kind {
            DrumVoiceKind::Kick => {
                voice
                    .kick_pitch
                    .trigger((tune + attack_control * 0.15).min(1.0), decay, sr)
            }
            DrumVoiceKind::Snare => {
                voice
                    .filter
                    .set_bandpass(900.0 + tone * 4_500.0, 0.8 + tone * 2.5, sr);
                voice.filter2.set_highpass(450.0 + tone * 900.0, 0.8, sr);
            }
            DrumVoiceKind::Hat => {
                voice.filter.set_highpass(4_000.0 + tune * 5_500.0, 0.9, sr);
                voice
                    .filter2
                    .set_bandpass(6_000.0 + tune * 5_000.0, 1.2, sr);
            }
            DrumVoiceKind::Tom => {
                voice.tom_pitch.trigger_tom(tune, tone, decay, sr);
                voice
                    .filter
                    .set_bandpass(350.0 + tone * 1_900.0, 0.7 + tone * 1.4, sr);
                voice.filter2.set_highpass(90.0 + tone * 260.0, 0.8, sr);
            }
            DrumVoiceKind::Cymbal => {
                voice.filter.set_highpass(3_200.0 + tone * 4_800.0, 0.8, sr);
                voice
                    .filter2
                    .set_bandpass(5_000.0 + tune * 5_500.0, 1.0 + tone * 1.2, sr);
            }
            DrumVoiceKind::Rimshot => {
                let tune_multiplier = 2.0_f32.powf(tune * 2.0 - 1.0);
                let decay_multiplier = 4.0_f32.powf(decay_control * 2.0 - 1.0);
                let body = 1.25 - tone;
                let crack = 0.25 + tone;
                let mut gains = [0.5 * body, body, crack];
                let reference_energy = 1.265_625_f32;
                let energy = gains.iter().map(|gain| gain * gain).sum::<f32>();
                let normalization = (reference_energy / energy.max(f32::EPSILON)).sqrt();
                for gain in &mut gains {
                    *gain *= normalization * peak;
                }
                if accent {
                    gains[2] *= 1.2;
                }
                voice.rimshot_phases = [0.0; 3];
                voice.rimshot_frequencies =
                    [222.0, 500.0, 1_000.0].map(|frequency| frequency * tune_multiplier);
                voice.rimshot_amplitudes = gains;
                voice.rimshot_decay_coefficients = [0.045, 0.020, 0.005]
                    .map(|seconds| (DRUM_SILENCE.ln() / (seconds * decay_multiplier * sr)).exp());
                voice.rimshot_attack_samples = (attack * sr).round().max(1.0) as u32;
                voice.filter.set_highpass(180.0, 0.707, sr);
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
        voice.level.set(
            locks
                .percent(ParameterId::Level)
                .unwrap_or(track.level)
                .get() as f32,
            smoothing,
        );
        voice.delay_send.set(
            locks
                .percent(ParameterId::DelaySend)
                .unwrap_or(track.delay_send)
                .normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks
                .percent(ParameterId::ReverbSend)
                .unwrap_or(track.reverb_send)
                .normalized(),
            smoothing,
        );
        voice.pan.set(
            locks.percent(ParameterId::Pan).unwrap_or(track.pan).get() as f32,
            smoothing,
        );
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
        if let Instrument::Fm(p) = t.instrument {
            voice.kind = SynthVoiceKind::Fm;
            voice.env.set_profile(EnvelopeProfile::Generic);
            let algorithm = locks.fm_algorithm().unwrap_or(p.algorithm);
            voice.fm_algorithm = algorithm;
            let carrier_count = (0..4)
                .filter(|operator| algorithm.is_carrier(*operator))
                .count() as f32;
            voice
                .fm_carrier_normalization
                .set(carrier_count.sqrt().recip(), smoothing);
            voice.fm_topology_smoothing = smoothing != 0;
            for operator in 0..4 {
                let ratio_id = ParameterId::fm_operator(operator, FmOperatorField::Ratio).unwrap();
                let level_id = ParameterId::fm_operator(operator, FmOperatorField::Level).unwrap();
                let feedback_id =
                    ParameterId::fm_operator(operator, FmOperatorField::Feedback).unwrap();
                voice.fm_ratios[operator].set(
                    locks
                        .fm_ratio(ratio_id)
                        .unwrap_or(p.operators[operator].ratio)
                        .value(),
                    smoothing,
                );
                voice.fm_levels[operator].set(
                    locks
                        .percent(level_id)
                        .unwrap_or(p.operators[operator].level)
                        .get() as f32,
                    smoothing,
                );
                voice.fm_feedback[operator].set(
                    locks
                        .percent(feedback_id)
                        .unwrap_or(p.operators[operator].feedback)
                        .get() as f32,
                    smoothing,
                );
                voice.fm_carriers[operator].set(
                    if algorithm.is_carrier(operator) {
                        1.0
                    } else {
                        0.0
                    },
                    smoothing,
                );
                for target in 0..4 {
                    voice.fm_routes[operator][target].set(
                        if algorithm.routes(operator, target) {
                            1.0
                        } else {
                            0.0
                        },
                        smoothing,
                    );
                }
            }
            voice.fm_brightness.set(
                locks
                    .percent(ParameterId::Brightness)
                    .unwrap_or(p.brightness)
                    .get() as f32,
                smoothing,
            );
            voice.env.configure_percent(
                locks.percent(ParameterId::Attack).unwrap_or(p.attack).get(),
                locks.percent(ParameterId::Decay).unwrap_or(p.decay).get(),
                locks
                    .percent(ParameterId::Sustain)
                    .unwrap_or(p.sustain)
                    .get(),
                locks
                    .percent(ParameterId::Release)
                    .unwrap_or(p.release)
                    .get(),
                smoothing,
            );
            voice.level.set(
                locks.percent(ParameterId::Level).unwrap_or(t.level).get() as f32,
                smoothing,
            );
            voice.delay_send.set(
                locks
                    .percent(ParameterId::DelaySend)
                    .unwrap_or(t.delay_send)
                    .normalized(),
                smoothing,
            );
            voice.reverb_send.set(
                locks
                    .percent(ParameterId::ReverbSend)
                    .unwrap_or(t.reverb_send)
                    .normalized(),
                smoothing,
            );
            Self::apply_synth_pan(t, locks, voice, smoothing);
            voice.locks = locks;
            return;
        }
        let (cutoff, resonance, filter_envelope, attack, decay, sustain, release) =
            match t.instrument {
                Instrument::Bass(p) => {
                    voice.kind = SynthVoiceKind::Bass;
                    voice.sub_mode = crate::dsp::SubOscillatorMode::OneOctave;
                    voice.noise_level.set(0.0, smoothing);
                    voice.wave = locks.waveform().unwrap_or(p.waveform);
                    voice.bass_decay_percent.set(
                        locks.percent(ParameterId::Decay).unwrap_or(p.decay).get() as f32,
                        smoothing,
                    );
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
                    voice.kind = SynthVoiceKind::Chord;
                    voice.sub_mode = crate::dsp::SubOscillatorMode::OneOctave;
                    voice.noise_level.set(
                        locks
                            .percent(ParameterId::Noise)
                            .unwrap_or(p.noise)
                            .normalized()
                            * 0.35,
                        smoothing,
                    );
                    voice.env.set_profile(EnvelopeProfile::Chord);
                    voice.oscillator_mix.set(
                        locks
                            .percent(ParameterId::OscillatorMix)
                            .unwrap_or(p.oscillator_mix)
                            .get() as f32,
                        smoothing,
                    );
                    voice.pulse_width.set(
                        locks
                            .percent(ParameterId::PulseWidth)
                            .unwrap_or(p.pulse_width)
                            .get() as f32,
                        smoothing,
                    );
                    voice.sub_oscillator.set(
                        locks
                            .percent(ParameterId::SubOscillator)
                            .unwrap_or(p.sub_oscillator)
                            .get() as f32,
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
                    voice.kind = SynthVoiceKind::Lead;
                    voice.sub_mode = match locks.lead_sub_mode().unwrap_or(p.sub_mode) {
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
                    voice.noise_level.set(
                        locks
                            .percent(ParameterId::Noise)
                            .unwrap_or(p.noise)
                            .normalized(),
                        smoothing,
                    );
                    voice.keyboard_tracking = locks
                        .percent(ParameterId::KeyboardTracking)
                        .unwrap_or(p.keyboard_tracking)
                        .get() as f32;
                    voice.env.set_profile(EnvelopeProfile::Lead);
                    voice.oscillator_mix.set(
                        locks
                            .percent(ParameterId::OscillatorMix)
                            .unwrap_or(p.oscillator_mix)
                            .get() as f32,
                        smoothing,
                    );
                    voice.pulse_width.set(
                        locks
                            .percent(ParameterId::PulseWidth)
                            .unwrap_or(p.pulse_width)
                            .get() as f32,
                        smoothing,
                    );
                    voice.sub_oscillator.set(
                        locks
                            .percent(ParameterId::SubOscillator)
                            .unwrap_or(p.sub_oscillator)
                            .get() as f32,
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
        voice.cutoff_percent.set(
            locks.percent(ParameterId::Cutoff).unwrap_or(cutoff).get() as f32,
            smoothing,
        );
        voice.resonance_percent.set(
            locks
                .percent(ParameterId::Resonance)
                .unwrap_or(resonance)
                .get() as f32,
            smoothing,
        );
        voice.filter_env_percent.set(
            locks
                .percent(ParameterId::FilterEnvelope)
                .unwrap_or(filter_envelope)
                .get() as f32,
            smoothing,
        );
        if voice.kind != SynthVoiceKind::Bass {
            voice.env.configure_percent(
                locks.percent(ParameterId::Attack).unwrap_or(attack).get(),
                locks.percent(ParameterId::Decay).unwrap_or(decay).get(),
                locks.percent(ParameterId::Sustain).unwrap_or(sustain).get(),
                locks.percent(ParameterId::Release).unwrap_or(release).get(),
                smoothing,
            );
        }
        voice.level.set(
            locks.percent(ParameterId::Level).unwrap_or(t.level).get() as f32,
            smoothing,
        );
        voice.delay_send.set(
            locks
                .percent(ParameterId::DelaySend)
                .unwrap_or(t.delay_send)
                .normalized(),
            smoothing,
        );
        voice.reverb_send.set(
            locks
                .percent(ParameterId::ReverbSend)
                .unwrap_or(t.reverb_send)
                .normalized(),
            smoothing,
        );
        Self::apply_synth_pan(t, locks, voice, smoothing);
        voice.locks = locks;
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
        let hard_retrigger = !legato_slide && !lead_legato;
        let lead_time = match project.tracks[track].instrument {
            Instrument::Lead(p) => voice
                .locks
                .percent(ParameterId::PortamentoTime)
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
        voice.idle_cleanup_done = false;
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
                voice.bass_vca.retrigger();
                voice
                    .bass_filter_envelope
                    .trigger(voice.bass_decay_percent.value());
            }
        } else if hard_retrigger {
            voice.env.retrigger();
        }
        if hard_retrigger {
            voice.begin_note_transition();
        }
        voice.slide_armed =
            matches!(voice.kind, SynthVoiceKind::Bass | SynthVoiceKind::Lead) && trigger.slide;
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
        track: usize,
        trigger: SynthTrigger,
        locks: ParameterLocks,
        pool: &mut ChordVoicePool,
    ) {
        let shape = match project.tracks[track].instrument {
            Instrument::Chord(_) => trigger.chord_shape.unwrap_or(ChordShape::TriadRoot),
            Instrument::Fm(_) => trigger.chord_shape.unwrap_or(ChordShape::Single),
            _ => return,
        };
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
            Self::trigger_arpeggio_tone(project, sr, track, pool);
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
        for (index, (voice, midi)) in pool.voices
            [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + voice_count]
            .iter_mut()
            .zip(midis.into_iter().take(voice_count))
            .enumerate()
        {
            voice.voicing_pan_offset = Self::voicing_pan_offset(voice_count, index);
            let frequency = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            Self::configure_synth_voice_frequency(
                project, sr, track, frequency, trigger, locks, voice,
            );
            Self::apply_synth_pan(project.tracks[track], locks, voice, 0);
        }
        pool.voice_count = voice_count;
        pool.group_voice_counts[pool.group] = voice_count;
        pool.active = true;
    }

    pub(super) fn trigger_arpeggio_tone(
        project: &AudioProject,
        sr: f32,
        track: usize,
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
        voice.voicing_pan_offset = Self::voicing_pan_offset(count, index.min(count - 1));
        let frequency = 440.0 * 2.0_f32.powf((midis[index.min(count - 1)] as f32 - 69.0) / 12.0);
        Self::configure_synth_voice_frequency(
            project,
            sr,
            track,
            frequency,
            pool.arpeggio_trigger,
            pool.arpeggio_locks,
            voice,
        );
        Self::apply_synth_pan(project.tracks[track], pool.arpeggio_locks, voice, 0);
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

    fn apply_voicing_tie(
        project: &AudioProject,
        track: usize,
        locks: ParameterLocks,
        pool: &mut ChordVoicePool,
        smoothing: u32,
    ) {
        for voice in &mut pool.voices
            [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
        {
            Self::apply_synth_params_core(project, track, locks, voice, smoothing);
        }
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
                FM_TRACK_INDEX => self
                    .fm_chord
                    .voices
                    .get(self.fm_chord.group * CHORD_GROUP_SIZE)
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
                FM_TRACK_INDEX => self
                    .preview_fm_chord
                    .voices
                    .get(self.preview_fm_chord.group * CHORD_GROUP_SIZE)
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
        }
        if self.preview_chord.active {
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
        }
        for voice in &mut self.fm_chord.voices {
            if !voice.is_idle() {
                Self::apply_synth_params_core(
                    &self.project,
                    FM_TRACK_INDEX,
                    voice.locks,
                    voice,
                    smoothing,
                );
            }
        }
        for voice in &mut self.preview_fm_chord.voices {
            if !voice.is_idle() {
                Self::apply_synth_params_core(
                    &self.project,
                    FM_TRACK_INDEX,
                    voice.locks,
                    voice,
                    smoothing,
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
        self.reset_preview_lfos(track);
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
        self.configure_track_effects(
            track,
            self.locks_at(track, step),
            ParameterSmoothing::Default.samples(self.sr),
            true,
        );
        if track < DRUM_TRACK_COUNT {
            let (accent, recipe) =
                match self.project.patterns[self.active_pattern].tracks[track].steps[step] {
                    Some(StepEvent::Trigger { accent, recipe, .. }) => (accent, recipe),
                    _ => (self.project.tracks[track].input_accent, DrumRecipeSlot::ONE),
                };
            self.restart_preview_trigger_lfos(track);
            self.trigger_preview_drum(track, accent, recipe, self.locks_at(track, step));
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
            Some(StepEvent::LeadNote {
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
                    Some(StepEvent::LeadNote {
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
                    _ => return,
                }
            }
            _ => (
                self.project.tracks[track].input_degree,
                self.project.tracks[track].input_octave,
                self.project.tracks[track].input_accent,
                false,
                matches!(track, CHORD_TRACK_INDEX | FM_TRACK_INDEX)
                    .then_some(self.project.tracks[track].input_chord_shape),
                if matches!(track, CHORD_TRACK_INDEX | FM_TRACK_INDEX) {
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
        self.restart_preview_trigger_lfos(track);
        if matches!(track, CHORD_TRACK_INDEX | FM_TRACK_INDEX) {
            let pool = if track == CHORD_TRACK_INDEX {
                &mut self.preview_chord
            } else {
                &mut self.preview_fm_chord
            };
            Self::trigger_chord(&self.project, self.sr, track, trigger, locks, pool);
            let remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
            for voice in &mut pool.voices
                [pool.group * CHORD_GROUP_SIZE..pool.group * CHORD_GROUP_SIZE + pool.voice_count]
            {
                voice.remaining = remaining;
            }
            if pool.arpeggiated {
                let interval = self.sr as f64 * 60.0 / self.project.globals.tempo_bpm as f64
                    * pool.arpeggio.rate.beats();
                pool.preview_remaining = (interval * pool.arpeggio.order_len as f64).ceil() as u64;
            }
            return;
        }
        let v = &mut self.preview[track - SYNTH_TRACK_START];
        Self::configure_synth_voice(&self.project, self.sr, track, trigger, locks, v);
        v.remaining = (self.sr * 60.0 / self.project.globals.tempo_bpm as f32) as u32;
    }
    fn swing_delay(&self, global_step: usize, track: usize, step_samples: u32) -> u32 {
        if global_step % 2 == 1 {
            step_samples * u32::from(self.project.tracks[track].swing.get()) / 100
        } else {
            0
        }
    }

    fn trigger_allowed_for(&mut self, track: usize, step: usize, event: Option<StepEvent>) -> bool {
        let condition_allowed = event
            .and_then(|event| event.condition())
            .map(|condition| self.condition_passes(track, step, condition))
            .unwrap_or(true);
        match event.and_then(|event| event.condition()) {
            Some(_) if condition_allowed => self.probability_passes(track),
            Some(_) => false,
            None => condition_allowed,
        }
    }

    fn schedule_track_actions(
        &mut self,
        track: usize,
        step: usize,
        global_step: usize,
        step_samples: u32,
        trigger_allowed: bool,
        schedule: TrackActionSchedule,
    ) {
        let sequence = self.project.patterns[self.active_pattern].tracks[track];
        let event = sequence.steps[step];
        let current_delay = self.swing_delay(global_step, track, step_samples);
        let next_delay = self.swing_delay(global_step.wrapping_add(1), track, step_samples);
        let previous_delay = self.swing_delay(global_step.wrapping_sub(1), track, step_samples);
        let horizon = step_samples + next_delay - current_delay;
        let micro_offset = event
            .and_then(|event| event.microtiming())
            .map(|value| i64::from(step_samples) * i64::from(value.get()) / 100)
            .unwrap_or(0);
        let count = event.and_then(|event| event.retrigger_count()).unwrap_or(1);
        let last_offset = horizon * u32::from(count.saturating_sub(1)) / u32::from(count);
        let next_slot = step_samples + next_delay;
        let cutoff = schedule.next_early_start.unwrap_or(next_slot);
        // The cutoff is exclusive: a following event must win the sample at its
        // own scheduled start.  If the complete burst cannot fit after clamping
        // its base hit, retriggers are compressed into the remaining interval.
        let max_start = cutoff.saturating_sub(1).saturating_sub(last_offset);
        let requested_start = i64::from(current_delay) + micro_offset;
        let min_start = if schedule.lookahead {
            i64::from(previous_delay) - i64::from(step_samples)
        } else {
            0
        };
        let start = requested_start.clamp(min_start, i64::from(max_start));
        let burst_horizon = horizon.min((i64::from(cutoff) - start).max(0) as u32);
        let origin = if schedule.lookahead {
            i64::from(step_samples)
        } else {
            0
        };
        let remaining_start = (origin + start).max(0) as u32;
        self.enqueue(ScheduledTrackAction {
            remaining: remaining_start,
            track: track as u8,
            step: step as u8,
            retrigger: false,
            trigger_allowed,
        });
        if trigger_allowed {
            for hit in 1..count {
                self.enqueue(ScheduledTrackAction {
                    remaining: remaining_start + burst_horizon * u32::from(hit) / u32::from(count),
                    track: track as u8,
                    step: step as u8,
                    retrigger: true,
                    trigger_allowed: true,
                });
            }
        }
    }

    fn early_start_from_previous_boundary(
        &self,
        track: usize,
        step: usize,
        global_step: usize,
        step_samples: u32,
    ) -> Option<u32> {
        let event = self.project.patterns[self.active_pattern].tracks[track].steps[step];
        let microtiming = event.and_then(|event| event.microtiming())?;
        let current_delay = self.swing_delay(global_step, track, step_samples);
        let requested_start =
            i64::from(current_delay) + i64::from(step_samples) * i64::from(microtiming.get()) / 100;
        if requested_start >= 0 {
            return None;
        }
        let previous_delay = self.swing_delay(global_step.wrapping_sub(1), track, step_samples);
        let start = requested_start.max(i64::from(previous_delay) - i64::from(step_samples));
        Some((i64::from(step_samples) + start) as u32)
    }

    fn transition_due_at(&self, global_step: usize) -> bool {
        if global_step % 16 != 0 {
            return false;
        }
        if self.queued_song.is_some() || self.queued_pattern.is_some() {
            return true;
        }
        self.song_mode
            && global_step > 0
            && self.song_bar + 1 >= self.project.song[self.active_song].bars
    }

    pub(super) fn boundary(&mut self, global_step: usize) {
        let mut transitioned = false;
        if global_step % 16 == 0 {
            if let Some(entry) = self.queued_song {
                self.activate_song(entry);
                transitioned = true;
            } else if let Some(pattern) = self.queued_pattern {
                self.song_mode = false;
                self.status.song_mode.store(false, Ordering::Release);
                self.activate_pattern(pattern);
                transitioned = true;
            } else if self.song_mode && global_step > 0 {
                let previous_pattern = self.active_pattern;
                if !self.advance_song() {
                    self.command(crate::audio::AudioCommand::Stop);
                    return;
                }
                transitioned = self.active_pattern != previous_pattern;
            }
        }
        if transitioned {
            for voice in &mut self.synth {
                voice.gate_off();
                voice.active = false;
            }
            Self::release_chord(&mut self.chord);
            Self::release_chord(&mut self.fm_chord);
        }
        for track in 0..TRACK_COUNT {
            let step = self.next_steps[track];
            self.status.playheads[track].store(step as u8, Ordering::Release);
            let sequence = self.project.patterns[self.active_pattern].tracks[track];
            let event = sequence.steps[step];
            let prearmed = self.early_armed[track] == Some(step as u8);
            let step_samples = self.clock.step_samples().round() as u32;
            let next_global_step = global_step.wrapping_add(1);
            let next_step = (step + 1) % sequence.step_count as usize;
            let next_early_start = (!self.transition_due_at(next_global_step))
                .then(|| {
                    self.early_start_from_previous_boundary(
                        track,
                        next_step,
                        next_global_step,
                        step_samples,
                    )
                })
                .flatten();
            if !prearmed {
                let trigger_allowed = self.trigger_allowed_for(track, step, event);
                self.schedule_track_actions(
                    track,
                    step,
                    global_step,
                    step_samples,
                    trigger_allowed,
                    TrackActionSchedule {
                        lookahead: false,
                        next_early_start,
                    },
                );
            }
            self.early_armed[track] = None;
            self.next_steps[track] = next_step;
            if next_early_start.is_some() {
                let next_event =
                    self.project.patterns[self.active_pattern].tracks[track].steps[next_step];
                let trigger_allowed = self.trigger_allowed_for(track, next_step, next_event);
                self.schedule_track_actions(
                    track,
                    next_step,
                    next_global_step,
                    step_samples,
                    trigger_allowed,
                    TrackActionSchedule {
                        lookahead: true,
                        next_early_start: None,
                    },
                );
                self.early_armed[track] = Some(next_step as u8);
            }
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
                && let Some(StepEvent::Trigger {
                    accent,
                    recipe,
                    locks,
                    ..
                }) = sequence.steps[step]
            {
                self.restart_trigger_lfos(track);
                self.trigger_drum(track, accent, recipe, locks);
            }
            return;
        }
        let vi = track - SYNTH_TRACK_START;
        let active = match track {
            CHORD_TRACK_INDEX => self.chord.active,
            FM_TRACK_INDEX => self.fm_chord.active,
            _ => self.synth[vi].active,
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
                self.restart_trigger_lfos(track);
                if track == CHORD_TRACK_INDEX {
                    Self::trigger_chord(
                        &self.project,
                        self.sr,
                        track,
                        trigger,
                        locks,
                        &mut self.chord,
                    );
                } else if track == FM_TRACK_INDEX {
                    Self::trigger_chord(
                        &self.project,
                        self.sr,
                        track,
                        trigger,
                        locks,
                        &mut self.fm_chord,
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
            GateAction::Hold if !retrigger => {
                if matches!(sequence.steps[step], Some(StepEvent::Tie { .. })) {
                    let locks = self.locks_at(track, step);
                    if track == CHORD_TRACK_INDEX {
                        Self::apply_voicing_tie(
                            &self.project,
                            track,
                            locks,
                            &mut self.chord,
                            ParameterSmoothing::Default.samples(self.sr),
                        );
                    } else if track == FM_TRACK_INDEX {
                        Self::apply_voicing_tie(
                            &self.project,
                            track,
                            locks,
                            &mut self.fm_chord,
                            ParameterSmoothing::Default.samples(self.sr),
                        );
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
                } else if track == FM_TRACK_INDEX {
                    Self::release_chord(&mut self.fm_chord);
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
