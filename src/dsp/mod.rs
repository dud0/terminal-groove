use std::f32::consts::PI;

use crate::model::{
    ChorusMode, LfoConfig, LfoWaveform, ParameterId, ParameterLocks, SidechainParameters,
    TrackEffects,
};

pub fn exp_map(percent: u8, min: f32, max: f32) -> f32 {
    if percent == 0 {
        min
    } else {
        min * (max / min).powf(percent as f32 / 100.0)
    }
}

pub fn exp_map_f32(percent: f32, min: f32, max: f32) -> f32 {
    min * (max / min).powf(percent.clamp(0.0, 100.0) / 100.0)
}

/// A preallocated kick-keyed envelope follower used to generate a shared ducking gain.
#[derive(Clone, Copy, Debug)]
pub struct SidechainCompressor {
    sample_rate: f32,
    depth_db: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    envelope: f32,
    gain: f32,
}

impl SidechainCompressor {
    pub fn new(sample_rate: u32) -> Self {
        let mut compressor = Self {
            sample_rate: sample_rate as f32,
            depth_db: 0.0,
            attack_coefficient: 1.0,
            release_coefficient: 1.0,
            envelope: 0.0,
            gain: 1.0,
        };
        compressor.configure(SidechainParameters::default());
        compressor
    }

    pub fn configure(&mut self, parameters: SidechainParameters) {
        self.depth_db = parameters.depth_db().clamp(0.0, 18.0);
        let attack = (parameters.attack_ms() * 0.001).clamp(0.0005, 0.030);
        let release = (parameters.release_ms() * 0.001).clamp(0.040, 1.0);
        self.attack_coefficient = one_pole_coefficient(self.sample_rate, attack);
        self.release_coefficient = one_pole_coefficient(self.sample_rate, release);
        self.update_gain();
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain = 1.0;
    }

    #[cfg(test)]
    fn envelope(&self) -> f32 {
        self.envelope
    }

    pub fn current_gain(&self) -> f32 {
        self.gain
    }

    pub fn process_stereo(&mut self, input_l: f32, input_r: f32) -> f32 {
        let peak = input_l.abs().max(input_r.abs());
        if !peak.is_finite() {
            self.envelope = 0.0;
            self.gain = 1.0;
            return self.current_gain();
        }
        let peak = peak.clamp(0.0, 1.0);
        let coefficient = if peak > self.envelope {
            self.attack_coefficient
        } else {
            self.release_coefficient
        };
        self.envelope = (self.envelope + (peak - self.envelope) * coefficient).clamp(0.0, 1.0);
        self.update_gain();
        self.gain
    }

    fn update_gain(&mut self) {
        let gain = 10.0_f32.powf(-(self.depth_db * self.envelope) / 20.0);
        self.gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }
}

fn one_pole_coefficient(sample_rate: f32, seconds: f32) -> f32 {
    1.0 - (-1.0 / (sample_rate.max(1.0) * seconds)).exp()
}

#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    phase: f32,
    held: f32,
    smoothed: f32,
    rng: u32,
    seed: u32,
    active: bool,
    cached_config: Option<LfoConfig>,
    cached_tempo_bpm: u16,
    cached_sample_rate: f32,
    increment: f32,
    smoothing_coefficient: f32,
}

impl Lfo {
    pub fn new(seed: u32) -> Self {
        let seed = seed.max(1);
        Self {
            phase: 0.0,
            held: 0.0,
            smoothed: 0.0,
            rng: seed,
            seed,
            active: false,
            cached_config: None,
            cached_tempo_bpm: 0,
            cached_sample_rate: 0.0,
            increment: 0.0,
            smoothing_coefficient: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.smoothed = 0.0;
        self.rng = self.seed;
        self.held = 0.0;
        self.active = false;
    }

    pub fn disable(&mut self) {
        self.active = false;
        self.smoothed = 0.0;
    }

    #[cfg(test)]
    fn value(&self) -> f32 {
        self.smoothed
    }

    pub fn restart(&mut self, config: LfoConfig, tempo_bpm: u16, sample_rate: f32) -> f32 {
        self.activate(config);
        self.refresh_cache(config, tempo_bpm, sample_rate);
        let raw = self.raw_value(config.waveform);
        self.smoothed = raw;
        self.advance_phase(config.waveform);
        raw
    }

    pub fn next(&mut self, config: Option<LfoConfig>, tempo_bpm: u16, sample_rate: f32) -> f32 {
        let Some(config) = config.filter(|config| config.enabled) else {
            self.disable();
            return 0.0;
        };
        if !self.active {
            return self.restart(config, tempo_bpm, sample_rate);
        }
        self.refresh_cache(config, tempo_bpm, sample_rate);
        let raw = self.raw_value(config.waveform);
        self.smoothed += (raw - self.smoothed) * self.smoothing_coefficient;
        self.advance_phase(config.waveform);
        self.smoothed
    }

    fn activate(&mut self, config: LfoConfig) {
        self.phase = config.start_phase.normalized().fract();
        self.rng = self.seed;
        self.held = self.random_bipolar();
        self.active = true;
    }

    fn refresh_cache(&mut self, config: LfoConfig, tempo_bpm: u16, sample_rate: f32) {
        if self.cached_config != Some(config)
            || self.cached_tempo_bpm != tempo_bpm
            || self.cached_sample_rate != sample_rate
        {
            self.cached_config = Some(config);
            self.cached_tempo_bpm = tempo_bpm;
            self.cached_sample_rate = sample_rate;
            self.increment = config.rate.hz(tempo_bpm) / sample_rate;
            self.smoothing_coefficient = 1.0 - (-1.0 / (sample_rate * 0.005).max(1.0)).exp();
        }
    }

    fn raw_value(&self, waveform: LfoWaveform) -> f32 {
        match waveform {
            LfoWaveform::Sine => (std::f32::consts::TAU * self.phase).sin(),
            LfoWaveform::Triangle => (2.0 / PI) * (std::f32::consts::TAU * self.phase).sin().asin(),
            LfoWaveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWaveform::Saw => self.phase * 2.0 - 1.0,
            LfoWaveform::SampleAndHold => self.held,
        }
    }

    fn advance_phase(&mut self, waveform: LfoWaveform) {
        self.phase += self.increment.max(0.0);
        if self.phase >= 1.0 {
            self.phase = self.phase.fract();
            if waveform == LfoWaveform::SampleAndHold {
                self.held = self.random_bipolar();
            }
        }
    }

    fn random_bipolar(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x as i32 as f32 / i32::MAX as f32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}
impl Smoother {
    pub fn new(v: f32) -> Self {
        Self {
            current: v,
            target: v,
            step: 0.0,
            remaining: 0,
        }
    }
    pub fn set(&mut self, v: f32, samples: u32) {
        self.target = v;
        if samples == 0 {
            self.current = v;
            self.remaining = 0
        } else {
            self.remaining = samples;
            self.step = (v - self.current) / samples as f32
        }
    }
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.current += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target
            }
        }
        self.current
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    pub fn is_smoothing(&self) -> bool {
        self.remaining != 0
    }
}

#[derive(Clone, Copy, Debug)]
struct EffectAllpass {
    state: f32,
}

impl EffectAllpass {
    const fn new() -> Self {
        Self { state: 0.0 }
    }

    fn process(&mut self, input: f32, coefficient: f32) -> f32 {
        let output = -coefficient * input + self.state;
        self.state = input + coefficient * output;
        if output.is_finite() && self.state.is_finite() {
            output
        } else {
            self.state = 0.0;
            0.0
        }
    }

    fn clear(&mut self) {
        self.state = 0.0;
    }
}

pub const FLANGER_MIN_DELAY_MS: f32 = 0.1;
const FLANGER_MAX_DELAY_SECONDS: f32 = 0.015;
const PHASER_CONTROL_FRAMES: u8 = 8;

pub fn bit_crusher_bit_depth(bits_percent: f32) -> u8 {
    (16.0 - bits_percent.clamp(0.0, 100.0) * 0.14).round() as u8
}

pub fn bit_crusher_rate_divisor(rate_percent: f32) -> f32 {
    2.0_f32.powf(rate_percent.clamp(0.0, 100.0) * 0.06)
}

/// Returns the center, effective depth, and physical delay endpoints for the
/// flanger controls.  Depth is constrained before modulation rather than
/// clamping the waveform, so the lowest part of the cycle remains smooth.
pub fn flanger_delay_geometry(delay_percent: f32, depth_percent: f32) -> (f32, f32, f32, f32) {
    let center_ms = 0.2 + delay_percent.clamp(0.0, 100.0) * 0.098;
    let requested_depth_ms = depth_percent.clamp(0.0, 100.0) * 0.05;
    let depth_ms = requested_depth_ms.min(center_ms - FLANGER_MIN_DELAY_MS);
    (
        center_ms,
        depth_ms,
        center_ms - depth_ms,
        center_ms + depth_ms,
    )
}

#[derive(Clone, Copy, Debug)]
struct FlangerControls {
    rate: f32,
    feedback: f32,
    mix: f32,
}

#[derive(Debug)]
pub struct TrackEffectChain {
    processing: bool,
    chorus: StereoChorus,
    distortion_active: bool,
    bit_crusher_active: bool,
    phaser_active: bool,
    flanger_active: bool,
    distortion_tail_remaining: u32,
    phaser_tail_remaining: u32,
    flanger_tail_remaining: u32,
    distortion_quiet_samples: u32,
    phaser_quiet_samples: u32,
    flanger_quiet_samples: u32,
    distortion_drive: Smoother,
    distortion_tone: Smoother,
    distortion_mix: Smoother,
    distortion_state_l: f32,
    distortion_state_r: f32,
    distortion_gain: f32,
    distortion_coefficient: f32,
    cached_distortion_drive: f32,
    cached_distortion_tone: f32,
    bit_crusher_bits: Smoother,
    bit_crusher_rate: Smoother,
    bit_crusher_mix: Smoother,
    bit_crusher_phase: f32,
    bit_crusher_held_l: f32,
    bit_crusher_held_r: f32,
    bit_crusher_has_sample: bool,
    bit_crusher_rate_increment: f32,
    cached_bit_crusher_rate: f32,
    phaser_rate: Smoother,
    phaser_depth: Smoother,
    phaser_feedback: Smoother,
    phaser_mix: Smoother,
    phaser_phase: f32,
    phaser_feedback_l: f32,
    phaser_feedback_r: f32,
    phaser_l: [EffectAllpass; 4],
    phaser_r: [EffectAllpass; 4],
    phaser_sweep: f32,
    phaser_rate_hz: f32,
    phaser_coefficient_l: f32,
    phaser_coefficient_r: f32,
    phaser_coefficient_step_l: f32,
    phaser_coefficient_step_r: f32,
    phaser_control_remaining: u8,
    cached_phaser_depth: f32,
    cached_phaser_rate: f32,
    flanger_rate: Smoother,
    flanger_delay: Smoother,
    flanger_depth: Smoother,
    flanger_feedback: Smoother,
    flanger_mix: Smoother,
    flanger_phase: f32,
    flanger_write: usize,
    flanger_l: Box<[f32]>,
    flanger_r: Box<[f32]>,
    sample_rate: f32,
    flanger_rate_hz: f32,
    flanger_center_samples: f32,
    flanger_depth_samples: f32,
    cached_flanger_rate: f32,
    cached_flanger_delay: f32,
    cached_flanger_depth: f32,
}

