use crate::model::{ParameterLocks,Percent,ProjectV1,STEP_COUNT,StepEvent,TrackKind,tie_source};
use std::{collections::VecDeque,time::{Duration,Instant}};

#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub enum Scope { Base, Lock }
#[derive(Clone,Debug,PartialEq,Eq)] pub enum EditError { InvalidTrack,InvalidStep,NotSynth,NotDrum,EmptyLock,InvalidTie }
impl std::fmt::Display for EditError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{}",match self {Self::InvalidTrack=>"invalid track",Self::InvalidStep=>"invalid step",Self::NotSynth=>"action requires a synth track",Self::NotDrum=>"action requires a drum track",Self::EmptyLock=>"cannot lock an empty step",Self::InvalidTie=>"tie requires a preceding note"})}}

#[derive(Clone)] struct Revision { before:ProjectV1,after:ProjectV1,coalesce:Option<CoalesceKey>,at:Instant }
#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub struct CoalesceKey(pub usize,pub usize,pub u8);

pub struct Editor { pub project:ProjectV1,saved:ProjectV1,undo:VecDeque<Revision>,redo:Vec<Revision> }
impl Editor {
 pub fn new(project:ProjectV1)->Self{Self{saved:project.clone(),project,undo:VecDeque::new(),redo:Vec::new()}}
 pub fn is_dirty(&self)->bool{self.project!=self.saved}
 pub fn mark_saved(&mut self){self.saved=self.project.clone();}
 pub fn replace_loaded(&mut self,p:ProjectV1){self.project=p.clone();self.saved=p;self.undo.clear();self.redo.clear();}
 pub fn edit<F>(&mut self,key:Option<CoalesceKey>,f:F)->Result<bool,EditError> where F:FnOnce(&mut ProjectV1)->Result<(),EditError>{
   let before=self.project.clone();f(&mut self.project)?;if before==self.project{return Ok(false)}
   let now=Instant::now();let merge=key.is_some()&&self.undo.back().is_some_and(|r|r.coalesce==key&&now.duration_since(r.at)<=Duration::from_millis(300));
   if merge {let r=self.undo.back_mut().unwrap();r.after=self.project.clone();r.at=now;} else {if self.undo.len()==256{self.undo.pop_front();}self.undo.push_back(Revision{before,after:self.project.clone(),coalesce:key,at:now});}
   self.redo.clear();Ok(true)
 }
 pub fn undo(&mut self)->bool{if let Some(r)=self.undo.pop_back(){self.project=r.before.clone();self.redo.push(r);true}else{false}}
 pub fn redo(&mut self)->bool{if let Some(r)=self.redo.pop(){self.project=r.after.clone();self.undo.push_back(r);true}else{false}}
 pub fn toggle_event(&mut self,track:usize,step:usize)->Result<bool,EditError>{self.edit(None,move|p|{
   let t=p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;if step>=STEP_COUNT{return Err(EditError::InvalidStep)}
   if t.steps[step].is_some(){clear_with_ties(t,step);return Ok(())}
   t.steps[step]=Some(if t.kind==TrackKind::Synth {StepEvent::Note{degree:t.input_degree.unwrap(),octave:t.input_octave.unwrap(),locks:Default::default()}}else{StepEvent::Trigger{locks:Default::default()}});Ok(())
 })}
 pub fn set_note(&mut self,track:usize,step:usize,degree:u8)->Result<bool,EditError>{self.edit(None,move|p|{
   let t=p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;if t.kind!=TrackKind::Synth{return Err(EditError::NotSynth)}if step>=16||!(1..=8).contains(&degree){return Err(EditError::InvalidStep)}
   let locks=match t.steps[step].take(){Some(e)=>e.locks().clone(),None=>Default::default()};let octave=t.input_octave.unwrap();t.input_degree=Some(degree);t.steps[step]=Some(StepEvent::Note{degree,octave,locks});cleanup_invalid_ties(t);Ok(())
 })}
 pub fn toggle_tie(&mut self,track:usize,step:usize)->Result<bool,EditError>{self.edit(None,move|p|{
   let t=p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;if t.kind!=TrackKind::Synth{return Err(EditError::NotSynth)}if step>=16{return Err(EditError::InvalidStep)}
   match t.steps[step].take(){Some(StepEvent::Tie{..})=>{cleanup_invalid_ties(t);Ok(())},old=>{let locks=old.as_ref().map(|x|x.locks().clone()).unwrap_or_default();t.steps[step]=Some(StepEvent::Tie{locks});if tie_source(&t.steps,step).is_none(){t.steps[step]=old;return Err(EditError::InvalidTie)}Ok(())}}
 })}
 pub fn clear(&mut self,track:usize,step:usize)->Result<bool,EditError>{self.edit(None,move|p|{let t=p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;if step>=16{return Err(EditError::InvalidStep)}clear_with_ties(t,step);Ok(())})}
 pub fn set_level(&mut self,track:usize,step:usize,scope:Scope,value:Percent,key:Option<CoalesceKey>)->Result<bool,EditError>{self.edit(key,move|p|{let t=p.tracks.get_mut(track).ok_or(EditError::InvalidTrack)?;match scope{Scope::Base=>t.level=value,Scope::Lock=>t.steps.get_mut(step).ok_or(EditError::InvalidStep)?.as_mut().ok_or(EditError::EmptyLock)?.locks_mut().level=Some(value)}Ok(())})}
}
fn clear_with_ties(t:&mut crate::model::Track,step:usize){t.steps[step]=None;cleanup_invalid_ties(t)}
fn cleanup_invalid_ties(t:&mut crate::model::Track){loop{let bad=(0..16).find(|&i|matches!(t.steps[i],Some(StepEvent::Tie{..}))&&tie_source(&t.steps,i).is_none());if let Some(i)=bad{t.steps[i]=None}else{break}}}

pub fn percentage_key(c:char)->Option<Percent>{match c{'`'=>Percent::new(0),'1'..='9'=>Percent::new(c.to_digit(10).unwrap() as u8*10),'0'=>Percent::new(100),_=>None}}

#[cfg(test)] mod tests {use super::*;
 #[test]fn undo_dirty_redo(){let mut e=Editor::new(ProjectV1::new());e.toggle_event(0,0).unwrap();assert!(e.is_dirty());assert!(e.undo());assert!(!e.is_dirty());assert!(e.redo());assert!(e.is_dirty());}
 #[test]fn edit_invalidates_redo(){let mut e=Editor::new(ProjectV1::new());e.toggle_event(0,0).unwrap();e.undo();e.toggle_event(0,1).unwrap();assert!(!e.redo());}
 #[test]fn tie_cleanup_is_atomic(){let mut e=Editor::new(ProjectV1::new());e.set_note(3,0,1).unwrap();e.toggle_tie(3,1).unwrap();e.toggle_tie(3,2).unwrap();e.clear(3,0).unwrap();assert!(e.project.tracks[3].steps[1].is_none());e.undo();assert!(matches!(e.project.tracks[3].steps[2],Some(StepEvent::Tie{..})));}
 #[test]fn direct_percent(){assert_eq!(percentage_key('`').unwrap().get(),0);assert_eq!(percentage_key('0').unwrap().get(),100);}
}
