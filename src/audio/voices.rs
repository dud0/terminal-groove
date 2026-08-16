use crate::dsp::{
    Adsr, BassAccentEnvelope, BassFilter, BassFilterEnvelope, BassVcaEnvelope, Biquad, ChordFilter,
    DeClickRamp, LeadFilter, NoiseSource, PolyBlepOsc, Smoother, SubOscillatorMode,
    additive_source_gains,
};
use crate::model::{
    ArpeggioConfig, ArpeggioRate, ArpeggioType, ChordShape, FmAlgorithm, ParameterLocks, Waveform,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SynthVoiceKind {
    Bass,
    Chord,
    Lead,
    Fm,
}

pub(super) struct SynthVoice {
    pub(super) osc: PolyBlepOsc,
    pub(super) sub_osc: PolyBlepOsc,
    pub(super) sub_osc_2: PolyBlepOsc,
    pub(super) sub_mode: SubOscillatorMode,
    pub(super) noise: NoiseSource,
    pub(super) noise_level: Smoother,
    pub(super) keyboard_tracking: f32,
    pub(super) env: Adsr,
    pub(super) bass_filter: BassFilter,
    pub(super) bass_vca: BassVcaEnvelope,
    pub(super) bass_filter_envelope: BassFilterEnvelope,
    pub(super) bass_accent_envelope: BassAccentEnvelope,
    pub(super) bass_decay_percent: Smoother,
    pub(super) chord_filter: ChordFilter,
    pub(super) lead_filter: LeadFilter,
    pub(super) chord_highpass: Biquad,
    pub(super) fm_filter: Biquad,
    pub(super) fm_phases: [f32; 4],
    pub(super) fm_previous: [f32; 4],
    pub(super) fm_algorithm: FmAlgorithm,
    pub(super) fm_ratios: [Smoother; 4],
    pub(super) fm_levels: [Smoother; 4],
    pub(super) fm_feedback: [Smoother; 4],
    pub(super) fm_routes: [[Smoother; 4]; 4],
    pub(super) fm_carriers: [Smoother; 4],
    pub(super) fm_carrier_normalization: Smoother,
    pub(super) fm_topology_smoothing: bool,
    pub(super) fm_brightness: Smoother,
    pub(super) freq: Smoother,
    pub(super) wave: Waveform,
    pub(super) oscillator_mix: Smoother,
    pub(super) pulse_width: Smoother,
    pub(super) sub_oscillator: Smoother,
    pub(super) cutoff_percent: Smoother,
    pub(super) cached_cutoff_percent: f32,
    pub(super) cached_cutoff_hz: f32,
    pub(super) cached_cutoff_kind: SynthVoiceKind,
    pub(super) filter_control_remaining: u8,
    pub(super) resonance_percent: Smoother,
    pub(super) filter_env_percent: Smoother,
    pub(super) locks: ParameterLocks,
    pub(super) active: bool,
    pub(super) idle_cleanup_done: bool,
    pub(super) remaining: u32,
    pub(super) accent_gain: Smoother,
    pub(super) accent_filter: Smoother,
    pub(super) declick: DeClickRamp,
    pub(super) slide_armed: bool,
    pub(super) kind: SynthVoiceKind,
    pub(super) level: Smoother,
    pub(super) delay_send: Smoother,
    pub(super) reverb_send: Smoother,
    pub(super) pan: Smoother,
    pub(super) voicing_pan_offset: f32,
    pan_cache: f32,
    pan_left: f32,
    pan_right: f32,
    pan_left_step: f32,
    pan_right_step: f32,
    pan_control_remaining: u8,
    oscillator_mix_cache: f32,
    oscillator_mix_cos: f32,
    oscillator_mix_sin: f32,
    oscillator_mix_cos_step: f32,
    oscillator_mix_sin_step: f32,
    oscillator_mix_control_remaining: u8,
}

#[derive(Clone, Copy)]
pub(super) struct SynthTrigger {
    pub(super) degree: u8,
    pub(super) octave: u8,
    pub(super) accent: bool,
    pub(super) slide: bool,
    pub(super) chord_shape: Option<ChordShape>,
    pub(super) arpeggio: ArpeggioConfig,
}

pub(super) const CHORD_GROUP_SIZE: usize = 4;

#[derive(Clone, Copy, Debug)]
pub(super) struct ArpeggioState {
    pub(super) enabled: bool,
    pub(super) kind: ArpeggioType,
    pub(super) rate: ArpeggioRate,
    pub(super) shape: ChordShape,
    pub(super) order: [u8; 8],
    pub(super) order_len: u8,
    pub(super) position: u8,
    pub(super) phase: f64,
    pub(super) random: u32,
}

impl Default for ArpeggioState {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: ArpeggioType::default(),
            rate: ArpeggioRate::default(),
            shape: ChordShape::default(),
            order: [0; 8],
            order_len: 0,
            position: 0,
            phase: 0.0,
            random: 0x6d2b_79f5,
        }
    }
}