impl TrackEffectChain {
    pub fn new(sample_rate: u32) -> Self {
        let flanger_buffer_size = ((sample_rate as f32 * FLANGER_MAX_DELAY_SECONDS).ceil()
            as usize)
            .saturating_add(2)
            .max(3);
        let flanger_l = vec![0.0; flanger_buffer_size].into_boxed_slice();
        let flanger_r = vec![0.0; flanger_buffer_size].into_boxed_slice();
        let sample_rate_f32 = sample_rate as f32;
        let initial_phaser_coefficient = Self::phaser_coefficient(0.0, 0.0, sample_rate_f32);
        Self {
            processing: false,
            chorus: StereoChorus::new(sample_rate),
            distortion_active: false,
            bit_crusher_active: false,
            phaser_active: false,
            flanger_active: false,
            distortion_tail_remaining: 0,
            phaser_tail_remaining: 0,
            flanger_tail_remaining: 0,
            distortion_quiet_samples: 0,
            phaser_quiet_samples: 0,
            flanger_quiet_samples: 0,
            distortion_drive: Smoother::new(0.0),
            distortion_tone: Smoother::new(50.0),
            distortion_mix: Smoother::new(0.0),
            distortion_state_l: 0.0,
            distortion_state_r: 0.0,
            distortion_gain: 1.0,
            distortion_coefficient: 1.0,
            cached_distortion_drive: f32::NAN,
            cached_distortion_tone: f32::NAN,
            bit_crusher_bits: Smoother::new(50.0),
            bit_crusher_rate: Smoother::new(50.0),
            bit_crusher_mix: Smoother::new(0.0),
            bit_crusher_phase: 0.0,
            bit_crusher_held_l: 0.0,
            bit_crusher_held_r: 0.0,
            bit_crusher_has_sample: false,
            bit_crusher_rate_increment: 0.125,
            cached_bit_crusher_rate: f32::NAN,
            phaser_rate: Smoother::new(25.0),
            phaser_depth: Smoother::new(50.0),
            phaser_feedback: Smoother::new(20.0),
            phaser_mix: Smoother::new(0.0),
            phaser_phase: 0.0,
            phaser_feedback_l: 0.0,
            phaser_feedback_r: 0.0,
            phaser_l: [EffectAllpass::new(); 4],
            phaser_r: [EffectAllpass::new(); 4],
            phaser_sweep: 0.0,
            phaser_rate_hz: 0.05,
            phaser_coefficient_l: initial_phaser_coefficient,
            phaser_coefficient_r: initial_phaser_coefficient,
            phaser_coefficient_step_l: 0.0,
            phaser_coefficient_step_r: 0.0,
            phaser_control_remaining: 0,
            cached_phaser_depth: f32::NAN,
            cached_phaser_rate: f32::NAN,
            flanger_rate: Smoother::new(25.0),
            flanger_delay: Smoother::new(18.0),
            flanger_depth: Smoother::new(50.0),
            flanger_feedback: Smoother::new(20.0),
            flanger_mix: Smoother::new(0.0),
            flanger_phase: 0.0,
            flanger_write: 0,
            flanger_l,
            flanger_r,
            sample_rate: sample_rate_f32,
            flanger_rate_hz: 0.05,
            flanger_center_samples: sample_rate as f32 * 0.0002,
            flanger_depth_samples: 0.0,
            cached_flanger_rate: f32::NAN,
            cached_flanger_delay: f32::NAN,
            cached_flanger_depth: f32::NAN,
        }
    }

    pub fn configure(&mut self, effects: TrackEffects, locks: ParameterLocks, samples: u32) {
        let chorus_mode = locks.chorus().unwrap_or(effects.chorus);
        self.chorus.configure(match chorus_mode {
            ChorusMode::Off => 0,
            ChorusMode::I => 1,
            ChorusMode::Ii => 2,
        });
        let was_processing = self.processing
            || self.chorus.is_active()
            || self.distortion_mix.current > 0.0
            || self.bit_crusher_mix.current > 0.0
            || self.phaser_mix.current > 0.0
            || self.flanger_mix.current > 0.0;
        self.distortion_drive.set(
            locks
                .percent(ParameterId::DistortionDrive)
                .unwrap_or(effects.distortion.drive)
                .get() as f32,
            samples,
        );
        self.distortion_tone.set(
            locks
                .percent(ParameterId::DistortionTone)
                .unwrap_or(effects.distortion.tone)
                .get() as f32,
            samples,
        );
        self.distortion_mix.set(
            locks
                .percent(ParameterId::DistortionMix)
                .unwrap_or(effects.distortion.mix)
                .get() as f32,
            samples,
        );
        self.bit_crusher_bits.set(
            locks
                .percent(ParameterId::BitCrusherBits)
                .unwrap_or(effects.bit_crusher.bits)
                .get() as f32,
            samples,
        );
        self.bit_crusher_rate.set(
            locks
                .percent(ParameterId::BitCrusherRate)
                .unwrap_or(effects.bit_crusher.rate)
                .get() as f32,
            samples,
        );
        self.bit_crusher_mix.set(
            locks
                .percent(ParameterId::BitCrusherMix)
                .unwrap_or(effects.bit_crusher.mix)
                .get() as f32,
            samples,
        );
        self.phaser_rate.set(
            locks
                .percent(ParameterId::PhaserRate)
                .unwrap_or(effects.phaser.rate)
                .get() as f32,
            samples,
        );
        self.phaser_depth.set(
            locks
                .percent(ParameterId::PhaserDepth)
                .unwrap_or(effects.phaser.depth)
                .get() as f32,
            samples,
        );
        self.phaser_feedback.set(
            locks
                .percent(ParameterId::PhaserFeedback)
                .unwrap_or(effects.phaser.feedback)
                .get() as f32,
            samples,
        );
        self.phaser_mix.set(
            locks
                .percent(ParameterId::PhaserMix)
                .unwrap_or(effects.phaser.mix)
                .get() as f32,
            samples,
        );
        self.flanger_rate.set(
            locks
                .percent(ParameterId::FlangerRate)
                .unwrap_or(effects.flanger.rate)
                .get() as f32,
            samples,
        );
        self.flanger_delay.set(
            locks
                .percent(ParameterId::FlangerDelay)
                .unwrap_or(effects.flanger.delay)
                .get() as f32,
            samples,
        );
        self.flanger_depth.set(
            locks
                .percent(ParameterId::FlangerDepth)
                .unwrap_or(effects.flanger.depth)
                .get() as f32,
            samples,
        );
        self.flanger_feedback.set(
            locks
                .percent(ParameterId::FlangerFeedback)
                .unwrap_or(effects.flanger.feedback)
                .get() as f32,
            samples,
        );
        self.flanger_mix.set(
            locks
                .percent(ParameterId::FlangerMix)
                .unwrap_or(effects.flanger.mix)
                .get() as f32,
            samples,
        );
        self.processing = was_processing
            || chorus_mode != ChorusMode::Off
            || self.distortion_mix.target > 0.0
            || self.bit_crusher_mix.target > 0.0
            || self.phaser_mix.target > 0.0
            || self.flanger_mix.target > 0.0;
    }

    pub fn process(&mut self, input: f32) -> (f32, f32) {
        self.process_stereo(input, input)
    }

