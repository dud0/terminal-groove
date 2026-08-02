use crate::{
    audio::{Audio, AudioCommand},
    model::{
        DelayDivision, GlobalParameterId, ParameterId, ParameterValue, Percent, ProjectV1, Scale,
        StepEvent, TrackKind, Waveform,
    },
    persistence,
    reducer::{Editor, Scope},
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
};
use std::{io::stdout, path::PathBuf, sync::atomic::Ordering, time::Duration};

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, crossterm::cursor::Hide)?;
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Mode {
    Navigation,
    ParameterEdit(ParameterId),
    GlobalEdit(GlobalParameterId),
    TempoInput(String),
    FileInput(FileAction, String),
    OpenConfirm(PathBuf),
    Error(String),
    Help,
    QuitConfirm,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileAction {
    SaveAs,
    Open,
}
pub struct App {
    pub editor: Editor,
    row: usize,
    step: usize,
    global: usize,
    scope: Scope,
    mode: Mode,
    status: String,
    path: Option<PathBuf>,
    pending_open: Option<PathBuf>,
    pending_quit: bool,
    quit: bool,
    playhead: Option<usize>,
    playing: bool,
    paused: bool,
}
impl App {
    pub fn new(project: ProjectV1, path: Option<PathBuf>) -> Self {
        Self {
            editor: Editor::new(project),
            row: 0,
            step: 0,
            global: 0,
            scope: Scope::Base,
            mode: Mode::Navigation,
            status: "Ready".into(),
            path,
            pending_open: None,
            pending_quit: false,
            quit: false,
            playhead: None,
            playing: false,
            paused: false,
        }
    }
}

pub fn run(project: ProjectV1, path: Option<PathBuf>, audio: &mut Audio) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
        old_hook(info)
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new(project, path);
    while !app.quit {
        refresh_audio_status(&mut app, audio);
        terminal.draw(|f| draw(f, &app, audio))?;
        if event::poll(Duration::from_millis(8))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == event::KeyEventKind::Press {
                    handle_key(&mut app, audio, k)?
                }
            }
        }
    }
    Ok(())
}

fn handle_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    if matches!(a.mode, Mode::Error(_)) {
        if matches!(k.code, KeyCode::Esc | KeyCode::Enter) {
            a.mode = Mode::Navigation;
        }
        return Ok(());
    }
    if matches!(a.mode, Mode::FileInput(_, _)) {
        return handle_file_input(a, audio, k);
    }
    if matches!(a.mode, Mode::TempoInput(_)) {
        return handle_tempo_input(a, audio, k);
    }
    if matches!(a.mode, Mode::OpenConfirm(_)) {
        return handle_open_confirm(a, audio, k);
    }
    if a.mode == Mode::Help {
        if matches!(k.code, KeyCode::Esc | KeyCode::Char('?')) {
            a.mode = Mode::Navigation
        }
        return Ok(());
    }
    if a.mode == Mode::QuitConfirm {
        match k.code {
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if a.path.is_none() {
                    a.pending_quit = true;
                    a.mode = Mode::FileInput(FileAction::SaveAs, String::new())
                } else {
                    save(a)?;
                    if !a.editor.is_dirty() {
                        a.quit = true
                    }
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => a.quit = true,
            KeyCode::Esc | KeyCode::Char('c') => a.mode = Mode::Navigation,
            _ => {}
        }
        return Ok(());
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        match k.code {
            KeyCode::Char('q' | 'Q') => {
                if a.editor.is_dirty() {
                    a.mode = Mode::QuitConfirm
                } else {
                    a.quit = true
                }
            }
            KeyCode::Char('s' | 'S') if k.modifiers.contains(KeyModifiers::SHIFT) => {
                a.mode = Mode::FileInput(FileAction::SaveAs, String::new())
            }
            KeyCode::Char('s' | 'S') => {
                if a.path.is_some() {
                    save(a)?
                } else {
                    a.mode = Mode::FileInput(FileAction::SaveAs, String::new())
                }
            }
            KeyCode::Char('o' | 'O') => a.mode = Mode::FileInput(FileAction::Open, String::new()),
            KeyCode::Char('z' | 'Z') => {
                if audio.available_commands() == 0 {
                    a.status = "Audio command queue full; undo rejected".into();
                } else if a.editor.undo() {
                    a.status = "Undid edit".into();
                    sync_project(a, audio);
                } else {
                    a.status = "Nothing to undo".into()
                }
            }
            KeyCode::Char('y' | 'Y') => {
                if audio.available_commands() == 0 {
                    a.status = "Audio command queue full; redo rejected".into();
                } else if a.editor.redo() {
                    a.status = "Redid edit".into();
                    sync_project(a, audio);
                } else {
                    a.status = "Nothing to redo".into()
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if matches!(a.mode, Mode::ParameterEdit(_)) && handle_parameter_key(a, audio, k)? {
        return Ok(());
    }
    if matches!(a.mode, Mode::GlobalEdit(_)) && handle_global_key(a, audio, k)? {
        return Ok(());
    }
    match k.code {
        KeyCode::Char('?') => a.mode = Mode::Help,
        KeyCode::Char(' ') => {
            if audio.send(AudioCommand::PlayPause).is_ok() {
                a.playing = !a.playing;
                a.paused = !a.playing;
                a.status = if a.playing { "Playing" } else { "Paused" }.into()
            } else {
                a.status = "Audio command queue full".into()
            }
        }
        KeyCode::Char('.') => {
            if audio.send(AudioCommand::Stop).is_ok() {
                a.playing = false;
                a.paused = false;
                a.playhead = None;
                a.status = "Stopped and reset".into()
            } else {
                a.status = "Audio command queue full".into()
            }
        }
        KeyCode::Char('o') if a.row > 0 => {
            if audio
                .send(AudioCommand::Audition {
                    track: (a.row - 1) as u8,
                    step: a.step as u8,
                })
                .is_ok()
            {
                a.status = "Auditioning selection".into()
            } else {
                a.status = "Audio command queue full".into()
            }
        }
        KeyCode::Up => {
            a.row = a.row.saturating_sub(1);
            a.scope = Scope::Base
        }
        KeyCode::Down => {
            a.row = (a.row + 1).min(6);
            a.scope = Scope::Base
        }
        KeyCode::Left => {
            if a.row == 0 {
                a.global = (a.global + 5) % 6
            } else {
                a.step = (a.step + 15) % 16
            }
        }
        KeyCode::Right => {
            if a.row == 0 {
                a.global = (a.global + 1) % 6
            } else {
                a.step = (a.step + 1) % 16
            }
        }
        KeyCode::Enter if a.row == 0 => enter_global_edit(a, global_id(a.global)),
        KeyCode::Enter if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_event(track, step))
                && sync_project(a, audio)
                && !a.playing
                && a.editor.project.tracks[track].steps[step].is_some()
            {
                let _ = audio.send(AudioCommand::Audition {
                    track: track as u8,
                    step: step as u8,
                });
            }
        }
        KeyCode::Backspace | KeyCode::Delete if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.clear(track, step)) {
                sync_track(a, audio, track)
            }
        }
        KeyCode::Char('p') if a.row > 0 => {
            a.scope = if a.scope == Scope::Base {
                Scope::Lock
            } else {
                Scope::Base
            }
        }
        KeyCode::Char('m') if a.row > 0 => {
            let ti = a.row - 1;
            if audio.available_commands() == 0 {
                a.status = "Audio command queue full; edit rejected".into();
                return Ok(());
            }
            let _ = a.editor.edit(None, |p| {
                p.tracks[ti].muted = !p.tracks[ti].muted;
                Ok(())
            });
            sync_project(a, audio);
        }
        KeyCode::Char(c) if a.row == 0 => {
            if let Some(id) = global_shortcut(c) {
                a.global = id as usize;
                if id == GlobalParameterId::Scale {
                    edit_global(a, audio, id, |g| {
                        g.scale = if g.scale == Scale::Major {
                            Scale::NaturalMinor
                        } else {
                            Scale::Major
                        }
                    });
                    a.mode = Mode::Navigation;
                } else {
                    enter_global_edit(a, id)
                }
            }
        }
        KeyCode::Char('[') if a.row > 3 => change_octave(a, audio, -1),
        KeyCode::Char(']') if a.row > 3 => change_octave(a, audio, 1),
        KeyCode::Char(c) if a.row > 0 && !(a.row > 3 && (c == 't' || ('1'..='8').contains(&c))) => {
            if let Some(parameter) = parameter_shortcut(a.editor.project.tracks[a.row - 1].kind, c)
            {
                if parameter.is_waveform() {
                    toggle_waveform(a, audio)?;
                } else {
                    enter_parameter_edit(a, parameter);
                }
            }
        }
        KeyCode::Char('t') if a.row > 3 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_tie(track, step)) {
                sync_track(a, audio, track)
            }
        }
        KeyCode::Char(c @ '1'..='8') if a.row > 3 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| {
                e.set_note(track, step, c.to_digit(10).unwrap() as u8)
            }) && sync_project(a, audio)
                && !a.playing
            {
                let _ = audio.send(AudioCommand::Audition {
                    track: track as u8,
                    step: step as u8,
                });
            }
        }
        KeyCode::Esc => a.scope = Scope::Base,
        _ => {}
    }
    Ok(())
}