impl ArpeggioState {
    pub(super) fn reset(
        &mut self,
        shape: ChordShape,
        kind: ArpeggioType,
        rate: ArpeggioRate,
        sr: f32,
        bpm: u16,
    ) {
        self.enabled = true;
        self.kind = kind;
        self.rate = rate;
        self.shape = shape;
        self.position = 0;
        self.phase = sr as f64 * 60.0 / bpm as f64 * rate.beats();
        self.rebuild_order();
    }

    pub(super) fn rebuild_order(&mut self) {
        let n = self.shape.degrees().len().min(4);
        self.order_len = match self.kind {
            ArpeggioType::Up | ArpeggioType::Down | ArpeggioType::Random => n as u8,
            ArpeggioType::UpDown | ArpeggioType::DownUp => {
                if n > 1 {
                    (n * 2 - 2) as u8
                } else {
                    n as u8
                }
            }
        };
        let length = self.order_len as usize;
        match self.kind {
            ArpeggioType::Up | ArpeggioType::Random => {
                for (i, slot) in self.order[..length].iter_mut().enumerate() {
                    *slot = i as u8;
                }
            }
            ArpeggioType::Down => {
                for (i, slot) in self.order[..length].iter_mut().enumerate() {
                    *slot = (n - i - 1) as u8;
                }
            }
            ArpeggioType::UpDown => {
                for i in 0..length {
                    self.order[i] = if i < n { i } else { length - i } as u8;
                }
            }
            ArpeggioType::DownUp => {
                for i in 0..length {
                    self.order[i] = if i < n { n - i - 1 } else { i - n + 1 } as u8;
                }
            }
        }
        if self.kind == ArpeggioType::Random {
            self.shuffle();
        }
    }

    pub(super) fn shuffle(&mut self) {
        for i in (1..self.order_len as usize).rev() {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            let j = (self.random as usize) % (i + 1);
            self.order.swap(i, j);
        }
    }

    pub(super) fn current_voice(&self) -> usize {
        self.order[self.position as usize] as usize
    }

    pub(super) fn tick(&mut self, sr: f32, bpm: u16) -> bool {
        if !self.enabled {
            return false;
        }
        self.phase -= 1.0;
        if self.phase <= 0.0 {
            self.phase += sr as f64 * 60.0 / bpm as f64 * self.rate.beats();
            self.position += 1;
            if self.position >= self.order_len {
                self.position = 0;
                if self.kind == ArpeggioType::Random {
                    self.shuffle();
                }
            }
            true
        } else {
            false
        }
    }
}

pub(super) struct ChordVoicePool {
    pub(super) voices: [SynthVoice; CHORD_GROUP_SIZE * 2],
    pub(super) group: usize,
    pub(super) voice_count: usize,
    pub(super) group_voice_counts: [usize; 2],
    pub(super) active: bool,
    pub(super) arpeggiated: bool,
    pub(super) arpeggio: ArpeggioState,
    pub(super) arpeggio_trigger: SynthTrigger,
    pub(super) arpeggio_locks: ParameterLocks,
    pub(super) preview_remaining: u64,
}