    pub fn process_stereo(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        if !self.processing {
            return (input_l, input_r);
        }
        let distortion_mix = self.distortion_mix.next_value() / 100.0;
        let bit_crusher_mix = self.bit_crusher_mix.next_value() / 100.0;
        let phaser_mix = self.phaser_mix.next_value() / 100.0;
        let flanger_mix = self.flanger_mix.next_value() / 100.0;
        let input_active = stereo_peak(input_l, input_r) > SILENCE_THRESHOLD;
        let run_distortion =
            (distortion_mix > 0.0 && input_active) || self.distortion_tail_remaining > 0;
        let distorted_l = if run_distortion {
            let drive = self.distortion_drive.next_value();
            let tone = self.distortion_tone.next_value();
            self.update_distortion_cache(drive, tone);
            let processed = Self::distort_sample(
                if distortion_mix > 0.0 { input_l } else { 0.0 },
                self.distortion_gain,
                self.distortion_coefficient,
                distortion_mix,
                &mut self.distortion_state_l,
            );
            if distortion_mix > 0.0 {
                processed
            } else {
                input_l
            }
        } else {
            input_l
        };
        let distorted_r = if run_distortion {
            let processed = Self::distort_sample(
                if distortion_mix > 0.0 { input_r } else { 0.0 },
                self.distortion_gain,
                self.distortion_coefficient,
                distortion_mix,
                &mut self.distortion_state_r,
            );
            if distortion_mix > 0.0 {
                processed
            } else {
                input_r
            }
        } else {
            input_r
        };
        if run_distortion {
            let state_peak = stereo_peak(self.distortion_state_l, self.distortion_state_r);
            self.distortion_active = Self::update_tail_activity(
                distortion_mix > 0.0 && input_active,
                state_peak,
                64,
                self.tail_limit(),
                &mut self.distortion_tail_remaining,
                &mut self.distortion_quiet_samples,
            );
            if !self.distortion_active {
                self.clear_distortion();
            }
        } else {
            self.distortion_active = false;
        }

        let bit_crusher_input_active = stereo_peak(distorted_l, distorted_r) > SILENCE_THRESHOLD;
        let run_bit_crusher =
            bit_crusher_mix > 0.0 && (bit_crusher_input_active || self.bit_crusher_active);
        let (crushed_l, crushed_r) = if run_bit_crusher {
            let bits = bit_crusher_bit_depth(self.bit_crusher_bits.next_value());
            let rate = self.bit_crusher_rate.next_value();
            self.update_bit_crusher_cache(rate);
            if !self.bit_crusher_has_sample {
                self.bit_crusher_held_l = Self::bit_crush_sample(distorted_l, bits);
                self.bit_crusher_held_r = Self::bit_crush_sample(distorted_r, bits);
                self.bit_crusher_has_sample = true;
            } else {
                self.bit_crusher_phase += self.bit_crusher_rate_increment;
                if self.bit_crusher_phase >= 1.0 {
                    self.bit_crusher_phase = self.bit_crusher_phase.fract();
                    self.bit_crusher_held_l = Self::bit_crush_sample(distorted_l, bits);
                    self.bit_crusher_held_r = Self::bit_crush_sample(distorted_r, bits);
                }
            }
            let output = (
                finite_or_zero(
                    distorted_l * (1.0 - bit_crusher_mix)
                        + self.bit_crusher_held_l * bit_crusher_mix,
                ),
                finite_or_zero(
                    distorted_r * (1.0 - bit_crusher_mix)
                        + self.bit_crusher_held_r * bit_crusher_mix,
                ),
            );
            self.bit_crusher_active = bit_crusher_input_active
                || stereo_peak(self.bit_crusher_held_l, self.bit_crusher_held_r)
                    > SILENCE_THRESHOLD;
            if !self.bit_crusher_active {
                self.clear_bit_crusher();
            }
            output
        } else {
            self.bit_crusher_active = false;
            if bit_crusher_mix == 0.0 {
                self.clear_bit_crusher();
            }
            (distorted_l, distorted_r)
        };

        let (chorused_l, chorused_r) = self.chorus.process_stereo(crushed_l, crushed_r);

        let phaser_input_active = stereo_peak(chorused_l, chorused_r) > SILENCE_THRESHOLD;
        let run_phaser =
            (phaser_mix > 0.0 && phaser_input_active) || self.phaser_tail_remaining > 0;
        let (processed_l, processed_r) = if run_phaser {
            let phaser_rate = self.phaser_rate.next_value();
            let phaser_depth = self.phaser_depth.next_value();
            let phaser_feedback = (self.phaser_feedback.next_value() / 100.0).clamp(0.0, 0.9);
            self.update_phaser_cache(phaser_rate, phaser_depth);
            self.update_phaser_coefficients();
            let feedback =
                (self.phaser_feedback_l + self.phaser_feedback_r) * 0.5 * phaser_feedback;
            // Once the user-facing mix ramp reaches zero, keep draining the
            // feedback state silently. Re-expanding the tail to a derived wet
            // mix here would create a near-100% wet discontinuity.
            let effective_mix = phaser_mix;
            let left_input = if phaser_mix > 0.0 { chorused_l } else { 0.0 } + feedback;
            let right_input = if phaser_mix > 0.0 { chorused_r } else { 0.0 } - feedback;
            let left =
                Self::phaser_sample(&mut self.phaser_l, left_input, self.phaser_coefficient_l);
            let right =
                Self::phaser_sample(&mut self.phaser_r, right_input, self.phaser_coefficient_r);
            self.phaser_feedback_l = left;
            self.phaser_feedback_r = right;
            self.phaser_phase =
                (self.phaser_phase + self.phaser_rate_hz / self.sample_rate).fract();
            self.advance_phaser_coefficients();
            let output = (
                chorused_l * (1.0 - effective_mix) + left * effective_mix,
                chorused_r * (1.0 - effective_mix) + right * effective_mix,
            );
            self.phaser_active = Self::update_tail_activity(
                phaser_mix > 0.0 && phaser_input_active,
                stereo_peak(left, right),
                64,
                self.tail_limit(),
                &mut self.phaser_tail_remaining,
                &mut self.phaser_quiet_samples,
            );
            if !self.phaser_active {
                self.clear_phaser();
            }
            output
        } else {
            self.phaser_active = false;
            (chorused_l, chorused_r)
        };

        let flanger_input_active = stereo_peak(processed_l, processed_r) > SILENCE_THRESHOLD;
        let run_flanger =
            (flanger_mix > 0.0 && flanger_input_active) || self.flanger_tail_remaining > 0;
        if !run_flanger {
            self.flanger_active = false;
            self.processing = self.has_pending_parameters();
            return (processed_l, processed_r);
        }
        let flanger_rate = self.flanger_rate.next_value();
        let flanger_delay = self.flanger_delay.next_value();
        let flanger_depth = self.flanger_depth.next_value();
        let flanger_feedback = (self.flanger_feedback.next_value() / 100.0).clamp(0.0, 0.9);
        self.update_flanger_cache(flanger_rate, flanger_delay, flanger_depth);
        let effective_mix = flanger_mix;
        let flanger_dsp_l = if flanger_mix > 0.0 { processed_l } else { 0.0 };
        let flanger_dsp_r = if flanger_mix > 0.0 { processed_r } else { 0.0 };
        let (mut output_l, mut output_r, wet_peak) = self.flanger_sample(
            flanger_dsp_l,
            flanger_dsp_r,
            FlangerControls {
                rate: self.flanger_rate_hz,
                feedback: flanger_feedback,
                mix: effective_mix,
            },
        );
        if flanger_mix == 0.0 {
            output_l = processed_l;
            output_r = processed_r;
        }
        self.flanger_active = Self::update_tail_activity(
            flanger_mix > 0.0 && flanger_input_active,
            wet_peak,
            self.flanger_quiet_length(),
            self.tail_limit(),
            &mut self.flanger_tail_remaining,
            &mut self.flanger_quiet_samples,
        );
        if !self.flanger_active {
            self.clear_flanger();
        }
        self.processing = self.has_pending_parameters();
        (output_l, output_r)
    }

    fn distort_sample(input: f32, gain: f32, coefficient: f32, mix: f32, state: &mut f32) -> f32 {
        let clipped = (input * gain).tanh();
        *state += (clipped - *state) * coefficient;
        input * (1.0 - mix) + *state * mix
    }

    fn bit_crush_sample(input: f32, bits: u8) -> f32 {
        let scale = (1_u32 << (bits.clamp(2, 16) - 1)) as f32;
        (finite_or_zero(input).clamp(-1.0, 1.0) * scale).round() / scale
    }

    fn phaser_coefficient(phase: f32, sweep: f32, sample_rate: f32) -> f32 {
        let frequency = 300.0 * (sweep_value(phase) * sweep).exp();
        let frequency = frequency.clamp(300.0, (sample_rate * 0.45).max(301.0));
        let tangent = (std::f32::consts::PI * frequency / sample_rate).tan();
        ((1.0 - tangent) / (1.0 + tangent)).clamp(-0.98, 0.98)
    }

    fn update_phaser_coefficients(&mut self) {
        if self.phaser_control_remaining == 0 {
            self.phaser_coefficient_l =
                Self::phaser_coefficient(self.phaser_phase, self.phaser_sweep, self.sample_rate);
            self.phaser_coefficient_r = Self::phaser_coefficient(
                (self.phaser_phase + 0.5).fract(),
                self.phaser_sweep,
                self.sample_rate,
            );
            let future_phase = (self.phaser_phase
                + self.phaser_rate_hz / self.sample_rate * f32::from(PHASER_CONTROL_FRAMES))
            .fract();
            let target_l =
                Self::phaser_coefficient(future_phase, self.phaser_sweep, self.sample_rate);
            let target_r = Self::phaser_coefficient(
                (future_phase + 0.5).fract(),
                self.phaser_sweep,
                self.sample_rate,
            );
            self.phaser_coefficient_step_l =
                (target_l - self.phaser_coefficient_l) / f32::from(PHASER_CONTROL_FRAMES);
            self.phaser_coefficient_step_r =
                (target_r - self.phaser_coefficient_r) / f32::from(PHASER_CONTROL_FRAMES);
            self.phaser_control_remaining = PHASER_CONTROL_FRAMES;
        }
    }

    fn advance_phaser_coefficients(&mut self) {
        self.phaser_coefficient_l += self.phaser_coefficient_step_l;
        self.phaser_coefficient_r += self.phaser_coefficient_step_r;
        self.phaser_control_remaining -= 1;
    }

    fn phaser_sample(stages: &mut [EffectAllpass; 4], input: f32, coefficient: f32) -> f32 {
        let mut value = input;
        for stage in stages {
            value = stage.process(value, coefficient);
        }
        value
    }

    fn flanger_sample(
        &mut self,
        input_l: f32,
        input_r: f32,
        controls: FlangerControls,
    ) -> (f32, f32, f32) {
        let left_delay = self.flanger_delay_samples_cached(self.flanger_phase);
        let right_delay = self.flanger_delay_samples_cached((self.flanger_phase + 0.5).fract());
        let delayed_l = Self::read_delay(&self.flanger_l, self.flanger_write, left_delay);
        let delayed_r = Self::read_delay(&self.flanger_r, self.flanger_write, right_delay);
        self.flanger_l[self.flanger_write] =
            finite_or_zero(input_l + delayed_l * controls.feedback);
        self.flanger_r[self.flanger_write] =
            finite_or_zero(input_r + delayed_r * controls.feedback);
        self.flanger_write = (self.flanger_write + 1) % self.flanger_l.len();
        self.flanger_phase = (self.flanger_phase + controls.rate / self.sample_rate).fract();
        (
            finite_or_zero(input_l * (1.0 - controls.mix) + delayed_l * controls.mix),
            finite_or_zero(input_r * (1.0 - controls.mix) + delayed_r * controls.mix),
            stereo_peak(delayed_l, delayed_r),
        )
    }

    #[cfg(test)]
    fn flanger_delay_samples(&self, center_ms: f32, depth_ms: f32, phase: f32) -> f32 {
        let delay_ms = center_ms + (std::f32::consts::TAU * phase).sin() * depth_ms;
        (delay_ms.max(FLANGER_MIN_DELAY_MS) * self.sample_rate / 1_000.0)
            .clamp(1.0, (self.flanger_l.len() - 2) as f32)
    }

    fn flanger_delay_samples_cached(&self, phase: f32) -> f32 {
        (self.flanger_center_samples
            + self.flanger_depth_samples * (std::f32::consts::TAU * phase).sin())
        .max(1.0)
        .clamp(1.0, (self.flanger_l.len() - 2) as f32)
    }

    fn tail_limit(&self) -> u32 {
        (self.sample_rate * 2.0).round().max(1.0) as u32
    }

    #[cfg(test)]
    fn tail_length(&self) -> u32 {
        self.tail_limit()
    }

    fn flanger_quiet_length(&self) -> u32 {
        (self.sample_rate * 0.015).ceil() as u32 + 2
    }

    fn update_tail_activity(
        feeding: bool,
        state_peak: f32,
        quiet_length: u32,
        tail_limit: u32,
        tail_remaining: &mut u32,
        quiet_samples: &mut u32,
    ) -> bool {
        if feeding {
            *tail_remaining = tail_limit;
            *quiet_samples = 0;
            return true;
        }
        *tail_remaining = tail_remaining.saturating_sub(1);
        if state_peak <= SILENCE_THRESHOLD {
            *quiet_samples = quiet_samples.saturating_add(1);
        } else {
            *quiet_samples = 0;
        }
        *tail_remaining > 0 && *quiet_samples < quiet_length
    }

