use super::effects::{modulated_percent, pitch_modulated_frequency};
use super::voices::CHORD_GROUP_SIZE;
use super::{
    AudioProject, AudioStatus, ChordVoicePool, DrumVoice, ParameterId, Renderer, StepClock,
    SynthVoice, TRACK_COUNT, TrackEffectChain,
};
use crate::dsp::{
    Delay, EnvStage, Lfo, MasterLimiter, Reverb, SidechainCompressor, Smoother, exp_map_f32,
};
use crate::model::{MAX_STEP_COUNT, Waveform};
use rtrb::{Producer, RingBuffer};
use std::f32::consts::TAU;
use std::sync::{Arc, atomic::Ordering};
impl Renderer {
    fn refresh_preview_activity(&mut self) {
        self.preview_activity = std::array::from_fn(|track| self.preview_track_active(track));
    }

    fn preview_track_active(&self, track: usize) -> bool {
        let voice_active = if track < super::DRUM_TRACK_COUNT {
            !self.preview_drums[track].envelope.is_idle()
        } else if track == super::CHORD_TRACK_INDEX {
            self.preview_chord.active
                || self.preview_chord.arpeggiated
                || self.preview_chord.preview_remaining != 0
                || self
                    .preview_chord
                    .voices
                    .iter()
                    .any(|voice| voice.env.stage != EnvStage::Idle)
                || self.preview_chord.chorus.is_active()
        } else {
            self.preview[track - super::SYNTH_TRACK_START].env.stage != EnvStage::Idle
        };
        voice_active
            || self.preview_effects[track].is_active()
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
            playing: false,
            sr: sr as f32,
            status,
            drums: std::array::from_fn(|i| {
                DrumVoice::new(
                    [
                        0x1234_abcd,
                        0x9137_2468,
                        0xdead_beef,
                        0x71a2_4c9d,
                        0x4f83_d2b1,
                    ][i],
                )
            }),
            preview_drums: std::array::from_fn(|i| {
                DrumVoice::new(
                    [
                        0x4a31_27dd,
                        0xa187_4c29,
                        0x6d2b_f193,
                        0x8c1e_5a77,
                        0xb643_92e0,
                    ][i],
                )
            }),
            synth: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            preview: std::array::from_fn(|_| SynthVoice::new(sr as f32)),
            chord: ChordVoicePool::new(sr),
            preview_chord: ChordVoicePool::new(sr),
            effects: std::array::from_fn(|_| TrackEffectChain::new(sr)),
            preview_effects: std::array::from_fn(|_| TrackEffectChain::new(sr)),
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
            scheduled: [None; 32],
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
        if v.env.stage == EnvStage::Idle {
            return (0.0, 0.0, 0.0);
        }
        if v.remaining > 0 {
            v.remaining -= 1;
            if v.remaining == 0 {
                v.env.gate_off();
                v.active = false;
            }
        }
        let mut frequency = v.freq.next_value();
        if !v.bass {
            frequency = pitch_modulated_frequency(frequency, offsets[ParameterId::Pitch as usize]);
        }
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
        let accent_filter = v.accent_filter.next_value();
        let (minimum_cutoff, maximum_cutoff, envelope_octaves) = if v.bass {
            (80.0, 8_000.0_f32.min(sr * 0.45), 5.0)
        } else {
            (20.0, 20_000.0_f32.min(sr * 0.45), 6.0)
        };
        let cutoff_percent = modulated_percent(cutoff_base, cutoff_offset);
        let base_cutoff = if cutoff_offset == 0.0 {
            if cutoff_base != v.cached_cutoff_percent
                || v.cached_cutoff_bass != v.bass
                || v.cached_cutoff_hz > maximum_cutoff
            {
                v.cached_cutoff_percent = cutoff_base;
                v.cached_cutoff_bass = v.bass;
                v.cached_cutoff_hz = exp_map_f32(cutoff_base, minimum_cutoff, maximum_cutoff);
            }
            v.cached_cutoff_hz
        } else {
            exp_map_f32(cutoff_percent, minimum_cutoff, maximum_cutoff)
        };
        let cutoff = (base_cutoff
            * 2.0_f32.powf(env * (filter_env * envelope_octaves + accent_filter)))
        .min(maximum_cutoff);
        let resonance_percent = modulated_percent(
            v.resonance_percent.next_value(),
            offsets[ParameterId::Resonance as usize],
        );
        let accent_gain = v.accent_gain.next_value();
        let delay_send = v.delay_send.next_value();
        let reverb_send = v.reverb_send.next_value();
        let oversampled_rate = sr * 2.0;
        let bass_resonance = resonance_percent / 100.0;
        if v.bass {
            v.bass_filter
                .set_parameters(cutoff, bass_resonance, oversampled_rate);
        } else {
            v.roland_filter.set_parameters(
                cutoff,
                bass_resonance * if v.chord { 0.95 } else { 1.0 },
                oversampled_rate,
            );
        }
        let (mix, width, sub) = if v.bass {
            (0.0, 0.5, 0.0)
        } else {
            (
                modulated_percent(
                    v.oscillator_mix.next_value(),
                    offsets[ParameterId::OscillatorMix as usize],
                ) / 100.0,
                0.05 + modulated_percent(
                    v.pulse_width.next_value(),
                    offsets[ParameterId::PulseWidth as usize],
                ) / 100.0
                    * 0.90,
                modulated_percent(
                    v.sub_oscillator.next_value(),
                    offsets[ParameterId::SubOscillator as usize],
                ) / 100.0,
            )
        };
        let angle = mix * std::f32::consts::FRAC_PI_2;
        let mix_cos = angle.cos();
        let mix_sin = angle.sin();
        let mut filtered = 0.0;
        for _ in 0..2 {
            let osc = if v.bass {
                match v.wave {
                    Waveform::Saw => v.osc.next_saw(frequency, oversampled_rate),
                    Waveform::Square => v.osc.next_square(frequency, oversampled_rate),
                }
            } else {
                let (saw, pulse) = v.osc.next_saw_pulse(frequency, width, oversampled_rate);
                pulse * mix_cos
                    + saw * mix_sin
                    + v.sub_osc.next_square(frequency * 0.5, oversampled_rate) * sub
            };
            filtered += if v.bass {
                let driven = (osc * 1.35).tanh();
                v.bass_filter.process(driven)
            } else {
                let driven = (osc * if v.chord { 1.10 } else { 1.35 }).tanh();
                v.roland_filter.process(driven)
            };
        }
        filtered *= 0.5 / (1.0 + resonance_percent * 0.0035);
        (
            filtered
                * env
                * accent_gain
                * if v.bass {
                    5.0
                } else if v.chord {
                    1.15 * std::f32::consts::FRAC_1_SQRT_2
                } else {
                    2.0
                },
            delay_send,
            reverb_send,
        )
    }
    fn render_drum_raw(
        voice: &mut DrumVoice,
        track: usize,
        sr: f32,
        _level_offset: f32,
        pan_offset: f32,
    ) -> (f32, f32, f32, f32) {
        let raw = match track {
            0 => {
                let hz = voice.kick_pitch.next_value();
                let body = (TAU * voice.phase).sin();
                voice.phase = (voice.phase + hz / sr).fract();
                let click_env = (-(voice.envelope.elapsed as f32) / (sr * 0.003)).exp();
                body + voice.noise() * click_env * voice.attack * 0.35
            }
            1 => {
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
            2 => {
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
            3 => {
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
            _ => {
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
        };
        let sample = (raw * 1.15).tanh() * voice.envelope.next_value() * 1.40;
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
        let mut dry_l = 0.0;
        let mut dry_r = 0.0;
        let mut delay_l = 0.0;
        let mut delay_r = 0.0;
        let mut reverb_l = 0.0;
        let mut reverb_r = 0.0;
        for i in 0..super::DRUM_TRACK_COUNT {
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
            if i == 0 {
                self.sidechain
                    .process_stereo(effect_l * gain, effect_r * gain);
            }
            dry_l += effect_l * pl * gain;
            dry_r += effect_r * pr * gain;
            delay_l += effect_l * pl * delay_send * gain;
            delay_r += effect_r * pr * delay_send * gain;
            reverb_l += effect_l * pl * reverb_send * gain;
            reverb_r += effect_r * pr * reverb_send * gain;
        }
        let duck_gain = self.sidechain.current_gain();
        for i in 0..super::DRUM_TRACK_COUNT {
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
            dry_l += effect_l * pl * gain;
            dry_r += effect_r * pr * gain;
            delay_l += effect_l * pl * delay_send * gain;
            delay_r += effect_r * pr * delay_send * gain;
            reverb_l += effect_l * pl * reverb_send * gain;
            reverb_r += effect_r * pr * reverb_send * gain;
        }
        for i in [0, 2] {
            let track = i + super::SYNTH_TRACK_START;
            let (x, ds, rs) =
                Self::render_synth(&mut self.synth[i], self.sr, &self.lfo_offsets[track]);
            let (effect_l, effect_r) = self.effects[track].process(x);
            let pan = modulated_percent(
                self.synth[i].pan.next_value(),
                self.lfo_offsets[track][ParameterId::Pan as usize],
            );
            let (pl, pr) = self.synth[i].pan_gains(pan);
            let level = modulated_percent(
                self.synth[i].level.next_value(),
                self.lfo_offsets[track][ParameterId::Level as usize],
            ) / 100.0;
            let gain = self.mute[track].next_value() * level.powi(2);
            dry_l += effect_l * duck_gain * pl * gain;
            dry_r += effect_r * duck_gain * pr * gain;
            delay_l += effect_l * duck_gain * pl * ds * gain;
            delay_r += effect_r * duck_gain * pr * ds * gain;
            reverb_l += effect_l * duck_gain * pl * rs * gain;
            reverb_r += effect_r * duck_gain * pr * rs * gain;
            if !self.preview_activity[track] {
                continue;
            }
            let (x, ds, rs) = Self::render_synth(
                &mut self.preview[i],
                self.sr,
                &self.preview_lfo_offsets[track],
            );
            let (effect_l, effect_r) = self.preview_effects[track].process(x);
            let pan = modulated_percent(
                self.preview[i].pan.next_value(),
                self.preview_lfo_offsets[track][ParameterId::Pan as usize],
            );
            let (pl, pr) = self.preview[i].pan_gains(pan);
            let level = modulated_percent(
                self.preview[i].level.next_value(),
                self.preview_lfo_offsets[track][ParameterId::Level as usize],
            ) / 100.0;
            let gain = level.powi(2);
            dry_l += effect_l * pl * gain;
            dry_r += effect_r * pr * gain;
            delay_l += effect_l * pl * ds * gain;
            delay_r += effect_r * pr * ds * gain;
            reverb_l += effect_l * pl * rs * gain;
            reverb_r += effect_r * pr * rs * gain;
        }
        let mut chord_left = 0.0;
        let mut chord_right = 0.0;
        let mut chord_ds = 0.0;
        let mut chord_rs = 0.0;
        let chord_start = self.chord.group * CHORD_GROUP_SIZE;
        let chord_end = chord_start + self.chord.voice_count;
        let mut chord_has_active_send = false;
        let mut chord_tail_send = None;
        let mut chord_active_level = None;
        let mut chord_tail_level = None;
        for (index, voice) in self.chord.voices.iter_mut().enumerate() {
            let (x, ds, rs) =
                Self::render_synth(voice, self.sr, &self.lfo_offsets[super::CHORD_TRACK_INDEX]);
            let pan = modulated_percent(
                voice.pan.next_value(),
                self.lfo_offsets[super::CHORD_TRACK_INDEX][ParameterId::Pan as usize],
            );
            let (pl, pr) = voice.pan_gains(pan);
            chord_left += x * pl;
            chord_right += x * pr;
            if voice.env.stage != EnvStage::Idle {
                let level = modulated_percent(
                    voice.level.next_value(),
                    self.lfo_offsets[super::CHORD_TRACK_INDEX][ParameterId::Level as usize],
                ) / 100.0;
                if self.chord.active && (chord_start..chord_end).contains(&index) {
                    chord_active_level.get_or_insert(level.powi(2));
                    chord_ds = ds;
                    chord_rs = rs;
                    chord_has_active_send = true;
                } else if chord_tail_send.is_none() {
                    chord_tail_level = Some(level.powi(2));
                    chord_tail_send = Some((ds, rs));
                }
            }
        }
        if !chord_has_active_send {
            (chord_ds, chord_rs) = chord_tail_send.unwrap_or((0.0, 0.0));
        }
        let (chord_l, chord_r) = self.chord.chorus.process_stereo(chord_left, chord_right);
        let (chord_effect_l, chord_effect_r) =
            self.effects[super::CHORD_TRACK_INDEX].process_stereo(chord_l, chord_r);
        let chord_duck_gain = duck_gain;
        let chord_gain = self.mute[super::CHORD_TRACK_INDEX].next_value()
            * selected_chord_level(chord_active_level, chord_tail_level);
        dry_l += chord_effect_l * chord_duck_gain * chord_gain;
        dry_r += chord_effect_r * chord_duck_gain * chord_gain;
        delay_l += chord_effect_l * chord_duck_gain * chord_ds * chord_gain;
        delay_r += chord_effect_r * chord_duck_gain * chord_ds * chord_gain;
        reverb_l += chord_effect_l * chord_duck_gain * chord_rs * chord_gain;
        reverb_r += chord_effect_r * chord_duck_gain * chord_rs * chord_gain;

        let mut preview_left = 0.0;
        let mut preview_right = 0.0;
        let mut preview_ds = 0.0;
        let mut preview_rs = 0.0;
        let preview_start = self.preview_chord.group * CHORD_GROUP_SIZE;
        let preview_end = preview_start + self.preview_chord.voice_count;
        let mut preview_has_active_send = false;
        let mut preview_tail_send = None;
        let mut preview_active_level = None;
        let mut preview_tail_level = None;
        if self.preview_activity[super::CHORD_TRACK_INDEX] {
            for (index, voice) in self.preview_chord.voices.iter_mut().enumerate() {
                let (x, ds, rs) = Self::render_synth(
                    voice,
                    self.sr,
                    &self.preview_lfo_offsets[super::CHORD_TRACK_INDEX],
                );
                let pan = modulated_percent(
                    voice.pan.next_value(),
                    self.preview_lfo_offsets[super::CHORD_TRACK_INDEX][ParameterId::Pan as usize],
                );
                let (pl, pr) = voice.pan_gains(pan);
                preview_left += x * pl;
                preview_right += x * pr;
                if voice.env.stage != EnvStage::Idle {
                    let level = modulated_percent(
                        voice.level.next_value(),
                        self.preview_lfo_offsets[super::CHORD_TRACK_INDEX]
                            [ParameterId::Level as usize],
                    ) / 100.0;
                    if self.preview_chord.active && (preview_start..preview_end).contains(&index) {
                        preview_active_level.get_or_insert(level.powi(2));
                        preview_ds = ds;
                        preview_rs = rs;
                        preview_has_active_send = true;
                    } else if preview_tail_send.is_none() {
                        preview_tail_level = Some(level.powi(2));
                        preview_tail_send = Some((ds, rs));
                    }
                }
            }
        }
        if !preview_has_active_send {
            (preview_ds, preview_rs) = preview_tail_send.unwrap_or((0.0, 0.0));
        }
        if self.preview_activity[super::CHORD_TRACK_INDEX] {
            let (preview_l, preview_r) = self
                .preview_chord
                .chorus
                .process_stereo(preview_left, preview_right);
            let (preview_effect_l, preview_effect_r) =
                self.preview_effects[super::CHORD_TRACK_INDEX].process_stereo(preview_l, preview_r);
            let preview_gain = selected_chord_level(preview_active_level, preview_tail_level);
            dry_l += preview_effect_l * preview_gain;
            dry_r += preview_effect_r * preview_gain;
            delay_l += preview_effect_l * preview_ds * preview_gain;
            delay_r += preview_effect_r * preview_ds * preview_gain;
            reverb_l += preview_effect_l * preview_rs * preview_gain;
            reverb_r += preview_effect_r * preview_rs * preview_gain;
        }

        let (dl, dr) = self.delay.process(delay_l, delay_r);
        let (rl, rr) = self.reverb.process(reverb_l, reverb_r);
        let reverb_return = self.reverb_return.next_value();
        let (l, r) = self.dc.process(
            dry_l + dl * 0.45 + rl * reverb_return,
            dry_r + dr * 0.45 + rr * reverb_return,
        );
        if !(l.is_finite() && r.is_finite()) {
            self.status.non_finite.store(true, Ordering::Release);
            return (0.0, 0.0);
        }
        self.limiter.process(l, r)
    }

    pub(super) fn advance_chord_arpeggios(&mut self) {
        let bpm = self.project.globals.tempo_bpm;
        if self.chord.arpeggiated && self.chord.active && self.chord.arpeggio.tick(self.sr, bpm) {
            Self::trigger_arpeggio_tone(&self.project, self.sr, &mut self.chord);
        }
        if self.preview_chord.arpeggiated && self.preview_chord.active {
            if self.preview_chord.preview_remaining > 0 {
                self.preview_chord.preview_remaining -= 1;
            }
            if self.preview_chord.preview_remaining == 0 {
                Self::release_chord(&mut self.preview_chord);
            } else if self.preview_chord.arpeggio.tick(self.sr, bpm) {
                Self::trigger_arpeggio_tone(&self.project, self.sr, &mut self.preview_chord);
            }
        }
    }
}

fn selected_chord_level(active_level: Option<f32>, tail_level: Option<f32>) -> f32 {
    active_level.or(tail_level).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::selected_chord_level;

    #[test]
    fn active_chord_level_takes_precedence_over_releasing_tail() {
        assert_eq!(selected_chord_level(Some(0.25), Some(0.01)), 0.25);
        assert_eq!(selected_chord_level(None, Some(0.01)), 0.01);
        assert_eq!(selected_chord_level(None, None), 0.0);
    }
}
