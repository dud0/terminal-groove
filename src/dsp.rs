use std::f32::consts::PI;

use crate::model::{LfoConfig, LfoWaveform, ParameterLocks, SidechainParameters, TrackEffects};

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

pub fn equal_power_pan(pan: f32) -> (f32, f32) {
    let angle = pan.clamp(0.0, 100.0) * std::f32::consts::FRAC_PI_2 / 100.0;
    (angle.cos(), angle.sin())
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

    pub fn envelope(&self) -> f32 {
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

    pub fn value(&self) -> f32 {
        self.smoothed
    }

    pub fn next(&mut self, config: Option<LfoConfig>, tempo_bpm: u16, sample_rate: f32) -> f32 {
        let Some(config) = config.filter(|config| config.enabled) else {
            self.disable();
            return 0.0;
        };
        if !self.active {
            self.phase = 0.0;
            self.smoothed = 0.0;
            self.rng = self.seed;
            self.held = self.random_bipolar();
            self.active = true;
        }
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
        let raw = match config.waveform {
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
        };
        self.smoothed += (raw - self.smoothed) * self.smoothing_coefficient;
        self.phase += self.increment.max(0.0);
        if self.phase >= 1.0 {
            self.phase = self.phase.fract();
            if config.waveform == LfoWaveform::SampleAndHold {
                self.held = self.random_bipolar();
            }
        }
        self.smoothed
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

const FLANGER_BUFFER_SIZE: usize = 8_192;

#[derive(Clone, Copy, Debug)]
struct FlangerControls {
    rate: f32,
    feedback: f32,
    mix: f32,
}

#[derive(Debug)]
pub struct TrackEffectChain {
    processing: bool,
    distortion_active: bool,
    phaser_active: bool,
    flanger_active: bool,
    distortion_tail_remaining: u32,
    phaser_tail_remaining: u32,
    flanger_tail_remaining: u32,
    distortion_drive: Smoother,
    distortion_tone: Smoother,
    distortion_mix: Smoother,
    distortion_state_l: f32,
    distortion_state_r: f32,
    distortion_gain: f32,
    distortion_coefficient: f32,
    cached_distortion_drive: f32,
    cached_distortion_tone: f32,
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
        let flanger_l = vec![0.0; FLANGER_BUFFER_SIZE].into_boxed_slice();
        let flanger_r = vec![0.0; FLANGER_BUFFER_SIZE].into_boxed_slice();
        Self {
            processing: false,
            distortion_active: false,
            phaser_active: false,
            flanger_active: false,
            distortion_tail_remaining: 0,
            phaser_tail_remaining: 0,
            flanger_tail_remaining: 0,
            distortion_drive: Smoother::new(0.0),
            distortion_tone: Smoother::new(50.0),
            distortion_mix: Smoother::new(0.0),
            distortion_state_l: 0.0,
            distortion_state_r: 0.0,
            distortion_gain: 1.0,
            distortion_coefficient: 1.0,
            cached_distortion_drive: f32::NAN,
            cached_distortion_tone: f32::NAN,
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
            sample_rate: sample_rate as f32,
            flanger_rate_hz: 0.05,
            flanger_center_samples: sample_rate as f32 * 0.0002,
            flanger_depth_samples: 0.0,
            cached_flanger_rate: f32::NAN,
            cached_flanger_delay: f32::NAN,
            cached_flanger_depth: f32::NAN,
        }
    }

    pub fn configure(&mut self, effects: TrackEffects, locks: ParameterLocks, samples: u32) {
        let was_processing = self.processing
            || self.distortion_mix.current > 0.0
            || self.phaser_mix.current > 0.0
            || self.flanger_mix.current > 0.0;
        self.distortion_drive.set(
            locks
                .distortion_drive
                .unwrap_or(effects.distortion.drive)
                .get() as f32,
            samples,
        );
        self.distortion_tone.set(
            locks
                .distortion_tone
                .unwrap_or(effects.distortion.tone)
                .get() as f32,
            samples,
        );
        self.distortion_mix.set(
            locks.distortion_mix.unwrap_or(effects.distortion.mix).get() as f32,
            samples,
        );
        self.phaser_rate.set(
            locks.phaser_rate.unwrap_or(effects.phaser.rate).get() as f32,
            samples,
        );
        self.phaser_depth.set(
            locks.phaser_depth.unwrap_or(effects.phaser.depth).get() as f32,
            samples,
        );
        self.phaser_feedback.set(
            locks
                .phaser_feedback
                .unwrap_or(effects.phaser.feedback)
                .get() as f32,
            samples,
        );
        self.phaser_mix.set(
            locks.phaser_mix.unwrap_or(effects.phaser.mix).get() as f32,
            samples,
        );
        self.flanger_rate.set(
            locks.flanger_rate.unwrap_or(effects.flanger.rate).get() as f32,
            samples,
        );
        self.flanger_delay.set(
            locks.flanger_delay.unwrap_or(effects.flanger.delay).get() as f32,
            samples,
        );
        self.flanger_depth.set(
            locks.flanger_depth.unwrap_or(effects.flanger.depth).get() as f32,
            samples,
        );
        self.flanger_feedback.set(
            locks
                .flanger_feedback
                .unwrap_or(effects.flanger.feedback)
                .get() as f32,
            samples,
        );
        self.flanger_mix.set(
            locks.flanger_mix.unwrap_or(effects.flanger.mix).get() as f32,
            samples,
        );
        self.processing = was_processing
            || self.distortion_mix.target > 0.0
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
        let phaser_mix = self.phaser_mix.next_value() / 100.0;
        let flanger_mix = self.flanger_mix.next_value() / 100.0;
        let input_peak = finite_or_zero(input_l)
            .abs()
            .max(finite_or_zero(input_r).abs());
        let input_active = input_peak > SILENCE_THRESHOLD;
        let distortion_active =
            distortion_mix > 0.0 && (input_active || self.distortion_tail_remaining > 0);
        let phaser_active = if phaser_mix > 0.0 {
            input_active || self.phaser_tail_remaining > 0
        } else {
            !input_active && self.phaser_tail_remaining > 0
        };
        let flanger_active = if flanger_mix > 0.0 {
            input_active || self.flanger_tail_remaining > 0
        } else {
            !input_active && self.flanger_tail_remaining > 0
        };
        if !distortion_active && !phaser_active && !flanger_active {
            self.distortion_active = false;
            self.phaser_active = false;
            self.flanger_active = false;
            self.processing = self.has_pending_parameters();
            return (input_l, input_r);
        }

        self.distortion_active = distortion_active;
        self.phaser_active = phaser_active;
        self.flanger_active = flanger_active;

        let distorted_l = if distortion_active {
            let drive = self.distortion_drive.next_value();
            let tone = self.distortion_tone.next_value();
            self.update_distortion_cache(drive, tone);
            self.distortion_tail_remaining = if input_active {
                self.tail_length()
            } else {
                self.distortion_tail_remaining.saturating_sub(1)
            };
            Self::distort_sample(
                input_l,
                self.distortion_gain,
                self.distortion_coefficient,
                distortion_mix,
                &mut self.distortion_state_l,
            )
        } else {
            input_l
        };
        let distorted_r = if distortion_active {
            Self::distort_sample(
                input_r,
                self.distortion_gain,
                self.distortion_coefficient,
                distortion_mix,
                &mut self.distortion_state_r,
            )
        } else {
            input_r
        };

        let (processed_l, processed_r) = if phaser_active {
            let phaser_rate = self.phaser_rate.next_value();
            let phaser_depth = self.phaser_depth.next_value();
            let phaser_feedback = (self.phaser_feedback.next_value() / 100.0).clamp(0.0, 0.9);
            self.update_phaser_cache(phaser_rate, phaser_depth);
            self.phaser_tail_remaining = if input_active {
                self.tail_length()
            } else {
                self.phaser_tail_remaining.saturating_sub(1)
            };
            let feedback =
                (self.phaser_feedback_l + self.phaser_feedback_r) * 0.5 * phaser_feedback;
            // Once the user-facing mix ramp reaches zero, keep draining the
            // feedback state silently. Re-expanding the tail to a derived wet
            // mix here would create a near-100% wet discontinuity.
            let effective_mix = phaser_mix;
            let left_input = distorted_l + feedback;
            let right_input = distorted_r - feedback;
            let left = Self::phaser_sample(
                &mut self.phaser_l,
                left_input,
                self.phaser_phase,
                self.phaser_sweep,
                self.sample_rate,
            );
            let right = Self::phaser_sample(
                &mut self.phaser_r,
                right_input,
                (self.phaser_phase + 0.5).fract(),
                self.phaser_sweep,
                self.sample_rate,
            );
            self.phaser_feedback_l = left;
            self.phaser_feedback_r = right;
            self.phaser_phase =
                (self.phaser_phase + self.phaser_rate_hz / self.sample_rate).fract();
            (
                distorted_l * (1.0 - effective_mix) + left * effective_mix,
                distorted_r * (1.0 - effective_mix) + right * effective_mix,
            )
        } else {
            (distorted_l, distorted_r)
        };

        if !flanger_active {
            self.processing = self.has_pending_parameters();
            return (processed_l, processed_r);
        }
        let flanger_rate = self.flanger_rate.next_value();
        let flanger_delay = self.flanger_delay.next_value();
        let flanger_depth = self.flanger_depth.next_value();
        let flanger_feedback = (self.flanger_feedback.next_value() / 100.0).clamp(0.0, 0.9);
        self.update_flanger_cache(flanger_rate, flanger_delay, flanger_depth);
        self.flanger_tail_remaining = if input_active {
            self.tail_length()
        } else {
            self.flanger_tail_remaining.saturating_sub(1)
        };
        let effective_mix = flanger_mix;
        self.flanger_sample(
            processed_l,
            processed_r,
            FlangerControls {
                rate: self.flanger_rate_hz,
                feedback: flanger_feedback,
                mix: effective_mix,
            },
        )
    }

    fn distort_sample(input: f32, gain: f32, coefficient: f32, mix: f32, state: &mut f32) -> f32 {
        let clipped = (input * gain).tanh();
        *state += (clipped - *state) * coefficient;
        input * (1.0 - mix) + *state * mix
    }

    fn phaser_sample(
        stages: &mut [EffectAllpass; 4],
        input: f32,
        phase: f32,
        sweep: f32,
        sample_rate: f32,
    ) -> f32 {
        let frequency = 300.0 * (sweep_value(phase) * sweep).exp();
        let frequency = frequency.clamp(300.0, (sample_rate * 0.45).max(301.0));
        let tangent = (std::f32::consts::PI * frequency / sample_rate).tan();
        let coefficient = ((1.0 - tangent) / (1.0 + tangent)).clamp(-0.98, 0.98);
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
    ) -> (f32, f32) {
        let left_delay = self.flanger_delay_samples_cached(self.flanger_phase);
        let right_delay = self.flanger_delay_samples_cached((self.flanger_phase + 0.5).fract());
        let delayed_l = Self::read_delay(&self.flanger_l, self.flanger_write, left_delay);
        let delayed_r = Self::read_delay(&self.flanger_r, self.flanger_write, right_delay);
        self.flanger_l[self.flanger_write] =
            finite_or_zero(input_l + delayed_l * controls.feedback);
        self.flanger_r[self.flanger_write] =
            finite_or_zero(input_r + delayed_r * controls.feedback);
        self.flanger_write = (self.flanger_write + 1) % FLANGER_BUFFER_SIZE;
        self.flanger_phase = (self.flanger_phase + controls.rate / self.sample_rate).fract();
        (
            finite_or_zero(input_l * (1.0 - controls.mix) + delayed_l * controls.mix),
            finite_or_zero(input_r * (1.0 - controls.mix) + delayed_r * controls.mix),
        )
    }

    #[cfg(test)]
    fn flanger_delay_samples(&self, center_ms: f32, depth_ms: f32, phase: f32) -> f32 {
        let delay_ms = center_ms + (std::f32::consts::TAU * phase).sin() * depth_ms;
        (delay_ms.max(0.1) * self.sample_rate / 1_000.0)
            .clamp(1.0, (self.flanger_l.len() - 2) as f32)
    }

    fn flanger_delay_samples_cached(&self, phase: f32) -> f32 {
        (self.flanger_center_samples
            + self.flanger_depth_samples * (std::f32::consts::TAU * phase).sin())
        .max(1.0)
        .clamp(1.0, (self.flanger_l.len() - 2) as f32)
    }

    fn tail_length(&self) -> u32 {
        (self.sample_rate * 0.25).round().max(1.0) as u32
    }

    fn has_pending_parameters(&self) -> bool {
        self.distortion_mix.is_smoothing()
            || self.phaser_mix.is_smoothing()
            || self.flanger_mix.is_smoothing()
            || self.distortion_mix.target > 0.0
            || self.phaser_mix.target > 0.0
            || self.flanger_mix.target > 0.0
            || self.distortion_active
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
            self.flanger_center_samples =
                (0.2 + delay.clamp(0.0, 100.0) * 0.098) * self.sample_rate / 1_000.0;
            self.flanger_depth_samples =
                depth.clamp(0.0, 100.0) * 0.05 * self.sample_rate / 1_000.0;
        }
    }

    pub fn is_active(&self) -> bool {
        self.distortion_active || self.phaser_active || self.flanger_active
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

    pub fn clear(&mut self) {
        self.distortion_state_l = 0.0;
        self.distortion_state_r = 0.0;
        self.distortion_active = false;
        self.phaser_active = false;
        self.flanger_active = false;
        self.distortion_tail_remaining = 0;
        self.phaser_tail_remaining = 0;
        self.flanger_tail_remaining = 0;
        self.phaser_phase = 0.0;
        self.phaser_feedback_l = 0.0;
        self.phaser_feedback_r = 0.0;
        for stage in self.phaser_l.iter_mut().chain(self.phaser_r.iter_mut()) {
            stage.clear();
        }
        self.flanger_phase = 0.0;
        self.flanger_write = 0;
        self.flanger_l.fill(0.0);
        self.flanger_r.fill(0.0);
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
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
    Juno,
    Sh101,
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
    pub fn next_sample(&mut self) -> f32 {
        self.next_sample_modulated(0.0, 0.0, 0.0, 0.0)
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
            EnvelopeProfile::Juno => (0.001, 3.0, 0.002, 12.0, 0.002, 12.0),
            EnvelopeProfile::Sh101 => (0.0015, 4.0, 0.002, 10.0, 0.002, 10.0),
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

/// The 303's amplifier is a gate, rather than a second copy of its filter
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

    pub fn value(&self) -> f32 {
        self.value
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

pub struct Svf {
    ic1: f32,
    ic2: f32,
}
impl Svf {
    pub fn new() -> Self {
        Self { ic1: 0.0, ic2: 0.0 }
    }
    pub fn lowpass(&mut self, input: f32, cutoff: f32, q: f32, sr: f32) -> f32 {
        let g = (PI * (cutoff / sr).clamp(0.0001, 0.45)).tan();
        let k = 1.0 / q.clamp(0.1, 20.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let v1 = a1 * (self.ic1 + g * (input - self.ic2));
        let v2 = self.ic2 + g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        if v2.is_finite() {
            v2
        } else {
            self.ic1 = 0.0;
            self.ic2 = 0.0;
            0.0
        }
    }
}
impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

/// A compact nonlinear four-stage ladder used by the Chord and Lead voices.
pub struct LadderFilter {
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

impl LadderFilter {
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

    pub fn lowpass(&mut self, input: f32, cutoff: f32, resonance: f32, sr: f32) -> f32 {
        self.set_parameters(cutoff, resonance, sr);
        self.process(input)
    }

    pub fn set_parameters(&mut self, cutoff: f32, resonance: f32, sr: f32) {
        self.set_parameters_smoothed(cutoff, resonance, sr, 0);
    }

    pub fn set_parameters_smoothed(&mut self, cutoff: f32, resonance: f32, sr: f32, samples: u8) {
        if cutoff == self.cached_cutoff
            && resonance == self.cached_resonance
            && sr == self.cached_sr
        {
            return;
        }
        let initialize = !self.cached_sr.is_finite() || samples == 0;
        self.cached_cutoff = cutoff;
        self.cached_resonance = resonance;
        self.cached_sr = sr;
        let target_coefficient = 1.0 - (-2.0 * PI * cutoff.clamp(20.0, sr * 0.45) / sr).exp();
        let target_feedback = resonance.clamp(0.0, 1.0) * 3.85;
        if initialize {
            self.coefficient = target_coefficient;
            self.feedback = target_feedback;
            self.parameter_smoothing_remaining = 0;
        } else {
            self.coefficient_step = (target_coefficient - self.coefficient) / f32::from(samples);
            self.feedback_step = (target_feedback - self.feedback) / f32::from(samples);
            self.parameter_smoothing_remaining = samples;
        }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        if self.parameter_smoothing_remaining > 0 {
            self.coefficient += self.coefficient_step;
            self.feedback += self.feedback_step;
            self.parameter_smoothing_remaining -= 1;
        }
        let mut stage_input = (input - self.stages[3] * self.feedback).tanh();
        for stage in &mut self.stages {
            *stage += self.coefficient * (stage_input - stage.tanh());
            stage_input = stage.tanh();
        }
        let output = self.stages[3];
        if output.is_finite() {
            output
        } else {
            self.stages = [0.0; 4];
            0.0
        }
    }
}

impl Default for LadderFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// A three-pole, 18 dB/octave, transistor-ladder-inspired low-pass filter for
/// the Bass voice.  It is intended to be driven at 2x the host sample rate.
/// Parameter updates are cached and interpolated at control rate so cutoff
/// changes do not produce zipper noise in the audio callback.
pub struct Tb303Filter {
    stages: [f32; 3],
    coefficient: f32,
    feedback: f32,
    passband_gain: f32,
    coefficient_step: f32,
    feedback_step: f32,
    passband_gain_step: f32,
    parameter_smoothing_remaining: u8,
    cached_cutoff: f32,
    cached_resonance: f32,
    cached_sr: f32,
}

impl Tb303Filter {
    pub fn new() -> Self {
        Self {
            stages: [0.0; 3],
            coefficient: 0.0,
            feedback: 0.0,
            passband_gain: 1.0,
            coefficient_step: 0.0,
            feedback_step: 0.0,
            passband_gain_step: 0.0,
            parameter_smoothing_remaining: 0,
            cached_cutoff: f32::NAN,
            cached_resonance: f32::NAN,
            cached_sr: f32::NAN,
        }
    }

    pub fn reset(&mut self) {
        self.stages = [0.0; 3];
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
        let passband_gain = 1.0 / (1.0 + resonance * 0.32);
        let initialize = !self.cached_sr.is_finite() || samples == 0;
        self.cached_cutoff = cutoff;
        self.cached_resonance = resonance;
        self.cached_sr = sr;
        if initialize {
            self.coefficient = coefficient;
            self.feedback = feedback;
            self.passband_gain = passband_gain;
            self.parameter_smoothing_remaining = 0;
        } else {
            let samples = f32::from(samples);
            self.coefficient_step = (coefficient - self.coefficient) / samples;
            self.feedback_step = (feedback - self.feedback) / samples;
            self.passband_gain_step = (passband_gain - self.passband_gain) / samples;
            self.parameter_smoothing_remaining = samples as u8;
        }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        if self.parameter_smoothing_remaining > 0 {
            self.coefficient += self.coefficient_step;
            self.feedback += self.feedback_step;
            self.passband_gain += self.passband_gain_step;
            self.parameter_smoothing_remaining -= 1;
        }
        let mut stage_input = (input * 1.25 - self.stages[2] * self.feedback).tanh();
        for stage in &mut self.stages {
            *stage += self.coefficient * (stage_input - stage.tanh());
            stage_input = stage.tanh();
        }
        let output = self.stages[2] * self.passband_gain;
        if output.is_finite() && self.stages.iter().all(|state| state.is_finite()) {
            output
        } else {
            self.reset();
            0.0
        }
    }
}

impl Default for Tb303Filter {
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

    fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

/// A short, modulated stereo delay inspired by the fixed Juno chorus modes.
/// Storage is allocated when the renderer is built, never in the audio callback.
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
        let wet = std::f32::consts::FRAC_1_SQRT_2;
        let left = self.tap(base + modulation);
        let right = self.tap(base - modulation);
        (input * wet + left * wet, input * wet + right * wet)
    }

    pub fn process(&mut self, input: f32) -> (f32, f32) {
        self.process_stereo(input, input)
    }

    pub fn process_stereo(&mut self, left_input: f32, right_input: f32) -> (f32, f32) {
        let input_peak = safety(left_input).abs().max(safety(right_input).abs());
        if !self.active && self.fade_remaining == 0 && input_peak <= SILENCE_THRESHOLD {
            return (left_input, right_input);
        }
        if input_peak > SILENCE_THRESHOLD {
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
        self.highpass_l.clear();
        self.highpass_r.clear();
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
mod tests {
    use super::*;
    #[test]
    fn osc_bounded() {
        let mut o = PolyBlepOsc::default();
        for _ in 0..10000 {
            assert!(o.next_saw(440., 48000.).abs() <= 1.1);
        }
    }
    #[test]
    fn filter_finite() {
        let mut f = Svf::new();
        for c in [20., 20000., 30000.] {
            for _ in 0..1000 {
                assert!(f.lowpass(1., c, 10., 44100.).is_finite())
            }
        }
    }
    #[test]
    fn drum_biquad_filters_are_finite_and_reject_dc() {
        for configure in [
            Biquad::set_bandpass as fn(&mut Biquad, f32, f32, f32),
            Biquad::set_highpass,
        ] {
            let mut filter = Biquad::new();
            configure(&mut filter, 6_000.0, 5.6, 48_000.0);
            let mut output = 0.0;
            for _ in 0..48_000 {
                output = filter.process(1.0);
                assert!(output.is_finite());
            }
            assert!(output.abs() < 0.0001);
        }
    }
    #[test]
    fn reverb_highpass_rejects_dc_after_the_initial_transient() {
        let mut reverb = Reverb::new(8_000);
        let mut output = (0.0, 0.0);
        for _ in 0..40_000 {
            output = reverb.process(1.0, 1.0);
            assert!(output.0.is_finite() && output.1.is_finite());
        }
        assert!(output.0.abs().max(output.1.abs()) < 0.0001, "{output:?}");
    }
    #[test]
    fn delay_exact() {
        let mut d = Delay::new(100);
        d.configure(10, 0.);
        d.process(1., 0.);
        for _ in 0..9 {
            assert_eq!(d.process(0., 0.).0, 0.)
        }
        assert_eq!(d.process(0., 0.).0, 1.);
    }

    #[test]
    fn silent_effects_do_not_advance_when_inactive() {
        let mut chorus = StereoChorus::new(1_000);
        chorus.process(0.0);
        assert_eq!(chorus.pos, 0);

        let mut delay = Delay::new(1_000);
        delay.process(0.0, 0.0);
        assert_eq!(delay.pos, 0);

        let mut reverb = Reverb::new(1_000);
        reverb.process(0.0, 0.0);
        assert_eq!(reverb.pre_delay_pos, 0);
    }
    #[test]
    fn nonfinite_safe() {
        assert_eq!(safety(f32::NAN), 0.0);
        assert_eq!(safety(100.0), 100.0)
    }
    #[test]
    fn master_limiter_is_stereo_linked_and_respects_ceiling() {
        let mut limiter = MasterLimiter::new(1_000);
        let mut peak = 0.0_f32;
        let mut linked = false;
        for i in 0..500 {
            let input = if i == 20 { (8.0, 0.25) } else { (0.25, 0.25) };
            let (l, r) = limiter.process(input.0, input.1);
            peak = peak.max(l.abs()).max(r.abs());
            linked |= (l - r).abs() < 0.0001 && l.abs() > 0.01;
        }
        assert!(peak <= 0.891_251);
        assert!(linked);
    }
    struct NaiveLimiter {
        left: Vec<f32>,
        right: Vec<f32>,
        pos: usize,
        gain: f32,
        attack_coefficient: f32,
        release_coefficient: f32,
    }

    impl NaiveLimiter {
        fn new(sample_rate: u32) -> Self {
            let lookahead = ((sample_rate as f32 * 0.005).round() as usize).max(1);
            Self {
                left: vec![0.0; lookahead],
                right: vec![0.0; lookahead],
                pos: 0,
                gain: 1.0,
                attack_coefficient: (-1.0 / (sample_rate as f32 * 0.001)).exp(),
                release_coefficient: (-1.0 / (sample_rate as f32 * 0.080)).exp(),
            }
        }

        fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
            const MAKEUP: f32 = 1.995_262_3;
            const CEILING: f32 = 0.891_250_9;
            let delayed_l = self.left[self.pos];
            let delayed_r = self.right[self.pos];
            self.left[self.pos] = safety(l);
            self.right[self.pos] = safety(r);
            self.pos = (self.pos + 1) % self.left.len();
            let peak = self
                .left
                .iter()
                .chain(&self.right)
                .fold(0.0_f32, |peak, sample| peak.max((sample * MAKEUP).abs()));
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

        fn clear(&mut self) {
            self.left.fill(0.0);
            self.right.fill(0.0);
            self.pos = 0;
            self.gain = 1.0;
        }
    }

    #[test]
    fn master_limiter_matches_naive_sliding_window() {
        for sample_rate in [8_000, 44_100, 48_000] {
            let mut optimized = MasterLimiter::new(sample_rate);
            let mut reference = NaiveLimiter::new(sample_rate);
            let window = ((sample_rate as f32 * 0.005).round() as usize).max(1);
            for i in 0..(window * 4 + 19) {
                let input = match i {
                    0..=2 => (0.0, 0.0),
                    3 => (8.0, 0.25),
                    4 => (0.1, 0.9),
                    i if (5..=window + 2).contains(&i) => (0.35, 0.35),
                    _ if i % 11 == 0 => (-0.7, 0.2),
                    _ => (0.0, 0.0),
                };
                let actual = optimized.process(input.0, input.1);
                let expected = reference.process(input.0, input.1);
                assert!((actual.0 - expected.0).abs() < 0.000001, "left at {i}");
                assert!((actual.1 - expected.1).abs() < 0.000001, "right at {i}");

                if i == window + 7 {
                    optimized.clear();
                    reference.clear();
                }
            }
        }
    }

    #[test]
    fn smoother_reaches_target_without_jump() {
        let mut s = Smoother::new(0.0);
        s.set(1.0, 10);
        let values = (0..10).map(|_| s.next_value()).collect::<Vec<_>>();
        assert!((values[0] - 0.1).abs() < 0.0001);
        assert!((values[9] - 1.0).abs() < 0.0001);
        assert!(values.windows(2).all(|w| w[1] >= w[0]));
    }
    #[test]
    fn comb_feedback_reaches_minus_sixty_db_at_requested_time() {
        for sample_rate in [8_000.0_f32, 44_100.0, 48_000.0] {
            for time in [0.2_f32, 2.5, 10.0] {
                let delay_samples = (sample_rate * 0.025).round() as usize;
                let mut comb = Comb::new(delay_samples);
                comb.set_time(time, sample_rate, 0);
                let delay_seconds = comb.buffer.len() as f32 / sample_rate;
                let loops = time / delay_seconds;
                let decay = comb.feedback.current.powf(loops);
                assert!(
                    (decay - 0.001).abs() < 0.00001,
                    "{sample_rate} Hz, {time} s"
                );
            }
        }
    }
    #[test]
    fn comb_feedback_changes_smoothly() {
        let mut comb = Comb::new(100);
        comb.set_time(0.2, 1_000.0, 0);
        let initial = comb.feedback.current;
        comb.set_time(10.0, 1_000.0, 10);
        let target = comb.feedback.target;

        comb.process(0.0, 0.25);
        assert!(comb.feedback.current > initial && comb.feedback.current < target);
        for _ in 1..10 {
            comb.process(0.0, 0.25);
        }
        assert_eq!(comb.feedback.current, target);
    }
    #[test]
    fn longer_reverb_times_have_more_late_tail_energy() {
        fn late_energy(time: f32) -> f32 {
            let mut reverb = Reverb::new(8_000);
            reverb.set_time(time);
            (0..8_000)
                .map(|i| {
                    let input = if i == 0 { 0.1 } else { 0.0 };
                    let (l, r) = reverb.process(input, input);
                    if i >= 4_000 { l * l + r * r } else { 0.0 }
                })
                .sum()
        }

        let short = late_energy(0.2);
        let default = late_energy(2.5);
        let long = late_energy(10.0);
        assert!(
            short < default && default < long,
            "{short}, {default}, {long}"
        );
    }
    #[test]
    fn reverb_is_finite_and_bounded_at_time_extremes() {
        for time in [0.2, 10.0] {
            let mut reverb = Reverb::new(8_000);
            reverb.set_time(time);
            for i in 0..96_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(input, input);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
            }
        }
    }

    #[test]
    fn reverb_pre_delay_shifts_the_wet_onset() {
        fn first_output(pre_delay_ms: u16) -> usize {
            let mut reverb = Reverb::new(8_000);
            reverb.set_pre_delay_ms(pre_delay_ms);
            for _ in 0..64 {
                reverb.process(0.0, 0.0);
            }
            reverb.clear();
            for i in 0..2_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(input, input);
                if l.abs().max(r.abs()) > 0.000_001 {
                    return i;
                }
            }
            panic!("reverb did not produce an output");
        }

        let immediate = first_output(0);
        let delayed = first_output(20);
        assert!((delayed as isize - immediate as isize - 160).abs() <= 1);
    }

    #[test]
    fn brighter_reverb_has_more_late_tail_energy() {
        fn late_energy(tone: f32) -> f32 {
            let mut reverb = Reverb::new(8_000);
            reverb.set_tone(tone);
            (0..8_000)
                .map(|i| {
                    let input = if i == 0 { 0.1 } else { 0.0 };
                    let (l, r) = reverb.process(input, input);
                    if i >= 4_000 { l * l + r * r } else { 0.0 }
                })
                .sum()
        }

        assert!(late_energy(1.0) > late_energy(0.0));
    }

    #[test]
    fn lfo_rate_phase_reset_and_randomness_are_deterministic() {
        use crate::model::{LfoRate, Percent};

        let config = LfoConfig {
            rate: LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            },
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        };
        let mut lfo = Lfo::new(7);
        for _ in 0..50 {
            assert!(lfo.next(Some(config), 120, 1_000.0).is_finite());
        }
        assert!(lfo.phase.min(1.0 - lfo.phase) < 0.0001, "{}", lfo.phase);
        lfo.reset();
        assert_eq!(lfo.phase, 0.0);
        assert_eq!(lfo.value(), 0.0);

        let random = LfoConfig {
            waveform: LfoWaveform::SampleAndHold,
            ..config
        };
        let mut first = Lfo::new(99);
        let mut second = Lfo::new(99);
        for _ in 0..200 {
            assert_eq!(
                first.next(Some(random), 120, 1_000.0),
                second.next(Some(random), 120, 1_000.0)
            );
        }
    }

    #[test]
    fn all_lfo_waveforms_remain_bipolar_and_finite() {
        for waveform in LfoWaveform::ALL {
            let config = LfoConfig {
                waveform,
                ..Default::default()
            };
            let mut lfo = Lfo::new(123);
            for _ in 0..48_000 {
                let value = lfo.next(Some(config), 240, 48_000.0);
                assert!(value.is_finite() && (-1.0..=1.0).contains(&value));
            }
        }
    }

    #[test]
    fn pulse_width_extremes_are_band_limited_and_finite() {
        for width in [0.05, 0.5, 0.95] {
            let mut oscillator = PolyBlepOsc::default();
            for _ in 0..48_000 {
                let (_, pulse) = oscillator.next_saw_pulse(440.0, width, 48_000.0);
                assert!(pulse.is_finite() && pulse.abs() <= 2.0);
            }
        }
    }

    #[test]
    fn juno_chorus_modes_are_deterministic_finite_and_stereo() {
        let render = || {
            let mut chorus = StereoChorus::new(48_000);
            chorus.configure(2);
            (0..4_000)
                .map(|sample| chorus.process(if sample == 0 { 1.0 } else { 0.0 }))
                .collect::<Vec<_>>()
        };
        let first = render();
        let second = render();
        assert_eq!(first, second);
        assert!(
            first
                .iter()
                .all(|(left, right)| left.is_finite() && right.is_finite())
        );
        assert!(
            first
                .iter()
                .any(|(left, right)| (left - right).abs() > 0.000_001)
        );
    }

    #[test]
    fn juno_chorus_handles_float_wrap_at_44100_hz() {
        for mode in 1..=2 {
            let mut chorus = StereoChorus::new(44_100);
            chorus.configure(mode);
            for sample in 0..200_000 {
                let (left, right) = chorus.process(if sample % 127 == 0 { 1.0 } else { 0.0 });
                assert!(left.is_finite() && right.is_finite());
            }
        }
    }

    #[test]
    fn juno_chorus_tap_handles_a_near_zero_negative_read() {
        let mut chorus = StereoChorus::new(44_100);
        chorus.pos = 0;
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| chorus.tap(0.00001))).is_ok()
        );
    }

    #[test]
    fn track_effect_chain_bypasses_exactly_and_remains_finite() {
        let mut chain = TrackEffectChain::new(48_000);
        chain.configure(TrackEffects::default(), ParameterLocks::default(), 0);
        for sample in [0.0, 0.1, -0.7, 1.0] {
            assert_eq!(chain.process(sample), (sample, sample));
        }
        assert_eq!(chain.process_stereo(0.25, -0.75), (0.25, -0.75));

        let effects = TrackEffects {
            distortion: crate::model::DistortionParameters {
                drive: crate::model::Percent::new(100).unwrap(),
                tone: crate::model::Percent::new(100).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
            phaser: crate::model::PhaserParameters {
                rate: crate::model::Percent::new(100).unwrap(),
                depth: crate::model::Percent::new(100).unwrap(),
                feedback: crate::model::Percent::new(90).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
            flanger: crate::model::FlangerParameters {
                rate: crate::model::Percent::new(100).unwrap(),
                delay: crate::model::Percent::new(100).unwrap(),
                depth: crate::model::Percent::new(100).unwrap(),
                feedback: crate::model::Percent::new(90).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
        };
        chain.configure(effects, ParameterLocks::default(), 0);
        let mut stereo = false;
        for sample in 0..48_000 {
            let (left, right) = chain.process(if sample == 0 { 1.0 } else { 0.0 });
            assert!(left.is_finite() && right.is_finite());
            stereo |= (left - right).abs() > 0.000_001;
        }
        assert!(stereo);
    }

    #[test]
    fn disabling_flanger_drains_feedback_without_restoring_wet_mix() {
        let mut chain = TrackEffectChain::new(8_000);
        let mut effects = TrackEffects::default();
        effects.flanger.feedback = crate::model::Percent::new(90).unwrap();
        effects.flanger.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.flanger_l.fill(0.75);
        chain.flanger_r.fill(-0.75);
        chain.flanger_tail_remaining = chain.tail_length();

        chain.configure(TrackEffects::default(), ParameterLocks::default(), 1);
        let output = chain.process_stereo(0.0, 0.0);

        assert_eq!(chain.flanger_mix.value(), 0.0);
        assert!(chain.flanger_active, "feedback should drain internally");
        assert_eq!(output, (0.0, 0.0), "zero mix must remain an exact bypass");
    }

    #[test]
    fn disabling_phaser_drains_feedback_without_restoring_wet_mix() {
        let mut chain = TrackEffectChain::new(8_000);
        let mut effects = TrackEffects::default();
        effects.phaser.feedback = crate::model::Percent::new(90).unwrap();
        effects.phaser.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.phaser_feedback_l = 0.75;
        chain.phaser_feedback_r = -0.5;
        chain.phaser_l[0].state = 0.5;
        chain.phaser_r[0].state = -0.5;
        chain.phaser_tail_remaining = chain.tail_length();

        chain.configure(TrackEffects::default(), ParameterLocks::default(), 1);
        let output = chain.process_stereo(0.0, 0.0);

        assert_eq!(chain.phaser_mix.value(), 0.0);
        assert!(chain.phaser_active, "feedback should drain internally");
        assert_eq!(output, (0.0, 0.0), "zero mix must remain an exact bypass");
    }

    #[test]
    fn flanger_delay_is_at_least_one_sample_at_low_sample_rate() {
        let mut chain = TrackEffectChain::new(100);
        let delay = chain.flanger_delay_samples(0.2, 0.0, 0.0);
        assert_eq!(delay, 1.0);

        let last = chain.flanger_l.len() - 1;
        chain.flanger_l[last] = 0.25;
        chain.flanger_l[0] = 4.0;
        assert_eq!(
            TrackEffectChain::read_delay(&chain.flanger_l, 0, delay),
            0.25
        );
    }

    #[test]
    fn track_effect_chain_is_deterministic() {
        let effects = TrackEffects {
            distortion: crate::model::DistortionParameters {
                drive: crate::model::Percent::new(60).unwrap(),
                tone: crate::model::Percent::new(30).unwrap(),
                mix: crate::model::Percent::new(75).unwrap(),
            },
            phaser: crate::model::PhaserParameters {
                rate: crate::model::Percent::new(25).unwrap(),
                depth: crate::model::Percent::new(50).unwrap(),
                feedback: crate::model::Percent::new(80).unwrap(),
                mix: crate::model::Percent::new(60).unwrap(),
            },
            flanger: crate::model::FlangerParameters {
                rate: crate::model::Percent::new(35).unwrap(),
                delay: crate::model::Percent::new(18).unwrap(),
                depth: crate::model::Percent::new(70).unwrap(),
                feedback: crate::model::Percent::new(65).unwrap(),
                mix: crate::model::Percent::new(55).unwrap(),
            },
        };
        let mut first = TrackEffectChain::new(8_000);
        let mut second = TrackEffectChain::new(8_000);
        first.configure(effects, ParameterLocks::default(), 0);
        second.configure(effects, ParameterLocks::default(), 0);
        for sample in 0..10_000 {
            let input = ((sample as f32) * 0.017).sin() * 0.4;
            assert_eq!(first.process(input), second.process(input));
        }
    }

    #[test]
    fn sidechain_zero_depth_is_unity() {
        let mut compressor = SidechainCompressor::new(48_000);
        let parameters = SidechainParameters {
            depth: crate::model::Percent::ZERO,
            ..SidechainParameters::default()
        };
        compressor.configure(parameters);
        for _ in 0..100 {
            assert_eq!(compressor.process_stereo(1.0, 0.5), 1.0);
        }
    }

    #[test]
    fn sidechain_attack_and_release_follow_peak_envelope() {
        let mut compressor = SidechainCompressor::new(1_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(0).unwrap(),
            release: crate::model::Percent::new(0).unwrap(),
        });
        let first = compressor.process_stereo(1.0, -0.5);
        assert!(compressor.envelope() > 0.0);
        assert!(first < 1.0);
        let ducked = (0..100)
            .map(|_| compressor.process_stereo(1.0, 1.0))
            .last()
            .unwrap();
        assert!(ducked < 0.2);
        let before_release = compressor.envelope();
        for _ in 0..100 {
            compressor.process_stereo(0.0, 0.0);
        }
        assert!(compressor.envelope() < before_release);
        assert!(compressor.current_gain() > ducked);
    }

    #[test]
    fn sidechain_maximum_attenuation_reset_and_finite_output() {
        let mut compressor = SidechainCompressor::new(48_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(100).unwrap(),
            release: crate::model::Percent::new(100).unwrap(),
        });
        for _ in 0..100_000 {
            compressor.process_stereo(2.0, -2.0);
        }
        assert!((compressor.current_gain() - 10.0_f32.powf(-18.0 / 20.0)).abs() < 0.0001);
        assert!(
            compressor
                .process_stereo(f32::NAN, f32::INFINITY)
                .is_finite()
        );
        compressor.reset();
        assert_eq!(compressor.envelope(), 0.0);
        assert_eq!(compressor.current_gain(), 10.0_f32.powf(-0.0));
    }

    #[test]
    fn sidechain_recovers_after_nonfinite_detector_input() {
        let mut compressor = SidechainCompressor::new(48_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(0).unwrap(),
            ..SidechainParameters::default()
        });
        assert_eq!(compressor.process_stereo(f32::NAN, f32::NAN), 1.0);
        assert_eq!(compressor.envelope(), 0.0);
        for _ in 0..1_000 {
            compressor.process_stereo(1.0, 1.0);
        }
        assert!(compressor.envelope() > 0.99);
        assert!(compressor.current_gain() < 0.13);
    }

    #[test]
    fn bass_vca_holds_after_its_independent_filter_contour_has_decayed() {
        let mut vca = BassVcaEnvelope::new(8_000.0);
        let mut contour = BassFilterEnvelope::new(8_000.0);
        vca.gate_on();
        contour.trigger(0.0);
        for _ in 0..1_600 {
            vca.next_sample();
            contour.next_sample();
        }
        assert!(vca.value() > 0.99, "a held Bass gate must remain audible");
        assert!(
            contour.value() < 0.001,
            "minimum decay should only close the filter contour"
        );
    }

    #[test]
    fn bass_vca_releases_with_fixed_timing() {
        let mut vca = BassVcaEnvelope::new(8_000.0);
        vca.gate_on();
        for _ in 0..80 {
            vca.next_sample();
        }
        vca.gate_off();
        for _ in 0..500 {
            vca.next_sample();
        }
        assert!(vca.is_idle());
    }

    #[test]
    fn bass_accent_retriggers_smoothly_then_decays() {
        let mut accent = BassAccentEnvelope::new(8_000.0);
        accent.trigger(true);
        let first = accent.next_sample();
        assert!(first > 0.0 && first < 1.0);
        for _ in 0..40 {
            accent.next_sample();
        }
        let peak = accent.value();
        assert!(peak > 0.7);
        for _ in 0..160 {
            accent.next_sample();
        }
        let decaying = accent.value();
        assert!(decaying > 0.1 && decaying < peak);

        accent.trigger(true);
        let retriggered = accent.next_sample();
        assert!(retriggered > decaying && retriggered < 1.0);
        for _ in 0..1_600 {
            accent.next_sample();
        }
        assert!(accent.value() < 0.001, "accent contour must not sustain");
    }

    #[test]
    fn tb303_filter_is_finite_at_extreme_parameters_and_sample_rates() {
        for sample_rate in [8_000.0, 44_100.0, 96_000.0] {
            for cutoff in [20.0, sample_rate * 0.42] {
                for resonance in [0.0, 1.0] {
                    let mut filter = Tb303Filter::new();
                    filter.set_parameters_smoothed(cutoff, resonance, sample_rate * 2.0, 0);
                    for sample in 0..20_000 {
                        let input = ((sample as f32) * 0.071).sin() * 2.0;
                        assert!(filter.process(input).is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn tb303_filter_has_three_pole_lowpass_rolloff() {
        fn rms_at(frequency: f32) -> f32 {
            let sample_rate = 48_000.0;
            let mut filter = Tb303Filter::new();
            filter.set_parameters_smoothed(250.0, 0.0, sample_rate, 0);
            let mut energy = 0.0;
            let mut count = 0;
            for sample in 0..48_000 {
                let input = (std::f32::consts::TAU * frequency * sample as f32 / sample_rate).sin();
                let output = filter.process(input);
                if sample > 24_000 {
                    energy += output * output;
                    count += 1;
                }
            }
            (energy / count as f32).sqrt()
        }

        let low = rms_at(80.0);
        let high = rms_at(640.0);
        let attenuation_db = 20.0 * (high / low).log10();
        assert!(
            attenuation_db < -15.0,
            "expected 18 dB/octave-like attenuation, got {attenuation_db:.1} dB"
        );
    }
}
