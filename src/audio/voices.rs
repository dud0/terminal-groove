use crate::dsp::{Adsr, Biquad, LadderFilter, PolyBlepOsc, Smoother, StereoChorus};
use crate::model::{
    ArpeggioConfig, ArpeggioRate, ArpeggioType, ChordShape, ParameterLocks, Waveform,
};

pub(super) struct SynthVoice {
    pub(super) osc: PolyBlepOsc,
    pub(super) sub_osc: PolyBlepOsc,
    pub(super) env: Adsr,
    pub(super) bass_filter: LadderFilter,
    pub(super) roland_filter: LadderFilter,
    pub(super) freq: Smoother,
    pub(super) wave: Waveform,
    pub(super) oscillator_mix: Smoother,
    pub(super) pulse_width: Smoother,
    pub(super) sub_oscillator: Smoother,
    pub(super) cutoff_percent: Smoother,
    pub(super) resonance_percent: Smoother,
    pub(super) filter_env_percent: Smoother,
    pub(super) locks: ParameterLocks,
    pub(super) active: bool,
    pub(super) remaining: u32,
    pub(super) accent_gain: Smoother,
    pub(super) accent_filter: Smoother,
    pub(super) slide_armed: bool,
    pub(super) bass: bool,
    pub(super) chord: bool,
    pub(super) level: Smoother,
    pub(super) delay_send: Smoother,
    pub(super) reverb_send: Smoother,
    pub(super) pan: Smoother,
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
    pub(super) active: bool,
    pub(super) chorus: StereoChorus,
    pub(super) arpeggiated: bool,
    pub(super) arpeggio: ArpeggioState,
    pub(super) arpeggio_trigger: SynthTrigger,
    pub(super) arpeggio_locks: ParameterLocks,
    pub(super) preview_remaining: u64,
}

impl ChordVoicePool {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            voices: std::array::from_fn(|_| SynthVoice::new(sample_rate as f32)),
            group: 1,
            voice_count: 0,
            active: false,
            chorus: StereoChorus::new(sample_rate),
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
pub(super) const REVERB_RETURN_GAIN: f32 = 0.5;

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
    pub(super) envelope: DrumEnvelope,
    pub(super) kick_pitch: KickPitchEnvelope,
    pub(super) tom_pitch: KickPitchEnvelope,
    pub(super) phase: f32,
    pub(super) phase2: f32,
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
    pub(super) locks: ParameterLocks,
}

#[derive(Clone, Copy)]
pub(super) struct DrumControls {
    pub(super) tune: f32,
    pub(super) tone: f32,
    pub(super) snappy: f32,
    pub(super) decay: f32,
    pub(super) attack: f32,
}

impl DrumVoice {
    pub(super) fn new(seed: u32) -> Self {
        Self {
            envelope: DrumEnvelope::new(),
            kick_pitch: KickPitchEnvelope::new(),
            tom_pitch: KickPitchEnvelope::new(),
            phase: 0.0,
            phase2: 0.0,
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
}
impl SynthVoice {
    pub(super) fn new(sr: f32) -> Self {
        Self {
            osc: Default::default(),
            sub_osc: Default::default(),
            env: Adsr::new(sr),
            bass_filter: Default::default(),
            roland_filter: Default::default(),
            freq: Smoother::new(110.0),
            wave: Waveform::Saw,
            oscillator_mix: Smoother::new(70.0),
            pulse_width: Smoother::new(50.0),
            sub_oscillator: Smoother::new(0.0),
            cutoff_percent: Smoother::new(65.0),
            resonance_percent: Smoother::new(10.0),
            filter_env_percent: Smoother::new(25.0),
            locks: ParameterLocks::default(),
            active: false,
            remaining: 0,
            accent_gain: Smoother::new(1.0),
            accent_filter: Smoother::new(0.0),
            slide_armed: false,
            bass: false,
            chord: false,
            level: Smoother::new(0.0),
            delay_send: Smoother::new(0.0),
            reverb_send: Smoother::new(0.0),
            pan: Smoother::new(50.0),
        }
    }
}
