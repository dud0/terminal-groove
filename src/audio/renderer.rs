use super::effects::{modulated_percent, pitch_modulated_frequency};
use super::voices::{CHORD_GROUP_SIZE, DrumVoiceKind, SynthVoiceKind};
use super::{
    AudioProject, AudioStatus, ChordVoicePool, DrumVoice, ParameterId, Renderer, StepClock,
    SynthVoice, TRACK_COUNT, TrackEffectChain, effect_slot,
};
use crate::dsp::{
    Delay, EnvStage, Lfo, MasterLimiter, Reverb, SidechainCompressor, Smoother, exp_map_f32,
};
use crate::model::{FmOperatorField, MAX_STEP_COUNT, Waveform};
use rtrb::{Producer, RingBuffer};
use std::f32::consts::TAU;
use std::sync::{Arc, atomic::Ordering};

const BASS_OUTPUT_GAIN: f32 = 0.8;

#[derive(Default)]
struct MixBus {
    dry_l: f32,
    dry_r: f32,
    delay_l: f32,
    delay_r: f32,
    reverb_l: f32,
    reverb_r: f32,
}

impl MixBus {
    #[inline]
    fn add(&mut self, left: f32, right: f32, delay_send: f32, reverb_send: f32, gain: f32) {
        self.dry_l += left * gain;
        self.dry_r += right * gain;
        self.delay_l += left * delay_send * gain;
        self.delay_r += right * delay_send * gain;
        self.reverb_l += left * reverb_send * gain;
        self.reverb_r += right * reverb_send * gain;
    }
}

fn render_voicing_groups(
    pool: &mut ChordVoicePool,
    effects: &mut [TrackEffectChain; 2],
    sample_rate: f32,
    lfo_offsets: &[f32; ParameterId::ALL.len()],
    output_gain: f32,
    mix: &mut MixBus,
) {
    for (group, effect) in effects.iter_mut().enumerate() {
        let start = group * CHORD_GROUP_SIZE;
        let voice_count = pool.group_voice_counts[group];
        let end = start + voice_count;
        let mut left = 0.0;
        let mut right = 0.0;
        let mut all_idle = voice_count != 0;
        for voice in &mut pool.voices[start..end] {
            if voice.is_idle() {
                Renderer::render_synth(voice, sample_rate, lfo_offsets);
                continue;
            }
            all_idle = false;
            let (x, _, _) = Renderer::render_synth(voice, sample_rate, lfo_offsets);
            let pan = modulated_percent(
                voice.pan.next_value(),
                lfo_offsets[ParameterId::Pan as usize],
            );
            let (pan_l, pan_r) = voice.pan_gains(pan);
            left += x * pan_l;
            right += x * pan_r;
        }
        if all_idle {
            pool.group_voice_counts[group] = 0;
        }
        if voice_count == 0 && !effect.needs_processing() {
            continue;
        }
        let control_voice = &mut pool.voices[start];
        let level = modulated_percent(
            control_voice.level.next_value(),
            lfo_offsets[ParameterId::Level as usize],
        ) / 100.0;
        let delay_send = control_voice.delay_send.next_value();
        let reverb_send = control_voice.reverb_send.next_value();
        let (effect_l, effect_r) = effect.process_stereo(left, right);
        let gain = output_gain * level.powi(2);
        mix.add(effect_l, effect_r, delay_send, reverb_send, gain);
    }
}

/// The Lead filter is calibrated for 50% keyboard tracking around C3. A
/// reference-centered mapping keeps the existing cutoff control useful while
/// making higher notes naturally brighter and lower notes darker.
pub(super) fn lead_keyboard_tracked_cutoff(base_cutoff: f32, frequency: f32, tracking: f32) -> f32 {
    let reference = 130.8128;
    let key_ratio = frequency.max(1.0) / reference;
    base_cutoff * key_ratio.powf(tracking.clamp(0.0, 100.0) / 100.0)
}

impl Renderer {
    fn refresh_preview_activity(&mut self) {
        self.preview_activity = std::array::from_fn(|track| self.preview_track_active(track));
    }

    fn preview_track_active(&self, track: usize) -> bool {
        let voice_active = if self.project.tracks[track].instrument_kind().is_drum() {
            !self.preview_drums[track].envelope.is_idle()
        } else if self.project.tracks[track]
            .instrument_kind()
            .supports_voicing()
        {
            let pool = &self.preview_voicings[track];
            let effects = &self.preview_voicing_effects[track];
            pool.active
                || pool.arpeggiated
                || pool.preview_remaining != 0
                || pool
                    .voices
                    .iter()
                    .any(|voice| voice.env.stage != EnvStage::Idle)
                || effects.iter().any(TrackEffectChain::is_active)
        } else {
            !self.preview[track].is_idle()
        };
        voice_active
            || effect_slot(track).is_some_and(|slot| self.preview_effects[slot].is_active())
            || self
                .preview_scheduled
                .iter()
                .flatten()
                .any(|action| action.track as usize == track)
    }