    fn has_pending_parameters(&self) -> bool {
        self.chorus.mode != 0
            || self.chorus.is_active()
            || self.distortion_mix.is_smoothing()
            || self.bit_crusher_mix.is_smoothing()
            || self.phaser_mix.is_smoothing()
            || self.flanger_mix.is_smoothing()
            || self.distortion_mix.target > 0.0
            || self.bit_crusher_mix.target > 0.0
            || self.phaser_mix.target > 0.0
            || self.flanger_mix.target > 0.0
            || self.distortion_active
            || self.bit_crusher_active
            || self.phaser_active
            || self.flanger_active
    }

    fn update_distortion_cache(&mut self, drive: f32, tone: f32) {
        if drive == self.cached_distortion_drive && tone == self.cached_distortion_tone {
            return;
        }
        self.cached_distortion_drive = drive;
        self.cached_distortion_tone = tone;
        self.distortion_gain = exp_map_f32(drive, 1.0, 31.622_776);
        let cutoff = exp_map_f32(tone, 700.0, (18_000.0_f32).min(self.sample_rate * 0.45));
        self.distortion_coefficient =
            1.0 - (-std::f32::consts::TAU * cutoff / self.sample_rate).exp();
    }

    fn update_bit_crusher_cache(&mut self, rate: f32) {
        if rate == self.cached_bit_crusher_rate {
            return;
        }
        self.cached_bit_crusher_rate = rate;
        self.bit_crusher_rate_increment = bit_crusher_rate_divisor(rate).recip();
    }

    fn update_phaser_cache(&mut self, rate: f32, depth: f32) {
        if rate != self.cached_phaser_rate || depth != self.cached_phaser_depth {
            self.cached_phaser_rate = rate;
            self.cached_phaser_depth = depth;
            self.phaser_sweep = (8_000.0_f32 / 300.0).ln() * depth / 100.0;
            self.phaser_rate_hz = exp_map_f32(rate, 0.05, 8.0);
        }
    }

    fn update_flanger_cache(&mut self, rate: f32, delay: f32, depth: f32) {
        if rate != self.cached_flanger_rate
            || delay != self.cached_flanger_delay
            || depth != self.cached_flanger_depth
        {
            self.cached_flanger_rate = rate;
            self.cached_flanger_delay = delay;
            self.cached_flanger_depth = depth;
            self.flanger_rate_hz = exp_map_f32(rate, 0.05, 8.0);
            let (center_ms, depth_ms, _, _) = flanger_delay_geometry(delay, depth);
            self.flanger_center_samples = center_ms * self.sample_rate / 1_000.0;
            self.flanger_depth_samples = depth_ms * self.sample_rate / 1_000.0;
        }
    }

    pub fn is_active(&self) -> bool {
        self.chorus.is_active()
            || self.distortion_active
            || self.bit_crusher_active
            || self.phaser_active
            || self.flanger_active
    }

    fn read_delay(buffer: &[f32], write: usize, delay: f32) -> f32 {
        let mut position = write as f32 - delay;
        if position < 0.0 {
            position += buffer.len() as f32;
        }
        let first = position.floor() as usize % buffer.len();
        let second = (first + 1) % buffer.len();
        let fraction = position.fract();
        finite_or_zero(buffer[first] * (1.0 - fraction) + buffer[second] * fraction)
    }

    fn clear_distortion(&mut self) {
        self.distortion_state_l = 0.0;
        self.distortion_state_r = 0.0;
        self.distortion_active = false;
        self.distortion_tail_remaining = 0;
        self.distortion_quiet_samples = 0;
    }

    fn clear_bit_crusher(&mut self) {
        self.bit_crusher_active = false;
        self.bit_crusher_phase = 0.0;
        self.bit_crusher_held_l = 0.0;
        self.bit_crusher_held_r = 0.0;
        self.bit_crusher_has_sample = false;
    }

    fn clear_phaser(&mut self) {
        self.phaser_active = false;
        self.phaser_tail_remaining = 0;
        self.phaser_quiet_samples = 0;
        self.phaser_feedback_l = 0.0;
        self.phaser_feedback_r = 0.0;
        for stage in self.phaser_l.iter_mut().chain(self.phaser_r.iter_mut()) {
            stage.clear();
        }
    }

    fn clear_flanger(&mut self) {
        self.flanger_active = false;
        self.flanger_tail_remaining = 0;
        self.flanger_quiet_samples = 0;
        self.flanger_write = 0;
        self.flanger_l.fill(0.0);
        self.flanger_r.fill(0.0);
    }

    pub fn clear(&mut self) {
        self.chorus.clear();
        self.clear_distortion();
        self.clear_bit_crusher();
        self.clear_phaser();
        self.clear_flanger();
        self.phaser_phase = 0.0;
        self.phaser_coefficient_l =
            Self::phaser_coefficient(0.0, self.phaser_sweep, self.sample_rate);
        self.phaser_coefficient_r =
            Self::phaser_coefficient(0.5, self.phaser_sweep, self.sample_rate);
        self.phaser_coefficient_step_l = 0.0;
        self.phaser_coefficient_step_r = 0.0;
        self.phaser_control_remaining = 0;
        self.flanger_phase = 0.0;
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn stereo_peak(left: f32, right: f32) -> f32 {
    finite_or_zero(left).abs().max(finite_or_zero(right).abs())
}

fn sweep_value(phase: f32) -> f32 {
    (std::f32::consts::TAU * phase).sin() * 0.5 + 0.5
}

impl Default for TrackEffectChain {
    fn default() -> Self {
        Self::new(44_100)
    }
}

#[derive(Default)]
pub struct PolyBlepOsc {
    phase: f32,
}

/// The sub oscillator choices used by the hardware-inspired voices.  The
/// persisted control remains a level macro for compatibility; the voice
/// chooses its instrument-appropriate divider internally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubOscillatorMode {
    OneOctave,
    TwoOctaves,
    TwoOctavesNarrowPulse,
}

impl SubOscillatorMode {
    pub fn ratio(self) -> f32 {
        match self {
            Self::OneOctave => 0.5,
            Self::TwoOctaves => 0.25,
            Self::TwoOctavesNarrowPulse => 0.25,
        }
    }
}

/// Map the legacy oscillator macro to additive source gains.  Both sources
/// are still generated at the same phase, while the end points remain the
/// original pulse-only and saw-only settings.
pub fn additive_source_gains(mix: f32) -> (f32, f32) {
    let mix = mix.clamp(0.0, 1.0);
    ((1.0 - mix).sqrt(), mix.sqrt())
}

/// A small deterministic xorshift source.  Each synth voice owns one so
/// preview rendering and overlapping chord tails cannot share noise state.
#[derive(Clone, Copy, Debug)]
pub struct NoiseSource {
    state: u32,
}

impl NoiseSource {
    pub fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x6d2b_79f5 } else { seed },
        }
    }

    pub fn next_sample(&mut self) -> f32 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.state = value;
        value as i32 as f32 / i32::MAX as f32
    }
}

impl PolyBlepOsc {
    fn blep(t: f32, dt: f32) -> f32 {
        if t < dt {
            let x = t / dt;
            x + x - x * x - 1.0
        } else if t > 1.0 - dt {
            let x = (t - 1.0) / dt;
            x * x + x + x + 1.0
        } else {
            0.0
        }
    }
    pub fn next_saw(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = (hz / sr).clamp(0.0, 0.49);
        let mut x = 2.0 * self.phase - 1.0;
        x -= Self::blep(self.phase, dt);
        self.phase = (self.phase + dt).fract();
        x
    }
    pub fn next_square(&mut self, hz: f32, sr: f32) -> f32 {
        self.next_pulse(hz, 0.5, sr)
    }

    pub fn next_sub(&mut self, hz: f32, mode: SubOscillatorMode, sr: f32) -> f32 {
        match mode {
            SubOscillatorMode::TwoOctavesNarrowPulse => {
                self.next_pulse(hz * mode.ratio(), 0.25, sr)
            }
            _ => self.next_square(hz * mode.ratio(), sr),
        }
    }
    pub fn next_pulse(&mut self, hz: f32, width: f32, sr: f32) -> f32 {
        let dt = (hz / sr).clamp(0.0, 0.49);
        let width = width.clamp(0.05, 0.95);
        let mut x = if self.phase < width { 1.0 } else { -1.0 };
        x += Self::blep(self.phase, dt);
        x -= Self::blep((self.phase + 1.0 - width).fract(), dt);
        self.phase = (self.phase + dt).fract();
        x
    }