fn handle_parameter_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::ParameterEdit(parameter) = a.mode else {
        return Ok(false);
    };
    let track = a.row.saturating_sub(1);
    if a.row == 0 {
        a.mode = Mode::Navigation;
        return Ok(true);
    }
    match k.code {
        KeyCode::Left => {
            a.step = (a.step + 15) % 16;
            Ok(true)
        }
        KeyCode::Right => {
            a.step = (a.step + 1) % 16;
            Ok(true)
        }
        KeyCode::Up | KeyCode::Down => {
            let delta = if k.modifiers.contains(KeyModifiers::SHIFT) {
                10
            } else {
                1
            };
            let delta = if k.code == KeyCode::Up { delta } else { -delta };
            let current = match a.editor.parameter_value(track, a.step, a.scope, parameter) {
                Ok(ParameterValue::Percent(v)) => v,
                Ok(ParameterValue::Waveform(_)) => return Ok(true),
                Err(e) => {
                    a.status = e.to_string();
                    return Ok(true);
                }
            };
            let value = ParameterValue::Percent(current.saturating_add(delta));
            set_parameter(a, audio, parameter, value, true);
            Ok(true)
        }
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            Ok(true)
        }
        KeyCode::Char('p') => {
            a.scope = if a.scope == Scope::Base {
                Scope::Lock
            } else {
                Scope::Base
            };
            a.status = format!("Scope {}", scope_name(a.scope));
            Ok(true)
        }
        KeyCode::Char(' ') | KeyCode::Char('.') => Ok(false),
        KeyCode::Char(c) => {
            if let Some(next) = parameter_shortcut(a.editor.project.tracks[track].kind, c) {
                if next.is_waveform() {
                    toggle_waveform(a, audio)?;
                } else {
                    enter_parameter_edit(a, next);
                }
                return Ok(true);
            }
            if let Some(value) = crate::reducer::percentage_key(c) {
                if !parameter.is_waveform()
                    && set_parameter(a, audio, parameter, ParameterValue::Percent(value), false)
                {
                    a.mode = Mode::Navigation;
                }
            }
            Ok(true)
        }
        KeyCode::Enter | KeyCode::Esc => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation;
            a.status = "Parameter editing finished".into();
            Ok(true)
        }
        KeyCode::Backspace | KeyCode::Delete if a.scope == Scope::Lock => {
            if audio.available_commands() == 0 {
                a.status = "Audio command queue full; edit rejected".into();
                return Ok(true);
            }
            match a.editor.clear_parameter_lock(track, a.step, parameter) {
                Ok(true) => {
                    sync_step(a, audio, track, a.step);
                    a.mode = Mode::Navigation;
                }
                Ok(false) => {
                    a.status = "No lock to clear".into();
                    a.mode = Mode::Navigation;
                }
                Err(e) => a.status = e.to_string(),
            }
            Ok(true)
        }
        _ => Ok(true),
    }
}

