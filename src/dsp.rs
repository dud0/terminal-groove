use std::f32::consts::PI;

use crate::model::{LfoConfig, LfoWaveform, ParameterLocks, TrackEffects};

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

#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    phase: f32,
    held: f32,
    smoothed: f32,
    rng: u32,
    seed: u32,
    active: bool,
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
        let smoothing = 1.0 - (-1.0 / (sample_rate * 0.005).max(1.0)).exp();
        self.smoothed += (raw - self.smoothed) * smoothing;
        let increment = config.rate.hz(tempo_bpm) / sample_rate;
        self.phase += increment.max(0.0);
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

#[derive(Clone, Copy, Debug)]
pub struct TrackEffectChain {
    distortion_drive: Smoother,
    distortion_tone: Smoother,
    distortion_mix: Smoother,
    distortion_state: f32,
    phaser_rate: Smoother,
    phaser_depth: Smoother,
    phaser_feedback: Smoother,
    phaser_mix: Smoother,
    phaser_phase: f32,
    phaser_feedback_l: f32,
    phaser_feedback_r: f32,
    phaser_l: [EffectAllpass; 4],
    phaser_r: [EffectAllpass; 4],
    sample_rate: f32,
}

impl TrackEffectChain {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            distortion_drive: Smoother::new(0.0),
            distortion_tone: Smoother::new(50.0),
            distortion_mix: Smoother::new(0.0),
            distortion_state: 0.0,
            phaser_rate: Smoother::new(25.0),
            phaser_depth: Smoother::new(50.0),
            phaser_feedback: Smoother::new(20.0),
            phaser_mix: Smoother::new(0.0),
            phaser_phase: 0.0,
            phaser_feedback_l: 0.0,
            phaser_feedback_r: 0.0,
            phaser_l: [EffectAllpass::new(); 4],
            phaser_r: [EffectAllpass::new(); 4],
            sample_rate: sample_rate as f32,
        }
    }

    pub fn configure(&mut self, effects: TrackEffects, locks: ParameterLocks, samples: u32) {
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
    }

    pub fn process(&mut self, input: f32) -> (f32, f32) {
        let drive = self.distortion_drive.next_value();
        let tone = self.distortion_tone.next_value();
        let distortion_mix = self.distortion_mix.next_value() / 100.0;
        let phaser_rate = self.phaser_rate.next_value();
        let phaser_depth = self.phaser_depth.next_value();
        let phaser_feedback = (self.phaser_feedback.next_value() / 100.0).clamp(0.0, 0.9);
        let phaser_mix = self.phaser_mix.next_value() / 100.0;
        if distortion_mix <= 0.0 && phaser_mix <= 0.0 {
            return (input, input);
        }

        let gain = exp_map_f32(drive, 1.0, 31.622_776);
        let clipped = (input * gain).tanh();
        let cutoff = exp_map_f32(tone, 700.0, (18_000.0_f32).min(self.sample_rate * 0.45));
        let coefficient = 1.0 - (-std::f32::consts::TAU * cutoff / self.sample_rate).exp();
        self.distortion_state += (clipped - self.distortion_state) * coefficient;
        let distorted = input * (1.0 - distortion_mix) + self.distortion_state * distortion_mix;

        if phaser_mix <= 0.0 {
            return (distorted, distorted);
        }
        let sweep = (8_000.0_f32 / 300.0).ln() * phaser_depth / 100.0;
        let rate = exp_map_f32(phaser_rate, 0.05, 8.0);
        let feedback = (self.phaser_feedback_l + self.phaser_feedback_r) * 0.5 * phaser_feedback;
        let left_input = distorted + feedback;
        let right_input = distorted - feedback;
        let left = Self::phaser_sample(
            &mut self.phaser_l,
            left_input,
            self.phaser_phase,
            sweep,
            self.sample_rate,
        );
        let right = Self::phaser_sample(
            &mut self.phaser_r,
            right_input,
            (self.phaser_phase + 0.5).fract(),
            sweep,
            self.sample_rate,
        );
        self.phaser_feedback_l = left;
        self.phaser_feedback_r = right;
        self.phaser_phase = (self.phaser_phase + rate / self.sample_rate).fract();
        (
            distorted * (1.0 - phaser_mix) + left * phaser_mix,
            distorted * (1.0 - phaser_mix) + right * phaser_mix,
        )
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

    pub fn clear(&mut self) {
        self.distortion_state = 0.0;
        self.phaser_phase = 0.0;
        self.phaser_feedback_l = 0.0;
        self.phaser_feedback_r = 0.0;
        for stage in self.phaser_l.iter_mut().chain(self.phaser_r.iter_mut()) {
            stage.clear();
        }
    }
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
        }
    }
    pub fn set_profile(&mut self, profile: EnvelopeProfile) {
        self.profile = profile;
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
        let attack_percent = (self.attack_percent.next_value() + attack_offset).clamp(0.0, 100.0);
        let decay_percent = (self.decay_percent.next_value() + decay_offset).clamp(0.0, 100.0);
        let sustain =
            (self.sustain_percent.next_value() + sustain_offset).clamp(0.0, 100.0) / 100.0;
        let release_percent =
            (self.release_percent.next_value() + release_offset).clamp(0.0, 100.0);
        let (attack_min, attack_max, decay_min, decay_max, release_min, release_max) =
            match self.profile {
                EnvelopeProfile::Generic => (0.001, 2.0, 0.005, 3.0, 0.005, 5.0),
                EnvelopeProfile::Juno => (0.001, 3.0, 0.002, 12.0, 0.002, 12.0),
                EnvelopeProfile::Sh101 => (0.0015, 4.0, 0.002, 10.0, 0.002, 10.0),
            };
        let attack = if attack_percent == 0.0 {
            0.0
        } else {
            exp_map_f32(attack_percent, attack_min, attack_max)
        };
        let decay = exp_map_f32(decay_percent, decay_min, decay_max);
        let release = exp_map_f32(release_percent, release_min, release_max);
        match self.stage {
            EnvStage::Idle => {}
            EnvStage::Attack => {
                if attack <= 0.0 {
                    self.value = 1.0;
                    self.stage = EnvStage::Decay
                } else {
                    let coefficient = 1.0 - (-6.907_755 / (attack * self.sr).max(1.0)).exp();
                    self.value += (1.0 - self.value) * coefficient;
                    if self.value >= 0.999 {
                        self.value = 1.0;
                        self.stage = EnvStage::Decay
                    }
                }
            }
            EnvStage::Decay => {
                let coefficient = 1.0 - (-6.907_755 / (decay * self.sr).max(1.0)).exp();
                self.value += (sustain - self.value) * coefficient;
                if (self.value - sustain).abs() <= 0.001 {
                    self.value = sustain;
                    self.stage = EnvStage::Sustain
                }
            }
            EnvStage::Sustain => self.value = sustain,
            EnvStage::Release => {
                let coefficient = (-6.907_755 / (release * self.sr).max(1.0)).exp();
                self.value *= coefficient;
                if self.value <= 0.0001 {
                    self.value = 0.0;
                    self.stage = EnvStage::Idle
                }
            }
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

/// A compact nonlinear four-stage ladder used by the Bass voice.
pub struct LadderFilter {
    stages: [f32; 4],
}

impl LadderFilter {
    pub fn new() -> Self {
        Self { stages: [0.0; 4] }
    }

    pub fn lowpass(&mut self, input: f32, cutoff: f32, resonance: f32, sr: f32) -> f32 {
        let coefficient = 1.0 - (-2.0 * PI * cutoff.clamp(20.0, sr * 0.45) / sr).exp();
        let feedback = resonance.clamp(0.0, 1.0) * 3.85;
        let mut stage_input = (input - self.stages[3] * feedback).tanh();
        for stage in &mut self.stages {
            *stage += coefficient * (stage_input - stage.tanh());
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
        output
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
        self.phase = 0.0;
        self.old_phase = 0.0;
        self.fade_remaining = 0;
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
}
impl Reverb {
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
    }
    fn pre_delay_tap(buffer: &[f32], pos: usize, delay: usize, input: f32) -> f32 {
        if delay == 0 {
            input
        } else {
            buffer[(pos + buffer.len() - delay) % buffer.len()]
        }
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
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
        self.feedback = feedback.clamp(0.0, 0.95)
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
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

/// A fixed, stereo-linked lookahead limiter. All storage is allocated at construction.
pub struct MasterLimiter {
    left: Vec<f32>,
    right: Vec<f32>,
    pos: usize,
    gain: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl MasterLimiter {
    pub fn new(sample_rate: u32) -> Self {
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

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
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

    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.pos = 0;
        self.gain = 1.0;
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
}