    pub fn next_saw_pulse(&mut self, hz: f32, width: f32, sr: f32) -> (f32, f32) {
        let dt = (hz / sr).clamp(0.0, 0.49);
        let width = width.clamp(0.05, 0.95);
        let mut saw = 2.0 * self.phase - 1.0;
        saw -= Self::blep(self.phase, dt);
        let mut pulse = if self.phase < width { 1.0 } else { -1.0 };
        pulse += Self::blep(self.phase, dt);
        pulse -= Self::blep((self.phase + 1.0 - width).fract(), dt);
        self.phase = (self.phase + dt).fract();
        (saw, pulse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EnvelopeProfile {
    Generic,
    Chord,
    Lead,
}

pub struct Adsr {
    pub stage: EnvStage,
    value: f32,
    attack_percent: Smoother,
    decay_percent: Smoother,
    sustain_percent: Smoother,
    release_percent: Smoother,
    sr: f32,
    profile: EnvelopeProfile,
    cached_attack_percent: f32,
    cached_decay_percent: f32,
    cached_release_percent: f32,
    cached_attack: f32,
    cached_decay: f32,
    cached_release: f32,
    cached_attack_coefficient: f32,
    cached_decay_coefficient: f32,
    cached_release_coefficient: f32,
    modulated_attack: f32,
    modulated_attack_coefficient: f32,
    modulated_decay_coefficient: f32,
    modulated_release_coefficient: f32,
    modulation_refresh_remaining: u8,
}
impl Adsr {
    pub fn new(sr: f32) -> Self {
        Self {
            stage: EnvStage::Idle,
            value: 0.0,
            attack_percent: Smoother::new(0.0),
            decay_percent: Smoother::new(25.0),
            sustain_percent: Smoother::new(70.0),
            release_percent: Smoother::new(15.0),
            sr,
            profile: EnvelopeProfile::Generic,
            cached_attack_percent: f32::NAN,
            cached_decay_percent: f32::NAN,
            cached_release_percent: f32::NAN,
            cached_attack: 0.0,
            cached_decay: 0.005,
            cached_release: 0.005,
            cached_attack_coefficient: 1.0,
            cached_decay_coefficient: 1.0,
            cached_release_coefficient: 1.0,
            modulated_attack: 0.0,
            modulated_attack_coefficient: 1.0,
            modulated_decay_coefficient: 1.0,
            modulated_release_coefficient: 1.0,
            modulation_refresh_remaining: 0,
        }
    }
    pub fn set_profile(&mut self, profile: EnvelopeProfile) {
        if self.profile != profile {
            self.profile = profile;
            self.cached_attack_percent = f32::NAN;
        }
    }
    pub fn configure_percent(&mut self, a: u8, d: u8, s: u8, r: u8, samples: u32) {
        self.attack_percent.set(a as f32, samples);
        self.decay_percent.set(d as f32, samples);
        self.sustain_percent.set(s as f32, samples);
        self.release_percent.set(r as f32, samples);
    }
    pub fn gate_on(&mut self) {
        self.stage = EnvStage::Attack
    }
    pub fn gate_off(&mut self) {
        if self.stage != EnvStage::Idle {
            self.stage = EnvStage::Release
        }
    }
    pub fn reset(&mut self) {
        self.stage = EnvStage::Idle;
        self.value = 0.0;
    }
    #[cfg(test)]
    pub(crate) fn parameter_values(&self) -> (f32, f32, f32, f32) {
        (
            self.attack_percent.value(),
            self.decay_percent.value(),
            self.sustain_percent.value(),
            self.release_percent.value(),
        )
    }
    pub fn next_sample_modulated(
        &mut self,
        attack_offset: f32,
        decay_offset: f32,
        sustain_offset: f32,
        release_offset: f32,
    ) -> f32 {
        let attack_base = self.attack_percent.next_value();
        let decay_base = self.decay_percent.next_value();
        let sustain =
            (self.sustain_percent.next_value() + sustain_offset).clamp(0.0, 100.0) / 100.0;
        let release_base = self.release_percent.next_value();
        let (attack, attack_coefficient, decay_coefficient, release_coefficient) =
            if attack_offset == 0.0 && decay_offset == 0.0 && release_offset == 0.0 {
                self.refresh_cached_times(attack_base, decay_base, release_base);
                (
                    self.cached_attack,
                    self.cached_attack_coefficient,
                    self.cached_decay_coefficient,
                    self.cached_release_coefficient,
                )
            } else if self.modulation_refresh_remaining == 0 {
                let attack_percent = (attack_base + attack_offset).clamp(0.0, 100.0);
                let decay_percent = (decay_base + decay_offset).clamp(0.0, 100.0);
                let release_percent = (release_base + release_offset).clamp(0.0, 100.0);
                let (attack_min, attack_max, decay_min, decay_max, release_min, release_max) =
                    self.profile_ranges();
                let attack = if attack_percent == 0.0 {
                    0.0
                } else {
                    exp_map_f32(attack_percent, attack_min, attack_max)
                };
                let decay = exp_map_f32(decay_percent, decay_min, decay_max);
                let release = exp_map_f32(release_percent, release_min, release_max);
                self.modulated_attack = attack;
                self.modulated_attack_coefficient = if attack == 0.0 {
                    1.0
                } else {
                    1.0 - (-6.907_755 / (attack * self.sr).max(1.0)).exp()
                };
                self.modulated_decay_coefficient =
                    1.0 - (-6.907_755 / (decay * self.sr).max(1.0)).exp();
                self.modulated_release_coefficient =
                    (-6.907_755 / (release * self.sr).max(1.0)).exp();
                self.modulation_refresh_remaining = 8;
                (
                    self.modulated_attack,
                    self.modulated_attack_coefficient,
                    self.modulated_decay_coefficient,
                    self.modulated_release_coefficient,
                )
            } else {
                (
                    self.modulated_attack,
                    self.modulated_attack_coefficient,
                    self.modulated_decay_coefficient,
                    self.modulated_release_coefficient,
                )
            };
        self.modulation_refresh_remaining = self.modulation_refresh_remaining.saturating_sub(1);
        match self.stage {
            EnvStage::Idle => {}
            EnvStage::Attack => {
                if attack <= 0.0 {
                    self.value = 1.0;
                    self.stage = EnvStage::Decay
                } else {
                    self.value += (1.0 - self.value) * attack_coefficient;
                    if self.value >= 0.999 {
                        self.value = 1.0;
                        self.stage = EnvStage::Decay
                    }
                }
            }
            EnvStage::Decay => {
                self.value += (sustain - self.value) * decay_coefficient;
                if (self.value - sustain).abs() <= 0.001 {
                    self.value = sustain;
                    self.stage = EnvStage::Sustain
                }
            }
            EnvStage::Sustain => self.value = sustain,
            EnvStage::Release => {
                self.value *= release_coefficient;
                if self.value <= 0.0001 {
                    self.value = 0.0;
                    self.stage = EnvStage::Idle
                }
            }
        }
        self.value
    }

    fn profile_ranges(&self) -> (f32, f32, f32, f32, f32, f32) {
        match self.profile {
            EnvelopeProfile::Generic => (0.001, 2.0, 0.005, 3.0, 0.005, 5.0),
            EnvelopeProfile::Chord => (0.001, 3.0, 0.002, 12.0, 0.002, 12.0),
            EnvelopeProfile::Lead => (0.0015, 4.0, 0.002, 10.0, 0.002, 10.0),
        }
    }

    fn refresh_cached_times(&mut self, attack: f32, decay: f32, release: f32) {
        if attack == self.cached_attack_percent
            && decay == self.cached_decay_percent
            && release == self.cached_release_percent
        {
            return;
        }
        self.cached_attack_percent = attack;
        self.cached_decay_percent = decay;
        self.cached_release_percent = release;
        let (attack_min, attack_max, decay_min, decay_max, release_min, release_max) =
            self.profile_ranges();
        self.cached_attack = if attack == 0.0 {
            0.0
        } else {
            exp_map_f32(attack, attack_min, attack_max)
        };
        self.cached_decay = exp_map_f32(decay, decay_min, decay_max);
        self.cached_release = exp_map_f32(release, release_min, release_max);
        self.cached_attack_coefficient = if self.cached_attack == 0.0 {
            1.0
        } else {
            1.0 - (-6.907_755 / (self.cached_attack * self.sr).max(1.0)).exp()
        };
        self.cached_decay_coefficient =
            1.0 - (-6.907_755 / (self.cached_decay * self.sr).max(1.0)).exp();
        self.cached_release_coefficient =
            (-6.907_755 / (self.cached_release * self.sr).max(1.0)).exp();
    }
}

/// The Bass amplifier is a gate, rather than a second copy of its filter
/// contour.  Its deliberately fixed timing keeps held notes present after the
/// filter sweep has completed.
pub struct BassVcaEnvelope {
    value: f32,
    gate: bool,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl BassVcaEnvelope {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            value: 0.0,
            gate: false,
            // A short but non-zero rise avoids discontinuities at note starts.
            attack_coefficient: 1.0 - (-6.907_755 / (0.003 * sr)).exp(),
            // This is intentionally independent of the Bass Decay control.
            release_coefficient: (-6.907_755 / (0.055 * sr)).exp(),
        }
    }

    pub fn gate_on(&mut self) {
        self.gate = true;
    }

    pub fn gate_off(&mut self) {
        self.gate = false;
    }

    pub fn is_idle(&self) -> bool {
        !self.gate && self.value <= 0.001
    }

    #[cfg(test)]
    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn next_sample(&mut self) -> f32 {
        if self.gate {
            self.value += (1.0 - self.value) * self.attack_coefficient;
        } else {
            self.value *= self.release_coefficient;
            if self.value <= 0.001 {
                self.value = 0.0;
            }
        }
        self.value
    }
}

/// Exponential filter contour for the Bass voice.  It has no sustain stage:
/// the decay knob changes timbre, not how long a held note remains audible.
pub struct BassFilterEnvelope {
    value: f32,
    coefficient: f32,
    sample_rate: f32,
    cached_decay_percent: f32,
}

impl BassFilterEnvelope {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            value: 0.0,
            coefficient: 0.0,
            sample_rate: sample_rate.max(1.0),
            cached_decay_percent: f32::NAN,
        }
    }

    pub fn trigger(&mut self, decay_percent: f32) {
        self.set_decay(decay_percent);
        self.value = 1.0;
    }

    pub fn set_decay(&mut self, decay_percent: f32) {
        let decay_percent = decay_percent.clamp(0.0, 100.0);
        if decay_percent == self.cached_decay_percent {
            return;
        }
        self.cached_decay_percent = decay_percent;
        let seconds = exp_map_f32(decay_percent, 0.080, 2.0);
        self.coefficient = (-6.907_755 / (seconds * self.sample_rate).max(1.0)).exp();
    }

    #[cfg(test)]
    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn reset(&mut self) {
        self.value = 0.0;
    }

    pub fn next_sample(&mut self) -> f32 {
        self.value *= self.coefficient;
        if self.value <= 0.0001 {
            self.value = 0.0;
        }
        self.value
    }
}

/// A short accent contour.  It is separate from both gate and filter contour,
/// so consecutive accents retrigger deterministically and ties leave it alone.
pub struct BassAccentEnvelope {
    value: f32,
    stage: BassAccentStage,
    attack_coefficient: f32,
    coefficient: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BassAccentStage {
    Idle,
    Attack,
    Decay,
}

impl BassAccentEnvelope {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            value: 0.0,
            stage: BassAccentStage::Idle,
            attack_coefficient: 1.0 - (-6.907_755 / (0.003 * sample_rate.max(1.0))).exp(),
            coefficient: (-6.907_755 / (0.180 * sample_rate.max(1.0))).exp(),
        }
    }

    pub fn trigger(&mut self, accented: bool) {
        self.stage = if accented {
            BassAccentStage::Attack
        } else {
            BassAccentStage::Decay
        };
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn reset(&mut self) {
        self.value = 0.0;
        self.stage = BassAccentStage::Idle;
    }

    pub fn next_sample(&mut self) -> f32 {
        match self.stage {
            BassAccentStage::Idle => {}
            BassAccentStage::Attack => {
                self.value += (1.0 - self.value) * self.attack_coefficient;
                if self.value >= 0.999 {
                    self.value = 1.0;
                    self.stage = BassAccentStage::Decay;
                }
            }
            BassAccentStage::Decay => {
                self.value *= self.coefficient;
            }
        }
        if self.value <= 0.0001 {
            self.value = 0.0;
            self.stage = BassAccentStage::Idle;
        }
        self.value
    }
}

