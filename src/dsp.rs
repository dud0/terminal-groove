use std::f32::consts::PI;

pub fn exp_map(percent: u8, min: f32, max: f32) -> f32 {
    if percent == 0 {
        min
    } else {
        min * (max / min).powf(percent as f32 / 100.0)
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
        let dt = (hz / sr).clamp(0.0, 0.49);
        let mut x = if self.phase < 0.5 { 1.0 } else { -1.0 };
        x += Self::blep(self.phase, dt);
        x -= Self::blep((self.phase + 0.5).fract(), dt);
        self.phase = (self.phase + dt).fract();
        x
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
pub struct Adsr {
    pub stage: EnvStage,
    value: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    sr: f32,
}
impl Adsr {
    pub fn new(sr: f32) -> Self {
        Self {
            stage: EnvStage::Idle,
            value: 0.0,
            attack: 0.0,
            decay: 0.1,
            sustain: 0.7,
            release: 0.1,
            sr,
        }
    }
    pub fn configure(&mut self, a: f32, d: f32, s: f32, r: f32) {
        self.attack = a;
        self.decay = d;
        self.sustain = s;
        self.release = r
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
        match self.stage {
            EnvStage::Idle => {}
            EnvStage::Attack => {
                if self.attack <= 0.0 {
                    self.value = 1.0;
                    self.stage = EnvStage::Decay
                } else {
                    self.value += 1.0 / (self.attack * self.sr);
                    if self.value >= 1.0 {
                        self.value = 1.0;
                        self.stage = EnvStage::Decay
                    }
                }
            }
            EnvStage::Decay => {
                self.value -= (1.0 - self.sustain) / (self.decay * self.sr).max(1.0);
                if self.value <= self.sustain {
                    self.value = self.sustain;
                    self.stage = EnvStage::Sustain
                }
            }
            EnvStage::Sustain => self.value = self.sustain,
            EnvStage::Release => {
                self.value -= self.value.max(0.0001) / (self.release * self.sr).max(1.0);
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
}

struct Comb {
    buffer: Vec<f32>,
    pos: usize,
    damped: f32,
}
impl Comb {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(2)],
            pos: 0,
            damped: 0.0,
        }
    }
    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.buffer[self.pos];
        self.damped += (output - self.damped) * (1.0 - damping);
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
    left: [Comb; 4],
    right: [Comb; 4],
    allpass_l: [Allpass; 2],
    allpass_r: [Allpass; 2],
    feedback: f32,
}
impl Reverb {
    pub fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / 44_100.0;
        let size = |n: usize| (n as f32 * scale).round() as usize;
        Self {
            left: [
                Comb::new(size(1116)),
                Comb::new(size(1188)),
                Comb::new(size(1277)),
                Comb::new(size(1356)),
            ],
            right: [
                Comb::new(size(1139)),
                Comb::new(size(1211)),
                Comb::new(size(1300)),
                Comb::new(size(1379)),
            ],
            allpass_l: [Allpass::new(size(556)), Allpass::new(size(441))],
            allpass_r: [Allpass::new(size(579)), Allpass::new(size(464))],
            feedback: 0.82,
        }
    }
    pub fn set_time(&mut self, seconds: f32) {
        self.feedback = (0.55 + 0.41 * ((seconds.clamp(0.2, 10.0) - 0.2) / 9.8)).clamp(0.0, 0.96);
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let input = (l + r) * 0.12;
        let mut ol = 0.0;
        let mut or = 0.0;
        for comb in &mut self.left {
            ol += comb.process(input, self.feedback, 0.25);
        }
        for comb in &mut self.right {
            or += comb.process(input, self.feedback, 0.25);
        }
        for ap in &mut self.allpass_l {
            ol = ap.process(ol);
        }
        for ap in &mut self.allpass_r {
            or = ap.process(or);
        }
        (safety(ol * 0.25), safety(or * 0.25))
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
        self.left[self.pos] = l + dr * self.feedback;
        self.right[self.pos] = r + dl * self.feedback;
        self.pos = (self.pos + 1) % self.left.len();
        (dl, dr)
    }
    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0)
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
    if x.is_finite() {
        x / (1.0 + x.abs())
    } else {
        0.0
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
        assert!(safety(100.).abs() < 1.)
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
    fn reverb_is_finite_and_bounded_at_time_extremes() {
        for time in [0.2, 10.0] {
            let mut reverb = Reverb::new(8_000);
            reverb.set_time(time);
            for i in 0..40_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(input, input);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
            }
        }
    }
}