fn parameter_shortcut(kind: TrackKind, c: char) -> Option<ParameterId> {
    match (kind, c) {
        (_, 'v') => Some(ParameterId::Level),
        (_, 'y') => Some(ParameterId::DelaySend),
        (_, 'b') => Some(ParameterId::ReverbSend),
        (TrackKind::Kick | TrackKind::Snare | TrackKind::Hat, 't') => Some(ParameterId::Tone),
        (TrackKind::Kick | TrackKind::Snare | TrackKind::Hat, 'd') => Some(ParameterId::Decay),
        (TrackKind::Synth, 'w') => Some(ParameterId::Waveform),
        (TrackKind::Synth, 'c') => Some(ParameterId::Cutoff),
        (TrackKind::Synth, 'R') => Some(ParameterId::Resonance),
        (TrackKind::Synth, 'f') => Some(ParameterId::FilterEnvelope),
        (TrackKind::Synth, 'a') => Some(ParameterId::Attack),
        (TrackKind::Synth, 'd') => Some(ParameterId::Decay),
        (TrackKind::Synth, 's') => Some(ParameterId::Sustain),
        (TrackKind::Synth, 'r') => Some(ParameterId::Release),
        _ => None,
    }
}

fn enter_parameter_edit(a: &mut App, parameter: ParameterId) {
    a.mode = Mode::ParameterEdit(parameter);
    a.status = format!("Editing {}", parameter_name(parameter));
}

fn set_parameter(
    a: &mut App,
    audio: &mut Audio,
    parameter: ParameterId,
    value: ParameterValue,
    keep_editing: bool,
) -> bool {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return false;
    }
    let track = a.row - 1;
    let step = a.step;
    let key = keep_editing.then_some(coalesce_key(track, step, parameter));
    match a
        .editor
        .set_parameter(track, step, a.scope, parameter, value, key)
    {
        Ok(true) => {
            let synced = sync_parameter_change(a, audio, track, step);
            if synced && keep_editing {
                a.status = format!("{} set", parameter_name(parameter));
            }
            synced
        }
        Ok(false) => {
            a.status = "No change".into();
            true
        }
        Err(e) => {
            a.status = e.to_string();
            false
        }
    }
}

fn coalesce_key(track: usize, step: usize, parameter: ParameterId) -> crate::reducer::CoalesceKey {
    crate::reducer::CoalesceKey(track, step, parameter as u8)
}

fn toggle_waveform(a: &mut App, audio: &mut Audio) -> Result<()> {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return Ok(());
    }
    let track = a.row - 1;
    match a.editor.toggle_waveform(track, a.step, a.scope) {
        Ok(true) => {
            if sync_parameter_change(a, audio, track, a.step) {
                a.status = format!("Waveform {}", waveform_name(a, track, a.step));
            }
        }
        Ok(false) => a.status = "No change".into(),
        Err(e) => a.status = e.to_string(),
    }
    Ok(())
}

fn waveform_name(a: &App, track: usize, step: usize) -> &'static str {
    match a
        .editor
        .parameter_value(track, step, a.scope, ParameterId::Waveform)
    {
        Ok(ParameterValue::Waveform(Waveform::Square)) => "square",
        Ok(ParameterValue::Waveform(Waveform::Saw)) => "saw",
        _ => "unchanged",
    }
}
fn apply<F: FnOnce(&mut Editor) -> Result<bool, crate::reducer::EditError>>(
    a: &mut App,
    audio: &Audio,
    f: F,
) -> bool {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return false;
    }
    match f(&mut a.editor) {
        Ok(true) => {
            a.status = "Edit applied".into();
            true
        }
        Ok(false) => {
            a.status = "No change".into();
            false
        }
        Err(e) => {
            a.status = e.to_string();
            false
        }
    }
}
fn sync_step(a: &mut App, audio: &mut Audio, track: usize, step: usize) -> bool {
    let _ = (track, step);
    sync_project(a, audio)
}
fn sync_parameter_change(a: &mut App, audio: &mut Audio, track: usize, step: usize) -> bool {
    if a.scope == Scope::Base {
        sync_track_parameters(a, audio, track)
    } else {
        sync_step(a, audio, track, step)
    }
}
fn sync_track_parameters(a: &mut App, audio: &mut Audio, track: usize) -> bool {
    let _ = track;
    sync_project(a, audio)
}
fn sync_track(a: &mut App, audio: &mut Audio, track: usize) {
    let _ = track;
    sync_project(a, audio);
}
fn sync_project(a: &mut App, audio: &mut Audio) -> bool {
    if audio.send(Audio::snapshot(&a.editor.project)).is_err() {
        a.editor.undo();
        a.status = "Audio command queue full; edit rejected".into();
        false
    } else {
        true
    }
}
fn change_octave(a: &mut App, audio: &mut Audio, d: i8) {
    let ti = a.row - 1;
    let track_name = a.editor.project.tracks[ti].name.clone();
    let old = a.editor.project.tracks[ti].input_octave.unwrap();
    let new = adjusted_octave(old, d);
    if old == new {
        a.status = format!("{track_name} input octave already at {new}");
        return;
    }
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return;
    }
    let changed = a.editor.edit(None, |p| {
        p.tracks[ti].input_octave = Some(new);
        Ok(())
    });
    if matches!(changed, Ok(true)) && sync_project(a, audio) {
        a.status = format!("{track_name} input octave: {new}");
    }
}

