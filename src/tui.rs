use crate::{audio::{Audio,AudioCommand},model::{ProjectV1,StepEvent},persistence,reducer::{Editor,Scope}};
use anyhow::Result;
use crossterm::{event::{self,Event,KeyCode,KeyEvent,KeyModifiers},execute,terminal::{EnterAlternateScreen,LeaveAlternateScreen,disable_raw_mode,enable_raw_mode}};
use ratatui::{Terminal,backend::CrosstermBackend,layout::{Constraint,Direction,Layout,Rect},style::{Color,Modifier,Style},text::{Line,Span},widgets::{Block,Borders,Clear,Paragraph,Row,Table,Wrap}};
use std::{io::stdout,path::PathBuf,time::Duration};

struct TerminalGuard;
impl TerminalGuard{fn enter()->Result<Self>{enable_raw_mode()?;execute!(stdout(),EnterAlternateScreen,crossterm::cursor::Hide)?;Ok(Self)}}
impl Drop for TerminalGuard{fn drop(&mut self){let _=disable_raw_mode();let _=execute!(stdout(),crossterm::cursor::Show,LeaveAlternateScreen);}}

#[derive(Clone,Copy,PartialEq,Eq)] enum Mode{Navigation,Help,QuitConfirm}
pub struct App {pub editor:Editor,row:usize,step:usize,global:usize,scope:Scope,mode:Mode,status:String,path:Option<PathBuf>,quit:bool,playhead:Option<usize>,playing:bool}
impl App{pub fn new(project:ProjectV1,path:Option<PathBuf>)->Self{Self{editor:Editor::new(project),row:0,step:0,global:0,scope:Scope::Base,mode:Mode::Navigation,status:"Ready".into(),path,quit:false,playhead:None,playing:false}}

pub fn run(project:ProjectV1,path:Option<PathBuf>,audio:&mut Audio)->Result<()> {
 let _guard=TerminalGuard::enter()?;let old_hook=std::panic::take_hook();std::panic::set_hook(Box::new(move|info|{let _=disable_raw_mode();let _=execute!(stdout(),crossterm::cursor::Show,LeaveAlternateScreen);old_hook(info)}));
 let mut terminal=Terminal::new(CrosstermBackend::new(stdout()))?;let mut app=App::new(project,path);
 while !app.quit {terminal.draw(|f|draw(f,&app,audio))?;if event::poll(Duration::from_millis(8))? {if let Event::Key(k)=event::read()?{if k.kind==event::KeyEventKind::Press{handle_key(&mut app,audio,k)?}}}}
 Ok(())
}

fn handle_key(a:&mut App,audio:&mut Audio,k:KeyEvent)->Result<()>{
 if a.mode==Mode::Help{if matches!(k.code,KeyCode::Esc|KeyCode::Char('?')){a.mode=Mode::Navigation}return Ok(())}
 if a.mode==Mode::QuitConfirm{match k.code{KeyCode::Char('s')|KeyCode::Char('S')=>{save(a)?;if !a.editor.is_dirty(){a.quit=true}},KeyCode::Char('d')|KeyCode::Char('D')=>a.quit=true,KeyCode::Esc|KeyCode::Char('c')=>a.mode=Mode::Navigation,_=>{}}return Ok(())}
 if k.modifiers.contains(KeyModifiers::CONTROL){match k.code{KeyCode::Char('q')=>{if a.editor.is_dirty(){a.mode=Mode::QuitConfirm}else{a.quit=true}},KeyCode::Char('s')=>save(a)?,KeyCode::Char('z')=>{if a.editor.undo(){a.status="Undid edit".into();sync_project(a,audio)}else{a.status="Nothing to undo".into()}},KeyCode::Char('y')=>{if a.editor.redo(){a.status="Redid edit".into();sync_project(a,audio)}else{a.status="Nothing to redo".into()}},_=>{}}return Ok(())}
 match k.code{
  KeyCode::Char('?')=>a.mode=Mode::Help,KeyCode::Char(' ')=>{if audio.send(AudioCommand::PlayPause).is_ok(){a.playing=!a.playing;a.status=if a.playing{"Playing"}else{"Paused"}.into()}else{a.status="Audio command queue full".into()}},KeyCode::Char('.')=>{if audio.send(AudioCommand::Stop).is_ok(){a.playing=false;a.playhead=None;a.status="Stopped and reset".into()}else{a.status="Audio command queue full".into()}},
  KeyCode::Up=>{a.row=a.row.saturating_sub(1);a.scope=Scope::Base},KeyCode::Down=>{a.row=(a.row+1).min(6);a.scope=Scope::Base},
  KeyCode::Left=>if a.row==0{a.global=(a.global+5)%6}else{a.step=(a.step+15)%16},KeyCode::Right=>if a.row==0{a.global=(a.global+1)%6}else{a.step=(a.step+1)%16},
  KeyCode::Enter if a.row>0=>{let (track,step)=(a.row-1,a.step);if apply(a,|e|e.toggle_event(track,step)){sync_step(a,audio,track,step)}},KeyCode::Backspace|KeyCode::Delete if a.row>0=>{let (track,step)=(a.row-1,a.step);if apply(a,|e|e.clear(track,step)){sync_track(a,audio,track)}},
  KeyCode::Char('p') if a.row>0=>{a.scope=if a.scope==Scope::Base{Scope::Lock}else{Scope::Base}},KeyCode::Char('m') if a.row>0=>{let ti=a.row-1;let _=a.editor.edit(None,|p|{p.tracks[ti].muted=!p.tracks[ti].muted;Ok(())});let muted=a.editor.project.tracks[ti].muted;if audio.send(AudioCommand::SetMute{track:ti as u8,muted}).is_err(){a.editor.undo();a.status="Audio command queue full; edit rejected".into()}},
  KeyCode::Char('t') if a.row>3=>{let (track,step)=(a.row-1,a.step);if apply(a,|e|e.toggle_tie(track,step)){sync_track(a,audio,track)}},KeyCode::Char(c @ '1'..='8') if a.row>3=>{let (track,step)=(a.row-1,a.step);if apply(a,|e|e.set_note(track,step,c.to_digit(10).unwrap() as u8)){sync_track(a,audio,track)}},
  KeyCode::Char('[') if a.row>3=>change_octave(a,-1),KeyCode::Char(']') if a.row>3=>change_octave(a,1),KeyCode::Esc=>a.scope=Scope::Base,_=>{}
 } Ok(())
}
fn apply<F:FnOnce(&mut Editor)->Result<bool,crate::reducer::EditError>>(a:&mut App,f:F)->bool{match f(&mut a.editor){Ok(true)=>{a.status="Edit applied".into();true},Ok(false)=>{a.status="No change".into();false},Err(e)=>{a.status=e.to_string();false}}}
fn sync_step(a:&mut App,audio:&mut Audio,track:usize,step:usize){let event=a.editor.project.tracks[track].steps[step].clone();if audio.send(AudioCommand::SetStep{track:track as u8,step:step as u8,event}).is_err(){a.editor.undo();a.status="Audio command queue full; edit rejected".into()}}
fn sync_track(a:&mut App,audio:&mut Audio,track:usize){if audio.available_commands()<16{a.editor.undo();a.status="Audio command queue full; edit rejected".into();return}for step in 0..16{let event=a.editor.project.tracks[track].steps[step].clone();let _=audio.send(AudioCommand::SetStep{track:track as u8,step:step as u8,event});}}
fn sync_project(a:&mut App,audio:&mut Audio){for track in 0..6{for step in 0..16{let event=a.editor.project.tracks[track].steps[step].clone();if audio.send(AudioCommand::SetStep{track:track as u8,step:step as u8,event}).is_err(){a.status="Audio command queue full while synchronizing".into();return}}let muted=a.editor.project.tracks[track].muted;let _=audio.send(AudioCommand::SetMute{track:track as u8,muted});}}
fn change_octave(a:&mut App,d:i8){let ti=a.row-1;let _=a.editor.edit(None,|p|{let old=p.tracks[ti].input_octave.unwrap();p.tracks[ti].input_octave=Some((old as i8+d).clamp(0,7) as u8);Ok(())});}
fn save(a:&mut App)->Result<()> {if let Some(path)=a.path.clone(){match persistence::save_atomic(&path,&a.editor.project){Ok(())=>{a.editor.mark_saved();a.status=format!("Saved {}",path.display())},Err(e)=>a.status=e.to_string()}}else{a.status="No path: start with a project path to enable Ctrl+S".into()}Ok(())}

fn draw(f:&mut ratatui::Frame,a:&App,audio:&Audio){let area=f.area();if area.width<80||area.height<24{f.render_widget(Paragraph::new(format!("terminal-groove needs 80x24\nCurrent: {}x{}\nCtrl+Q quit  ? help",area.width,area.height)).block(Block::bordered().title("Terminal too small")),area);return}
 let chunks=Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(2),Constraint::Length(3),Constraint::Length(9),Constraint::Length(5),Constraint::Length(2),Constraint::Min(1)]).split(area);
 let dirty=if a.editor.is_dirty(){" *"}else{""};let file=a.path.as_ref().and_then(|p|p.file_name()).and_then(|n|n.to_str()).unwrap_or("Untitled");let transport=if a.playing{"PLAY"}else{"STOP/PAUSE"};
 f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" terminal-groove ",Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),Span::raw(format!(" {file}{dirty} | audio: {} | {transport} | {} BPM",audio.device_name,a.editor.project.globals.tempo_bpm))])),chunks[0]);
 let g=&a.editor.project.globals;let globals=[format!("Tempo {}",g.tempo_bpm),format!("Delay {}",g.delay_division),format!("Feedback {}",g.delay_feedback),format!("Reverb {:.1}s",g.reverb_time_seconds),format!("Key {}",g.key),format!("Scale {}",g.scale)];let line=globals.iter().enumerate().map(|(i,s)|Span::styled(format!(" {s} "),if a.row==0&&a.global==i{Style::default().reversed()}else{Style::default()})).collect::<Vec<_>>();f.render_widget(Paragraph::new(Line::from(line)).block(Block::bordered().title("Globals")),chunks[1]);
 let rows=a.editor.project.tracks.iter().enumerate().map(|(ti,t)|{let mut cells:Vec<ratatui::widgets::Cell>=Vec::with_capacity(18);cells.push(if t.muted{"M".into()}else{" ".into()});cells.push(t.name.clone().into());for (si,s) in t.steps.iter().enumerate(){let (symbol,lock)=match s{None=>(".",false),Some(StepEvent::Trigger{locks})=>("x",!locks.is_empty()),Some(StepEvent::Note{degree,locks,..})=>(("12345678".get(*degree as usize-1..*degree as usize).unwrap()),!locks.is_empty()),Some(StepEvent::Tie{locks})=>("-",!locks.is_empty())};let text=if lock{format!("{symbol}*")}else{format!(" {symbol}")};let mut style=Style::default();if a.row==ti+1&&a.step==si{style=style.reversed()}if a.playhead==Some(si){style=style.fg(Color::Yellow).add_modifier(Modifier::BOLD)}cells.push(ratatui::widgets::Cell::from(text).style(style))}Row::new(cells)});
 let mut widths=vec![Constraint::Length(1),Constraint::Length(8)];widths.extend((0..16).map(|_|Constraint::Length(2)));f.render_widget(Table::new(rows,widths).header(Row::new(std::iter::once(" ").chain(std::iter::once("Track")).chain((1..=16).map(|n|if n%4==1{"|"}else{" "})))).block(Block::bordered().title("Pattern  . empty  x trigger  1-8 note  - tie  * lock")),chunks[2]);
 let detail=if a.row==0{format!("GLOBAL | selected {}",globals[a.global])}else{let t=&a.editor.project.tracks[a.row-1];format!("{} | step {} | {:?} | level {} mute {} delay {} reverb {}",if a.scope==Scope::Base{"BASE"}else{"LOCK"},a.step+1,t.kind,t.level,t.muted,t.delay_send,t.reverb_send)};f.render_widget(Paragraph::new(detail).block(Block::bordered().title("Parameter detail")),chunks[3]);
 f.render_widget(Paragraph::new(format!("Mode: {} | {}",match a.mode{Mode::Navigation=>"Navigation",Mode::Help=>"Help",Mode::QuitConfirm=>"Unsaved confirmation"},a.status)),chunks[4]);f.render_widget(Paragraph::new("↑↓ row  ←→ step/control  Enter event  1-8 note  t tie  p BASE/LOCK  m mute  Space play/pause  . stop  ? help  Ctrl+S save  Ctrl+Q quit").wrap(Wrap{trim:true}),chunks[5]);
 if a.mode==Mode::Help{popup(f,area,"Help","All sound is synthesized.\nNavigation: arrows, Enter, Delete.\nTracks: p scope, v level, m mute, y delay, b reverb.\nDrums: t tone, d decay. Synth: 1-8 note, [ ] octave, t tie, w waveform, c cutoff, R resonance, f envelope, a/d/s/r ADSR.\nGlobal: t tempo, y delay, f feedback, r reverb, k key, s scale.\nAnywhere: Space play/pause, . stop, o audition, Ctrl+S save, Ctrl+O open, Ctrl+Z/Y undo/redo, Ctrl+Q quit.\nEsc or ? closes help.")}
 if a.mode==Mode::QuitConfirm{popup(f,area,"Unsaved changes","Save [S]  Discard [D]  Cancel [Esc]")}
}
fn popup(f:&mut ratatui::Frame,area:Rect,title:&str,text:&str){let r=Rect{x:area.x+10,y:area.y+5,width:area.width-20,height:(area.height-10).max(5)};f.render_widget(Clear,r);f.render_widget(Paragraph::new(text).wrap(Wrap{trim:false}).block(Block::default().borders(Borders::ALL).title(title)),r)}