    pub(super) fn new(project: AudioProject, sr: u32, status: Arc<AudioStatus>) -> Self {
        let (retire, _discarded) = RingBuffer::new(32);
        Self::new_with_retirement(Box::new(project), sr, status, retire)
    }

    pub(super) fn new_with_retirement(
        project: Box<AudioProject>,
        sr: u32,
        status: Arc<AudioStatus>,
        retire: Producer<Box<AudioProject>>,
    ) -> Self {
        let tempo_bpm = project.globals.tempo_bpm;
        let reverb_return = project.globals.reverb_return.normalized();
        let muted: [bool; TRACK_COUNT] = std::array::from_fn(|i| project.tracks[i].muted);
        let mut r = Self {
            project,
            retire,
            pending: None,
            clock: StepClock::new(sr, tempo_bpm),
            next_steps: [0; TRACK_COUNT],
            active_pattern: 0,
            queued_pattern: None,
            song_mode: false,
            active_song: 0,
            queued_song: None,
            song_bar: 0,
            playing: false,
            sr: sr as f32,
            status: status.clone(),
            recording: super::recording::RecordingProducer::disconnected(status),
            drums: std::array::from_fn(|i| {
                DrumVoice::new(0x1234_abcd ^ (i as u32).wrapping_mul(0x9e37_79b9))
            }),
            preview_drums: std::array::from_fn(|i| {
                DrumVoice::new(0x4a31_27dd ^ (i as u32).wrapping_mul(0x7f4a_7c15))
            }),
            synth: std::array::from_fn(|index| {
                SynthVoice::new_with_seed(
                    sr as f32,
                    0x1357_9bdf ^ (index as u32).wrapping_mul(0x9e37_79b9),
                )
            }),
            preview: std::array::from_fn(|index| {
                SynthVoice::new_with_seed(
                    sr as f32,
                    0x2468_ace1 ^ (index as u32).wrapping_mul(0x7f4a_7c15),
                )
            }),
            voicings: std::array::from_fn(|_| ChordVoicePool::new(sr)),
            preview_voicings: std::array::from_fn(|_| ChordVoicePool::new(sr)),
            effects: std::array::from_fn(|_| TrackEffectChain::new(sr)),
            preview_effects: std::array::from_fn(|_| TrackEffectChain::new(sr)),
            voicing_effects: std::array::from_fn(|_| {
                std::array::from_fn(|_| TrackEffectChain::new(sr))
            }),
            preview_voicing_effects: std::array::from_fn(|_| {
                std::array::from_fn(|_| TrackEffectChain::new(sr))
            }),
            sidechain: SidechainCompressor::new(sr),
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            reverb_return: Smoother::new(reverb_return),
            dc: Default::default(),
            limiter: MasterLimiter::new(sr),
            mute: std::array::from_fn(|i| Smoother::new((!muted[i]) as u8 as f32)),
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
            lfo_destinations: [[ParameterId::Level; ParameterId::ALL.len()]; TRACK_COUNT],
            lfo_destination_count: [0; TRACK_COUNT],
            preview_activity: [false; TRACK_COUNT],
            scheduled: [None; super::SCHEDULED_ACTION_COUNT],
            early_armed: [None; TRACK_COUNT],
            cycle_counts: [[0; MAX_STEP_COUNT]; TRACK_COUNT],
            condition_rng: std::array::from_fn(|i| {
                0x8a5c_9d31 ^ (i as u32).wrapping_mul(0x9e37_79b9)
            }),
            probability_rng: std::array::from_fn(|i| {
                0x3c6e_f372 ^ (i as u32).wrapping_mul(0x7f4a_7c15)
            }),
            preview_scheduled: [None; 24],
        };
        r.rebuild_lfo_destinations();
        r.configure_effects(0);
        r
    }

    pub(super) fn render_synth(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        // Chord pools keep a spare group for released notes.  Once an
        // envelope is idle there is no signal to render, so avoid running
        // oscillators and filters for those voices on every callback sample.
        if v.is_idle() {
            if !v.idle_cleanup_done {
                v.voicing_pan_offset = 0.0;
                match v.kind {
                    SynthVoiceKind::Bass => {
                        v.bass_filter.reset();
                        v.bass_filter_envelope.reset();
                        v.bass_accent_envelope.reset();
                    }
                    SynthVoiceKind::Chord => {
                        v.chord_filter.reset();
                        v.chord_highpass.clear_state();
                    }
                    SynthVoiceKind::Lead => v.lead_filter.reset(),
                    SynthVoiceKind::Fm => v.fm_filter.clear_state(),
                }
                v.idle_cleanup_done = true;
            }
            v.declick.reset();
            return (0.0, 0.0, 0.0);
        }
        let (output, delay_send, reverb_send) = match v.kind {
            SynthVoiceKind::Bass => Self::render_bass(v, sr, offsets),
            SynthVoiceKind::Chord => Self::render_chord(v, sr, offsets),
            SynthVoiceKind::Lead => Self::render_lead(v, sr, offsets),
            SynthVoiceKind::Fm => Self::render_fm(v, sr, offsets),
        };
        (v.process_output(output), delay_send, reverb_send)
    }