fn adjusted_octave(octave: u8, delta: i8) -> u8 {
    (octave as i8 + delta).clamp(0, 7) as u8
}
fn save(a: &mut App) -> Result<()> {
    if let Some(path) = a.path.clone() {
        match persistence::save_atomic(&path, &a.editor.project) {
            Ok(()) => {
                a.editor.mark_saved();
                a.status = format!("Saved {}", path.display())
            }
            Err(e) => a.status = e.to_string(),
        }
    } else {
        a.status = "No path: start with a project path to enable Ctrl+S".into()
    }
    Ok(())
}

fn resolved_path(input: &str) -> Result<PathBuf> {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
fn handle_file_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::FileInput(action, mut input) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Esc => {
            a.pending_open = None;
            a.pending_quit = false;
            a.mode = Mode::Navigation
        }
        KeyCode::Backspace => {
            input.pop();
            a.mode = Mode::FileInput(action, input)
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            input.push(c);
            a.mode = Mode::FileInput(action, input)
        }
        KeyCode::Enter => {
            if input.is_empty() {
                a.status = "Path cannot be empty".into();
                return Ok(());
            }
            match resolved_path(&input) {
                Ok(path) => match action {
                    FileAction::SaveAs => {
                        match persistence::save_atomic(&path, &a.editor.project) {
                            Ok(()) => {
                                a.path = Some(path.clone());
                                a.editor.mark_saved();
                                a.status = format!("Saved {}", path.display());
                                a.mode = Mode::Navigation;
                                if let Some(open) = a.pending_open.take() {
                                    open_project(a, audio, open)
                                } else if a.pending_quit {
                                    a.quit = true
                                }
                            }
                            Err(e) => a.mode = Mode::Error(e.to_string()),
                        }
                    }
                    FileAction::Open => {
                        if a.editor.is_dirty() {
                            a.mode = Mode::OpenConfirm(path)
                        } else {
                            open_project(a, audio, path)
                        }
                    }
                },
                Err(e) => a.mode = Mode::Error(e.to_string()),
            }
        }
        _ => {}
    }
    Ok(())
}
fn handle_open_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::OpenConfirm(path) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Char('s' | 'S') => {
            if a.path.is_none() {
                a.pending_open = Some(path);
                a.mode = Mode::FileInput(FileAction::SaveAs, String::new())
            } else {
                save(a)?;
                if !a.editor.is_dirty() {
                    open_project(a, audio, path)
                }
            }
        }
        KeyCode::Char('d' | 'D') => open_project(a, audio, path),
        KeyCode::Esc | KeyCode::Char('c' | 'C') => a.mode = Mode::Navigation,
        _ => {}
    }
    Ok(())
}
fn open_project(a: &mut App, audio: &mut Audio, path: PathBuf) {
    match persistence::load(&path) {
        Ok(project) => {
            if audio.available_commands() < 2 {
                a.mode = Mode::Error("Audio command queue full; project was not opened".into());
                return;
            }
            let _ = audio.send(AudioCommand::Stop);
            if audio.send(Audio::snapshot(&project)).is_err() {
                a.mode = Mode::Error("Audio command queue full; project was not opened".into());
                return;
            }
            a.editor.replace_loaded(project);
            a.path = Some(path.clone());
            a.row = 0;
            a.global = 0;
            a.step = 0;
            a.scope = Scope::Base;
            a.playing = false;
            a.paused = false;
            a.playhead = None;
            a.status = format!("Opened {}", path.display());
            a.mode = Mode::Navigation;
        }
        Err(e) => a.mode = Mode::Error(e.to_string()),
    }
}