/// Shared implementation for the two four-pole low-pass calibrations.  The
/// wrappers below intentionally expose separate instrument types so their
/// resonance behavior cannot silently converge again.
struct CalibratedFourPole {
    stages: [f32; 4],
    coefficient: f32,
    feedback: f32,
    coefficient_step: f32,
    feedback_step: f32,
    parameter_smoothing_remaining: u8,
    cached_cutoff: f32,
    cached_resonance: f32,
    cached_sr: f32,
}

#[derive(Clone, Copy)]
struct FourPoleCalibration {
    cutoff_scale: f32,
    feedback_scale: f32,
}

impl CalibratedFourPole {
    fn new() -> Self {
        Self {
            stages: [0.0; 4],
            coefficient: 0.0,
            feedback: 0.0,
            coefficient_step: 0.0,
            feedback_step: 0.0,
            parameter_smoothing_remaining: 0,
            cached_cutoff: f32::NAN,
            cached_resonance: f32::NAN,
            cached_sr: f32::NAN,
        }
    }

    fn set_parameters_smoothed(
        &mut self,
        cutoff: f32,
        resonance: f32,
        sr: f32,
        samples: u8,
        calibration: FourPoleCalibration,
    ) {
        let sr = sr.max(1.0);
        let cutoff = (cutoff * calibration.cutoff_scale).clamp(20.0, sr * 0.45);
        let resonance = resonance.clamp(0.0, 1.0);
        if cutoff == self.cached_cutoff
            && resonance == self.cached_resonance
            && sr == self.cached_sr
        {
            return;
        }
        let initialize = samples == 0 || !self.cached_sr.is_finite();
        self.cached_cutoff = cutoff;
        self.cached_resonance = resonance;
        self.cached_sr = sr;
        let coefficient = 1.0 - (-2.0 * PI * cutoff / sr).exp();
        // Keep the feedback below the unstable region.  The final stage is
        // still allowed to ring strongly, but extreme resonance remains
        // finite and does not turn the filter into an uncontrolled oscillator.
        let feedback = (resonance * calibration.feedback_scale).min(4.15);
        if initialize {
            self.coefficient = coefficient;
            self.feedback = feedback;
            self.parameter_smoothing_remaining = 0;
        } else {
            let samples = f32::from(samples);
            self.coefficient_step = (coefficient - self.coefficient) / samples;
            self.feedback_step = (feedback - self.feedback) / samples;
            self.parameter_smoothing_remaining = samples as u8;
        }
    }

    fn process(&mut self, input: f32, input_drive: f32) -> f32 {
        if self.parameter_smoothing_remaining > 0 {
            self.coefficient += self.coefficient_step;
            self.feedback += self.feedback_step;
            self.parameter_smoothing_remaining -= 1;
        }
        let mut stage_input = (input * input_drive - self.stages[3] * self.feedback).tanh();
        for stage in &mut self.stages {
            *stage += self.coefficient * (stage_input - stage.tanh());
            stage_input = stage.tanh();
        }
        let output = self.stages[3];
        if output.is_finite() && self.stages.iter().all(|state| state.is_finite()) {
            output
        } else {
            self.reset();
            0.0
        }
    }

    fn reset(&mut self) {
        self.stages = [0.0; 4];
    }
}

/// Four-pole low-pass calibration for the Chord voice.
pub struct ChordFilter {
    inner: CalibratedFourPole,
}

impl ChordFilter {
    pub fn new() -> Self {
        Self {
            inner: CalibratedFourPole::new(),
        }
    }

    pub fn set_parameters_smoothed(&mut self, cutoff: f32, resonance: f32, sr: f32, samples: u8) {
        self.inner.set_parameters_smoothed(
            cutoff,
            resonance,
            sr,
            samples,
            FourPoleCalibration {
                cutoff_scale: 0.92,
                feedback_scale: 3.45,
            },
        );
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.inner.process(input, 1.08)
    }

    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

impl Default for ChordFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Four-pole low-pass calibration for the Lead voice. Its
/// stronger drive and feedback retain the sharper, more aggressive response
/// of the monophonic instrument.
pub struct LeadFilter {
    inner: CalibratedFourPole,
}

impl LeadFilter {
    pub fn new() -> Self {
        Self {
            inner: CalibratedFourPole::new(),
        }
    }

    pub fn set_parameters_smoothed(&mut self, cutoff: f32, resonance: f32, sr: f32, samples: u8) {
        self.inner.set_parameters_smoothed(
            cutoff,
            resonance,
            sr,
            samples,
            FourPoleCalibration {
                cutoff_scale: 1.06,
                feedback_scale: 4.05,
            },
        );
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.inner.process(input, 1.42)
    }

    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

impl Default for LeadFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// A four-pole, diode-ladder-inspired nonlinear low-pass filter for the Bass
/// voice.  Its transition around cutoff is deliberately gentle, while its
/// far-stopband response approaches 24 dB/octave.  It is intended to be driven
/// at 2x the host sample rate. Parameter updates are cached and interpolated at
/// control rate so cutoff changes do not produce zipper noise in the callback.
pub struct BassFilter {
    stages: [f32; 4],
    coefficient: f32,
    feedback: f32,
    coefficient_step: f32,
    feedback_step: f32,
    parameter_smoothing_remaining: u8,
    cached_cutoff: f32,
    cached_resonance: f32,
    cached_sr: f32,
}

impl BassFilter {
    pub fn new() -> Self {
        Self {
            stages: [0.0; 4],
            coefficient: 0.0,
            feedback: 0.0,
            coefficient_step: 0.0,
            feedback_step: 0.0,
            parameter_smoothing_remaining: 0,
            cached_cutoff: f32::NAN,
            cached_resonance: f32::NAN,
            cached_sr: f32::NAN,
        }
    }

    pub fn reset(&mut self) {
        self.stages = [0.0; 4];
    }

    pub fn set_parameters_smoothed(&mut self, cutoff: f32, resonance: f32, sr: f32, samples: u8) {
        if cutoff == self.cached_cutoff
            && resonance == self.cached_resonance
            && sr == self.cached_sr
        {
            return;
        }
        let sr = sr.max(1.0);
        let cutoff = cutoff.clamp(20.0, sr * 0.42);
        let resonance = resonance.clamp(0.0, 1.0);
        let coefficient = 1.0 - (-2.0 * PI * cutoff / sr).exp();
        // The conservative feedback ceiling, paired with the tanh stages,
        // keeps self-oscillation character finite at extreme controls.
        let feedback = resonance * 2.75;
        let initialize = !self.cached_sr.is_finite() || samples == 0;
        self.cached_cutoff = cutoff;
        self.cached_resonance = resonance;
        self.cached_sr = sr;
        if initialize {
            self.coefficient = coefficient;
            self.feedback = feedback;
            self.parameter_smoothing_remaining = 0;
        } else {
            let samples = f32::from(samples);
            self.coefficient_step = (coefficient - self.coefficient) / samples;
            self.feedback_step = (feedback - self.feedback) / samples;
            self.parameter_smoothing_remaining = samples as u8;
        }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        if self.parameter_smoothing_remaining > 0 {
            self.coefficient += self.coefficient_step;
            self.feedback += self.feedback_step;
            self.parameter_smoothing_remaining -= 1;
        }
        let mut stage_input = (input * 1.25 - self.stages[3] * self.feedback).tanh();
        for stage in &mut self.stages {
            *stage += self.coefficient * (stage_input - stage.tanh());
            stage_input = stage.tanh();
        }
        let output = self.stages[3];
        if output.is_finite() && self.stages.iter().all(|state| state.is_finite()) {
            output
        } else {
            self.reset();
            0.0
        }
    }
}

impl Default for BassFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// A transposed-direct-form-II biquad using the Web Audio/RBJ filter equations.
/// Coefficients can be changed without clearing the delay state, matching a
/// continuously running Web Audio `BiquadFilterNode`.
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }
    fn common(frequency: f32, q: f32, sample_rate: f32) -> (f32, f32, f32) {
        let frequency = frequency.clamp(1.0, sample_rate * 0.499);
        let omega = 2.0 * PI * frequency / sample_rate;
        let alpha = omega.sin() / (2.0 * q.max(0.0001));
        (omega.cos(), omega.sin(), alpha)
    }
    pub fn set_bandpass(&mut self, frequency: f32, q: f32, sample_rate: f32) {
        let (cos, _sin, alpha) = Self::common(frequency, q, sample_rate);
        self.set(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha);
    }
    pub fn set_highpass(&mut self, frequency: f32, q: f32, sample_rate: f32) {
        let (cos, _sin, alpha) = Self::common(frequency, q, sample_rate);
        self.set(
            (1.0 + cos) * 0.5,
            -(1.0 + cos),
            (1.0 + cos) * 0.5,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        );
    }
    pub fn set_lowpass(&mut self, frequency: f32, q: f32, sample_rate: f32) {
        let (cos, _sin, alpha) = Self::common(frequency, q, sample_rate);
        self.set(
            (1.0 - cos) * 0.5,
            1.0 - cos,
            (1.0 - cos) * 0.5,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        );
    }
    pub fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        if output.is_finite() {
            output
        } else {
            self.z1 = 0.0;
            self.z2 = 0.0;
            0.0
        }
    }

    pub fn clear_state(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[cfg(test)]
    pub(crate) fn coefficients(&self) -> [f32; 5] {
        [self.b0, self.b1, self.b2, self.a1, self.a2]
    }
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

/// A short, modulated stereo delay implementing the fixed track chorus modes.
/// Storage is allocated when the renderer is built, never in the audio callback.
#[derive(Debug)]
pub struct StereoChorus {
    buffer: Vec<f32>,
    pos: usize,
    phase: f32,
    old_phase: f32,
    sample_rate: f32,
    mode: u8,
    old_mode: u8,
    fade_remaining: u32,
    fade_length: u32,
    active: bool,
    tail_remaining: usize,
}

impl StereoChorus {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            buffer: vec![0.0; (sample_rate as f32 * 0.025).ceil() as usize + 2],
            pos: 0,
            phase: 0.0,
            old_phase: 0.0,
            sample_rate: sample_rate as f32,
            mode: 0,
            old_mode: 0,
            fade_remaining: 0,
            fade_length: (sample_rate as f32 * 0.005).round().max(1.0) as u32,
            active: false,
            tail_remaining: 0,
        }
    }

    pub fn configure(&mut self, mode: u8) {
        let mode = mode.min(2);
        if mode != self.mode {
            self.old_mode = self.mode;
            self.old_phase = self.phase;
            self.mode = mode;
            self.fade_remaining = self.fade_length;
        }
    }

    fn tap(&self, delay_samples: f32) -> f32 {
        let read = (self.pos as f32 - delay_samples).rem_euclid(self.buffer.len() as f32);
        // `rem_euclid` can round a tiny negative remainder up to the modulus
        // in `f32`.  Keep the integer index wrapped as well, otherwise a
        // read exactly at `buffer.len()` panics in the audio callback.
        let first = (read.floor() as usize) % self.buffer.len();
        let second = (first + 1) % self.buffer.len();
        let fraction = read.fract();
        self.buffer[first] + (self.buffer[second] - self.buffer[first]) * fraction
    }

    fn mode_sample(&self, input: f32, mode: u8, phase: f32) -> (f32, f32) {
        let (base_ms, depth_ms) = match mode {
            1 => (15.0, 1.5),
            2 => (12.0, 2.5),
            _ => return (input, input),
        };
        let base = base_ms * self.sample_rate / 1_000.0;
        let depth = depth_ms * self.sample_rate / 1_000.0;
        let modulation = depth * (std::f32::consts::TAU * phase).sin();
        // Slightly dry-biased equal-power calibration keeps engaging a mode
        // close to unity in the presence of correlated delay taps.
        const DRY: f32 = 0.84;
        const WET: f32 = 0.54;
        let left = self.tap(base + modulation);
        let right = self.tap(base - modulation);
        (input * DRY + left * WET, input * DRY + right * WET)
    }

    #[cfg(test)]
    pub fn process(&mut self, input: f32) -> (f32, f32) {
        self.process_stereo(input, input)
    }

    pub fn process_stereo(&mut self, left_input: f32, right_input: f32) -> (f32, f32) {
        if self.mode == 0 && self.fade_remaining == 0 {
            self.active = false;
            self.tail_remaining = 0;
            return (left_input, right_input);
        }
        let input_peak = safety(left_input).abs().max(safety(right_input).abs());
        if !self.active && self.fade_remaining == 0 && input_peak <= SILENCE_THRESHOLD {
            return (left_input, right_input);
        }
        if input_peak > SILENCE_THRESHOLD && (self.mode != 0 || self.fade_remaining != 0) {
            self.active = true;
            self.tail_remaining = self.buffer.len();
        }
        let input = (left_input + right_input) * 0.5;
        self.buffer[self.pos] = input;
        let next = self.mode_sample(input, self.mode, self.phase);
        let output = if self.fade_remaining > 0 {
            let old = self.mode_sample(input, self.old_mode, self.old_phase);
            let mix = 1.0 - self.fade_remaining as f32 / self.fade_length as f32;
            self.fade_remaining -= 1;
            (
                left_input + old.0 + (next.0 - old.0) * mix - input,
                right_input + old.1 + (next.1 - old.1) * mix - input,
            )
        } else {
            (left_input + next.0 - input, right_input + next.1 - input)
        };
        self.pos = (self.pos + 1) % self.buffer.len();
        let rate = if self.mode == 2 { 0.8 } else { 0.5 };
        let old_rate = if self.old_mode == 2 { 0.8 } else { 0.5 };
        self.phase = (self.phase + rate / self.sample_rate).fract();
        self.old_phase = (self.old_phase + old_rate / self.sample_rate).fract();
        if input_peak <= SILENCE_THRESHOLD {
            self.tail_remaining = self.tail_remaining.saturating_sub(1);
            if self.tail_remaining == 0 && self.fade_remaining == 0 {
                self.active = false;
            }
        }
        output
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
        self.phase = 0.0;
        self.old_phase = 0.0;
        self.fade_remaining = 0;
        self.active = false;
        self.tail_remaining = 0;
    }

    pub fn is_active(&self) -> bool {
        self.active || self.fade_remaining != 0
    }
}

