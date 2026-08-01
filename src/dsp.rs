use std::f32::consts::PI;

pub fn exp_map(percent:u8,min:f32,max:f32)->f32 {if percent==0{min}else{min*(max/min).powf(percent as f32/100.0)}}

#[derive(Clone,Copy,Debug)] pub struct Smoother {current:f32,target:f32,step:f32,remaining:u32}
impl Smoother {pub fn new(v:f32)->Self{Self{current:v,target:v,step:0.0,remaining:0}} pub fn set(&mut self,v:f32,samples:u32){self.target=v;if samples==0{self.current=v;self.remaining=0}else{self.remaining=samples;self.step=(v-self.current)/samples as f32}} pub fn next(&mut self)->f32{if self.remaining>0{self.current+=self.step;self.remaining-=1;if self.remaining==0{self.current=self.target}}self.current}}

#[derive(Default)] pub struct PolyBlepOsc {phase:f32}
impl PolyBlepOsc {
 fn blep(t:f32,dt:f32)->f32{if t<dt{let x=t/dt;x+x-x*x-1.0}else if t>1.0-dt{let x=(t-1.0)/dt;x*x+x+x+1.0}else{0.0}}
 pub fn next_saw(&mut self,hz:f32,sr:f32)->f32{let dt=(hz/sr).clamp(0.0,0.49);let mut x=2.0*self.phase-1.0;x-=Self::blep(self.phase,dt);self.phase=(self.phase+dt).fract();x}
 pub fn next_square(&mut self,hz:f32,sr:f32)->f32{let dt=(hz/sr).clamp(0.0,0.49);let mut x=if self.phase<0.5{1.0}else{-1.0};x+=Self::blep(self.phase,dt);x-=Self::blep((self.phase+0.5).fract(),dt);self.phase=(self.phase+dt).fract();x}
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub enum EnvStage{Idle,Attack,Decay,Sustain,Release}
pub struct Adsr {pub stage:EnvStage,value:f32,attack:f32,decay:f32,sustain:f32,release:f32,sr:f32}
impl Adsr {pub fn new(sr:f32)->Self{Self{stage:EnvStage::Idle,value:0.0,attack:0.0,decay:.1,sustain:.7,release:.1,sr}} pub fn configure(&mut self,a:f32,d:f32,s:f32,r:f32){self.attack=a;self.decay=d;self.sustain=s;self.release=r} pub fn gate_on(&mut self){self.stage=EnvStage::Attack} pub fn gate_off(&mut self){if self.stage!=EnvStage::Idle{self.stage=EnvStage::Release}} pub fn next(&mut self)->f32{match self.stage{EnvStage::Idle=>{},EnvStage::Attack=>{if self.attack<=0.0{self.value=1.0;self.stage=EnvStage::Decay}else{self.value+=1.0/(self.attack*self.sr);if self.value>=1.0{self.value=1.0;self.stage=EnvStage::Decay}}},EnvStage::Decay=>{self.value-=(1.0-self.sustain)/(self.decay*self.sr).max(1.0);if self.value<=self.sustain{self.value=self.sustain;self.stage=EnvStage::Sustain}},EnvStage::Sustain=>self.value=self.sustain,EnvStage::Release=>{self.value-=self.value.max(0.0001)/(self.release*self.sr).max(1.0);if self.value<=0.0001{self.value=0.0;self.stage=EnvStage::Idle}}}self.value}}

pub struct Svf {ic1:f32,ic2:f32}
impl Svf {pub fn new()->Self{Self{ic1:0.0,ic2:0.0}} pub fn lowpass(&mut self,input:f32,cutoff:f32,q:f32,sr:f32)->f32{let g=(PI*(cutoff/sr).clamp(0.0001,0.45)).tan();let k=1.0/q.clamp(0.1,20.0);let a1=1.0/(1.0+g*(g+k));let v1=a1*(self.ic1+g*(input-self.ic2));let v2=self.ic2+g*v1;self.ic1=2.0*v1-self.ic1;self.ic2=2.0*v2-self.ic2;if v2.is_finite(){v2}else{self.ic1=0.0;self.ic2=0.0;0.0}}}
impl Default for Svf{fn default()->Self{Self::new()}}

pub struct Delay {left:Vec<f32>,right:Vec<f32>,pos:usize,delay:usize,feedback:f32}
impl Delay {pub fn new(sample_rate:u32)->Self{let n=(sample_rate as usize*7).max(2);Self{left:vec![0.0;n],right:vec![0.0;n],pos:0,delay:1,feedback:.3}} pub fn configure(&mut self,samples:usize,feedback:f32){self.delay=samples.clamp(1,self.left.len()-1);self.feedback=feedback.clamp(0.0,.95)} pub fn process(&mut self,l:f32,r:f32)->(f32,f32){let read=(self.pos+self.left.len()-self.delay)%self.left.len();let dl=self.left[read];let dr=self.right[read];self.left[self.pos]=l+dr*self.feedback;self.right[self.pos]=r+dl*self.feedback;self.pos=(self.pos+1)%self.left.len();(dl,dr)} pub fn clear(&mut self){self.left.fill(0.0);self.right.fill(0.0)}}

pub struct DcBlock {xl:f32,yl:f32,xr:f32,yr:f32}
impl DcBlock {pub fn new()->Self{Self{xl:0.,yl:0.,xr:0.,yr:0.}} pub fn process(&mut self,l:f32,r:f32)->(f32,f32){let ol=l-self.xl+.995*self.yl;let or=r-self.xr+.995*self.yr;self.xl=l;self.yl=ol;self.xr=r;self.yr=or;(safety(ol),safety(or))}}
pub fn safety(x:f32)->f32{if x.is_finite(){x/(1.0+x.abs())}else{0.0}}

#[cfg(test)] mod tests{use super::*;
 #[test]fn osc_bounded(){let mut o=PolyBlepOsc::default();for _ in 0..10000{assert!(o.next_saw(440.,48000.).abs()<=1.1);}}
 #[test]fn filter_finite(){let mut f=Svf::new();for c in [20.,20000.,30000.]{for _ in 0..1000{assert!(f.lowpass(1.,c,10.,44100.).is_finite())}}}
 #[test]fn delay_exact(){let mut d=Delay::new(100);d.configure(10,0.);d.process(1.,0.);for _ in 0..9{assert_eq!(d.process(0.,0.).0,0.)}assert_eq!(d.process(0.,0.).0,1.);}
 #[test]fn nonfinite_safe(){assert_eq!(safety(f32::NAN),0.0);assert!(safety(100.).abs()<1.)}
}