impl ChordVoicePool {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            voices: std::array::from_fn(|index| {
                SynthVoice::new_with_seed(
                    sample_rate as f32,
                    0x91e1_0da5 ^ (index as u32).wrapping_mul(0x9e37_79b9),
                )
            }),
            group: 1,
            voice_count: 0,
            group_voice_counts: [0; 2],
            active: false,
            arpeggiated: false,
            arpeggio: ArpeggioState::default(),
            arpeggio_trigger: SynthTrigger {
                degree: 1,
                octave: 3,
                accent: false,
                slide: false,
                chord_shape: None,
                arpeggio: ArpeggioConfig::default(),
            },
            arpeggio_locks: ParameterLocks::default(),
            preview_remaining: 0,
        }
    }
}

pub(super) const DRUM_SILENCE: f32 = 0.0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DrumVoiceKind {
    Kick,
    Snare,
    Hat,
    Tom,
    Cymbal,
    Rimshot,
}

pub(super) struct DrumEnvelope {
    pub(super) value: f32,
    pub(super) start: f32,
    pub(super) peak: f32,
    pub(super) attack_samples: u32,
    pub(super) decay_samples: u32,
    pub(super) elapsed: u32,
}
impl DrumEnvelope {
    pub(super) fn new() -> Self {
        Self {
            value: DRUM_SILENCE,
            start: DRUM_SILENCE,
            peak: DRUM_SILENCE,
            attack_samples: 1,
            decay_samples: 1,
            elapsed: 1,
        }
    }
    pub(super) fn trigger(&mut self, peak: f32, attack: f32, decay: f32, sr: f32) {
        self.start = self.value.max(DRUM_SILENCE);
        self.peak = peak;
        self.attack_samples = (attack * sr).round().max(1.0) as u32;
        self.decay_samples = (decay * sr).round().max(self.attack_samples as f32 + 1.0) as u32;
        self.elapsed = 0;
    }
    pub(super) fn is_idle(&self) -> bool {
        self.elapsed >= self.decay_samples
    }
    pub(super) fn next_value(&mut self) -> f32 {
        if self.elapsed < self.attack_samples {
            let t = self.elapsed as f32 / self.attack_samples as f32;
            self.value = self.start * (self.peak / self.start).powf(t);
        } else if self.elapsed < self.decay_samples {
            let t = (self.elapsed - self.attack_samples) as f32
                / (self.decay_samples - self.attack_samples) as f32;
            self.value = self.peak * (DRUM_SILENCE / self.peak).powf(t);
        } else {
            self.value = DRUM_SILENCE;
        }
        self.elapsed = self.elapsed.saturating_add(1);
        self.value
    }
}

pub(super) struct KickPitchEnvelope {
    pub(super) value: f32,
    pub(super) start: f32,
    pub(super) peak: f32,
    pub(super) settled: f32,
    pub(super) rise_samples: u32,
    pub(super) fall_samples: u32,
    pub(super) elapsed: u32,
}
impl KickPitchEnvelope {
    pub(super) fn new() -> Self {
        Self {
            value: 75.0,
            start: 75.0,
            peak: 75.0,
            settled: 48.0,
            rise_samples: 1,
            fall_samples: 1,
            elapsed: 1,
        }
    }
    pub(super) fn trigger(&mut self, tone: f32, decay: f32, sr: f32) {
        self.start = self.value.max(20.0);
        self.peak = 110.0 + tone * 170.0;
        self.settled = 45.0 + tone * 25.0;
        self.rise_samples = (0.0015 * sr).round().max(1.0) as u32;
        self.fall_samples = (decay.min(0.13) * sr)
            .round()
            .max(self.rise_samples as f32 + 1.0) as u32;
        self.elapsed = 0;
    }
    pub(super) fn trigger_tom(&mut self, tune: f32, tone: f32, decay: f32, sr: f32) {
        self.start = self.value.max(30.0);
        self.peak = 180.0 + tune * 240.0;
        self.settled = 80.0 + tune * 140.0;
        self.rise_samples = (0.001 * sr).round().max(1.0) as u32;
        self.fall_samples = (0.025 + tone * 0.085)
            .min(decay * 0.8)
            .mul_add(sr, 0.0)
            .round()
            .max(self.rise_samples as f32 + 1.0) as u32;
        self.elapsed = 0;
    }
    pub(super) fn next_value(&mut self) -> f32 {
        if self.elapsed < self.rise_samples {
            let t = self.elapsed as f32 / self.rise_samples as f32;
            self.value = self.start + (self.peak - self.start) * t;
        } else if self.elapsed < self.fall_samples {
            let t = (self.elapsed - self.rise_samples) as f32
                / (self.fall_samples - self.rise_samples) as f32;
            self.value = self.peak * (self.settled / self.peak).powf(t);
        } else {
            self.value = self.settled;
        }
        self.elapsed = self.elapsed.saturating_add(1);
        self.value
    }
}