    fn advance_voice_gate(v: &mut SynthVoice) {
        if v.remaining > 0 {
            v.remaining -= 1;
            if v.remaining == 0 {
                v.gate_off();
                v.active = false;
            }
        }
    }

    fn render_bass(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        Self::advance_voice_gate(v);
        let frequency = v.freq.next_value();
        let env = v.bass_vca.next_sample();
        let cutoff_base = v.cutoff_percent.next_value();
        let cutoff_offset = offsets[ParameterId::Cutoff as usize];
        let bass_decay = modulated_percent(
            v.bass_decay_percent.next_value(),
            offsets[ParameterId::Decay as usize],
        );
        let filter_env = modulated_percent(
            v.filter_env_percent.next_value(),
            offsets[ParameterId::FilterEnvelope as usize],
        ) / 100.0;
        let filter_contour = v.bass_filter_envelope.next_sample();
        let accent_filter = v.bass_accent_envelope.next_sample() * 1.15;
        let minimum_cutoff = 80.0;
        let maximum_cutoff = 8_000.0_f32.min(sr * 0.45);
        let envelope_octaves = 5.0;
        let resonance_percent = modulated_percent(
            v.resonance_percent.next_value(),
            offsets[ParameterId::Resonance as usize],
        );
        let accent_gain = 1.0 + v.bass_accent_envelope.value() * 0.413;
        let delay_send = v.delay_send.next_value();
        let reverb_send = v.reverb_send.next_value();
        let oversampled_rate = sr * 2.0;
        let bass_resonance = resonance_percent / 100.0;
        if v.filter_control_remaining == 0 {
            v.bass_filter_envelope.set_decay(bass_decay);
            let cutoff_percent = modulated_percent(cutoff_base, cutoff_offset);
            let base_cutoff = if cutoff_offset == 0.0 {
                if cutoff_base != v.cached_cutoff_percent
                    || v.cached_cutoff_kind != SynthVoiceKind::Bass
                    || v.cached_cutoff_hz > maximum_cutoff
                {
                    v.cached_cutoff_percent = cutoff_base;
                    v.cached_cutoff_kind = SynthVoiceKind::Bass;
                    v.cached_cutoff_hz = exp_map_f32(cutoff_base, minimum_cutoff, maximum_cutoff);
                }
                v.cached_cutoff_hz
            } else {
                exp_map_f32(cutoff_percent, minimum_cutoff, maximum_cutoff)
            };
            let cutoff = (base_cutoff
                * 2.0_f32.powf(filter_contour * filter_env * envelope_octaves + accent_filter))
            .min(maximum_cutoff);
            v.bass_filter
                .set_parameters_smoothed(cutoff, bass_resonance, oversampled_rate, 16);
            v.filter_control_remaining = 8;
        }
        v.filter_control_remaining -= 1;
        let mut filtered = 0.0;
        for _ in 0..2 {
            let osc = match v.wave {
                Waveform::Saw => v.osc.next_saw(frequency, oversampled_rate),
                Waveform::Square => v.osc.next_square(frequency, oversampled_rate),
            };
            filtered += v.bass_filter.process((osc * 1.35).tanh());
        }
        filtered *= 0.5;
        (
            filtered * env * accent_gain * BASS_OUTPUT_GAIN,
            delay_send,
            reverb_send,
        )
    }

    fn render_chord(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        Self::render_poly_voice(v, sr, offsets, SynthVoiceKind::Chord)
    }

    fn render_lead(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        Self::render_poly_voice(v, sr, offsets, SynthVoiceKind::Lead)
    }