fn global_id(index: usize) -> GlobalParameterId {
    [
        GlobalParameterId::Tempo,
        GlobalParameterId::DelayDivision,
        GlobalParameterId::DelayFeedback,
        GlobalParameterId::ReverbTime,
        GlobalParameterId::Key,
        GlobalParameterId::Scale,
    ][index]
}
fn global_shortcut(c: char) -> Option<GlobalParameterId> {
    match c {
        't' => Some(GlobalParameterId::Tempo),
        'y' => Some(GlobalParameterId::DelayDivision),
        'f' => Some(GlobalParameterId::DelayFeedback),
        'r' => Some(GlobalParameterId::ReverbTime),
        'k' => Some(GlobalParameterId::Key),
        's' => Some(GlobalParameterId::Scale),
        _ => None,
    }
}
fn enter_global_edit(a: &mut App, id: GlobalParameterId) {
    a.mode = if id == GlobalParameterId::Tempo {
        Mode::TempoInput(String::new())
    } else {
        Mode::GlobalEdit(id)
    };
    a.status = format!("Editing {}", global_name(id));
}
fn global_name(id: GlobalParameterId) -> &'static str {
    match id {
        GlobalParameterId::Tempo => "tempo",
        GlobalParameterId::DelayDivision => "delay division",
        GlobalParameterId::DelayFeedback => "delay feedback",
        GlobalParameterId::ReverbTime => "reverb time",
        GlobalParameterId::Key => "key",
        GlobalParameterId::Scale => "scale",
    }
}
fn edit_global<F: FnOnce(&mut crate::model::Globals)>(
    a: &mut App,
    audio: &mut Audio,
    id: GlobalParameterId,
    f: F,
) {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return;
    }
    let changed = a
        .editor
        .edit(
            Some(crate::reducer::CoalesceKey(usize::MAX, 0, id as u8)),
            |p| {
                f(&mut p.globals);
                Ok(())
            },
        )
        .unwrap_or(false);
    if changed && sync_project(a, audio) {
        a.status = format!("{} updated", global_name(id))
    }
}
fn handle_global_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::GlobalEdit(id) = a.mode else {
        return Ok(false);
    };
    match k.code {
        KeyCode::Esc | KeyCode::Enter => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation
        }
        KeyCode::Char(c) => {
            if let Some(next) = global_shortcut(c) {
                a.global = next as usize;
                enter_global_edit(a, next)
            } else if id == GlobalParameterId::DelayFeedback {
                if let Some(v) = crate::reducer::percentage_key(c) {
                    let v = Percent::new(v.get().min(95)).unwrap();
                    edit_global(a, audio, id, move |g| g.delay_feedback = v);
                    a.mode = Mode::Navigation
                }
            }
        }
        KeyCode::Up | KeyCode::Down => {
            let direction = if k.code == KeyCode::Up { 1 } else { -1 };
            match id {
                GlobalParameterId::DelayDivision => edit_global(a, audio, id, move |g| {
                    let i = DelayDivision::ALL
                        .iter()
                        .position(|x| *x == g.delay_division)
                        .unwrap() as i32;
                    g.delay_division = DelayDivision::ALL[(i + direction).clamp(0, 10) as usize]
                }),
                GlobalParameterId::DelayFeedback => {
                    let d: i16 = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        10
                    } else {
                        1
                    };
                    let direction = direction as i16;
                    edit_global(a, audio, id, move |g| {
                        g.delay_feedback = Percent::new(
                            (g.delay_feedback.get() as i16 + direction * d).clamp(0, 95) as u8,
                        )
                        .unwrap()
                    })
                }
                GlobalParameterId::ReverbTime => {
                    let d = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        1.0
                    } else {
                        0.1
                    };
                    edit_global(a, audio, id, move |g| {
                        g.reverb_time_seconds = ((g.reverb_time_seconds + direction as f32 * d)
                            * 10.0)
                            .round()
                            .clamp(2.0, 100.0)
                            / 10.0
                    })
                }
                GlobalParameterId::Key => {
                    edit_global(a, audio, id, move |g| g.key = g.key.shifted(direction))
                }
                GlobalParameterId::Scale => edit_global(a, audio, id, |g| {
                    g.scale = if g.scale == Scale::Major {
                        Scale::NaturalMinor
                    } else {
                        Scale::Major
                    }
                }),
                GlobalParameterId::Tempo => {}
            }
        }
        _ => {}
    }
    Ok(true)
}
fn handle_tempo_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::TempoInput(mut input) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Esc => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation
        }
        KeyCode::Backspace => {
            input.pop();
            a.mode = Mode::TempoInput(input)
        }
        KeyCode::Char(c @ '0'..='9') if input.len() < 3 => {
            input.push(c);
            a.mode = Mode::TempoInput(input)
        }
        KeyCode::Up | KeyCode::Down => {
            let d = if k.modifiers.contains(KeyModifiers::SHIFT) {
                5
            } else {
                1
            };
            let d = if k.code == KeyCode::Up { d } else { -d };
            edit_global(a, audio, GlobalParameterId::Tempo, move |g| {
                g.tempo_bpm = (g.tempo_bpm as i16 + d).clamp(40, 240) as u16
            })
        }
        KeyCode::Enter => match input.parse::<u16>() {
            Ok(v @ 40..=240) => {
                edit_global(a, audio, GlobalParameterId::Tempo, move |g| g.tempo_bpm = v);
                a.editor.end_coalescing();
                a.mode = Mode::Navigation
            }
            _ => a.status = "Tempo must be an integer from 40 to 240".into(),
        },
        _ => {}
    }
    Ok(())
}
fn refresh_audio_status(a: &mut App, audio: &Audio) {
    a.playing = audio.status.running.load(Ordering::Acquire);
    a.paused = audio.status.paused.load(Ordering::Acquire);
    let p = audio.status.playhead.load(Ordering::Acquire);
    a.playhead = (p < 16).then_some(p as usize);
    if audio.status.failed.load(Ordering::Acquire) {
        a.status = "Audio stream failed; editing and saving remain available".into()
    } else if audio.status.non_finite.swap(false, Ordering::AcqRel) {
        a.status = "Audio DSP produced a non-finite value; output was silenced".into()
    }
}

fn parameter_name(parameter: ParameterId) -> &'static str {
    match parameter {
        ParameterId::Level => "level",
        ParameterId::DelaySend => "delay send",
        ParameterId::ReverbSend => "reverb send",
        ParameterId::Tone => "tone",
        ParameterId::Decay => "decay",
        ParameterId::Waveform => "waveform",
        ParameterId::Cutoff => "cutoff",
        ParameterId::Resonance => "resonance",
        ParameterId::FilterEnvelope => "filter envelope",
        ParameterId::Attack => "attack",
        ParameterId::Sustain => "sustain",
        ParameterId::Release => "release",
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Base => "BASE",
        Scope::Lock => "LOCK",
    }
}

fn mode_name(mode: &Mode) -> String {
    match mode {
        Mode::Navigation => "Navigation".into(),
        Mode::ParameterEdit(parameter) => {
            format!("Parameter edit ({})", parameter_name(*parameter))
        }
        Mode::GlobalEdit(id) => format!("Global edit ({})", global_name(*id)),
        Mode::TempoInput(_) => "Tempo numeric input".into(),
        Mode::FileInput(_, _) => "File-path input".into(),
        Mode::OpenConfirm(_) => "Unsaved confirmation".into(),
        Mode::Error(_) => "Error dialog".into(),
        Mode::Help => "Help".into(),
        Mode::QuitConfirm => "Unsaved confirmation".into(),
    }
}

fn waveform_short(waveform: Waveform) -> &'static str {
    match waveform {
        Waveform::Square => "Q",
        Waveform::Saw => "S",
    }
}