pub(super) struct DrumVoice {
    pub(super) kind: DrumVoiceKind,
    pub(super) envelope: DrumEnvelope,
    pub(super) kick_pitch: KickPitchEnvelope,
    pub(super) tom_pitch: KickPitchEnvelope,
    pub(super) phase: f32,
    pub(super) phase2: f32,
    pub(super) rimshot_phases: [f32; 3],
    pub(super) rimshot_frequencies: [f32; 3],
    pub(super) rimshot_amplitudes: [f32; 3],
    pub(super) rimshot_decay_coefficients: [f32; 3],
    pub(super) rimshot_attack_samples: u32,
    pub(super) metallic: [PolyBlepOsc; 6],
    pub(super) filter: Biquad,
    pub(super) filter2: Biquad,
    pub(super) noise: u32,
    pub(super) tune: f32,
    pub(super) tone: f32,
    pub(super) snappy: f32,
    pub(super) attack: f32,
    pub(super) accent: bool,
    pub(super) level: Smoother,
    pub(super) delay_send: Smoother,
    pub(super) reverb_send: Smoother,
    pub(super) pan: Smoother,
    pan_cache: f32,
    pan_left: f32,
    pan_right: f32,
    pan_left_step: f32,
    pan_right_step: f32,
    pan_control_remaining: u8,
    pub(super) locks: ParameterLocks,
}

#[derive(Clone, Copy)]
pub(super) struct DrumControls {
    pub(super) kind: DrumVoiceKind,
    pub(super) tune: f32,
    pub(super) tone: f32,
    pub(super) snappy: f32,
    pub(super) decay: f32,
    pub(super) attack: f32,
}