    fn render_fm(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
    ) -> (f32, f32, f32) {
        Self::advance_voice_gate(v);
        let frequency =
            pitch_modulated_frequency(v.freq.next_value(), offsets[ParameterId::Pitch as usize])
                .clamp(1.0, sr * 0.45);
        let env = v.env.next_sample_modulated(
            offsets[ParameterId::Attack as usize],
            offsets[ParameterId::Decay as usize],
            offsets[ParameterId::Sustain as usize],
            offsets[ParameterId::Release as usize],
        );
        let brightness = modulated_percent(
            v.fm_brightness.next_value(),
            offsets[ParameterId::Brightness as usize],
        );
        let accent_index = 1.0 + v.accent_filter.next_value();
        let mut ratios = [1.0; 4];
        let mut levels = [0.0; 4];
        let mut feedback = [0.0; 4];
        let mut routes = [[0.0; 4]; 4];
        let mut carriers = [0.0; 4];
        let topology_smoothing = v.fm_topology_smoothing;
        for operator in 0..4 {
            let level_id = ParameterId::fm_operator(operator, FmOperatorField::Level).unwrap();
            let feedback_id =
                ParameterId::fm_operator(operator, FmOperatorField::Feedback).unwrap();
            ratios[operator] = if topology_smoothing {
                v.fm_ratios[operator].next_value()
            } else {
                v.fm_ratios[operator].value()
            };
            levels[operator] = modulated_percent(
                v.fm_levels[operator].next_value(),
                offsets[level_id as usize],
            ) / 100.0;
            feedback[operator] = modulated_percent(
                v.fm_feedback[operator].next_value(),
                offsets[feedback_id as usize],
            ) / 100.0;
            if topology_smoothing {
                carriers[operator] = v.fm_carriers[operator].next_value();
                for (target, route) in routes[operator].iter_mut().enumerate() {
                    *route = v.fm_routes[operator][target].next_value();
                }
            }
        }
        let carrier_normalization = if topology_smoothing {
            v.fm_carrier_normalization.next_value()
        } else {
            v.fm_carrier_normalization.value()
        };
        let oversampled_rate = sr * 4.0;
        let mut phase_steps = [0.0; 4];
        let mut modulation_indices = [0.0; 4];
        let mut carrier_gains = [0.0; 4];
        let mut active_operators = [false; 4];
        for operator in 0..4 {
            phase_steps[operator] = (frequency * ratios[operator])
                .clamp(1.0, oversampled_rate * 0.45)
                / oversampled_rate;
            modulation_indices[operator] =
                12.0 * levels[operator] * levels[operator] * accent_index;
            carrier_gains[operator] = levels[operator]
                * if topology_smoothing {
                    carriers[operator]
                } else {
                    if v.fm_algorithm.is_carrier(operator) {
                        1.0
                    } else {
                        0.0
                    }
                };
            active_operators[operator] = levels[operator] > 0.0
                && (carrier_gains[operator] > 0.0
                    || if topology_smoothing {
                        routes[operator].iter().any(|route| *route > 0.0)
                    } else {
                        (0..4).any(|target| v.fm_algorithm.routes(operator, target))
                    });
        }
        if !topology_smoothing {
            for (operator, operator_routes) in routes.iter_mut().enumerate() {
                for (target, route) in operator_routes.iter_mut().enumerate() {
                    *route = if v.fm_algorithm.routes(operator, target) {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
        } else if v.fm_topology_smoothing {
            v.fm_topology_smoothing = v.fm_routes.iter().flatten().any(Smoother::is_smoothing)
                || v.fm_carriers.iter().any(Smoother::is_smoothing)
                || v.fm_ratios.iter().any(Smoother::is_smoothing)
                || v.fm_carrier_normalization.is_smoothing();
        }
        let mut sample = 0.0;
        for _ in 0..4 {
            let mut operator_samples = [0.0; 4];
            for operator in (0..4).rev() {
                if active_operators[operator] {
                    let mut phase_modulation =
                        v.fm_previous[operator] * feedback[operator] * 0.95 * std::f32::consts::PI;
                    for source in operator + 1..4 {
                        phase_modulation += operator_samples[source]
                            * modulation_indices[source]
                            * routes[source][operator];
                    }
                    let value = (TAU * v.fm_phases[operator] + phase_modulation).sin();
                    operator_samples[operator] = if value.is_finite() { value } else { 0.0 };
                    v.fm_previous[operator] = operator_samples[operator];
                    sample += operator_samples[operator] * carrier_gains[operator];
                } else {
                    v.fm_previous[operator] = 0.0;
                }
                v.fm_phases[operator] = (v.fm_phases[operator] + phase_steps[operator]).fract();
            }
        }
        sample *= 0.25 * carrier_normalization;
        if v.filter_control_remaining == 0 {
            let cutoff = exp_map_f32(brightness, 200.0, 20_000.0_f32.min(sr * 0.45));
            v.fm_filter.set_lowpass(cutoff, 0.707, sr);
            v.filter_control_remaining = 8;
        }
        v.filter_control_remaining -= 1;
        let filtered = v.fm_filter.process(sample);
        let output = if filtered.is_finite() {
            (filtered * 1.4).tanh()
        } else {
            v.fm_filter.clear_state();
            0.0
        };
        (
            output * env * v.accent_gain.next_value() * 1.25,
            v.delay_send.next_value(),
            v.reverb_send.next_value(),
        )
    }

    fn render_poly_voice(
        v: &mut SynthVoice,
        sr: f32,
        offsets: &[f32; ParameterId::ALL.len()],
        kind: SynthVoiceKind,
    ) -> (f32, f32, f32) {
        Self::advance_voice_gate(v);
        let frequency =
            pitch_modulated_frequency(v.freq.next_value(), offsets[ParameterId::Pitch as usize]);
        let env = v.env.next_sample_modulated(
            offsets[ParameterId::Attack as usize],
            offsets[ParameterId::Decay as usize],
            offsets[ParameterId::Sustain as usize],
            offsets[ParameterId::Release as usize],
        );
        let cutoff_base = v.cutoff_percent.next_value();
        let cutoff_offset = offsets[ParameterId::Cutoff as usize];
        let filter_env = modulated_percent(
            v.filter_env_percent.next_value(),
            offsets[ParameterId::FilterEnvelope as usize],
        ) / 100.0;
        let resonance_percent = modulated_percent(
            v.resonance_percent.next_value(),
            offsets[ParameterId::Resonance as usize],
        );
        let accent_filter = v.accent_filter.next_value();
        let minimum_cutoff = 20.0;
        let maximum_cutoff = 20_000.0_f32.min(sr * 0.45);
        let envelope_octaves = if kind == SynthVoiceKind::Chord {
            5.5
        } else {
            6.0
        };
        if v.filter_control_remaining == 0 {
            let cutoff_percent = modulated_percent(cutoff_base, cutoff_offset);
            let base_cutoff = if cutoff_offset == 0.0 {
                if cutoff_base != v.cached_cutoff_percent
                    || v.cached_cutoff_kind != kind
                    || v.cached_cutoff_hz > maximum_cutoff
                {
                    v.cached_cutoff_percent = cutoff_base;
                    v.cached_cutoff_kind = kind;
                    v.cached_cutoff_hz = exp_map_f32(cutoff_base, minimum_cutoff, maximum_cutoff);
                }
                v.cached_cutoff_hz
            } else {
                exp_map_f32(cutoff_percent, minimum_cutoff, maximum_cutoff)
            };
            let mut cutoff = (base_cutoff
                * 2.0_f32.powf(env * filter_env * envelope_octaves + accent_filter))
            .min(maximum_cutoff);
            if kind == SynthVoiceKind::Lead {
                cutoff = lead_keyboard_tracked_cutoff(cutoff, frequency, v.keyboard_tracking)
                    .min(maximum_cutoff);
            }
            match kind {
                SynthVoiceKind::Chord => {
                    // The fixed low corner is intentionally subtle: it gives
                    // the Chord path its high-pass architecture without
                    // adding a new persisted panel control.
                    v.chord_filter.set_parameters_smoothed(
                        cutoff,
                        resonance_percent / 100.0,
                        sr * 2.0,
                        16,
                    );
                }
                SynthVoiceKind::Lead => v.lead_filter.set_parameters_smoothed(
                    cutoff,
                    resonance_percent / 100.0,
                    sr * 2.0,
                    16,
                ),
                SynthVoiceKind::Bass => unreachable!(),
                SynthVoiceKind::Fm => unreachable!(),
            }
            v.filter_control_remaining = 8;
        }
        v.filter_control_remaining -= 1;
        let mix = modulated_percent(
            v.oscillator_mix.next_value(),
            offsets[ParameterId::OscillatorMix as usize],
        ) / 100.0;
        let width = 0.05
            + modulated_percent(
                v.pulse_width.next_value(),
                offsets[ParameterId::PulseWidth as usize],
            ) / 100.0
                * 0.90;
        let sub = modulated_percent(
            v.sub_oscillator.next_value(),
            offsets[ParameterId::SubOscillator as usize],
        ) / 100.0;
        let noise_level = v.noise_level.next_value();
        let (pulse_gain, saw_gain) = v.oscillator_mix_gains(mix);
        let mut filtered = 0.0;
        for _ in 0..2 {
            let (saw, pulse) = if saw_gain == 0.0 {
                (0.0, v.osc.next_pulse(frequency, width, sr * 2.0))
            } else if pulse_gain == 0.0 {
                (v.osc.next_saw(frequency, sr * 2.0), 0.0)
            } else {
                v.osc.next_saw_pulse(frequency, width, sr * 2.0)
            };
            let sub_sample = match v.sub_mode {
                crate::dsp::SubOscillatorMode::OneOctave => {
                    v.sub_osc.next_sub(frequency, v.sub_mode, sr * 2.0)
                }
                crate::dsp::SubOscillatorMode::TwoOctaves
                | crate::dsp::SubOscillatorMode::TwoOctavesNarrowPulse => {
                    v.sub_osc_2.next_sub(frequency, v.sub_mode, sr * 2.0)
                }
            };
            let noise = if noise_level > 0.0 {
                v.noise.next_sample() * noise_level
            } else {
                0.0
            };
            let source = pulse * pulse_gain + saw * saw_gain + sub_sample * sub + noise;
            filtered += match kind {
                SynthVoiceKind::Chord => v.chord_filter.process(v.chord_highpass.process(source)),
                SynthVoiceKind::Lead => v.lead_filter.process(source),
                SynthVoiceKind::Bass => unreachable!(),
                SynthVoiceKind::Fm => unreachable!(),
            };
        }
        filtered *= 0.5;
        let output_gain = if kind == SynthVoiceKind::Chord {
            1.15 * std::f32::consts::FRAC_1_SQRT_2
        } else {
            1.85
        };
        (
            filtered * env * v.accent_gain.next_value() * output_gain,
            v.delay_send.next_value(),
            v.reverb_send.next_value(),
        )
    }
    pub(super) fn render_drum_raw(
        voice: &mut DrumVoice,
        _track: usize,
        sr: f32,
        _level_offset: f32,
        pan_offset: f32,
    ) -> (f32, f32, f32, f32) {
        let raw = match voice.kind {
            DrumVoiceKind::Kick => {
                let hz = voice.kick_pitch.next_value();
                let body = (TAU * voice.phase).sin();
                voice.phase = (voice.phase + hz / sr).fract();
                let click_env = (-(voice.envelope.elapsed as f32) / (sr * 0.003)).exp();
                body + voice.noise() * click_env * voice.attack * 0.35
            }
            DrumVoiceKind::Snare => {
                let hz = 150.0 + voice.tune * 150.0;
                let lower = 1.0 - 4.0 * (voice.phase - 0.5).abs();
                let upper = 1.0 - 4.0 * (voice.phase2 - 0.5).abs();
                voice.phase = (voice.phase + hz / sr).fract();
                voice.phase2 = (voice.phase2 + hz * 1.72 / sr).fract();
                let noise = voice.noise();
                let noise = voice.filter.process(voice.filter2.process(noise));
                let body = lower * (0.42 - voice.tone * 0.12) + upper * (0.12 + voice.tone * 0.18);
                body + noise * (0.25 + voice.snappy * 0.72)
            }
            DrumVoiceKind::Hat => {
                const RATIOS: [f32; 6] = [1.0, 1.447, 1.617, 1.926, 2.502, 2.663];
                let base = 310.0 + voice.tune * 360.0;
                let mut metal = 0.0;
                for (osc, ratio) in voice.metallic.iter_mut().zip(RATIOS) {
                    metal += osc.next_square(base * ratio, sr);
                }
                metal /= RATIOS.len() as f32;
                let source = metal * 0.82 + voice.noise() * 0.18;
                let bright = voice.filter.process(source);
                bright * 0.75 + voice.filter2.process(source) * 0.25
            }
            DrumVoiceKind::Tom => {
                let hz = voice.tom_pitch.next_value();
                let lower = 1.0 - 4.0 * (voice.phase - 0.5).abs();
                let upper = 1.0 - 4.0 * (voice.phase2 - 0.5).abs();
                voice.phase = (voice.phase + hz / sr).fract();
                voice.phase2 = (voice.phase2 + hz * 1.48 / sr).fract();
                let body = lower * (0.58 - voice.tone * 0.20) + upper * (0.16 + voice.tone * 0.26);
                let click_env = (-(voice.envelope.elapsed as f32) / (sr * 0.004)).exp();
                let click = voice.noise() * click_env * (0.08 + voice.tone * 0.16);
                body + click
            }
            DrumVoiceKind::Cymbal => {
                const RATIOS: [f32; 6] = [1.0, 1.342, 1.483, 1.773, 2.113, 2.641];
                let base = 240.0 + voice.tune * 480.0;
                let mut metal = 0.0;
                for (osc, ratio) in voice.metallic.iter_mut().zip(RATIOS) {
                    metal += osc.next_square(base * ratio, sr);
                }
                metal /= RATIOS.len() as f32;
                let source =
                    metal * (0.92 - voice.tone * 0.30) + voice.noise() * (0.08 + voice.tone * 0.30);
                let bright = voice.filter.process(source);
                bright * 0.82 + voice.filter2.process(source) * 0.18
            }
            DrumVoiceKind::Rimshot => {
                let attack = (voice.envelope.elapsed as f32
                    / voice.rimshot_attack_samples.max(1) as f32)
                    .min(1.0);
                let mut modes = 0.0;
                for mode in 0..3 {
                    modes +=
                        (TAU * voice.rimshot_phases[mode]).sin() * voice.rimshot_amplitudes[mode];
                    voice.rimshot_phases[mode] =
                        (voice.rimshot_phases[mode] + voice.rimshot_frequencies[mode] / sr).fract();
                    voice.rimshot_amplitudes[mode] *= voice.rimshot_decay_coefficients[mode];
                }
                voice.filter.process(modes * attack)
            }
        };
        let envelope = voice.envelope.next_value();
        let sample = if voice.kind == DrumVoiceKind::Rimshot {
            (raw * 1.35).tanh() * 0.82
        } else {
            (raw * 1.15).tanh() * envelope * 1.40
        };
        (
            sample,
            voice.delay_send.next_value(),
            voice.reverb_send.next_value(),
            modulated_percent(voice.pan.next_value(), pan_offset),
        )
    }

    fn render_drum_input(
        voice: &mut DrumVoice,
        track: usize,
        sr: f32,
        level_offset: f32,
        pan_offset: f32,
    ) -> (f32, f32, f32, f32) {
        if voice.envelope.is_idle() {
            return (
                0.0,
                voice.delay_send.next_value(),
                voice.reverb_send.next_value(),
                modulated_percent(voice.pan.next_value(), pan_offset),
            );
        }
        Self::render_drum_raw(voice, track, sr, level_offset, pan_offset)
    }

    #[cfg(test)]
    pub(super) fn render_drum(
        voice: &mut DrumVoice,
        track: usize,
        sr: f32,
        level_offset: f32,
        pan_offset: f32,
    ) -> (f32, f32, f32, f32) {
        let (sample, delay_send, reverb_send, pan) =
            Self::render_drum_raw(voice, track, sr, level_offset, pan_offset);
        let level = modulated_percent(voice.level.next_value(), level_offset) / 100.0;
        (sample * level.powi(2), delay_send, reverb_send, pan)
    }
    pub(super) fn next(&mut self) -> (f32, f32) {
        self.refresh_preview_activity();
        if self.playing {
            self.advance_lfos();
        }
        self.advance_preview_lfos();
        self.advance_preview_scheduled();
        if self.playing
            && let Some(global_step) = self.clock.tick()
        {
            self.boundary(global_step)
        }
        if self.playing {
            self.advance_scheduled();
        }
        self.advance_chord_arpeggios();
        let mut mix = MixBus::default();
        let mut kick_key_l = 0.0;
        let mut kick_key_r = 0.0;
        for i in 0..TRACK_COUNT {
            let kind = self.project.tracks[i].instrument_kind();
            if !kind.is_drum() {
                continue;
            }
            let (x, delay_send, reverb_send, pan) = Self::render_drum_input(
                &mut self.drums[i],
                i,
                self.sr,
                self.lfo_offsets[i][ParameterId::Level as usize],
                self.lfo_offsets[i][ParameterId::Pan as usize],
            );
            let (effect_l, effect_r) = self.effects[i].process(x);
            let (pl, pr) = self.drums[i].pan_gains(pan);
            let level = modulated_percent(
                self.drums[i].level.next_value(),
                self.lfo_offsets[i][ParameterId::Level as usize],
            ) / 100.0;
            let gain = self.mute[i].next_value() * level.powi(2);
            if kind == crate::model::TrackKind::Kick {
                kick_key_l += effect_l * gain;
                kick_key_r += effect_r * gain;
            }
            mix.add(effect_l * pl, effect_r * pr, delay_send, reverb_send, gain);
            if !self.preview_activity[i] {
                continue;
            }
            let (x, delay_send, reverb_send, pan) = Self::render_drum_input(
                &mut self.preview_drums[i],
                i,
                self.sr,
                self.preview_lfo_offsets[i][ParameterId::Level as usize],
                self.preview_lfo_offsets[i][ParameterId::Pan as usize],
            );
            let (effect_l, effect_r) = self.preview_effects[i].process(x);
            let (pl, pr) = self.preview_drums[i].pan_gains(pan);
            let level = modulated_percent(
                self.preview_drums[i].level.next_value(),
                self.preview_lfo_offsets[i][ParameterId::Level as usize],
            ) / 100.0;
            let gain = level.powi(2);
            mix.add(effect_l * pl, effect_r * pr, delay_send, reverb_send, gain);
        }
        self.sidechain.process_stereo(kick_key_l, kick_key_r);
        let duck_gain = self.sidechain.current_gain();
        for track in 0..TRACK_COUNT {
            let kind = self.project.tracks[track].instrument_kind();
            if !kind.is_pitched() {
                continue;
            }
            if kind.supports_voicing() {
                let mute = self.mute[track].next_value();
                render_voicing_groups(
                    &mut self.voicings[track],
                    &mut self.voicing_effects[track],
                    self.sr,
                    &self.lfo_offsets[track],
                    mute * duck_gain,
                    &mut mix,
                );
                if self.preview_activity[track] {
                    render_voicing_groups(
                        &mut self.preview_voicings[track],
                        &mut self.preview_voicing_effects[track],
                        self.sr,
                        &self.preview_lfo_offsets[track],
                        1.0,
                        &mut mix,
                    );
                }
                continue;
            }
            let (x, ds, rs) =
                Self::render_synth(&mut self.synth[track], self.sr, &self.lfo_offsets[track]);
            let (effect_l, effect_r) = self.effects[track].process(x);
            let pan = modulated_percent(
                self.synth[track].pan.next_value(),
                self.lfo_offsets[track][ParameterId::Pan as usize],
            );
            let (pl, pr) = self.synth[track].pan_gains(pan);
            let level = modulated_percent(
                self.synth[track].level.next_value(),
                self.lfo_offsets[track][ParameterId::Level as usize],
            ) / 100.0;
            let gain = self.mute[track].next_value() * level.powi(2);
            mix.add(
                effect_l * duck_gain * pl,
                effect_r * duck_gain * pr,
                ds,
                rs,
                gain,
            );
            if !self.preview_activity[track] {
                continue;
            }
            let (x, ds, rs) = Self::render_synth(
                &mut self.preview[track],
                self.sr,
                &self.preview_lfo_offsets[track],
            );
            let (effect_l, effect_r) = self.preview_effects[track].process(x);
            let pan = modulated_percent(
                self.preview[track].pan.next_value(),
                self.preview_lfo_offsets[track][ParameterId::Pan as usize],
            );
            let (pl, pr) = self.preview[track].pan_gains(pan);
            let level = modulated_percent(
                self.preview[track].level.next_value(),
                self.preview_lfo_offsets[track][ParameterId::Level as usize],
            ) / 100.0;
            let gain = level.powi(2);
            mix.add(effect_l * pl, effect_r * pr, ds, rs, gain);
        }

        let (dl, dr) = self.delay.process(mix.delay_l, mix.delay_r);
        let (rl, rr) = self.reverb.process(mix.reverb_l, mix.reverb_r);
        let reverb_return = self.reverb_return.next_value();
        let (l, r) = self.dc.process(
            mix.dry_l + dl * 0.45 + rl * reverb_return,
            mix.dry_r + dr * 0.45 + rr * reverb_return,
        );
        if !(l.is_finite() && r.is_finite()) {
            self.status.non_finite.store(true, Ordering::Release);
            return (0.0, 0.0);
        }
        self.limiter.process(l, r)
    }

    pub(super) fn advance_chord_arpeggios(&mut self) {
        let bpm = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            if !self.project.tracks[track]
                .instrument_kind()
                .supports_voicing()
            {
                continue;
            }
            let pool = &mut self.voicings[track];
            if pool.arpeggiated && pool.active && pool.arpeggio.tick(self.sr, bpm) {
                Self::trigger_arpeggio_tone(&self.project, self.sr, track, pool);
            }
            let preview = &mut self.preview_voicings[track];
            if preview.arpeggiated && preview.active {
                if preview.preview_remaining > 0 {
                    preview.preview_remaining -= 1;
                }
                if preview.preview_remaining == 0 {
                    Self::release_chord(preview);
                } else if preview.arpeggio.tick(self.sr, bpm) {
                    Self::trigger_arpeggio_tone(&self.project, self.sr, track, preview);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MixBus, lead_keyboard_tracked_cutoff};

    #[test]
    fn mix_bus_routes_dry_and_send_gains_without_state_or_allocation() {
        let mut mix = MixBus::default();
        mix.add(0.5, -0.25, 0.4, 0.2, 0.8);
        for (actual, expected) in [
            (mix.dry_l, 0.4),
            (mix.dry_r, -0.2),
            (mix.delay_l, 0.16),
            (mix.delay_r, -0.08),
            (mix.reverb_l, 0.08),
            (mix.reverb_r, -0.04),
        ] {
            assert!((actual - expected).abs() < f32::EPSILON * 2.0);
        }
    }

    #[test]
    fn lead_keyboard_tracking_is_centered_on_c3_and_moves_cutoff_by_half_octaves() {
        let base = 1_000.0;
        let c3 = lead_keyboard_tracked_cutoff(base, 130.8128, 50.0);
        let c4 = lead_keyboard_tracked_cutoff(base, 261.6256, 50.0);
        let c2 = lead_keyboard_tracked_cutoff(base, 65.4064, 50.0);
        assert!((c3 - base).abs() < 0.01);
        assert!((c4 / base - 2.0_f32.sqrt()).abs() < 0.01);
        assert!((c2 / base - 2.0_f32.sqrt().recip()).abs() < 0.01);
    }
}