pub struct Delay {
    left: Vec<f32>,
    right: Vec<f32>,
    pos: usize,
    delay: usize,
    old_delay: usize,
    fade_remaining: usize,
    fade_length: usize,
    configured: bool,
    feedback: f32,
    damp_l: f32,
    damp_r: f32,
    damp_coefficient: f32,
    highpass_x_l: f32,
    highpass_x_r: f32,
    highpass_y_l: f32,
    highpass_y_r: f32,
    active: bool,
    tail_remaining: usize,
    tail_samples: usize,
}

struct Comb {
    buffer: Vec<f32>,
    pos: usize,
    damped: f32,
    feedback: Smoother,
}
impl Comb {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(2)],
            pos: 0,
            damped: 0.0,
            feedback: Smoother::new(0.0),
        }
    }
    fn set_time(&mut self, seconds: f32, sample_rate: f32, smoothing_samples: u32) {
        let delay_seconds = self.buffer.len() as f32 / sample_rate;
        let feedback = 10.0_f32
            .powf(-3.0 * delay_seconds / seconds)
            .clamp(0.0, 0.9999);
        self.feedback.set(feedback, smoothing_samples);
    }
    fn process(&mut self, input: f32, damping: f32) -> f32 {
        let output = self.buffer[self.pos];
        self.damped += (output - self.damped) * (1.0 - damping);
        let feedback = self.feedback.next_value();
        self.buffer[self.pos] = input + self.damped * feedback;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.damped = 0.0;
    }
}

struct Allpass {
    buffer: Vec<f32>,
    pos: usize,
}
impl Allpass {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(2)],
            pos: 0,
        }
    }
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.pos];
        let output = delayed - input;
        self.buffer[self.pos] = input + delayed * 0.5;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}

/// A compact Freeverb-style stereo reverberator with all storage allocated up front.
pub struct Reverb {
    left: [Comb; 8],
    right: [Comb; 8],
    allpass_l: [Allpass; 4],
    allpass_r: [Allpass; 4],
    sample_rate: f32,
    pre_delay_l: Vec<f32>,
    pre_delay_r: Vec<f32>,
    pre_delay_pos: usize,
    pre_delay: usize,
    old_pre_delay: usize,
    pre_delay_fade_remaining: usize,
    pre_delay_fade_length: usize,
    damping: Smoother,
    highpass_l: Biquad,
    highpass_r: Biquad,
    active: bool,
    tail_remaining: usize,
    tail_samples: usize,
    tail_seconds: f32,
}
impl Reverb {
    const HIGH_PASS_HZ: f32 = 180.0;