impl DrumVoice {
    pub(super) fn new(seed: u32) -> Self {
        Self {
            kind: DrumVoiceKind::Kick,
            envelope: DrumEnvelope::new(),
            kick_pitch: KickPitchEnvelope::new(),
            tom_pitch: KickPitchEnvelope::new(),
            phase: 0.0,
            phase2: 0.0,
            rimshot_phases: [0.0; 3],
            rimshot_frequencies: [222.0, 500.0, 1_000.0],
            rimshot_amplitudes: [0.0; 3],
            rimshot_decay_coefficients: [0.0; 3],
            rimshot_attack_samples: 1,
            metallic: std::array::from_fn(|_| PolyBlepOsc::default()),
            filter: Biquad::new(),
            filter2: Biquad::new(),
            noise: seed,
            tune: 0.5,
            tone: 0.5,
            snappy: 0.55,
            attack: 0.35,
            accent: false,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
            pan: Smoother::new(50.0),
            pan_cache: f32::NAN,
            pan_left: std::f32::consts::FRAC_1_SQRT_2,
            pan_right: std::f32::consts::FRAC_1_SQRT_2,
            pan_left_step: 0.0,
            pan_right_step: 0.0,
            pan_control_remaining: 0,
            locks: ParameterLocks::default(),
        }
    }
    pub(super) fn noise(&mut self) -> f32 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        x as i32 as f32 / i32::MAX as f32
    }

    pub(super) fn pan_gains(&mut self, pan: f32) -> (f32, f32) {
        if self.pan_control_remaining == 0 && pan != self.pan_cache {
            self.pan_cache = pan;
            let angle = pan.clamp(0.0, 100.0) * std::f32::consts::FRAC_PI_2 / 100.0;
            let target_left = angle.cos();
            let target_right = angle.sin();
            self.pan_left_step = (target_left - self.pan_left) / 8.0;
            self.pan_right_step = (target_right - self.pan_right) / 8.0;
            self.pan_control_remaining = 8;
        }
        if self.pan_control_remaining > 0 {
            self.pan_left += self.pan_left_step;
            self.pan_right += self.pan_right_step;
            self.pan_control_remaining -= 1;
        }
        (self.pan_left, self.pan_right)
    }
}
impl SynthVoice {
    #[cfg(test)]
    pub(super) fn new(sr: f32) -> Self {
        Self::new_with_seed(sr, 0x6d2b_79f5)
    }

    pub(super) fn new_with_seed(sr: f32, seed: u32) -> Self {
        let mut chord_highpass = Biquad::new();
        chord_highpass.set_highpass(32.0, 0.707, sr * 2.0);
        Self {
            osc: Default::default(),
            sub_osc: Default::default(),
            sub_osc_2: Default::default(),
            sub_mode: SubOscillatorMode::OneOctave,
            noise: NoiseSource::new(seed),
            noise_level: Smoother::new(0.0),
            keyboard_tracking: 50.0,
            env: Adsr::new(sr),
            bass_filter: Default::default(),
            bass_vca: BassVcaEnvelope::new(sr),
            bass_filter_envelope: BassFilterEnvelope::new(sr),
            bass_accent_envelope: BassAccentEnvelope::new(sr),
            bass_decay_percent: Smoother::new(40.0),
            chord_filter: Default::default(),
            lead_filter: Default::default(),
            chord_highpass,
            fm_filter: Biquad::new(),
            fm_phases: [0.0; 4],
            fm_previous: [0.0; 4],
            fm_algorithm: FmAlgorithm::Cascade,
            fm_ratios: [
                Smoother::new(1.0),
                Smoother::new(2.0),
                Smoother::new(1.0),
                Smoother::new(1.0),
            ],
            fm_levels: [
                Smoother::new(100.0),
                Smoother::new(35.0),
                Smoother::new(0.0),
                Smoother::new(0.0),
            ],
            fm_feedback: [
                Smoother::new(0.0),
                Smoother::new(8.0),
                Smoother::new(0.0),
                Smoother::new(0.0),
            ],
            fm_routes: {
                let mut routes = [[Smoother::new(0.0); 4]; 4];
                routes[3][2] = Smoother::new(1.0);
                routes[2][1] = Smoother::new(1.0);
                routes[1][0] = Smoother::new(1.0);
                routes
            },
            fm_carriers: [
                Smoother::new(1.0),
                Smoother::new(0.0),
                Smoother::new(0.0),
                Smoother::new(0.0),
            ],
            fm_carrier_normalization: Smoother::new(1.0),
            fm_topology_smoothing: false,
            fm_brightness: Smoother::new(72.0),
            freq: Smoother::new(110.0),
            wave: Waveform::Saw,
            oscillator_mix: Smoother::new(70.0),
            pulse_width: Smoother::new(50.0),
            sub_oscillator: Smoother::new(0.0),
            cutoff_percent: Smoother::new(65.0),
            cached_cutoff_percent: f32::NAN,
            cached_cutoff_hz: 0.0,
            cached_cutoff_kind: SynthVoiceKind::Bass,
            filter_control_remaining: 0,
            resonance_percent: Smoother::new(10.0),
            filter_env_percent: Smoother::new(25.0),
            locks: ParameterLocks::default(),
            active: false,
            idle_cleanup_done: true,
            remaining: 0,
            accent_gain: Smoother::new(1.0),
            accent_filter: Smoother::new(0.0),
            declick: DeClickRamp::new(sr),
            slide_armed: false,
            kind: SynthVoiceKind::Bass,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
            pan: Smoother::new(50.0),
            voicing_pan_offset: 0.0,
            pan_cache: f32::NAN,
            pan_left: std::f32::consts::FRAC_1_SQRT_2,
            pan_right: std::f32::consts::FRAC_1_SQRT_2,
            pan_left_step: 0.0,
            pan_right_step: 0.0,
            pan_control_remaining: 0,
            oscillator_mix_cache: f32::NAN,
            oscillator_mix_cos: 1.0,
            oscillator_mix_sin: 0.0,
            oscillator_mix_cos_step: 0.0,
            oscillator_mix_sin_step: 0.0,
            oscillator_mix_control_remaining: 0,
        }
    }