fn parameter_summary(t: &crate::model::Track) -> String {
    match &t.instrument {
        crate::model::Instrument::Drum(p) => format!(
            "L{}Y{}B{}T{}D{}",
            t.level.get(),
            t.delay_send.get(),
            t.reverb_send.get(),
            p.tone.get(),
            p.decay.get()
        ),
        crate::model::Instrument::Synth(p) => format!(
            "L{}Y{}B{}W{}C{}Q{}F{}A{}D{}S{}R{}",
            t.level.get(),
            t.delay_send.get(),
            t.reverb_send.get(),
            waveform_short(p.waveform),
            p.cutoff.get(),
            p.resonance.get(),
            p.filter_envelope.get(),
            p.attack.get(),
            p.decay.get(),
            p.sustain.get(),
            p.release.get()
        ),
    }
}

fn track_label(t: &crate::model::Track) -> String {
    if t.kind == TrackKind::Synth {
        format!("{} O{}", t.name, t.input_octave.unwrap_or(3))
    } else {
        t.name.clone()
    }
}

fn step_cell(event: Option<&StepEvent>) -> String {
    match event {
        None => " . ".into(),
        Some(StepEvent::Trigger { locks }) if locks.is_empty() => " x ".into(),
        Some(StepEvent::Trigger { .. }) => "x* ".into(),
        Some(StepEvent::Note {
            degree,
            octave,
            locks,
        }) => format!(
            "{degree}{}{octave}",
            if locks.is_empty() { ':' } else { '*' }
        ),
        Some(StepEvent::Tie { locks }) if locks.is_empty() => " - ".into(),
        Some(StepEvent::Tie { .. }) => "-* ".into(),
    }
}

fn value_text(value: ParameterValue) -> String {
    match value {
        ParameterValue::Percent(value) => value.get().to_string(),
        ParameterValue::Waveform(value) => waveform_short(value).into(),
    }
}

fn effective_parameter_summary(a: &App, track: usize, step: usize) -> String {
    let t = &a.editor.project.tracks[track];
    let value = |parameter| {
        a.editor
            .parameter_value(track, step, Scope::Lock, parameter)
            .or_else(|_| {
                a.editor
                    .parameter_value(track, step, Scope::Base, parameter)
            })
            .map(value_text)
            .unwrap_or_else(|_| "-".into())
    };
    let common = format!(
        "L{}Y{}B{}",
        value(ParameterId::Level),
        value(ParameterId::DelaySend),
        value(ParameterId::ReverbSend)
    );
    match t.kind {
        TrackKind::Kick | TrackKind::Snare | TrackKind::Hat => format!(
            "{}T{}D{}",
            common,
            value(ParameterId::Tone),
            value(ParameterId::Decay)
        ),
        TrackKind::Synth => format!(
            "{}W{}C{}Q{}F{}A{}D{}S{}R{}",
            common,
            value(ParameterId::Waveform),
            value(ParameterId::Cutoff),
            value(ParameterId::Resonance),
            value(ParameterId::FilterEnvelope),
            value(ParameterId::Attack),
            value(ParameterId::Decay),
            value(ParameterId::Sustain),
            value(ParameterId::Release)
        ),
    }
}

fn physical_parameter_summary(a: &App, track: usize, step: usize) -> String {
    let t = &a.editor.project.tracks[track];
    let value = |parameter| {
        a.editor
            .parameter_value(track, step, Scope::Lock, parameter)
            .or_else(|_| {
                a.editor
                    .parameter_value(track, step, Scope::Base, parameter)
            })
            .ok()
    };
    let percent = |parameter| match value(parameter) {
        Some(ParameterValue::Percent(v)) => v.get(),
        _ => 0,
    };
    match t.kind {
        TrackKind::Kick => format!(
            "Physical: peak {:.0} Hz, fundamental {:.0} Hz, decay {:.0} ms",
            75.0 + percent(ParameterId::Tone) as f32 * 1.45,
            38.0 + percent(ParameterId::Tone) as f32 * 0.20,
            80.0 + percent(ParameterId::Decay) as f32 * 7.5
        ),
        TrackKind::Snare => format!(
            "Physical: body {:.0} Hz, noise {:.0} Hz, decay {:.0} ms",
            145.0 + percent(ParameterId::Tone) as f32 * 1.7,
            800.0 + percent(ParameterId::Tone) as f32 * 52.0,
            50.0 + percent(ParameterId::Decay) as f32 * 5.0
        ),
        TrackKind::Hat => format!(
            "Physical: high-pass {:.1} kHz, decay {:.0} ms",
            2.8 + percent(ParameterId::Tone) as f32 * 0.09,
            25.0 + percent(ParameterId::Decay) as f32 * 3.2
        ),
        TrackKind::Synth => {
            let cutoff = crate::dsp::exp_map(percent(ParameterId::Cutoff), 20.0, 20_000.0);
            let time = |parameter, min, max| crate::dsp::exp_map(percent(parameter), min, max);
            format!(
                "Physical: cutoff {:.0} Hz, Q {:.2}, A {:.3}s D {:.3}s R {:.3}s",
                cutoff,
                0.707 + percent(ParameterId::Resonance) as f32 / 100.0 * (10.0 - 0.707),
                if percent(ParameterId::Attack) == 0 {
                    0.0
                } else {
                    time(ParameterId::Attack, 0.001, 2.0)
                },
                time(ParameterId::Decay, 0.005, 3.0),
                time(ParameterId::Release, 0.005, 5.0)
            )
        }
    }
}

fn lock_names(event: &StepEvent) -> String {
    let locks = event.locks();
    let mut names = Vec::new();
    if locks.level.is_some() {
        names.push("L");
    }
    if locks.delay_send.is_some() {
        names.push("Y");
    }
    if locks.reverb_send.is_some() {
        names.push("B");
    }
    if locks.tone.is_some() {
        names.push("T");
    }
    if locks.decay.is_some() {
        names.push("D");
    }
    if locks.waveform.is_some() {
        names.push("W");
    }
    if locks.cutoff.is_some() {
        names.push("C");
    }
    if locks.resonance.is_some() {
        names.push("Q");
    }
    if locks.filter_envelope.is_some() {
        names.push("F");
    }
    if locks.attack.is_some() {
        names.push("A");
    }
    if locks.sustain.is_some() {
        names.push("S");
    }
    if locks.release.is_some() {
        names.push("R");
    }
    if names.is_empty() {
        "none".into()
    } else {
        names.join(" ")
    }
}