    pub fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / 44_100.0;
        let size = |n: usize| (n as f32 * scale).round() as usize;
        let pre_delay_size = (sample_rate as f32 * 0.2).ceil() as usize + 2;
        let mut reverb = Self {
            left: [
                Comb::new(size(1116)),
                Comb::new(size(1188)),
                Comb::new(size(1277)),
                Comb::new(size(1356)),
                Comb::new(size(1422)),
                Comb::new(size(1491)),
                Comb::new(size(1557)),
                Comb::new(size(1617)),
            ],
            right: [
                Comb::new(size(1139)),
                Comb::new(size(1211)),
                Comb::new(size(1300)),
                Comb::new(size(1379)),
                Comb::new(size(1445)),
                Comb::new(size(1514)),
                Comb::new(size(1580)),
                Comb::new(size(1640)),
            ],
            allpass_l: [
                Allpass::new(size(556)),
                Allpass::new(size(441)),
                Allpass::new(size(341)),
                Allpass::new(size(225)),
            ],
            allpass_r: [
                Allpass::new(size(579)),
                Allpass::new(size(464)),
                Allpass::new(size(364)),
                Allpass::new(size(248)),
            ],
            sample_rate: sample_rate as f32,
            pre_delay_l: vec![0.0; pre_delay_size],
            pre_delay_r: vec![0.0; pre_delay_size],
            pre_delay_pos: 0,
            pre_delay: 0,
            old_pre_delay: 0,
            pre_delay_fade_remaining: 0,
            pre_delay_fade_length: (sample_rate as usize / 200).max(1),
            damping: Smoother::new(0.32),
            highpass_l: {
                let mut filter = Biquad::new();
                filter.set_highpass(
                    Self::HIGH_PASS_HZ,
                    std::f32::consts::FRAC_1_SQRT_2,
                    sample_rate as f32,
                );
                filter
            },
            highpass_r: {
                let mut filter = Biquad::new();
                filter.set_highpass(
                    Self::HIGH_PASS_HZ,
                    std::f32::consts::FRAC_1_SQRT_2,
                    sample_rate as f32,
                );
                filter
            },
            active: false,
            tail_remaining: 0,
            tail_samples: 1,
            tail_seconds: 2.5,
        };
        reverb.set_time(2.5);
        reverb.set_tone(0.5);
        reverb.set_pre_delay_ms(20);
        reverb.pre_delay_fade_remaining = 0;
        reverb
    }
    pub fn set_time(&mut self, seconds: f32) {
        self.set_time_smoothed(seconds, 0);
    }
    pub(crate) fn set_time_smoothed(&mut self, seconds: f32, smoothing_samples: u32) {
        let seconds = seconds.clamp(0.2, 10.0);
        self.tail_seconds = seconds;
        self.tail_samples = self.estimated_tail_samples();
        for comb in self.left.iter_mut().chain(self.right.iter_mut()) {
            comb.set_time(seconds, self.sample_rate, smoothing_samples);
        }
    }
    pub fn set_tone(&mut self, tone: f32) {
        self.set_tone_smoothed(tone, 0);
    }
    pub(crate) fn set_tone_smoothed(&mut self, tone: f32, smoothing_samples: u32) {
        let tone = tone.clamp(0.0, 1.0);
        let damping = 0.60 - tone * 0.56;
        self.damping.set(damping, smoothing_samples);
    }
    pub fn set_pre_delay_ms(&mut self, milliseconds: u16) {
        self.set_pre_delay_smoothed(milliseconds, 0);
    }
    pub(crate) fn set_pre_delay_smoothed(&mut self, milliseconds: u16, _smoothing_samples: u32) {
        let next = ((milliseconds as f32 * self.sample_rate / 1_000.0).round() as usize)
            .min(self.pre_delay_l.len() - 1);
        if next != self.pre_delay {
            self.old_pre_delay = self.pre_delay;
            self.pre_delay = next;
            self.pre_delay_fade_remaining = self.pre_delay_fade_length;
        }
        self.tail_samples = self.estimated_tail_samples();
    }
    fn pre_delay_tap(buffer: &[f32], pos: usize, delay: usize, input: f32) -> f32 {
        if delay == 0 {
            input
        } else {
            buffer[(pos + buffer.len() - delay) % buffer.len()]
        }
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let input_peak = safety(l).abs().max(safety(r).abs());
        if !self.active {
            if input_peak <= SILENCE_THRESHOLD {
                return (0.0, 0.0);
            }
            self.active = true;
        }
        if input_peak > SILENCE_THRESHOLD {
            self.tail_remaining = self.tail_samples;
        }
        let mut input_l =
            Self::pre_delay_tap(&self.pre_delay_l, self.pre_delay_pos, self.pre_delay, l);
        let mut input_r =
            Self::pre_delay_tap(&self.pre_delay_r, self.pre_delay_pos, self.pre_delay, r);
        if self.pre_delay_fade_remaining > 0 {
            let old_l =
                Self::pre_delay_tap(&self.pre_delay_l, self.pre_delay_pos, self.old_pre_delay, l);
            let old_r =
                Self::pre_delay_tap(&self.pre_delay_r, self.pre_delay_pos, self.old_pre_delay, r);
            let mix =
                1.0 - self.pre_delay_fade_remaining as f32 / self.pre_delay_fade_length as f32;
            input_l = old_l * (1.0 - mix) + input_l * mix;
            input_r = old_r * (1.0 - mix) + input_r * mix;
            self.pre_delay_fade_remaining -= 1;
        }
        self.pre_delay_l[self.pre_delay_pos] = l;
        self.pre_delay_r[self.pre_delay_pos] = r;
        self.pre_delay_pos = (self.pre_delay_pos + 1) % self.pre_delay_l.len();

        input_l = self.highpass_l.process(input_l);
        input_r = self.highpass_r.process(input_r);

        let center = (input_l + input_r) * 0.35;
        let input_l = center + input_l * 0.15;
        let input_r = center + input_r * 0.15;
        let damping = self.damping.next_value();
        let mut ol = 0.0;
        let mut or = 0.0;
        for comb in &mut self.left {
            ol += comb.process(input_l, damping);
        }
        for comb in &mut self.right {
            or += comb.process(input_r, damping);
        }
        for ap in &mut self.allpass_l {
            ol = ap.process(ol);
        }
        for ap in &mut self.allpass_r {
            or = ap.process(or);
        }
        if input_peak <= SILENCE_THRESHOLD {
            self.tail_remaining = self.tail_remaining.saturating_sub(1);
            if self.tail_remaining == 0 {
                self.active = false;
            }
        }
        (safety(ol * 0.125), safety(or * 0.125))
    }
    pub fn clear(&mut self) {
        for c in &mut self.left {
            c.clear();
        }
        for c in &mut self.right {
            c.clear();
        }
        for a in &mut self.allpass_l {
            a.clear();
        }
        for a in &mut self.allpass_r {
            a.clear();
        }
        self.pre_delay_l.fill(0.0);
        self.pre_delay_r.fill(0.0);
        self.pre_delay_pos = 0;
        self.pre_delay_fade_remaining = 0;
        self.highpass_l.clear_state();
        self.highpass_r.clear_state();
        self.active = false;
        self.tail_remaining = 0;
    }

    fn estimated_tail_samples(&self) -> usize {
        (self.pre_delay as f32 + self.sample_rate * self.tail_seconds * 4.0)
            .ceil()
            .max(1.0) as usize
    }
}
impl Delay {
    pub fn new(sample_rate: u32) -> Self {
        let n = (sample_rate as usize * 7).max(2);
        Self {
            left: vec![0.0; n],
            right: vec![0.0; n],
            pos: 0,
            delay: 1,
            old_delay: 1,
            fade_remaining: 0,
            fade_length: (sample_rate as usize / 100).max(1),
            configured: false,
            feedback: 0.3,
            damp_l: 0.0,
            damp_r: 0.0,
            damp_coefficient: 1.0 - (-2.0 * PI * 7_500.0 / sample_rate as f32).exp(),
            highpass_x_l: 0.0,
            highpass_x_r: 0.0,
            highpass_y_l: 0.0,
            highpass_y_r: 0.0,
            active: false,
            tail_remaining: 0,
            tail_samples: 1,
        }
    }
    pub fn configure(&mut self, samples: usize, feedback: f32) {
        let next = samples.clamp(1, self.left.len() - 1);
        if next != self.delay && self.configured {
            self.old_delay = self.delay;
            self.delay = next;
            self.fade_remaining = self.fade_length;
        } else {
            self.delay = next;
        }
        self.configured = true;
        self.feedback = feedback.clamp(0.0, 0.95);
        self.tail_samples = self.estimated_tail_samples();
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let input_peak = safety(l).abs().max(safety(r).abs());
        if !self.active {
            if input_peak <= SILENCE_THRESHOLD {
                return (0.0, 0.0);
            }
            self.active = true;
        }
        if input_peak > SILENCE_THRESHOLD {
            self.tail_remaining = self.tail_samples;
        }
        let read = (self.pos + self.left.len() - self.delay) % self.left.len();
        let mut dl = self.left[read];
        let mut dr = self.right[read];
        if self.fade_remaining > 0 {
            let old = (self.pos + self.left.len() - self.old_delay) % self.left.len();
            let mix = 1.0 - self.fade_remaining as f32 / self.fade_length as f32;
            dl = self.left[old] * (1.0 - mix) + dl * mix;
            dr = self.right[old] * (1.0 - mix) + dr * mix;
            self.fade_remaining -= 1;
        }
        self.damp_l += (dl - self.damp_l) * self.damp_coefficient;
        self.damp_r += (dr - self.damp_r) * self.damp_coefficient;
        let high_l = self.damp_l - self.highpass_x_l + 0.995 * self.highpass_y_l;
        let high_r = self.damp_r - self.highpass_x_r + 0.995 * self.highpass_y_r;
        self.highpass_x_l = self.damp_l;
        self.highpass_x_r = self.damp_r;
        self.highpass_y_l = high_l;
        self.highpass_y_r = high_r;
        self.left[self.pos] = l + high_r * self.feedback;
        self.right[self.pos] = r + high_l * self.feedback;
        self.pos = (self.pos + 1) % self.left.len();
        if input_peak <= SILENCE_THRESHOLD {
            self.tail_remaining = self.tail_remaining.saturating_sub(1);
            if self.tail_remaining == 0 {
                self.active = false;
            }
        }
        (dl, dr)
    }
    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.damp_l = 0.0;
        self.damp_r = 0.0;
        self.highpass_x_l = 0.0;
        self.highpass_x_r = 0.0;
        self.highpass_y_l = 0.0;
        self.highpass_y_r = 0.0;
        self.active = false;
        self.tail_remaining = 0;
    }

    fn estimated_tail_samples(&self) -> usize {
        let feedback = self.feedback.max(SILENCE_THRESHOLD);
        let repeats = (SILENCE_THRESHOLD.ln() / feedback.ln()).ceil().max(1.0) as usize;
        self.delay
            .saturating_mul(repeats.saturating_add(1))
            .saturating_add(self.fade_length)
            .max(1)
    }
}

pub struct DcBlock {
    xl: f32,
    yl: f32,
    xr: f32,
    yr: f32,
}
impl DcBlock {
    pub fn new() -> Self {
        Self {
            xl: 0.,
            yl: 0.,
            xr: 0.,
            yr: 0.,
        }
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let ol = l - self.xl + 0.995 * self.yl;
        let or = r - self.xr + 0.995 * self.yr;
        self.xl = l;
        self.yl = ol;
        self.xr = r;
        self.yr = or;
        (safety(ol), safety(or))
    }
}
impl Default for DcBlock {
    fn default() -> Self {
        Self::new()
    }
}
pub fn safety(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

const SILENCE_THRESHOLD: f32 = 1.0e-7;

/// A fixed, stereo-linked lookahead limiter. All storage is allocated at construction.
pub struct MasterLimiter {
    left: Vec<f32>,
    right: Vec<f32>,
    peaks: Vec<PeakEntry>,
    peak_head: usize,
    peak_tail: usize,
    peak_len: usize,
    sample_position: u64,
    pos: usize,
    gain: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

#[derive(Clone, Copy, Default)]
struct PeakEntry {
    value: f32,
    position: u64,
}

impl MasterLimiter {
    pub fn new(sample_rate: u32) -> Self {
        let lookahead = ((sample_rate as f32 * 0.005).round() as usize).max(1);
        Self {
            left: vec![0.0; lookahead],
            right: vec![0.0; lookahead],
            peaks: vec![PeakEntry::default(); lookahead],
            peak_head: 0,
            peak_tail: 0,
            peak_len: 0,
            sample_position: 0,
            pos: 0,
            gain: 1.0,
            attack_coefficient: (-1.0 / (sample_rate as f32 * 0.001)).exp(),
            release_coefficient: (-1.0 / (sample_rate as f32 * 0.080)).exp(),
        }
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        const MAKEUP: f32 = 1.995_262_3;
        const CEILING: f32 = 0.891_250_9;
        let delayed_l = self.left[self.pos];
        let delayed_r = self.right[self.pos];
        self.left[self.pos] = safety(l);
        self.right[self.pos] = safety(r);
        self.pos = (self.pos + 1) % self.left.len();
        let frame_peak = self.left[(self.pos + self.left.len() - 1) % self.left.len()] * MAKEUP;
        let frame_peak = frame_peak
            .abs()
            .max((self.right[(self.pos + self.right.len() - 1) % self.right.len()] * MAKEUP).abs());
        self.push_peak(frame_peak);
        let peak = self.peaks[self.peak_head].value;
        let target = if peak > CEILING { CEILING / peak } else { 1.0 };
        let coefficient = if target < self.gain {
            self.attack_coefficient
        } else {
            self.release_coefficient
        };
        self.gain = target + coefficient * (self.gain - target);
        let l = (delayed_l * MAKEUP * self.gain).clamp(-CEILING, CEILING);
        let r = (delayed_r * MAKEUP * self.gain).clamp(-CEILING, CEILING);
        (safety(l), safety(r))
    }

    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.peaks.fill(PeakEntry::default());
        self.peak_head = 0;
        self.peak_tail = 0;
        self.peak_len = 0;
        self.sample_position = 0;
        self.pos = 0;
        self.gain = 1.0;
    }

    fn push_peak(&mut self, value: f32) {
        let capacity = self.peaks.len();
        let position = self.sample_position;
        self.sample_position = self.sample_position.wrapping_add(1);

        while self.peak_len > 0 {
            let oldest = self.peaks[self.peak_head].position;
            let expired = position.wrapping_sub(oldest) >= capacity as u64;
            if !expired {
                break;
            }
            self.peak_head = (self.peak_head + 1) % capacity;
            self.peak_len -= 1;
        }

        while self.peak_len > 0 {
            let back = (self.peak_tail + capacity - 1) % capacity;
            if self.peaks[back].value > value {
                break;
            }
            self.peak_tail = back;
            self.peak_len -= 1;
        }

        self.peaks[self.peak_tail] = PeakEntry { value, position };
        self.peak_tail = (self.peak_tail + 1) % capacity;
        self.peak_len += 1;
    }
}

#[cfg(test)]
include!("tests.rs");