    pub(super) fn pan_gains(&mut self, pan: f32) -> (f32, f32) {
        if self.pan_control_remaining == 0 && pan != self.pan_cache {
            self.pan_cache = pan;
            let angle = pan.clamp(0.0, 100.0) * std::f32::consts::FRAC_PI_2 / 100.0;
            let target_left = angle.cos();
            let target_right = angle.sin();
            self.pan_left_step = (target_left - self.pan_left) / 8.0;
            self.pan_right_step = (target_right - self.pan_right) / 8.0;
            self.pan_control_remaining = 8;
        }
        if self.pan_control_remaining > 0 {
            self.pan_left += self.pan_left_step;
            self.pan_right += self.pan_right_step;
            self.pan_control_remaining -= 1;
        }
        (self.pan_left, self.pan_right)
    }

    pub(super) fn oscillator_mix_gains(&mut self, mix: f32) -> (f32, f32) {
        if self.oscillator_mix_control_remaining == 0 && mix != self.oscillator_mix_cache {
            self.oscillator_mix_cache = mix;
            let (target_cos, target_sin) = additive_source_gains(mix);
            self.oscillator_mix_cos_step = (target_cos - self.oscillator_mix_cos) / 8.0;
            self.oscillator_mix_sin_step = (target_sin - self.oscillator_mix_sin) / 8.0;
            self.oscillator_mix_control_remaining = 8;
        }
        if self.oscillator_mix_control_remaining > 0 {
            self.oscillator_mix_cos += self.oscillator_mix_cos_step;
            self.oscillator_mix_sin += self.oscillator_mix_sin_step;
            self.oscillator_mix_control_remaining -= 1;
        }
        (self.oscillator_mix_cos, self.oscillator_mix_sin)
    }

    pub(super) fn is_idle(&self) -> bool {
        if self.kind == SynthVoiceKind::Bass {
            self.bass_vca.is_idle()
        } else {
            self.env.stage == crate::dsp::EnvStage::Idle
        }
    }

    pub(super) fn begin_note_transition(&mut self) {
        self.declick.begin();
    }

    pub(super) fn process_output(&mut self, output: f32) -> f32 {
        self.declick.process(output)
    }

    pub(super) fn reset_to_idle(&mut self) {
        self.env.reset();
        self.declick.reset();
        self.active = false;
        self.idle_cleanup_done = true;
        self.remaining = 0;
        self.chord_filter.reset();
        self.lead_filter.reset();
        self.chord_highpass.clear_state();
        self.fm_filter.clear_state();
        self.fm_phases = [0.0; 4];
        self.fm_previous = [0.0; 4];
        self.fm_topology_smoothing = false;
        self.voicing_pan_offset = 0.0;
    }

    pub(super) fn gate_off(&mut self) {
        if self.kind == SynthVoiceKind::Bass {
            self.bass_vca.gate_off();
        } else {
            self.env.gate_off();
        }
    }
}