fn draw(f: &mut ratatui::Frame, a: &App, audio: &Audio) {
    draw_with_device(f, a, &audio.device_name);
}

fn draw_with_device(f: &mut ratatui::Frame, a: &App, device_name: &str) {
    let area = f.area();
    if area.width < 100 || area.height < 24 {
        f.render_widget(
            Paragraph::new(format!(
                "terminal-groove needs 100x24\nCurrent: {}x{}\nCtrl+Q quit  ? help",
                area.width, area.height
            ))
            .block(Block::bordered().title("Terminal too small")),
            area,
        );
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);
    let dirty = if a.editor.is_dirty() { " *" } else { "" };
    let file = a
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled");
    let transport = if a.playing {
        "PLAY"
    } else if a.paused {
        "PAUSE"
    } else {
        "STOP"
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " terminal-groove ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {file}{dirty} | audio: {} | {transport} | {} BPM",
                device_name, a.editor.project.globals.tempo_bpm
            )),
        ])),
        chunks[0],
    );
    let g = &a.editor.project.globals;
    let globals = [
        format!("Tempo {}", g.tempo_bpm),
        format!("Delay {}", g.delay_division),
        format!("Feedback {}", g.delay_feedback),
        format!("Reverb {:.1}s", g.reverb_time_seconds),
        format!("Key {}", g.key),
        format!("Scale {}", g.scale),
    ];
    let line = globals
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Span::styled(
                format!(" {s} "),
                if a.row == 0 && a.global == i {
                    Style::default().reversed()
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(Line::from(line)).block(Block::bordered().title("Globals")),
        chunks[1],
    );
    let rows = a.editor.project.tracks.iter().enumerate().map(|(ti, t)| {
        let mut cells: Vec<ratatui::widgets::Cell> = Vec::with_capacity(19);
        cells.push(if t.muted { "M".into() } else { " ".into() });
        cells.push(track_label(t).into());
        for (si, s) in t.steps.iter().enumerate() {
            let mut style = Style::default();
            if a.row == ti + 1 && a.step == si {
                style = style.reversed()
            }
            if a.playhead == Some(si) {
                style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD)
            }
            cells.push(ratatui::widgets::Cell::from(step_cell(s.as_ref())).style(style))
        }
        cells.push(ratatui::widgets::Cell::from(parameter_summary(t)));
        Row::new(cells)
    });
    let mut widths = vec![Constraint::Length(1), Constraint::Length(10)];
    widths.extend((0..16).map(|_| Constraint::Length(3)));
    widths.push(Constraint::Length(38));
    f.render_widget(
        Table::new(rows, widths)
            .column_spacing(0)
            .header(Row::new(
                std::iter::once(" ")
                    .chain(std::iter::once("Track / O"))
                    .chain((1..=16).map(|n| if n % 4 == 1 { " | " } else { "   " }))
                    .chain(std::iter::once("Base params (%)")),
            ))
            .block(
                Block::bordered()
                    .title("Pattern  . empty  x trigger  D:O note  D*O locked note  - tie  * lock"),
            ),
        chunks[2],
    );
    let detail = if a.row == 0 {
        format!("GLOBAL | selected {}", globals[a.global])
    } else {
        let track = a.row - 1;
        let t = &a.editor.project.tracks[track];
        let locks = t.steps[a.step]
            .as_ref()
            .map(lock_names)
            .unwrap_or_else(|| "none".into());
        let input_octave = if t.kind == TrackKind::Synth {
            format!(" | input octave {}", t.input_octave.unwrap_or(3))
        } else {
            String::new()
        };
        format!(
            "{} | step {} | {:?} | mute {} | locks: {}{}\nBase     {}\nEffective {}\n{}",
            scope_name(a.scope),
            a.step + 1,
            t.kind,
            t.muted,
            locks,
            input_octave,
            parameter_summary(t),
            effective_parameter_summary(a, track, a.step),
            physical_parameter_summary(a, track, a.step)
        )
    };
    f.render_widget(
        Paragraph::new(detail).block(Block::bordered().title("Parameter detail")),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new(format!("Mode: {} | {}", mode_name(&a.mode), a.status)),
        chunks[4],
    );
    f.render_widget(Paragraph::new("↑↓ row  ←→ step/control  Enter event  1-8 note  [ ] input octave  t tie  p BASE/LOCK  m mute  Space play/pause  . stop  ? help  Ctrl+S save  Ctrl+Q quit\nParams: V level  Y delay  B reverb  T tone  D decay  W waveform(S/Q)  C cutoff  Shift+R resonance  F filter-env  A attack  S sustain  R release").wrap(Wrap{trim:true}),chunks[5]);
    if a.mode == Mode::Help {
        popup(
            f,
            area,
            "Help",
            "All sound is synthesized.\nNavigation: arrows, Enter, Delete.\nTracks: p scope, v level, m mute, y delay, b reverb.\nDrums: t tone, d decay. Synth: 1-8 note, [ ] input octave, t tie, w waveform, c cutoff, R resonance, f envelope, a/d/s/r ADSR.\nGlobal: t tempo, y delay, f feedback, r reverb, k key, s scale.\nAnywhere: Space play/pause, . stop, o audition, Ctrl+S save, Ctrl+O open, Ctrl+Z/Y undo/redo, Ctrl+Q quit.\nEsc or ? closes help.",
        )
    }
    if a.mode == Mode::QuitConfirm {
        popup(
            f,
            area,
            "Unsaved changes",
            "Save [S]  Discard [D]  Cancel [Esc]",
        )
    }
    match &a.mode {
        Mode::TempoInput(input) => popup(
            f,
            area,
            "Tempo numeric input",
            &format!("Tempo: {input}_\nEnter confirms  Esc cancels  ↑/↓ adjusts current tempo"),
        ),
        Mode::FileInput(action, input) => {
            let title = if *action == FileAction::Open {
                "Open project"
            } else {
                "Save project as"
            };
            let resolved = resolved_path(input)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            popup(
                f,
                area,
                title,
                &format!("Path: {input}_\nResolved: {resolved}\nEnter confirms  Esc cancels"),
            );
        }
        Mode::OpenConfirm(path) => popup(
            f,
            area,
            "Unsaved changes",
            &format!(
                "Open {}?\nSave [S]  Discard [D]  Cancel [Esc]",
                path.display()
            ),
        ),
        Mode::Error(message) => popup(
            f,
            area,
            "Error",
            &format!("{message}\n\nEnter or Esc closes"),
        ),
        _ => {}
    }
}
fn popup(f: &mut ratatui::Frame, area: Rect, title: &str, text: &str) {
    let r = Rect {
        x: area.x + 10,
        y: area.y + 5,
        width: area.width - 20,
        height: (area.height - 10).max(5),
    };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        r,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn rendered(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_with_device(frame, app, "null"))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn parameter_shortcuts_follow_track_context() {
        assert_eq!(
            parameter_shortcut(TrackKind::Kick, 'v'),
            Some(ParameterId::Level)
        );
        assert_eq!(
            parameter_shortcut(TrackKind::Kick, 't'),
            Some(ParameterId::Tone)
        );
        assert_eq!(
            parameter_shortcut(TrackKind::Synth, 'd'),
            Some(ParameterId::Decay)
        );
        assert_eq!(
            parameter_shortcut(TrackKind::Synth, 'R'),
            Some(ParameterId::Resonance)
        );
        assert_eq!(parameter_shortcut(TrackKind::Synth, 't'), None);
    }

    #[test]
    fn parameter_summary_includes_every_track_value() {
        let project = ProjectV1::new();
        let drum = parameter_summary(&project.tracks[0]);
        assert_eq!(drum, "L80Y0B0T50D35");
        let synth = parameter_summary(&project.tracks[3]);
        assert_eq!(synth, "L80Y0B0WSC65Q10F25A0D25S70R15");
    }

    #[test]
    fn screen_renders_octaves_and_parameters_at_minimum_size() {
        let backend = TestBackend::new(100, 24);
        let mut project = ProjectV1::new();
        project.tracks[3].input_octave = Some(4);
        project.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            locks: Default::default(),
        });
        project.tracks[3].steps[1] = Some(StepEvent::Note {
            degree: 2,
            octave: 4,
            locks: crate::model::ParameterLocks {
                cutoff: Some(Percent::new(50).unwrap()),
                ..Default::default()
            },
        });
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::new(project, None);
        terminal
            .draw(|frame| draw_with_device(frame, &app, "null"))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("L80Y0B0T50D35"));
        assert!(rendered.contains("L80Y0B0WSC65Q10F25A0D25S70R15"));
        assert!(rendered.contains("Synth 1 O4"));
        assert!(rendered.contains("1:3"));
        assert!(rendered.contains("2*4"));
    }

    #[test]
    fn small_terminal_replaces_main_layout() {
        let app = App::new(ProjectV1::new(), None);
        let screen = rendered(&app, 99, 24);
        assert!(screen.contains("terminal-groove needs 100x24"));
        assert!(screen.contains("Current: 99x24"));
    }

    #[test]
    fn step_cells_show_note_octaves_and_locks() {
        assert_eq!(step_cell(None), " . ");
        assert_eq!(
            step_cell(Some(&StepEvent::Note {
                degree: 1,
                octave: 3,
                locks: Default::default(),
            })),
            "1:3"
        );
        assert_eq!(
            step_cell(Some(&StepEvent::Note {
                degree: 2,
                octave: 4,
                locks: crate::model::ParameterLocks {
                    cutoff: Some(Percent::new(50).unwrap()),
                    ..Default::default()
                },
            })),
            "2*4"
        );
        assert_eq!(
            step_cell(Some(&StepEvent::Tie {
                locks: Default::default(),
            })),
            " - "
        );
    }

    #[test]
    fn input_octave_adjustment_clamps_to_supported_range() {
        assert_eq!(adjusted_octave(3, 1), 4);
        assert_eq!(adjusted_octave(0, -1), 0);
        assert_eq!(adjusted_octave(7, 1), 7);
    }

    #[test]
    fn every_overlay_mode_has_a_visible_name() {
        assert_eq!(mode_name(&Mode::Navigation), "Navigation");
        assert_eq!(
            mode_name(&Mode::TempoInput(String::new())),
            "Tempo numeric input"
        );
        assert_eq!(
            mode_name(&Mode::FileInput(FileAction::Open, String::new())),
            "File-path input"
        );
        assert_eq!(mode_name(&Mode::Error("bad".into())), "Error dialog");
        assert_eq!(mode_name(&Mode::Help), "Help");
    }

    #[test]
    fn global_shortcuts_select_all_six_controls() {
        assert_eq!(global_shortcut('t'), Some(GlobalParameterId::Tempo));
        assert_eq!(global_shortcut('y'), Some(GlobalParameterId::DelayDivision));
        assert_eq!(global_shortcut('f'), Some(GlobalParameterId::DelayFeedback));
        assert_eq!(global_shortcut('r'), Some(GlobalParameterId::ReverbTime));
        assert_eq!(global_shortcut('k'), Some(GlobalParameterId::Key));
        assert_eq!(global_shortcut('s'), Some(GlobalParameterId::Scale));
        assert_eq!(global_shortcut('v'), None);
    }
}
