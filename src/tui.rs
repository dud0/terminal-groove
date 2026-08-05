use crate::{
    audio::{Audio, AudioCommand, ParameterSmoothing},
    model::{
        ChordShape, ChorusMode, DelayDivision, GlobalParameterId, LfoConfig, LfoDivision, LfoRate,
        LfoWaveform, MAX_STEP_COUNT, ParameterId, ParameterValue, Percent, Project, STEP_BANK_SIZE,
        STEP_ROW_SIZE, Scale, StepEvent, TRACK_COUNT, TrackKind, Waveform,
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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Row, Table, Wrap},
};
use std::{
    io::stdout,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const DIRECT_PARAMETER_RAMP: Duration = Duration::from_millis(30);

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

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Navigation,
    PatternDialog,
    ParameterEdit(ParameterId),
    LfoEdit {
        parameter: ParameterId,
        field: LfoField,
    },
    ChordEdit {
        shape: ChordShape,
    },
    GlobalEdit(GlobalParameterId),
    TempoInput(String),
    TrackLengthInput(String),
    FileInput(FileAction, String),
    OpenConfirm(PathBuf),
    Error(String),
    Help,
    QuitConfirm,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LfoField {
    Enabled,
    Waveform,
    RateMode,
    Rate,
    Depth,
}

impl LfoField {
    const ALL: [Self; 5] = [
        Self::Enabled,
        Self::Waveform,
        Self::RateMode,
        Self::Rate,
        Self::Depth,
    ];
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    playheads: [Option<usize>; TRACK_COUNT],
    playing: bool,
    paused: bool,
    pattern_cursor: usize,
    active_pattern: usize,
    queued_pattern: Option<usize>,
    fader_animations: Vec<FaderAnimation>,
}

#[derive(Clone, Copy)]
struct FaderAnimation {
    track: usize,
    step: usize,
    scope: Scope,
    parameter: ParameterId,
    from: Percent,
    to: Percent,
    started: Instant,
}
impl FaderAnimation {
    fn value_at(self, now: Instant) -> Percent {
        let elapsed = now.saturating_duration_since(self.started);
        let progress = (elapsed.as_secs_f32() / DIRECT_PARAMETER_RAMP.as_secs_f32()).min(1.0);
        Percent::new(
            (self.from.get() as f32 + (self.to.get() as f32 - self.from.get() as f32) * progress)
                .round() as u8,
        )
        .unwrap()
    }
    fn is_complete(self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= DIRECT_PARAMETER_RAMP
    }
}
impl App {
    pub fn new(project: Project, path: Option<PathBuf>) -> Self {
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
            playheads: [None; TRACK_COUNT],
            playing: false,
            paused: false,
            pattern_cursor: 0,
            active_pattern: 0,
            queued_pattern: None,
            fader_animations: Vec::new(),
        }
    }
    fn start_fader_animation(
        &mut self,
        track: usize,
        step: usize,
        parameter: ParameterId,
        from: Percent,
        to: Percent,
    ) {
        let now = Instant::now();
        let scope = self.scope;
        if let Some(existing) = self.fader_animations.iter_mut().find(|animation| {
            animation.track == track
                && animation.step == step
                && animation.scope == scope
                && animation.parameter == parameter
        }) {
            existing.from = existing.value_at(now);
            existing.to = to;
            existing.started = now;
        } else {
            self.fader_animations.push(FaderAnimation {
                track,
                step,
                scope,
                parameter,
                from,
                to,
                started: now,
            });
        }
    }
    fn animated_percent(
        &self,
        track: usize,
        step: usize,
        parameter: ParameterId,
        origin: ValueOrigin,
        target: Percent,
    ) -> Percent {
        let now = Instant::now();
        self.fader_animations
            .iter()
            .rev()
            .find(|animation| {
                animation.track == track
                    && animation.parameter == parameter
                    && animation.to == target
                    && match animation.scope {
                        Scope::Base => origin == ValueOrigin::Base,
                        Scope::Lock => {
                            self.scope == Scope::Lock
                                && animation.step == step
                                && origin == ValueOrigin::Lock
                        }
                    }
            })
            .map(|animation| animation.value_at(now))
            .unwrap_or(target)
    }
}

pub fn run(project: Project, path: Option<PathBuf>, audio: &mut Audio) -> Result<()> {
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
        app.fader_animations
            .retain(|animation| !animation.is_complete(Instant::now()));
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
    if matches!(a.mode, Mode::TrackLengthInput(_)) {
        return handle_track_length_input(a, audio, k);
    }
    if matches!(a.mode, Mode::OpenConfirm(_)) {
        return handle_open_confirm(a, audio, k);
    }
    if a.mode == Mode::PatternDialog {
        return handle_pattern_dialog(a, audio, k);
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
            KeyCode::Char('p' | 'P') => {
                a.pattern_cursor = a.editor.pattern().min(a.editor.project.patterns.len() - 1);
                a.mode = Mode::PatternDialog;
            }
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
                    normalize_cursor(a);
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
                    normalize_cursor(a);
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
    if matches!(a.mode, Mode::Navigation | Mode::ParameterEdit(_))
        && let Some(track) = track_jump_index(k)
    {
        select_track(a, track);
        return Ok(());
    }
    if matches!(a.mode, Mode::LfoEdit { .. }) && handle_lfo_key(a, audio, k)? {
        return Ok(());
    }
    if matches!(a.mode, Mode::ChordEdit { .. }) && handle_chord_key(a, audio, k)? {
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
                a.playheads = [None; TRACK_COUNT];
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
            move_step_vertical(a, false);
        }
        KeyCode::Down => {
            move_step_vertical(a, true);
        }
        KeyCode::Left => {
            if a.row == 0 {
                a.global = (a.global + GLOBAL_IDS.len() - 1) % GLOBAL_IDS.len()
            } else if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, false)
            } else {
                move_step(a, false)
            }
        }
        KeyCode::Right => {
            if a.row == 0 {
                a.global = (a.global + 1) % GLOBAL_IDS.len()
            } else if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, true)
            } else {
                move_step(a, true)
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
                sync_project(a, audio);
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
        KeyCode::Char('l') if a.row > 0 => {
            a.editor.end_coalescing();
            a.mode = Mode::TrackLengthInput(String::new());
            a.status = format!("Editing {} length", a.editor.project.tracks[a.row - 1].name);
        }
        KeyCode::Char('D') if a.row > 0 => duplicate_selected_track(a, audio),
        KeyCode::Char('A') if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_accent(track, step)) {
                sync_project(a, audio);
            }
        }
        KeyCode::Char('G') if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_slide(track, step)) {
                sync_project(a, audio);
            }
        }
        KeyCode::Char('C') if a.row == 5 => open_chord_editor(a),
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
                enter_parameter_edit(a, parameter);
            }
        }
        KeyCode::Char('t') if a.row > 3 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_tie(track, step)) {
                sync_project(a, audio);
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

fn move_step(a: &mut App, forward: bool) {
    let track = a.row - 1;
    let length = a.editor.project.tracks[track].steps.len();
    a.step = if forward {
        (a.step + 1) % length
    } else {
        (a.step + length - 1) % length
    };
}

fn track_jump_index(k: KeyEvent) -> Option<usize> {
    let KeyCode::Char(c) = k.code else {
        return None;
    };
    let shifted_symbol = match c {
        '!' => Some(0),
        '@' => Some(1),
        '#' => Some(2),
        '$' => Some(3),
        '%' => Some(4),
        '^' => Some(5),
        _ => None,
    };
    if shifted_symbol.is_some() {
        return shifted_symbol;
    }
    if k.modifiers.contains(KeyModifiers::SHIFT) {
        return c
            .to_digit(10)
            .filter(|&track| (1..=6).contains(&track))
            .map(|track| track as usize - 1);
    }
    None
}

fn select_track(a: &mut App, track: usize) {
    a.editor.end_coalescing();
    a.row = track + 1;
    a.step = a.step.min(a.editor.project.tracks[track].steps.len() - 1);

    match a.mode {
        Mode::ParameterEdit(parameter)
            if !parameter.is_valid_for(a.editor.project.tracks[track].kind) =>
        {
            let replacement = parameter_descriptors(a.editor.project.tracks[track].kind)[0].id;
            enter_parameter_edit(a, replacement);
        }
        _ => {}
    }
}

fn commit_pattern(a: &mut App, audio: &mut Audio, pattern: usize) -> bool {
    let previous = a.editor.pattern();
    if pattern >= a.editor.project.patterns.len() || audio.available_commands() < 2 {
        a.status = "Audio command queue full; pattern switch rejected".into();
        return false;
    }
    if pattern != previous && !a.editor.select_pattern(pattern) {
        return false;
    }
    if pattern != previous
        && audio
            .send(Audio::snapshot_for_pattern(
                &a.editor.project,
                a.editor.pattern(),
            ))
            .is_err()
    {
        let _ = a.editor.select_pattern(previous);
        a.status = "Audio command queue full; pattern switch rejected".into();
        return false;
    }
    if audio
        .send(AudioCommand::SelectPattern {
            pattern: pattern as u8,
        })
        .is_err()
    {
        a.status = "Audio command queue full; pattern switch rejected".into();
        return false;
    }
    a.pattern_cursor = pattern;
    a.step = 0;
    a.playheads = [None; TRACK_COUNT];
    a.status = if a.playing {
        format!("Pattern {} queued for next bar", pattern + 1)
    } else {
        format!("Pattern {} selected", pattern + 1)
    };
    true
}

fn adjacent_pattern_in_count(pattern: usize, forward: bool, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if forward {
        (pattern + 1) % count
    } else {
        (pattern + count - 1) % count
    }
}

fn handle_pattern_dialog(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Esc | KeyCode::Char('p' | 'P') => a.mode = Mode::Navigation,
        KeyCode::Left => {
            let pattern =
                adjacent_pattern_in_count(a.pattern_cursor, false, a.editor.project.patterns.len());
            a.pattern_cursor = pattern;
        }
        KeyCode::Right => {
            let pattern =
                adjacent_pattern_in_count(a.pattern_cursor, true, a.editor.project.patterns.len());
            a.pattern_cursor = pattern;
        }
        KeyCode::Home => a.pattern_cursor = 0,
        KeyCode::End => a.pattern_cursor = a.editor.project.patterns.len() - 1,
        KeyCode::Enter => {
            let pattern = a.pattern_cursor;
            if commit_pattern(a, audio, pattern) {
                a.mode = Mode::Navigation;
            }
        }
        KeyCode::Char('n' | 'N') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.insert_pattern_at(cursor),
            "Inserted pattern",
        ),
        KeyCode::Char('d') | KeyCode::Char('D') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.duplicate_pattern_at(cursor),
            "Duplicated pattern",
        ),
        KeyCode::Char('c' | 'C') => {
            if a.editor.copy_pattern_at(a.pattern_cursor) {
                a.status = format!("Copied pattern {}", a.pattern_cursor + 1);
            }
        }
        KeyCode::Char('x' | 'X') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.cut_pattern_at(cursor),
            "Cut pattern",
        ),
        KeyCode::Char('v' | 'V') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.paste_pattern_at(cursor),
            "Pasted pattern",
        ),
        KeyCode::Delete | KeyCode::Backspace => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.delete_pattern_at(cursor),
            "Deleted pattern",
        ),
        _ => {}
    }
    Ok(())
}

fn pattern_edit_at<F>(a: &mut App, audio: &mut Audio, f: F, message: &str)
where
    F: FnOnce(&mut Editor, usize) -> Result<(bool, usize), crate::reducer::EditError>,
{
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; pattern edit rejected".into();
        return;
    }
    match f(&mut a.editor, a.pattern_cursor) {
        Ok((true, cursor)) if sync_project(a, audio) => {
            a.pattern_cursor = cursor;
            a.status = format!("{message}: {}", cursor + 1);
        }
        Ok((true, _)) => {}
        Ok((false, _)) => a.status = "No change".into(),
        Err(e) => a.status = e.to_string(),
    }
}

fn move_step_page(a: &mut App, forward: bool) {
    a.editor.end_coalescing();
    move_step(a, forward);
}

fn move_step_bank(a: &mut App, forward: bool) {
    let track = a.row - 1;
    let length = a.editor.project.tracks[track].steps.len();
    let banks = length.div_ceil(STEP_BANK_SIZE);
    if banks <= 1 {
        return;
    }
    let bank = a.step / STEP_BANK_SIZE;
    let offset = a.step % STEP_BANK_SIZE;
    let next_bank = if forward {
        (bank + 1) % banks
    } else {
        (bank + banks - 1) % banks
    };
    a.step = (next_bank * STEP_BANK_SIZE + offset).min(length - 1);
}

fn move_step_vertical(a: &mut App, down: bool) {
    if a.row == 0 {
        if down {
            a.row = 1;
            a.step = 0;
            a.scope = Scope::Base;
        }
        return;
    }

    let track = a.row - 1;
    let step_row = a.step / STEP_ROW_SIZE;
    let column = a.step % STEP_ROW_SIZE;
    let length = a.editor.project.tracks[track].steps.len();
    if down {
        if step_row + 1 < length.div_ceil(STEP_ROW_SIZE) {
            a.step = ((step_row + 1) * STEP_ROW_SIZE + column).min(length - 1);
        } else if track + 1 < TRACK_COUNT {
            let destination = track + 1;
            a.row = destination + 1;
            a.step = column.min(a.editor.project.tracks[destination].steps.len() - 1);
        }
    } else if step_row > 0 {
        a.step -= STEP_ROW_SIZE;
    } else if track == 0 {
        a.row = 0;
        a.scope = Scope::Base;
    } else {
        let destination = track - 1;
        let destination_length = a.editor.project.tracks[destination].steps.len();
        a.row = destination + 1;
        a.step = ((destination_length - 1) / STEP_ROW_SIZE * STEP_ROW_SIZE + column)
            .min(destination_length - 1);
    }
}

fn normalize_cursor(a: &mut App) {
    a.pattern_cursor = a
        .pattern_cursor
        .min(a.editor.project.patterns.len().saturating_sub(1));
    if a.row > 0 {
        a.step = a
            .step
            .min(a.editor.project.tracks[a.row - 1].steps.len() - 1);
    }
}

fn set_selected_track_length(a: &mut App, audio: &mut Audio, length: usize, coalesce: bool) {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return;
    }
    let track = a.row - 1;
    let old_length = a.editor.project.tracks[track].steps.len();
    let old_events = a.editor.project.tracks[track]
        .steps
        .iter()
        .flatten()
        .count();
    let key = coalesce.then_some(crate::reducer::CoalesceKey(track, usize::MAX, u8::MAX));
    match a.editor.set_track_length(track, length, key) {
        Ok(true) => {
            normalize_cursor(a);
            if sync_project(a, audio) {
                let new_events = a.editor.project.tracks[track]
                    .steps
                    .iter()
                    .flatten()
                    .count();
                let removed = old_events.saturating_sub(new_events);
                a.status = format!(
                    "{} length: {} → {}{}",
                    a.editor.project.tracks[track].name,
                    old_length,
                    length,
                    if removed == 0 {
                        String::new()
                    } else {
                        format!("; removed {removed} programmed step(s)")
                    }
                );
            }
        }
        Ok(false) => a.status = "No change".into(),
        Err(e) => a.status = e.to_string(),
    }
}

fn duplicate_selected_track(a: &mut App, audio: &mut Audio) {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; duplication rejected".into();
        return;
    }
    let track = a.row - 1;
    let old_length = a.editor.project.tracks[track].steps.len();
    match a.editor.duplicate_track(track) {
        Ok(true) if sync_project(a, audio) => {
            a.status = format!(
                "{} doubled: {} → {}",
                a.editor.project.tracks[track].name,
                old_length,
                old_length * 2
            );
        }
        Ok(true) => {}
        Ok(false) => a.status = "No change".into(),
        Err(e) => a.status = e.to_string(),
    }
}

fn handle_track_length_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::TrackLengthInput(mut input) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Esc => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation;
        }
        KeyCode::Backspace => {
            input.pop();
            a.mode = Mode::TrackLengthInput(input);
        }
        KeyCode::Char(c @ '0'..='9') if input.len() < 2 => {
            input.push(c);
            a.mode = Mode::TrackLengthInput(input);
        }
        KeyCode::Up | KeyCode::Down => {
            let delta = if k.modifiers.contains(KeyModifiers::SHIFT) {
                STEP_BANK_SIZE
            } else {
                1
            };
            let current = a.editor.project.tracks[a.row - 1].steps.len();
            let next = if k.code == KeyCode::Up {
                current.saturating_add(delta).min(MAX_STEP_COUNT)
            } else {
                current.saturating_sub(delta).max(1)
            };
            set_selected_track_length(a, audio, next, true);
            a.mode = Mode::TrackLengthInput(String::new());
        }
        KeyCode::Enter => match input.parse::<usize>() {
            Ok(value @ 1..=MAX_STEP_COUNT) => {
                set_selected_track_length(a, audio, value, false);
                a.editor.end_coalescing();
                a.mode = Mode::Navigation;
            }
            _ => a.status = "Track length must be an integer from 1 to 64".into(),
        },
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
        KeyCode::PageUp | KeyCode::PageDown => {
            move_step_page(a, k.code == KeyCode::PageDown);
            Ok(true)
        }
        KeyCode::Left => {
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, false);
            } else {
                move_parameter_editor(a, false);
            }
            Ok(true)
        }
        KeyCode::Right => {
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, true);
            } else {
                move_parameter_editor(a, true);
            }
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
                Ok(ParameterValue::Waveform(waveform)) => {
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::Waveform(flipped_waveform(waveform)),
                        true,
                        false,
                    );
                    return Ok(true);
                }
                Ok(ParameterValue::Chorus(mode)) => {
                    let next = match (mode, k.code) {
                        (ChorusMode::Off, KeyCode::Up) => ChorusMode::I,
                        (ChorusMode::I, KeyCode::Up) => ChorusMode::Ii,
                        (ChorusMode::Ii, KeyCode::Down) => ChorusMode::I,
                        (ChorusMode::I, KeyCode::Down) => ChorusMode::Off,
                        _ => mode,
                    };
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::Chorus(next),
                        true,
                        false,
                    );
                    return Ok(true);
                }
                Ok(ParameterValue::Spread(mode)) => {
                    let next = match (mode, k.code) {
                        (crate::model::ChordSpread::Off, KeyCode::Up) => {
                            crate::model::ChordSpread::Narrow
                        }
                        (crate::model::ChordSpread::Narrow, KeyCode::Up) => {
                            crate::model::ChordSpread::Wide
                        }
                        (crate::model::ChordSpread::Wide, KeyCode::Down) => {
                            crate::model::ChordSpread::Narrow
                        }
                        (crate::model::ChordSpread::Narrow, KeyCode::Down) => {
                            crate::model::ChordSpread::Off
                        }
                        _ => mode,
                    };
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::Spread(next),
                        true,
                        false,
                    );
                    return Ok(true);
                }
                Err(e) => {
                    a.status = e.to_string();
                    return Ok(true);
                }
            };
            let value = ParameterValue::Percent(current.saturating_add(delta));
            set_parameter(a, audio, parameter, value, true, false);
            Ok(true)
        }
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            Ok(true)
        }
        KeyCode::Char('C') if track == 4 => {
            a.mode = Mode::Navigation;
            open_chord_editor(a);
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
        KeyCode::Char('L') => {
            open_lfo_editor(a, audio, parameter);
            Ok(true)
        }
        key if parameter_edit_passthrough(key) => Ok(false),
        KeyCode::Char(c) => {
            if let Some(next) = parameter_shortcut(a.editor.project.tracks[track].kind, c) {
                switch_parameter_editor(a, next);
                return Ok(true);
            }
            if let Some(value) = crate::reducer::percentage_key(c) {
                if !parameter.is_waveform() && !parameter.is_chorus() {
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::Percent(value),
                        true,
                        true,
                    );
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
                    sync_project(a, audio);
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

fn parameter_edit_passthrough(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::Char(' ')
            | KeyCode::Char('.')
            | KeyCode::Char('o')
            | KeyCode::Char('A')
            | KeyCode::Char('G')
    )
}

fn open_lfo_editor(a: &mut App, audio: &mut Audio, parameter: ParameterId) {
    let track = a.row.saturating_sub(1);
    let kind = a.editor.project.tracks[track].kind;
    if !parameter.supports_lfo(kind) {
        a.status = format!("{} cannot be LFO-modulated", parameter.display_name());
        return;
    }
    let existing = a.editor.lfo(track, parameter).ok().flatten();
    if existing.is_none() {
        if audio.available_commands() == 0 {
            a.status = "Audio command queue full; LFO edit rejected".into();
            return;
        }
        match a
            .editor
            .set_lfo(track, parameter, Some(LfoConfig::default()), None)
        {
            Ok(true) if sync_project(a, audio) => {}
            Ok(true) | Ok(false) => return,
            Err(error) => {
                a.status = error.to_string();
                return;
            }
        }
    }
    a.editor.end_coalescing();
    a.mode = Mode::LfoEdit {
        parameter,
        field: LfoField::Enabled,
    };
    a.status = format!("Editing track LFO for {}", parameter.display_name());
}

fn open_chord_editor(a: &mut App) {
    if a.row != 5 {
        a.status = "Chord shape is available on the Chord track only".into();
        return;
    }
    let track = &a.editor.project.tracks[4];
    let shape = match track.steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => chord_shape.unwrap_or_default(),
        Some(StepEvent::Tie { .. }) => {
            a.status = "Select the source Chord note to edit its shape".into();
            return;
        }
        None => track.input_chord_shape.unwrap_or_default(),
        _ => return,
    };
    a.mode = Mode::ChordEdit { shape };
    a.status = "Editing Chord shape".into();
}

fn handle_chord_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::ChordEdit { shape } = a.mode else {
        return Ok(false);
    };
    match k.code {
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('C') => {
            a.mode = Mode::Navigation;
            a.status = "Chord shape editing finished".into();
        }
        KeyCode::Char('?') => a.mode = Mode::Help,
        KeyCode::PageUp | KeyCode::PageDown => {
            move_chord_editor_step(a, k.code == KeyCode::PageDown);
            a.status = format!("Editing Chord shape at step {}", a.step + 1);
        }
        KeyCode::Up | KeyCode::Down => {
            let index = ChordShape::ALL
                .iter()
                .position(|value| *value == shape)
                .unwrap();
            let next = lfo_choice_index(index, ChordShape::ALL.len(), k.code);
            let next_shape = ChordShape::ALL[next];
            if next_shape != shape {
                let track = a.row - 1;
                let step = a.step;
                if apply(a, audio, |editor| {
                    editor.set_chord_shape(track, step, next_shape)
                }) {
                    sync_project(a, audio);
                    a.mode = Mode::ChordEdit { shape: next_shape };
                    a.status = format!("Chord shape set to {next_shape}");
                }
            }
        }
        _ => {}
    }
    Ok(true)
}

fn move_chord_editor_step(a: &mut App, forward: bool) {
    move_step_page(a, forward);
    let shape = selected_chord_shape(a, a.row - 1).unwrap_or_default();
    a.mode = Mode::ChordEdit { shape };
}

fn handle_lfo_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::LfoEdit { parameter, field } = a.mode.clone() else {
        return Ok(false);
    };
    let track = a.row.saturating_sub(1);
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => return Ok(false),
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            return Ok(true);
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('L') => {
            a.editor.end_coalescing();
            a.mode = Mode::ParameterEdit(parameter);
            a.status = "LFO editing finished".into();
            return Ok(true);
        }
        KeyCode::Backspace | KeyCode::Delete => {
            if set_lfo_config(a, audio, parameter, None, None) {
                a.mode = Mode::ParameterEdit(parameter);
                a.status = format!("Removed {} LFO", parameter.display_name());
            }
            return Ok(true);
        }
        KeyCode::Left | KeyCode::Right => {
            a.editor.end_coalescing();
            let index = LfoField::ALL
                .iter()
                .position(|value| *value == field)
                .unwrap();
            let next = if k.code == KeyCode::Right {
                (index + 1) % LfoField::ALL.len()
            } else {
                (index + LfoField::ALL.len() - 1) % LfoField::ALL.len()
            };
            a.mode = Mode::LfoEdit {
                parameter,
                field: LfoField::ALL[next],
            };
            return Ok(true);
        }
        _ => {}
    }

    let Some(mut config) = a.editor.lfo(track, parameter).ok().flatten() else {
        a.mode = Mode::ParameterEdit(parameter);
        return Ok(true);
    };
    let direction = if k.code == KeyCode::Down { -1 } else { 1 };
    let percent_delta: i16 = if k.modifiers.contains(KeyModifiers::SHIFT) {
        (direction * 10) as i16
    } else {
        direction as i16
    };
    let changed = match k.code {
        KeyCode::Up | KeyCode::Down => {
            let previous = config;
            match field {
                LfoField::Enabled => {
                    let current = usize::from(!config.enabled);
                    config.enabled = lfo_choice_index(current, 2, k.code) == 0;
                }
                LfoField::Waveform => {
                    let index = LfoWaveform::ALL
                        .iter()
                        .position(|waveform| *waveform == config.waveform)
                        .unwrap();
                    config.waveform =
                        LfoWaveform::ALL[lfo_choice_index(index, LfoWaveform::ALL.len(), k.code)];
                }
                LfoField::RateMode => {
                    let current = usize::from(matches!(config.rate, LfoRate::Free { .. }));
                    config.rate = match (lfo_choice_index(current, 2, k.code), config.rate) {
                        (1, LfoRate::Synced { .. }) => LfoRate::Free {
                            rate_percent: Percent::new(50).unwrap(),
                        },
                        (0, LfoRate::Free { .. }) => LfoRate::Synced {
                            division: LfoDivision::Quarter,
                        },
                        (_, rate) => rate,
                    };
                }
                LfoField::Rate => match &mut config.rate {
                    LfoRate::Synced { division } => {
                        let index = LfoDivision::ALL
                            .iter()
                            .position(|value| value == division)
                            .unwrap();
                        *division = LfoDivision::ALL
                            [lfo_choice_index(index, LfoDivision::ALL.len(), k.code)];
                    }
                    LfoRate::Free { rate_percent } => {
                        *rate_percent = rate_percent.saturating_add(percent_delta);
                    }
                },
                LfoField::Depth => config.depth = config.depth.saturating_add(percent_delta),
            }
            config != previous
        }
        KeyCode::Char(c) => crate::reducer::percentage_key(c).is_some_and(|value| match field {
            LfoField::Depth => {
                config.depth = value;
                true
            }
            LfoField::Rate => match &mut config.rate {
                LfoRate::Free { rate_percent } => {
                    *rate_percent = value;
                    true
                }
                LfoRate::Synced { .. } => false,
            },
            _ => false,
        }),
        _ => false,
    };
    if changed {
        let key =
            crate::reducer::CoalesceKey(track, usize::MAX, parameter as u8 ^ ((field as u8) << 4));
        if set_lfo_config(a, audio, parameter, Some(config), Some(key)) {
            a.status = format!("{} LFO updated", parameter.display_name());
        }
    }
    Ok(true)
}

fn lfo_choice_index(current: usize, len: usize, key: KeyCode) -> usize {
    match key {
        KeyCode::Up => current.saturating_sub(1),
        KeyCode::Down => current.saturating_add(1).min(len.saturating_sub(1)),
        _ => current,
    }
}

fn set_lfo_config(
    a: &mut App,
    audio: &mut Audio,
    parameter: ParameterId,
    config: Option<LfoConfig>,
    key: Option<crate::reducer::CoalesceKey>,
) -> bool {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; LFO edit rejected".into();
        return false;
    }
    let track = a.row - 1;
    match a.editor.set_lfo(track, parameter, config, key) {
        Ok(true) => sync_project(a, audio),
        Ok(false) => true,
        Err(error) => {
            a.status = error.to_string();
            false
        }
    }
}

fn parameter_shortcut(kind: TrackKind, c: char) -> Option<ParameterId> {
    parameter_descriptors(kind)
        .iter()
        .find(|descriptor| descriptor.shortcut.starts_with(c))
        .map(|descriptor| descriptor.id)
}

fn enter_parameter_edit(a: &mut App, parameter: ParameterId) {
    a.mode = Mode::ParameterEdit(parameter);
    a.status = format!("Editing {}", parameter.display_name());
}

fn switch_parameter_editor(a: &mut App, parameter: ParameterId) {
    a.editor.end_coalescing();
    enter_parameter_edit(a, parameter);
}

fn move_parameter_editor(a: &mut App, forward: bool) {
    let Mode::ParameterEdit(current) = a.mode else {
        return;
    };
    let descriptors = parameter_descriptors(a.editor.project.tracks[a.row - 1].kind);
    let index = descriptors
        .iter()
        .position(|descriptor| descriptor.id == current)
        .unwrap_or(0);
    let next = if forward {
        (index + 1) % descriptors.len()
    } else {
        (index + descriptors.len() - 1) % descriptors.len()
    };
    switch_parameter_editor(a, descriptors[next].id);
}

fn flipped_waveform(waveform: Waveform) -> Waveform {
    match waveform {
        Waveform::Square => Waveform::Saw,
        Waveform::Saw => Waveform::Square,
    }
}

fn set_parameter(
    a: &mut App,
    audio: &mut Audio,
    parameter: ParameterId,
    value: ParameterValue,
    keep_editing: bool,
    direct_entry: bool,
) -> bool {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; edit rejected".into();
        return false;
    }
    let track = a.row - 1;
    let step = a.step;
    let previous = direct_entry
        .then(|| {
            a.editor
                .parameter_value(track, step, a.scope, parameter)
                .ok()
        })
        .flatten()
        .and_then(|value| match value {
            ParameterValue::Percent(value) => Some(value),
            ParameterValue::Waveform(_) => None,
            ParameterValue::Chorus(_) => None,
            ParameterValue::Spread(_) => None,
        });
    let key = keep_editing.then_some(coalesce_key(track, step, parameter));
    match a
        .editor
        .set_parameter(track, step, a.scope, parameter, value, key)
    {
        Ok(true) => {
            let synced = sync_project_with_smoothing(a, audio, direct_entry);
            if synced {
                if let (Some(from), ParameterValue::Percent(to)) = (previous, value) {
                    a.start_fader_animation(track, step, parameter, from, to);
                }
            }
            if synced && keep_editing {
                a.status = format!("{} set", parameter.display_name());
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
fn sync_project(a: &mut App, audio: &mut Audio) -> bool {
    sync_project_with_smoothing(a, audio, false)
}

fn sync_project_with_smoothing(a: &mut App, audio: &mut Audio, direct_entry: bool) -> bool {
    let smoothing = if direct_entry {
        ParameterSmoothing::Fader
    } else {
        ParameterSmoothing::Default
    };
    if audio
        .send(Audio::snapshot_with_smoothing_for_pattern(
            &a.editor.project,
            a.editor.pattern(),
            smoothing,
        ))
        .is_err()
    {
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
            a.pattern_cursor = 0;
            a.active_pattern = 0;
            a.queued_pattern = None;
            a.playheads = [None; TRACK_COUNT];
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
        GlobalParameterId::ReverbTone,
        GlobalParameterId::ReverbPreDelay,
        GlobalParameterId::Key,
        GlobalParameterId::Scale,
    ][index % GLOBAL_IDS.len()]
}
fn global_shortcut(c: char) -> Option<GlobalParameterId> {
    match c {
        't' => Some(GlobalParameterId::Tempo),
        'y' => Some(GlobalParameterId::DelayDivision),
        'f' => Some(GlobalParameterId::DelayFeedback),
        'r' => Some(GlobalParameterId::ReverbTime),
        'b' => Some(GlobalParameterId::ReverbTone),
        'p' => Some(GlobalParameterId::ReverbPreDelay),
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
        GlobalParameterId::ReverbTone => "reverb tone",
        GlobalParameterId::ReverbPreDelay => "reverb pre-delay",
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
            } else if matches!(
                id,
                GlobalParameterId::DelayFeedback | GlobalParameterId::ReverbTone
            ) {
                if let Some(v) = crate::reducer::percentage_key(c) {
                    let max = if id == GlobalParameterId::DelayFeedback {
                        95
                    } else {
                        100
                    };
                    let v = Percent::new(v.get().min(max)).unwrap();
                    if id == GlobalParameterId::DelayFeedback {
                        edit_global(a, audio, id, move |g| g.delay_feedback = v);
                    } else {
                        edit_global(a, audio, id, move |g| g.reverb_tone = v);
                    }
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
                GlobalParameterId::ReverbTone => {
                    let d: i16 = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        10
                    } else {
                        1
                    };
                    edit_global(a, audio, id, move |g| {
                        g.reverb_tone = g.reverb_tone.saturating_add(direction as i16 * d)
                    })
                }
                GlobalParameterId::ReverbPreDelay => {
                    let d: i16 = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        10
                    } else {
                        1
                    };
                    edit_global(a, audio, id, move |g| {
                        g.reverb_pre_delay_ms = (g.reverb_pre_delay_ms as i16
                            + direction as i16 * d)
                            .clamp(0, 200) as u16
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
    a.active_pattern = usize::from(audio.status.active_pattern.load(Ordering::Acquire))
        .min(a.editor.project.patterns.len() - 1);
    let queued = audio.status.queued_pattern.load(Ordering::Acquire);
    a.queued_pattern =
        (queued != u8::MAX).then_some(usize::from(queued).min(a.editor.project.patterns.len() - 1));
    for track in 0..TRACK_COUNT {
        let step = audio.status.playheads[track].load(Ordering::Acquire);
        a.playheads[track] =
            (step < a.editor.project.tracks[track].steps.len() as u8).then_some(step as usize);
    }
    if audio.status.failed.load(Ordering::Acquire) {
        a.status = "Audio stream failed; editing and saving remain available".into()
    } else if audio.status.non_finite.swap(false, Ordering::AcqRel) {
        a.status = "Audio DSP produced a non-finite value; output was silenced".into()
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
        Mode::PatternDialog => "Pattern dialog".into(),
        Mode::ParameterEdit(parameter) => {
            format!("Parameter edit ({})", parameter.display_name())
        }
        Mode::LfoEdit { parameter, .. } => {
            format!("Track LFO edit ({})", parameter.display_name())
        }
        Mode::ChordEdit { shape } => format!("Chord shape edit ({shape})"),
        Mode::GlobalEdit(id) => format!("Global edit ({})", global_name(*id)),
        Mode::TempoInput(_) => "Tempo numeric input".into(),
        Mode::TrackLengthInput(_) => "Track length input".into(),
        Mode::FileInput(_, _) => "File-path input".into(),
        Mode::OpenConfirm(_) => "Unsaved confirmation".into(),
        Mode::Error(_) => "Error dialog".into(),
        Mode::Help => "Help".into(),
        Mode::QuitConfirm => "Unsaved confirmation".into(),
    }
}

fn track_label(t: &crate::model::Track) -> String {
    if matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
        format!("{} O{}", t.name, t.input_octave.unwrap_or(3))
    } else {
        t.name.clone()
    }
}

fn step_cell(event: Option<&StepEvent>) -> String {
    match event {
        None => " . ".into(),
        Some(StepEvent::Trigger { accent, locks }) if locks.is_empty() => {
            if *accent {
                " X ".into()
            } else {
                " x ".into()
            }
        }
        Some(StepEvent::Trigger { accent, .. }) => {
            if *accent {
                "X* ".into()
            } else {
                "x* ".into()
            }
        }
        Some(StepEvent::BassNote {
            degree,
            octave,
            accent,
            locks,
            ..
        })
        | Some(StepEvent::Note {
            degree,
            octave,
            accent,
            locks,
            ..
        }) => format!(
            "{degree}{}{octave}",
            match (*accent, locks.is_empty()) {
                (false, true) => ':',
                (true, true) => '!',
                (false, false) => '*',
                (true, false) => '#',
            }
        ),
        Some(StepEvent::Tie { locks }) if locks.is_empty() => " - ".into(),
        Some(StepEvent::Tie { .. }) => "-* ".into(),
    }
}

fn selected_accent(a: &App, track: usize) -> Option<(bool, Option<usize>)> {
    let event = a.editor.project.tracks[track].steps[a.step].as_ref()?;
    if let Some(accent) = event.accent() {
        return Some((accent, None));
    }
    let source = crate::model::tie_source(&a.editor.project.tracks[track].steps, a.step)?;
    a.editor.project.tracks[track].steps[source]
        .as_ref()?
        .accent()
        .map(|accent| (accent, Some(source)))
}

fn articulation_title(a: &App, track: usize) -> String {
    match selected_accent(a, track) {
        Some((accent, None)) => {
            let mut text = format!("[Shift+A] Accent {}", if accent { "on" } else { "off" });
            if let Some(StepEvent::BassNote { slide, .. }) =
                a.editor.project.tracks[track].steps[a.step]
            {
                text.push_str(&format!(
                    " · [Shift+G] Slide {}",
                    if slide { "on" } else { "off" }
                ));
            }
            text
        }
        Some((accent, Some(source))) => {
            format!(
                "Accent {} from step {}",
                if accent { "on" } else { "off" },
                source + 1
            )
        }
        None => "Accent —".into(),
    }
}

fn selected_chord_shape(a: &App, track: usize) -> Option<ChordShape> {
    if a.editor.project.tracks.get(track)?.kind != TrackKind::Chord {
        return None;
    }
    match a.editor.project.tracks[track].steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => Some(chord_shape.unwrap_or_default()),
        Some(StepEvent::Tie { .. }) => {
            crate::model::tie_source(&a.editor.project.tracks[track].steps, a.step).and_then(
                |source| match a.editor.project.tracks[track].steps[source].as_ref() {
                    Some(StepEvent::Note { chord_shape, .. }) => {
                        Some(chord_shape.unwrap_or_default())
                    }
                    _ => None,
                },
            )
        }
        None => Some(
            a.editor.project.tracks[track]
                .input_chord_shape
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ParameterDescriptor {
    id: ParameterId,
    label: &'static str,
    shortcut: &'static str,
    group: ParameterGroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParameterGroup {
    Mixer,
    Instrument,
    Filter,
    Envelope,
}

impl ParameterGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Mixer => "MIXER",
            Self::Instrument => "INSTRUMENT",
            Self::Filter => "FILTER",
            Self::Envelope => "ENVELOPE",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Mixer => Color::Cyan,
            Self::Instrument => Color::Green,
            Self::Filter => Color::Magenta,
            Self::Envelope => Color::Yellow,
        }
    }
}

const COMMON_PARAMETERS: [ParameterDescriptor; 4] = [
    ParameterDescriptor {
        id: ParameterId::Level,
        label: "Level",
        shortcut: "v",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::DelaySend,
        label: "Delay",
        shortcut: "y",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::ReverbSend,
        label: "Reverb",
        shortcut: "b",
        group: ParameterGroup::Mixer,
    },
    ParameterDescriptor {
        id: ParameterId::Pan,
        label: "Pan",
        shortcut: "n",
        group: ParameterGroup::Mixer,
    },
];

const KICK_PARAMETERS: [ParameterDescriptor; 7] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Attack,
        label: "Attack",
        shortcut: "a",
        group: ParameterGroup::Instrument,
    },
];

const SNARE_PARAMETERS: [ParameterDescriptor; 7] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Tone,
        label: "Tone",
        shortcut: "t",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Snappy,
        label: "Snappy",
        shortcut: "s",
        group: ParameterGroup::Instrument,
    },
];

const HAT_PARAMETERS: [ParameterDescriptor; 6] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    ParameterDescriptor {
        id: ParameterId::Tune,
        label: "Tune",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Instrument,
    },
];

const BASS_PARAMETERS: [ParameterDescriptor; 9] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    ParameterDescriptor {
        id: ParameterId::Waveform,
        label: "Wave",
        shortcut: "w",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Cutoff,
        label: "Cutoff",
        shortcut: "c",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Resonance,
        label: "Reson",
        shortcut: "R",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::FilterEnvelope,
        label: "Filt Env",
        shortcut: "f",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Envelope,
    },
];

const CHORD_PARAMETERS: [ParameterDescriptor; 16] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    ParameterDescriptor {
        id: ParameterId::OscillatorMix,
        label: "Osc Mix",
        shortcut: "w",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::PulseWidth,
        label: "Pulse W",
        shortcut: "P",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::SubOscillator,
        label: "Sub",
        shortcut: "u",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Chorus,
        label: "Chorus",
        shortcut: "h",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Spread,
        label: "Spread",
        shortcut: "e",
        group: ParameterGroup::Instrument,
    },
    ParameterDescriptor {
        id: ParameterId::Cutoff,
        label: "Cutoff",
        shortcut: "c",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Resonance,
        label: "Reson",
        shortcut: "R",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::FilterEnvelope,
        label: "Filt Env",
        shortcut: "f",
        group: ParameterGroup::Filter,
    },
    ParameterDescriptor {
        id: ParameterId::Attack,
        label: "Attack",
        shortcut: "a",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Decay,
        label: "Decay",
        shortcut: "d",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Sustain,
        label: "Sustain",
        shortcut: "s",
        group: ParameterGroup::Envelope,
    },
    ParameterDescriptor {
        id: ParameterId::Release,
        label: "Release",
        shortcut: "r",
        group: ParameterGroup::Envelope,
    },
];

const LEAD_PARAMETERS: [ParameterDescriptor; 14] = [
    COMMON_PARAMETERS[0],
    COMMON_PARAMETERS[1],
    COMMON_PARAMETERS[2],
    COMMON_PARAMETERS[3],
    CHORD_PARAMETERS[4],
    CHORD_PARAMETERS[5],
    CHORD_PARAMETERS[6],
    CHORD_PARAMETERS[9],
    CHORD_PARAMETERS[10],
    CHORD_PARAMETERS[11],
    CHORD_PARAMETERS[12],
    CHORD_PARAMETERS[13],
    CHORD_PARAMETERS[14],
    CHORD_PARAMETERS[15],
];

fn parameter_descriptors(kind: TrackKind) -> &'static [ParameterDescriptor] {
    match kind {
        TrackKind::Kick => &KICK_PARAMETERS,
        TrackKind::Snare => &SNARE_PARAMETERS,
        TrackKind::Hat => &HAT_PARAMETERS,
        TrackKind::Bass => &BASS_PARAMETERS,
        TrackKind::Chord => &CHORD_PARAMETERS,
        TrackKind::Lead => &LEAD_PARAMETERS,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueOrigin {
    Base,
    Lock,
}

fn lock_has_parameter(event: &StepEvent, parameter: ParameterId) -> bool {
    event.locks().get(parameter).is_some()
}

fn displayed_parameter(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> Option<(ParameterValue, ValueOrigin)> {
    let base = a
        .editor
        .parameter_value(track, step, Scope::Base, parameter)
        .ok()?;
    if a.scope == Scope::Base {
        let value = match base {
            ParameterValue::Percent(target) => ParameterValue::Percent(a.animated_percent(
                track,
                step,
                parameter,
                ValueOrigin::Base,
                target,
            )),
            value => value,
        };
        return Some((value, ValueOrigin::Base));
    }
    let locked = a
        .editor
        .project
        .tracks
        .get(track)
        .and_then(|t| t.steps.get(step))
        .and_then(Option::as_ref)
        .is_some_and(|event| lock_has_parameter(event, parameter));
    let value = a
        .editor
        .parameter_value(track, step, Scope::Lock, parameter)
        .unwrap_or(base);
    let origin = if locked {
        ValueOrigin::Lock
    } else {
        ValueOrigin::Base
    };
    let value = match value {
        ParameterValue::Percent(target) => {
            ParameterValue::Percent(a.animated_percent(track, step, parameter, origin, target))
        }
        value => value,
    };
    Some((value, origin))
}

fn fader_segments(value: u8) -> usize {
    ((value as usize * 10 + 50) / 100).min(10)
}

fn physical_parameter_readout(
    a: &App,
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> String {
    let Some((value, origin)) = displayed_parameter(a, track, step, parameter) else {
        return "unavailable".into();
    };
    let origin = match origin {
        ValueOrigin::Base => "BASE",
        ValueOrigin::Lock => "LOCK",
    };
    let physical = match value {
        ParameterValue::Waveform(Waveform::Square) => "Square".into(),
        ParameterValue::Waveform(Waveform::Saw) => "Saw".into(),
        ParameterValue::Chorus(ChorusMode::Off) => "Off".into(),
        ParameterValue::Chorus(ChorusMode::I) => "Mode I".into(),
        ParameterValue::Chorus(ChorusMode::Ii) => "Mode II".into(),
        ParameterValue::Spread(value) => value.to_string(),
        ParameterValue::Percent(value) => {
            let value = value.get();
            match (a.editor.project.tracks[track].kind, parameter) {
                (TrackKind::Kick, ParameterId::Tune) => format!(
                    "peak {:.0} Hz · fundamental {:.0} Hz",
                    110.0 + value as f32 * 1.70,
                    45.0 + value as f32 * 0.25
                ),
                (TrackKind::Kick, ParameterId::Decay) => {
                    format!("{:.0} ms", 80.0 + value as f32 * 7.5)
                }
                (TrackKind::Snare, ParameterId::Tone) => format!(
                    "body {:.0} Hz · noise {:.0} Hz",
                    145.0 + value as f32 * 1.7,
                    800.0 + value as f32 * 52.0
                ),
                (TrackKind::Snare, ParameterId::Tune) => {
                    format!("{:.0} Hz body", 150.0 + value as f32 * 1.5)
                }
                (TrackKind::Hat, ParameterId::Tune) => {
                    format!("{:.0} Hz source", 310.0 + value as f32 * 3.6)
                }
                (TrackKind::Hat, ParameterId::Decay) => {
                    format!("{:.0} ms", 25.0 * 32.0_f32.powf(value as f32 / 100.0))
                }
                (TrackKind::Bass, ParameterId::Cutoff) => {
                    format!("{:.0} Hz", crate::dsp::exp_map(value, 80.0, 8_000.0))
                }
                (TrackKind::Bass, ParameterId::Resonance) => {
                    format!("Q {:.2}", 0.707 + value as f32 / 100.0 * (14.0 - 0.707))
                }
                (TrackKind::Bass, ParameterId::Decay) => {
                    format!("{:.0} ms", crate::dsp::exp_map(value, 80.0, 2_000.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::Cutoff) => {
                    format!("{:.0} Hz", crate::dsp::exp_map(value, 20.0, 20_000.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::Resonance) => {
                    format!("{}% feedback", value)
                }
                (TrackKind::Chord, ParameterId::Attack) => {
                    let seconds = if value == 0 {
                        0.0
                    } else {
                        crate::dsp::exp_map(value, 0.001, 3.0)
                    };
                    format!("{seconds:.3} s")
                }
                (TrackKind::Lead, ParameterId::Attack) => {
                    let seconds = if value == 0 {
                        0.0
                    } else {
                        crate::dsp::exp_map(value, 0.0015, 4.0)
                    };
                    format!("{seconds:.3} s")
                }
                (TrackKind::Chord, ParameterId::Decay | ParameterId::Release) => {
                    format!("{:.3} s", crate::dsp::exp_map(value, 0.002, 12.0))
                }
                (TrackKind::Lead, ParameterId::Decay | ParameterId::Release) => {
                    format!("{:.3} s", crate::dsp::exp_map(value, 0.002, 10.0))
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::OscillatorMix) => {
                    format!("Pulse {}% · Saw {}%", 100 - value, value)
                }
                (TrackKind::Chord | TrackKind::Lead, ParameterId::PulseWidth) => {
                    format!("{:.0}% duty", 5.0 + value as f32 * 0.9)
                }
                _ => format!("{value}%"),
            }
        }
    };
    format!("{physical} · {origin}")
}

fn global_shortcut_text(id: GlobalParameterId) -> &'static str {
    match id {
        GlobalParameterId::Tempo => "t",
        GlobalParameterId::DelayDivision => "y",
        GlobalParameterId::DelayFeedback => "f",
        GlobalParameterId::ReverbTime => "r",
        GlobalParameterId::ReverbTone => "b",
        GlobalParameterId::ReverbPreDelay => "p",
        GlobalParameterId::Key => "k",
        GlobalParameterId::Scale => "s",
    }
}

const GLOBAL_IDS: [GlobalParameterId; 8] = [
    GlobalParameterId::Tempo,
    GlobalParameterId::DelayDivision,
    GlobalParameterId::DelayFeedback,
    GlobalParameterId::ReverbTime,
    GlobalParameterId::ReverbTone,
    GlobalParameterId::ReverbPreDelay,
    GlobalParameterId::Key,
    GlobalParameterId::Scale,
];

fn global_display_name(id: GlobalParameterId) -> &'static str {
    match id {
        GlobalParameterId::Tempo => "Tempo",
        GlobalParameterId::DelayDivision => "Delay",
        GlobalParameterId::DelayFeedback => "Feedback",
        GlobalParameterId::ReverbTime => "Reverb",
        GlobalParameterId::ReverbTone => "Tone",
        GlobalParameterId::ReverbPreDelay => "Pre-delay",
        GlobalParameterId::Key => "Key",
        GlobalParameterId::Scale => "Scale",
    }
}

fn global_value_text(g: &crate::model::Globals, id: GlobalParameterId) -> String {
    match id {
        GlobalParameterId::Tempo => format!("{} BPM", g.tempo_bpm),
        GlobalParameterId::DelayDivision => g.delay_division.to_string(),
        GlobalParameterId::DelayFeedback => format!("{}%", g.delay_feedback.get()),
        GlobalParameterId::ReverbTime => format!("{:.1} s", g.reverb_time_seconds),
        GlobalParameterId::ReverbTone => format!("{}%", g.reverb_tone.get()),
        GlobalParameterId::ReverbPreDelay => format!("{} ms", g.reverb_pre_delay_ms),
        GlobalParameterId::Key => g.key.to_string(),
        GlobalParameterId::Scale => g.scale.to_string(),
    }
}

fn global_control_text(g: &crate::model::Globals) -> Vec<String> {
    GLOBAL_IDS
        .iter()
        .map(|id| {
            format!(
                "[{}] {} {}",
                global_shortcut_text(*id),
                global_display_name(*id),
                global_value_text(g, *id)
            )
        })
        .collect()
}

fn render_global_cards(f: &mut ratatui::Frame, area: Rect, a: &App) {
    let panel = Block::bordered().title("Global controls  [←→] select  [Enter] edit");
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    let card_width = (inner.width / GLOBAL_IDS.len() as u16).min(18);
    let cards_width = card_width.saturating_mul(GLOBAL_IDS.len() as u16);
    let cards = Rect {
        x: inner.x + inner.width.saturating_sub(cards_width) / 2,
        y: inner.y + inner.height.saturating_sub(7) / 2,
        width: cards_width,
        height: 7.min(inner.height),
    };
    let g = &a.editor.project.globals;
    for (index, id) in GLOBAL_IDS.iter().enumerate() {
        let slot = Rect {
            x: cards.x + card_width * index as u16,
            y: cards.y,
            width: card_width,
            height: cards.height,
        };
        let active = a.row == 0 && a.global == index;
        let block = if active {
            Block::bordered()
                .border_type(BorderType::Double)
                .border_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().reversed())
        } else {
            Block::bordered()
        };
        let content = block.inner(slot);
        f.render_widget(block, slot);
        let style = if active {
            Style::default().reversed().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let lines = vec![
            Line::from(Span::styled(global_display_name(*id), style)),
            Line::from(Span::styled(global_value_text(g, *id), style)),
            Line::from(Span::styled(
                format!("[{}]", global_shortcut_text(*id)),
                style,
            )),
        ];
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), content);
    }
}

fn render_parameter_bank(f: &mut ratatui::Frame, area: Rect, a: &App, track: usize) {
    let t = &a.editor.project.tracks[track];
    let lock_editing = a.scope == Scope::Lock && matches!(a.mode, Mode::ParameterEdit(_));
    let chord_shape = selected_chord_shape(a, track)
        .map(|shape| format!(" · [C] Shape {shape}"))
        .unwrap_or_default();
    let title = if matches!(t.kind, TrackKind::Bass | TrackKind::Chord | TrackKind::Lead) {
        format!(
            "{} · Step {} · {}{} · [p] {} · [m] Mute {} · [o] Audition{}",
            track_label(t),
            a.step + 1,
            articulation_title(a, track),
            chord_shape,
            scope_name(a.scope),
            if t.muted { "on" } else { "off" },
            if lock_editing {
                " · !! LOCK PARAMETER EDITING !!"
            } else {
                ""
            }
        )
    } else {
        format!(
            "{} · Step {} · {} · [p] {} · [m] Mute {} · [o] Audition{}",
            t.name,
            a.step + 1,
            articulation_title(a, track),
            scope_name(a.scope),
            if t.muted { "on" } else { "off" },
            if lock_editing {
                " · !! LOCK PARAMETER EDITING !!"
            } else {
                ""
            }
        )
    };
    let panel = Block::bordered()
        .border_style(if lock_editing {
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        })
        .title(Line::from(title).style(if lock_editing {
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }))
        .title_bottom(
            "LOCK values show effective output · LOCK = step override · BASE = inherited/base",
        );
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    let descriptors = parameter_descriptors(t.kind);
    let slot_width = (inner.width / descriptors.len() as u16).min(10);
    let bank_width = slot_width.saturating_mul(descriptors.len() as u16);
    let bank = Rect {
        x: inner.x + inner.width.saturating_sub(bank_width) / 2,
        y: inner.y + 1,
        width: bank_width,
        height: inner.height.saturating_sub(1),
    };

    let mut group_start = 0;
    while group_start < descriptors.len() {
        let group = descriptors[group_start].group;
        let group_end = descriptors[group_start..]
            .iter()
            .position(|descriptor| descriptor.group != group)
            .map(|offset| group_start + offset)
            .unwrap_or(descriptors.len());
        let group_area = Rect {
            x: bank.x + slot_width * group_start as u16,
            y: inner.y,
            width: slot_width * (group_end - group_start) as u16,
            height: 1,
        };
        render_centered(
            f,
            group.label(),
            group_area,
            Style::default()
                .fg(group.color())
                .add_modifier(Modifier::BOLD),
        );
        group_start = group_end;
    }

    for (index, descriptor) in descriptors.iter().enumerate() {
        let slot = Rect {
            x: bank.x + slot_width * index as u16,
            y: bank.y,
            width: slot_width,
            height: bank.height,
        };
        let active = matches!(
            a.mode,
            Mode::ParameterEdit(parameter) if parameter == descriptor.id
        ) || matches!(
            a.mode,
            Mode::LfoEdit { parameter, .. } if parameter == descriptor.id
        );
        let group_color = descriptor.group.color();
        let block = if active {
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Double)
                .border_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().reversed())
        } else {
            Block::default()
        };
        let content = if active {
            block.inner(slot)
        } else {
            Rect {
                x: slot.x + 1,
                y: slot.y,
                width: slot.width.saturating_sub(2),
                height: slot.height,
            }
        };
        f.render_widget(block, slot);
        let style = if active {
            Style::default()
                .fg(group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(group_color)
                .add_modifier(Modifier::BOLD)
        };
        let Some((value, origin)) = displayed_parameter(a, track, a.step, descriptor.id) else {
            continue;
        };
        let value_label = match value {
            ParameterValue::Percent(value) => format!("{}%", value.get()),
            ParameterValue::Waveform(Waveform::Square) => "SQR".into(),
            ParameterValue::Waveform(Waveform::Saw) => "SAW".into(),
            ParameterValue::Chorus(ChorusMode::Off) => "OFF".into(),
            ParameterValue::Chorus(ChorusMode::I) => "I".into(),
            ParameterValue::Chorus(ChorusMode::Ii) => "II".into(),
            ParameterValue::Spread(value) => match value {
                crate::model::ChordSpread::Off => "OFF".into(),
                crate::model::ChordSpread::Narrow => "NAR".into(),
                crate::model::ChordSpread::Wide => "WIDE".into(),
            },
        };
        render_centered(f, &value_label, content, style);
        for segment in 0..10 {
            let segment_area = Rect {
                x: content.x,
                y: content.y + 1 + segment,
                width: content.width,
                height: 1,
            };
            let symbol = match value {
                ParameterValue::Percent(value) => {
                    let filled = fader_segments(value.get());
                    if usize::from(segment) >= 10 - filled {
                        "███"
                    } else {
                        "···"
                    }
                }
                ParameterValue::Waveform(Waveform::Saw) => {
                    if segment == 0 {
                        "●"
                    } else if segment == 9 {
                        "○"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Waveform(Waveform::Square) => {
                    if segment == 0 {
                        "○"
                    } else if segment == 9 {
                        "●"
                    } else {
                        "│"
                    }
                }
                ParameterValue::Chorus(mode) => {
                    let selected = match mode {
                        ChorusMode::Off => 9,
                        ChorusMode::I => 5,
                        ChorusMode::Ii => 0,
                    };
                    if segment == selected { "●" } else { "│" }
                }
                ParameterValue::Spread(mode) => {
                    let selected = match mode {
                        crate::model::ChordSpread::Off => 9,
                        crate::model::ChordSpread::Narrow => 5,
                        crate::model::ChordSpread::Wide => 0,
                    };
                    if segment == selected { "●" } else { "│" }
                }
            };
            let segment_style = if active {
                Style::default()
                    .fg(group_color)
                    .reversed()
                    .add_modifier(Modifier::BOLD)
            } else if symbol == "···" {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
                    .fg(group_color)
                    .add_modifier(Modifier::BOLD)
            };
            render_centered(f, symbol, segment_area, segment_style);
        }
        let label_area = Rect {
            x: content.x,
            y: content.y + 11,
            width: content.width,
            height: 1,
        };
        render_centered(f, descriptor.label, label_area, style);
        let origin_label = match origin {
            ValueOrigin::Base => "BASE",
            ValueOrigin::Lock => "LOCK",
        };
        let shortcut_area = Rect {
            x: content.x,
            y: content.y + 12,
            width: content.width,
            height: 1,
        };
        let shortcut_style = if active {
            Style::default()
                .fg(group_color)
                .reversed()
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(group_color)
                .add_modifier(Modifier::BOLD)
        };
        let origin_style = if origin == ValueOrigin::Lock {
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD)
                .add_modifier(if active {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                })
        } else {
            shortcut_style
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("[{}]", descriptor.shortcut), shortcut_style),
                Span::styled(origin_label, origin_style),
                Span::styled(
                    if t.lfos.get(descriptor.id).is_some() {
                        "~"
                    } else {
                        ""
                    },
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .alignment(Alignment::Center),
            shortcut_area,
        );
    }
}

fn render_centered(f: &mut ratatui::Frame, text: &str, area: Rect, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_owned(), style)))
            .alignment(Alignment::Center),
        area,
    );
}

fn draw(f: &mut ratatui::Frame, a: &App, audio: &Audio) {
    draw_with_device(f, a, &audio.device_name);
}

fn draw_with_device(f: &mut ratatui::Frame, a: &App, device_name: &str) {
    let area = f.area();
    if area.width < 120 || area.height < 34 {
        f.render_widget(
            Paragraph::new(format!(
                "terminal-groove needs 120x34\nCurrent: {}x{}\nCtrl+Q quit  ? help",
                area.width, area.height
            ))
            .block(Block::bordered().title("Terminal too small")),
            area,
        );
        return;
    }
    let details_height = if a.row == 0 { 9 } else { 16 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(9),
            Constraint::Length(details_height),
            Constraint::Length(2),
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
    let pattern_state = format!(
        "{} / {}",
        a.editor.pattern() + 1,
        a.editor.project.patterns.len()
    );
    let header = vec![
        Line::from(vec![
            Span::styled(
                " terminal-groove ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {file}{dirty} | audio: {} | {transport} | {pattern_state} | {} BPM",
                device_name, a.editor.project.globals.tempo_bpm
            )),
            Span::raw("  [Ctrl+P] Patterns"),
            Span::raw("  [Ctrl+S] Save [Ctrl+O] Open"),
        ]),
        Line::from("[Space] Play/Pause  [.] Stop  [Ctrl+Z/Y] Undo/Redo  [Ctrl+Q] Quit  [?] Help"),
    ];
    f.render_widget(Paragraph::new(header), chunks[0]);
    let g = &a.editor.project.globals;
    let globals = global_control_text(g);
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
        Paragraph::new(Line::from(line))
            .block(Block::bordered().title("Globals [←→] select [Enter] edit")),
        chunks[1],
    );
    let available_rows = chunks[2].height.saturating_sub(3) as usize;
    let heights = a
        .editor
        .project
        .tracks
        .iter()
        .map(|track| track.steps.len().div_ceil(STEP_ROW_SIZE))
        .collect::<Vec<_>>();
    let selected_track = a.row.saturating_sub(1).min(TRACK_COUNT - 1);
    let mut first_track = selected_track;
    let mut used_rows = heights[selected_track];
    while first_track > 0 && used_rows + heights[first_track - 1] <= available_rows {
        first_track -= 1;
        used_rows += heights[first_track];
    }
    let mut last_track = selected_track + 1;
    while last_track < TRACK_COUNT && used_rows + heights[last_track] <= available_rows {
        used_rows += heights[last_track];
        last_track += 1;
    }
    let mut rows = Vec::new();
    for (ti, &track_height) in heights
        .iter()
        .enumerate()
        .take(last_track)
        .skip(first_track)
    {
        let track = &a.editor.project.tracks[ti];
        let length = track.steps.len();
        for line_index in 0..track_height {
            let line_start = line_index * STEP_ROW_SIZE;
            let line_end = (line_start + STEP_ROW_SIZE).min(length);
            let mut cells: Vec<ratatui::widgets::Cell> = Vec::with_capacity(37);
            cells.push(if line_index == 0 && track.muted {
                "M".into()
            } else {
                " ".into()
            });
            cells.push(if line_index == 0 {
                track_label(track).into()
            } else {
                "↳".into()
            });
            cells.push(format!(" {:02}–{:02}", line_start + 1, line_end).into());
            cells.push(ratatui::widgets::Cell::from("│"));
            for slot in 0..STEP_ROW_SIZE {
                if slot == STEP_BANK_SIZE {
                    cells.push(ratatui::widgets::Cell::from("│"));
                }
                let step = line_start + slot;
                if step >= length {
                    cells.push(ratatui::widgets::Cell::from("   "));
                    continue;
                }
                let mut style = Style::default();
                if a.row == ti + 1 && a.step == step {
                    style = style.reversed();
                }
                if a.playheads[ti] == Some(step) {
                    style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
                }
                if matches!(
                    track.steps[step],
                    Some(StepEvent::BassNote { slide: true, .. })
                ) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                cells.push(
                    ratatui::widgets::Cell::from(step_cell(track.steps[step].as_ref()))
                        .style(style),
                );
            }
            rows.push(Row::new(cells));
        }
    }
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(1),
    ];
    widths.extend((0..STEP_BANK_SIZE).map(|_| Constraint::Length(3)));
    widths.push(Constraint::Length(1));
    widths.extend((0..STEP_BANK_SIZE).map(|_| Constraint::Length(3)));
    let mut header_cells = vec![
        ratatui::widgets::Cell::from("M"),
        ratatui::widgets::Cell::from("Track / O"),
        ratatui::widgets::Cell::from(" Range"),
        ratatui::widgets::Cell::from("│"),
    ];
    header_cells.extend((1..=STEP_BANK_SIZE).map(|n| format!(" {n:02}").into()));
    header_cells.push(ratatui::widgets::Cell::from("│"));
    header_cells.extend(((STEP_BANK_SIZE + 1)..=STEP_ROW_SIZE).map(|n| format!(" {n:02}").into()));
    let scroll_hint = match (first_track > 0, last_track < TRACK_COUNT) {
        (true, true) => "  ↑↓ more tracks",
        (true, false) => "  ↑ more tracks",
        (false, true) => "  ↓ more tracks",
        (false, false) => "",
    };
    let pattern_help = format!(
        "[↑↓] vertical  [←→] step  [Shift+←→] bank  [Shift+1..6] track  [l] length  [Shift+D] double{scroll_hint}"
    );
    let pattern_title = Line::from(format!("Pattern  {pattern_help}"));
    f.render_widget(
        Table::new(rows, widths)
            .column_spacing(0)
            .header(Row::new(header_cells))
            .block(
                Block::bordered().title(pattern_title).title_bottom(
                    ". empty   x/X normal/accent   D:O note   D!O accent   */# lock   underline slide   - tie",
                ),
            ),
        chunks[2],
    );
    if a.row == 0 {
        render_global_cards(f, chunks[3], a);
    } else {
        let track = a.row - 1;
        render_parameter_bank(f, chunks[3], a, track);
    }
    let lock_editing =
        a.scope == Scope::Lock && matches!(a.mode, Mode::ParameterEdit(_) | Mode::LfoEdit { .. });
    let mode_line = if lock_editing {
        let parameter = match a.mode {
            Mode::ParameterEdit(parameter) | Mode::LfoEdit { parameter, .. } => {
                parameter.display_name()
            }
            _ => unreachable!(),
        };
        Line::from(vec![
            Span::styled(
                " LOCK PARAMETER EDITING ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" ({parameter}) | {}", a.status)),
        ])
    } else {
        Line::from(format!("Mode: {} | {}", mode_name(&a.mode), a.status))
    };
    let mut status_lines = vec![mode_line];
    if let Mode::ParameterEdit(parameter) = a.mode {
        let track = a.row.saturating_sub(1);
        status_lines.push(Line::from(format!(
            "{} · [↑/↓] ±1  [Shift+↑/↓] ±10  [PageUp/Down] step  [Shift+1..6] track  [0-9] set percentage  [Shift+L] LFO  [Enter/Esc] finish  [Del] clear lock",
            physical_parameter_readout(a, track, a.step, parameter)
        )));
    } else if matches!(a.mode, Mode::LfoEdit { .. }) {
        status_lines.push(Line::from(
            "Track-level LFO · [←/→] field  [↑/↓] adjust  [0-9] set rate/depth  [Del] remove  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::ChordEdit { .. }) {
        status_lines.push(Line::from(
            "Chord shape · [↑/↓] select recipe  [PageUp/Down] step  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::GlobalEdit(_) | Mode::TempoInput(_)) {
        status_lines.push(Line::from(
            "[↑/↓] adjust  [←/→] select another control  [Enter/Esc] finish",
        ));
    } else if matches!(a.mode, Mode::TrackLengthInput(_)) {
        status_lines.push(Line::from(
            "Type 1–64 and press Enter; [↑/↓] ±1  [Shift+↑/↓] ±16  [Esc] finish",
        ));
    }
    f.render_widget(Paragraph::new(status_lines), chunks[4]);
    if a.mode == Mode::Help {
        popup(
            f,
            area,
            "Help",
            "All sound is synthesized.\nPatterns: Ctrl+P opens the horizontal dialog; arrows, Home, and End move the cursor. Enter selects or queues it. N insert, D duplicate, C copy, X cut, V paste, Delete remove.\nNavigation: arrows, Shift+←/→ jumps 16 steps, Shift+1..6 selects a track, Enter, Delete.\nParameter editing: PageUp/Down changes step, Shift+1..6 selects a track.\nEvents: Shift+A toggles accent; Shift+G toggles Bass slide.\nTracks: l length, Shift+D double, p scope, v level, n pan, m mute, y delay, b reverb.\nParameters: Shift+L adds/edits an eligible track LFO; ~ marks an assignment.\nKick: u tune, d decay, a attack. Snare: u tune, t tone, s snappy. Hat: u tune, d decay.\nBass: w waveform, c cutoff, R resonance, f envelope, d decay.\nChord/Lead: w oscillator mix, Shift+P pulse width, u sub, c/R/f filter, a/d/s/r ADSR. Chord: h chorus, e spread, C shape.\nGlobal: t tempo, y delay, f feedback, r reverb time, b tone, p pre-delay, k key, s scale.\nAnywhere: Space play/pause, . stop, o audition, Ctrl+S save, Ctrl+O open, Ctrl+Z/Y undo/redo, Ctrl+Q quit.\nEsc or ? closes help.",
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
        Mode::PatternDialog => render_pattern_popup(f, area, a),
        Mode::LfoEdit { parameter, field } => {
            let track = a.row - 1;
            if let Ok(Some(config)) = a.editor.lfo(track, *parameter) {
                render_lfo_popup(
                    f,
                    area,
                    *parameter,
                    config,
                    *field,
                    a.editor.project.globals.tempo_bpm,
                );
            }
        }
        Mode::ChordEdit { shape } => render_chord_popup(f, chunks[3], *shape, a.step),
        Mode::TempoInput(input) => popup(
            f,
            area,
            "Tempo numeric input",
            &format!("Tempo: {input}_\nEnter confirms  Esc cancels  ↑/↓ adjusts current tempo"),
        ),
        Mode::TrackLengthInput(input) => {
            let current = a.editor.project.tracks[a.row - 1].steps.len();
            popup(
                f,
                area,
                "Track length",
                &format!(
                    "Current: {current}\nLength: {input}_\nEnter confirms  Esc finishes  ↑/↓ ±1  Shift+↑/↓ ±16"
                ),
            )
        }
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

fn render_chord_popup(f: &mut ratatui::Frame, area: Rect, selected: ChordShape, step: usize) {
    let popup_area = lfo_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let panel = Block::bordered().title(format!("Chord shape · Step {}", step + 1));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);
    let choices = ChordShape::ALL.map(|shape| shape.to_string());
    let selected_index = ChordShape::ALL
        .iter()
        .position(|shape| *shape == selected)
        .unwrap();
    render_lfo_selector(
        f,
        Rect {
            height: inner.height.saturating_sub(2),
            ..inner
        },
        &choices,
        selected_index,
        Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(
        Paragraph::new("[↑/↓] select   [Enter/Esc] close").alignment(Alignment::Center),
        Rect {
            y: inner.y + inner.height.saturating_sub(1),
            height: 1.min(inner.height),
            ..inner
        },
    );
}

fn render_pattern_popup(f: &mut ratatui::Frame, area: Rect, a: &App) {
    let height = 7.min(area.height.saturating_sub(4));
    let width = area.width.saturating_sub(8).max(24);
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, popup_area);
    let block = Block::bordered().title(format!("Patterns ({})", a.editor.project.patterns.len()));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let cell_width = 6usize;
    let cursor_x = a.pattern_cursor.saturating_mul(cell_width);
    let visible_width = usize::from(inner.width);
    let scroll_x = cursor_x
        .saturating_add(cell_width)
        .saturating_sub(visible_width)
        .min(cursor_x)
        .min(usize::from(u16::MAX));
    let cells = a
        .editor
        .project
        .patterns
        .iter()
        .enumerate()
        .flat_map(|(index, pattern)| {
            let empty = pattern_is_empty(pattern);
            let cursor = index == a.pattern_cursor;
            let active = index == a.active_pattern;
            let queued = a.queued_pattern == Some(index);
            let marker = match (active, queued) {
                (true, true) => "▶⏭",
                (true, false) => "▶ ",
                (false, true) => "⏭ ",
                (false, false) => "  ",
            };
            let base = if empty { Color::DarkGray } else { Color::White };
            let style = if cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(base)
            };
            let marker_style = if cursor {
                style
            } else if active {
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else if queued {
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                style
            };
            [
                Span::styled(marker, marker_style),
                Span::styled(format!("{:>3} ", index + 1), style),
            ]
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(Line::from(cells)).scroll((0, scroll_x as u16)),
        Rect {
            height: 1.min(inner.height),
            ..inner
        },
    );
    let help_y = inner.y + 2.min(inner.height.saturating_sub(1));
    f.render_widget(
        Paragraph::new(vec![
            Line::from("▶ playing  ⏭ next  ·  ←/→ Home End  ·  Enter queue"),
            Line::from(
                "N insert · D duplicate · C copy · X cut · V paste · Delete remove · Esc close",
            ),
        ]),
        Rect {
            y: help_y,
            height: inner.height.saturating_sub(help_y.saturating_sub(inner.y)),
            ..inner
        },
    );
}

fn pattern_is_empty(pattern: &crate::model::Pattern) -> bool {
    pattern
        .tracks
        .iter()
        .all(|track| track.steps.iter().all(Option::is_none))
}

fn render_lfo_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    parameter: ParameterId,
    config: LfoConfig,
    selected: LfoField,
    tempo_bpm: u16,
) {
    let popup_area = lfo_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let panel = Block::bordered().title(format!("Track LFO · {}", parameter.display_name()));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);

    let controls_area = Rect {
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 5); 5])
        .split(controls_area);
    for (index, field) in LfoField::ALL.iter().enumerate() {
        render_lfo_control(
            f,
            columns[index],
            config,
            *field,
            selected == *field,
            tempo_bpm,
        );
    }

    let help_area = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(2),
        width: inner.width,
        height: 2.min(inner.height),
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from("[←/→] select   [↑/↓] adjust   [Shift+↑/↓] ±10"),
            Line::from("[0-9] set free rate/depth   [Del] remove   [Enter/Esc] close"),
        ])
        .alignment(Alignment::Center),
        help_area,
    );
}

fn render_lfo_control(
    f: &mut ratatui::Frame,
    area: Rect,
    config: LfoConfig,
    field: LfoField,
    active: bool,
    tempo_bpm: u16,
) {
    let accent = Color::LightCyan;
    let style = if active {
        Style::default()
            .fg(accent)
            .reversed()
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    };
    let label = match field {
        LfoField::Enabled => "Enabled",
        LfoField::Waveform => "Waveform",
        LfoField::RateMode => "Rate Mode",
        LfoField::Rate => "Rate",
        LfoField::Depth => "Depth",
    };
    let block = (if active {
        Block::bordered()
            .border_type(BorderType::Double)
            .border_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().reversed())
    } else {
        Block::bordered().border_style(Style::default().fg(accent))
    })
    .title(Line::from(Span::styled(label, style)));
    let content = block.inner(area);
    f.render_widget(block, area);
    if content.height == 0 {
        return;
    }

    match field {
        LfoField::Enabled => render_lfo_switch(f, content, "ON", "OFF", config.enabled, style),
        LfoField::Waveform => {
            let choices = ["Sine", "Triangle", "Square", "Saw", "Sample & hold"];
            let current = LfoWaveform::ALL
                .iter()
                .position(|waveform| *waveform == config.waveform)
                .unwrap();
            render_lfo_selector(f, content, &choices, current, style);
        }
        LfoField::RateMode => render_lfo_switch(
            f,
            content,
            "SYNCED",
            "FREE",
            matches!(config.rate, LfoRate::Synced { .. }),
            style,
        ),
        LfoField::Rate => match config.rate {
            LfoRate::Synced { division } => {
                let choices = LfoDivision::ALL.map(|value| value.to_string());
                let current = LfoDivision::ALL
                    .iter()
                    .position(|value| *value == division)
                    .unwrap();
                render_lfo_selector(
                    f,
                    Rect {
                        height: content.height.saturating_sub(1),
                        ..content
                    },
                    &choices,
                    current,
                    style,
                );
                render_centered(
                    f,
                    &format!("{:.3} Hz", config.rate.hz(tempo_bpm)),
                    Rect {
                        y: content.y + content.height.saturating_sub(1),
                        height: 1.min(content.height),
                        ..content
                    },
                    style,
                );
            }
            LfoRate::Free { rate_percent } => {
                render_centered(
                    f,
                    &format!("{}%", rate_percent.get()),
                    Rect {
                        height: 1,
                        ..content
                    },
                    style,
                );
                render_lfo_fader(
                    f,
                    Rect {
                        y: content.y + 1,
                        height: content.height.saturating_sub(2),
                        ..content
                    },
                    rate_percent.get(),
                    style,
                );
                render_centered(
                    f,
                    &format!("{:.3} Hz", config.rate.hz(tempo_bpm)),
                    Rect {
                        y: content.y + content.height.saturating_sub(1),
                        height: 1.min(content.height),
                        ..content
                    },
                    style,
                );
            }
        },
        LfoField::Depth => {
            render_centered(
                f,
                &format!("±{} pp", config.depth.get()),
                Rect {
                    height: 1,
                    ..content
                },
                style,
            );
            render_lfo_fader(
                f,
                Rect {
                    y: content.y + 1,
                    height: content.height.saturating_sub(1),
                    ..content
                },
                config.depth.get(),
                style,
            );
        }
    }
}

fn render_lfo_fader(f: &mut ratatui::Frame, area: Rect, value: u8, active_style: Style) {
    let height = area.height.min(10);
    let start_y = area.y + area.height.saturating_sub(height) / 2;
    let filled = fader_segments(value);
    for segment in 0..height {
        let is_filled = usize::from(segment) >= 10usize.saturating_sub(filled);
        let style = if is_filled {
            active_style
        } else {
            lfo_inactive_style(active_style)
        };
        render_centered(
            f,
            if is_filled { "███" } else { "···" },
            Rect {
                x: area.x,
                y: start_y + segment,
                width: area.width,
                height: 1,
            },
            style,
        );
    }
}

fn render_lfo_switch(
    f: &mut ratatui::Frame,
    area: Rect,
    top: &str,
    bottom: &str,
    top_selected: bool,
    style: Style,
) {
    if area.height == 0 {
        return;
    }
    let top_y = area.y;
    let bottom_y = area.y + area.height - 1;
    render_centered(
        f,
        &format!("{} {top}", if top_selected { "●" } else { "○" }),
        Rect { height: 1, ..area },
        if top_selected {
            style
        } else {
            lfo_inactive_style(style)
        },
    );
    for y in top_y + 1..bottom_y {
        render_centered(
            f,
            "│",
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            lfo_inactive_style(style),
        );
    }
    if area.height > 1 {
        render_centered(
            f,
            &format!("{} {bottom}", if top_selected { "○" } else { "●" }),
            Rect {
                y: bottom_y,
                height: 1,
                ..area
            },
            if top_selected {
                lfo_inactive_style(style)
            } else {
                style
            },
        );
    }
}

fn render_lfo_selector<T: AsRef<str>>(
    f: &mut ratatui::Frame,
    area: Rect,
    choices: &[T],
    selected: usize,
    style: Style,
) {
    if area.height == 0 || choices.is_empty() {
        return;
    }
    let visible = choices.len().min(usize::from(area.height));
    let half = visible / 2;
    let start = selected
        .saturating_sub(half)
        .min(choices.len().saturating_sub(visible));
    let y = area.y + area.height.saturating_sub(visible as u16) / 2;
    for (row, choice) in choices[start..start + visible].iter().enumerate() {
        let index = start + row;
        let text = if index == selected {
            format!("● {}", choice.as_ref())
        } else {
            format!("○ {}", choice.as_ref())
        };
        let choice_style = if index == selected {
            style
        } else {
            lfo_inactive_style(style)
        };
        render_centered(
            f,
            &text,
            Rect {
                x: area.x,
                y: y + row as u16,
                width: area.width,
                height: 1,
            },
            choice_style,
        );
    }
}

fn lfo_inactive_style(active_style: Style) -> Style {
    if active_style.add_modifier.contains(Modifier::REVERSED) {
        active_style
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn popup(f: &mut ratatui::Frame, area: Rect, title: &str, text: &str) {
    let r = popup_rect(area);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        r,
    )
}

fn lfo_popup_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(92);
    let height = area.height.saturating_sub(4).min(20);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn popup_rect(area: Rect) -> Rect {
    Rect {
        x: area.x + 10,
        y: area.y + 5,
        width: area.width - 20,
        height: (area.height - 10).max(5),
    }
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
            parameter_shortcut(TrackKind::Kick, 'u'),
            Some(ParameterId::Tune)
        );
        assert_eq!(
            parameter_shortcut(TrackKind::Chord, 'd'),
            Some(ParameterId::Decay)
        );
        assert_eq!(
            parameter_shortcut(TrackKind::Chord, 'R'),
            Some(ParameterId::Resonance)
        );
        assert_eq!(parameter_shortcut(TrackKind::Chord, 't'), None);
    }

    #[test]
    fn parameter_banks_have_contextual_order_and_shortcuts() {
        let drum = parameter_descriptors(TrackKind::Kick);
        assert_eq!(drum.len(), 7);
        assert_eq!(drum[0].shortcut, "v");
        assert_eq!(drum[0].group, ParameterGroup::Mixer);
        assert_eq!(drum[4].shortcut, "u");
        assert_eq!(drum[4].group, ParameterGroup::Instrument);
        let synth = parameter_descriptors(TrackKind::Chord);
        assert_eq!(synth.len(), 16);
        assert_eq!(synth[4].id, ParameterId::OscillatorMix);
        assert_eq!(synth[4].group, ParameterGroup::Instrument);
        assert_eq!(synth[5].id, ParameterId::PulseWidth);
        assert_eq!(synth[7].id, ParameterId::Chorus);
        assert_eq!(synth[9].group, ParameterGroup::Filter);
        assert_eq!(synth[10].shortcut, "R");
        assert_eq!(synth[12].group, ParameterGroup::Envelope);
        assert_ne!(
            ParameterGroup::Mixer.color(),
            ParameterGroup::Filter.color()
        );
    }

    #[test]
    fn parameter_editor_arrows_cycle_visible_controls_and_wrap() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.step = 4;
        app.scope = Scope::Lock;
        app.mode = Mode::ParameterEdit(ParameterId::Level);
        move_parameter_editor(&mut app, false);
        assert!(matches!(app.mode, Mode::ParameterEdit(ParameterId::Attack)));
        assert_eq!((app.row, app.step, app.scope), (1, 4, Scope::Lock));
        move_parameter_editor(&mut app, true);
        assert!(matches!(app.mode, Mode::ParameterEdit(ParameterId::Level)));

        app.row = 4;
        move_parameter_editor(&mut app, true);
        move_parameter_editor(&mut app, true);
        move_parameter_editor(&mut app, true);
        move_parameter_editor(&mut app, true);
        assert!(matches!(
            app.mode,
            Mode::ParameterEdit(ParameterId::Waveform)
        ));
    }

    #[test]
    fn page_step_navigation_moves_right_and_wraps_left() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.step = 0;
        move_step_page(&mut app, false);
        assert_eq!(app.step, 15);
        move_step_page(&mut app, true);
        assert_eq!(app.step, 0);
        move_step_page(&mut app, true);
        assert_eq!(app.step, 1);
    }

    #[test]
    fn pattern_navigation_wraps_across_the_dynamic_list() {
        assert_eq!(adjacent_pattern_in_count(0, false, 3), 2);
        assert_eq!(adjacent_pattern_in_count(2, true, 3), 0);
        assert_eq!(adjacent_pattern_in_count(1, false, 3), 0);
        assert_eq!(adjacent_pattern_in_count(1, true, 3), 2);
    }

    #[test]
    fn pattern_dialog_renders_horizontal_plain_numbered_states() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns.push(project.patterns[0].clone());
        project.patterns[1].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.mode = Mode::PatternDialog;
        app.pattern_cursor = 2;
        app.active_pattern = 0;
        app.queued_pattern = Some(1);

        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Patterns (3)"));
        assert!(screen.contains("1") && screen.contains("2") && screen.contains("3"));
        assert!(screen.contains("▶") && screen.contains("⏭"));
        assert!(!screen.contains("P001"));
        assert!(!screen.contains("used"));
    }

    #[test]
    fn shifted_track_numbers_select_the_expected_track() {
        for (key, track) in ('1'..='6').zip(0..TRACK_COUNT) {
            assert_eq!(
                track_jump_index(KeyEvent::new(KeyCode::Char(key), KeyModifiers::SHIFT)),
                Some(track)
            );
        }
        assert_eq!(
            track_jump_index(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            track_jump_index(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)),
            Some(0)
        );
        assert_eq!(
            track_jump_index(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::SHIFT)),
            Some(0)
        );
    }

    #[test]
    fn track_jump_clamps_step_and_replaces_incompatible_parameter() {
        let mut project = Project::new();
        project.tracks[0].steps.resize(4, None);
        let mut app = App::new(project, None);
        app.row = 4;
        app.step = 15;
        app.scope = Scope::Lock;
        app.mode = Mode::ParameterEdit(ParameterId::Cutoff);

        select_track(&mut app, 0);

        assert_eq!((app.row, app.step, app.scope), (1, 3, Scope::Lock));
        assert_eq!(app.mode, Mode::ParameterEdit(ParameterId::Level));
    }

    #[test]
    fn waveform_editor_switches_between_its_two_values() {
        assert_eq!(flipped_waveform(Waveform::Saw), Waveform::Square);
        assert_eq!(flipped_waveform(Waveform::Square), Waveform::Saw);
    }

    #[test]
    fn parameter_editing_passes_audition_key_to_global_handler() {
        assert!(parameter_edit_passthrough(KeyCode::Char('o')));
        assert!(parameter_edit_passthrough(KeyCode::Char('A')));
        assert!(parameter_edit_passthrough(KeyCode::Char('G')));
    }

    #[test]
    fn accent_readout_resolves_ties_to_their_source_note() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.editor.set_note(3, 0, 1).unwrap();
        app.editor.toggle_accent(3, 0).unwrap();
        app.editor.toggle_tie(3, 1).unwrap();
        assert_eq!(selected_accent(&app, 3), Some((true, None)));
        app.step = 1;
        assert_eq!(selected_accent(&app, 3), Some((true, Some(0))));
        assert_eq!(articulation_title(&app, 3), "Accent on from step 1");
    }

    #[test]
    fn fader_animation_interpolates_and_reaches_its_target() {
        let started = Instant::now();
        let animation = FaderAnimation {
            track: 0,
            step: 0,
            scope: Scope::Base,
            parameter: ParameterId::Level,
            from: Percent::new(20).unwrap(),
            to: Percent::new(80).unwrap(),
            started,
        };
        assert_eq!(animation.value_at(started).get(), 20);
        assert_eq!(
            animation
                .value_at(started + DIRECT_PARAMETER_RAMP / 2)
                .get(),
            50
        );
        assert_eq!(
            animation.value_at(started + DIRECT_PARAMETER_RAMP).get(),
            80
        );
        assert!(animation.is_complete(started + DIRECT_PARAMETER_RAMP));
    }

    #[test]
    fn fader_segments_round_to_nearest_ten_percent() {
        assert_eq!(fader_segments(0), 0);
        assert_eq!(fader_segments(4), 0);
        assert_eq!(fader_segments(5), 1);
        assert_eq!(fader_segments(65), 7);
        assert_eq!(fader_segments(94), 9);
        assert_eq!(fader_segments(95), 10);
        assert_eq!(fader_segments(100), 10);
    }

    #[test]
    fn screen_renders_fader_bank_and_local_shortcuts_at_minimum_size() {
        let backend = TestBackend::new(120, 34);
        let mut project = Project::new();
        project.tracks[3].input_octave = Some(4);
        project.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            locks: Default::default(),
        });
        project.tracks[3].steps[1] = Some(StepEvent::Note {
            degree: 2,
            octave: 4,
            accent: false,
            chord_shape: None,
            locks: crate::model::ParameterLocks {
                cutoff: Some(Percent::new(50).unwrap()),
                ..Default::default()
            },
        });
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(project, None);
        app.row = 4;
        terminal
            .draw(|frame| draw_with_device(frame, &app, "null"))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Level"));
        assert!(rendered.contains("Filt Env"));
        assert!(rendered.contains("MIXER"));
        assert!(rendered.contains("INSTRUMENT"));
        assert!(rendered.contains("FILTER"));
        assert!(rendered.contains("ENVELOPE"));
        assert!(rendered.contains("[v]BASE"));
        assert!(rendered.contains("[R]BASE"));
        assert!(!rendered.contains("L80Y0B0"));
        assert!(rendered.contains("Bass O4"));
        assert!(rendered.contains("1:3"));
        assert!(rendered.contains("2*4"));
    }

    #[test]
    fn small_terminal_replaces_main_layout() {
        let app = App::new(Project::new(), None);
        let screen = rendered(&app, 119, 34);
        assert!(screen.contains("terminal-groove needs 120x34"));
        assert!(screen.contains("Current: 119x34"));
    }

    #[test]
    fn lfo_modal_and_fader_badge_render_at_minimum_size() {
        let mut project = Project::new();
        project.tracks[3].lfos.cutoff = Some(LfoConfig::default());
        let mut app = App::new(project, None);
        app.row = 4;
        app.mode = Mode::LfoEdit {
            parameter: ParameterId::Cutoff,
            field: LfoField::Depth,
        };
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Track LFO · cutoff"));
        assert!(screen.contains("● Sine"));
        assert!(screen.contains("±10 pp"));
        assert!(screen.contains("███"));
        assert!(screen.contains("║"));
        assert!(screen.contains("~"));
    }

    #[test]
    fn chord_shape_modal_and_title_render_at_minimum_size() {
        let mut project = Project::new();
        project.tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::SeventhFirstInversion),
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.row = 5;
        let title_screen = rendered(&app, 120, 34);
        assert!(title_screen.contains("[C] Shape 3-5-7-1"));
        app.mode = Mode::ChordEdit {
            shape: ChordShape::SeventhFirstInversion,
        };
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Chord shape · Step 1"));
        assert!(screen.contains("● 3-5-7-1"));
        assert!(screen.contains("Chord shape · [↑/↓] select recipe  [PageUp/Down] step"));
    }

    #[test]
    fn chord_shape_editor_page_navigation_follows_each_step() {
        let mut project = Project::new();
        project.tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            locks: Default::default(),
        });
        project.tracks[4].steps[1] = Some(StepEvent::Note {
            degree: 2,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::Sus4SecondInversion),
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.row = 5;
        app.mode = Mode::ChordEdit {
            shape: ChordShape::TriadRoot,
        };

        move_chord_editor_step(&mut app, true);

        assert_eq!(app.step, 1);
        assert_eq!(
            app.mode,
            Mode::ChordEdit {
                shape: ChordShape::Sus4SecondInversion
            }
        );

        move_chord_editor_step(&mut app, false);
        assert_eq!(app.step, 0);
        assert_eq!(
            app.mode,
            Mode::ChordEdit {
                shape: ChordShape::TriadRoot
            }
        );
    }

    #[test]
    fn lfo_modal_is_centered_and_capped_on_large_terminals() {
        assert_eq!(
            lfo_popup_rect(Rect::new(0, 0, 120, 34)),
            Rect::new(14, 7, 92, 20)
        );
        assert_eq!(
            lfo_popup_rect(Rect::new(0, 0, 200, 50)),
            Rect::new(54, 15, 92, 20)
        );
    }

    #[test]
    fn lfo_control_bank_reports_synced_and_physical_free_rates() {
        let mut project = Project::new();
        project.tracks[3].lfos.cutoff = Some(LfoConfig::default());
        let mut app = App::new(project, None);
        app.row = 4;
        app.mode = Mode::LfoEdit {
            parameter: ParameterId::Cutoff,
            field: LfoField::Rate,
        };
        let synced = rendered(&app, 120, 34);
        assert!(synced.contains("SYNCED"));
        assert!(synced.contains("● 1/4"));
        assert!(synced.contains("○ 4 bars"));
        assert!(synced.contains("○ 1/16T"));
        assert!(synced.contains("2.000 Hz"));

        app.editor.project.tracks[3].lfos.cutoff = Some(LfoConfig {
            rate: LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            },
            ..Default::default()
        });
        let free = rendered(&app, 120, 34);
        assert!(free.contains("100%"));
        assert!(free.contains("20.000 Hz"));
    }

    #[test]
    fn lfo_option_arrows_follow_the_visual_list_direction() {
        let quarter = LfoDivision::ALL
            .iter()
            .position(|division| *division == LfoDivision::Quarter)
            .unwrap();
        assert_eq!(
            LfoDivision::ALL[lfo_choice_index(quarter, LfoDivision::ALL.len(), KeyCode::Up,)],
            LfoDivision::QuarterDotted
        );
        assert_eq!(
            LfoDivision::ALL[lfo_choice_index(quarter, LfoDivision::ALL.len(), KeyCode::Down,)],
            LfoDivision::QuarterTriplet
        );
        assert_eq!(lfo_choice_index(0, LfoDivision::ALL.len(), KeyCode::Up), 0);
        assert_eq!(
            lfo_choice_index(
                LfoDivision::ALL.len() - 1,
                LfoDivision::ALL.len(),
                KeyCode::Down,
            ),
            LfoDivision::ALL.len() - 1
        );
        assert_eq!(lfo_choice_index(0, 2, KeyCode::Up), 0);
        assert_eq!(lfo_choice_index(0, 2, KeyCode::Down), 1);
        assert_eq!(lfo_choice_index(1, 2, KeyCode::Up), 0);
        assert_eq!(lfo_choice_index(1, 2, KeyCode::Down), 1);
    }

    #[test]
    fn lfo_controls_are_laid_out_left_to_right() {
        let mut project = Project::new();
        project.tracks[3].lfos.cutoff = Some(LfoConfig::default());
        let mut app = App::new(project, None);
        app.row = 4;
        app.mode = Mode::LfoEdit {
            parameter: ParameterId::Cutoff,
            field: LfoField::Enabled,
        };
        let screen = rendered(&app, 120, 34);
        let enabled = screen.rfind("Enabled").unwrap();
        let waveform = screen.rfind("Waveform").unwrap();
        let rate_mode = screen.rfind("Rate Mode").unwrap();
        let rate = screen.rfind("Rate").unwrap();
        let depth = screen.rfind("Depth").unwrap();
        assert!(enabled < waveform);
        assert!(waveform < rate_mode);
        assert!(rate_mode < rate);
        assert!(rate < depth);
    }

    #[test]
    fn lock_scope_labels_explicit_and_inherited_values() {
        let mut project = Project::new();
        project.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            locks: crate::model::ParameterLocks {
                cutoff: Some(Percent::new(50).unwrap()),
                ..Default::default()
            },
        });
        let mut app = App::new(project, None);
        app.row = 4;
        app.scope = Scope::Lock;
        let (_, cutoff_origin) = displayed_parameter(&app, 3, 0, ParameterId::Cutoff).unwrap();
        let (_, level_origin) = displayed_parameter(&app, 3, 0, ParameterId::Level).unwrap();
        assert_eq!(cutoff_origin, ValueOrigin::Lock);
        assert_eq!(level_origin, ValueOrigin::Base);
    }

    #[test]
    fn lock_values_remain_displayed_after_track_navigation() {
        let mut project = Project::new();
        project.tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            locks: crate::model::ParameterLocks {
                level: Some(Percent::new(25).unwrap()),
                ..Default::default()
            },
        });
        let mut app = App::new(project, None);
        app.row = 1;
        app.scope = Scope::Lock;
        move_step_vertical(&mut app, true);
        move_step_vertical(&mut app, false);
        assert_eq!((app.row, app.step, app.scope), (1, 0, Scope::Lock));

        let (value, origin) = displayed_parameter(&app, 0, 0, ParameterId::Level).unwrap();
        assert_eq!(value, ParameterValue::Percent(Percent::new(25).unwrap()));
        assert_eq!(origin, ValueOrigin::Lock);
    }

    #[test]
    fn active_parameter_gets_a_visible_fader_outline_and_physical_readout() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.mode = Mode::ParameterEdit(ParameterId::Cutoff);
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("║"));
        assert!(screen.contains("Hz · BASE"));
    }

    #[test]
    fn lock_parameter_editing_has_a_prominent_banner() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.scope = Scope::Lock;
        app.mode = Mode::ParameterEdit(ParameterId::Cutoff);
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("LOCK PARAMETER EDITING"));
        assert!(screen.contains("(cutoff)"));

        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_with_device(frame, &app, "null"))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| { cell.fg == Color::Black && cell.bg == Color::LightYellow })
        );
    }

    #[test]
    fn base_parameter_editing_does_not_show_lock_editing_banner() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.mode = Mode::ParameterEdit(ParameterId::Cutoff);
        let screen = rendered(&app, 120, 34);
        assert!(!screen.contains("LOCK PARAMETER EDITING"));
    }

    #[test]
    fn locked_badge_uses_a_distinct_color() {
        let mut project = Project::new();
        project.tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            locks: crate::model::ParameterLocks {
                cutoff: Some(Percent::new(50).unwrap()),
                ..Default::default()
            },
        });
        let mut app = App::new(project, None);
        app.row = 4;
        app.scope = Scope::Lock;
        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_with_device(frame, &app, "null"))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "L" && cell.fg == Color::LightMagenta)
        );
    }

    #[test]
    fn global_cards_show_all_local_shortcuts() {
        let app = App::new(Project::new(), None);
        let screen = rendered(&app, 120, 34);
        for key in ["[t]", "[y]", "[f]", "[r]", "[k]", "[s]"] {
            assert!(screen.contains(key), "missing {key}");
        }
    }

    #[test]
    fn step_cells_show_note_octaves_and_locks() {
        assert_eq!(step_cell(None), " . ");
        assert_eq!(
            step_cell(Some(&StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                locks: Default::default(),
            })),
            "1:3"
        );
        assert_eq!(
            step_cell(Some(&StepEvent::Note {
                degree: 2,
                octave: 4,
                accent: false,
                chord_shape: None,
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
        assert_eq!(
            mode_name(&Mode::LfoEdit {
                parameter: ParameterId::Cutoff,
                field: LfoField::Depth,
            }),
            "Track LFO edit (cutoff)"
        );
        assert_eq!(
            mode_name(&Mode::TrackLengthInput(String::new())),
            "Track length input"
        );
    }

    #[test]
    fn global_shortcuts_select_all_eight_controls() {
        assert_eq!(global_shortcut('t'), Some(GlobalParameterId::Tempo));
        assert_eq!(global_shortcut('y'), Some(GlobalParameterId::DelayDivision));
        assert_eq!(global_shortcut('f'), Some(GlobalParameterId::DelayFeedback));
        assert_eq!(global_shortcut('r'), Some(GlobalParameterId::ReverbTime));
        assert_eq!(global_shortcut('b'), Some(GlobalParameterId::ReverbTone));
        assert_eq!(
            global_shortcut('p'),
            Some(GlobalParameterId::ReverbPreDelay)
        );
        assert_eq!(global_shortcut('k'), Some(GlobalParameterId::Key));
        assert_eq!(global_shortcut('s'), Some(GlobalParameterId::Scale));
        assert_eq!(global_shortcut('v'), None);
    }

    #[test]
    fn vertical_navigation_follows_physical_rows_without_track_cursors() {
        let mut project = Project::new();
        project.tracks[0].steps.resize(64, None);
        project.tracks[1].steps.resize(20, None);
        project.tracks[2].steps.resize(40, None);
        let mut app = App::new(project, None);
        app.row = 1;
        app.step = 5;
        app.scope = Scope::Lock;
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step, app.scope), (1, 37, Scope::Lock));
        move_step_vertical(&mut app, false);
        assert_eq!((app.row, app.step, app.scope), (1, 5, Scope::Lock));

        move_step_vertical(&mut app, true);
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step, app.scope), (2, 5, Scope::Lock));
        app.step = 19;
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step), (3, 19));
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step), (3, 39));
        move_step_vertical(&mut app, false);
        assert_eq!((app.row, app.step), (3, 7));

        app.row = 2;
        app.step = 5;
        move_step_vertical(&mut app, false);
        assert_eq!((app.row, app.step), (1, 37));
        app.row = 1;
        app.step = 0;
        move_step_vertical(&mut app, false);
        assert_eq!(app.row, 0);
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step), (1, 0));

        app.row = TRACK_COUNT;
        app.step = app.editor.project.tracks[TRACK_COUNT - 1].steps.len() - 1;
        move_step_vertical(&mut app, true);
        assert_eq!((app.row, app.step), (TRACK_COUNT, 15));
    }

    #[test]
    fn bank_navigation_handles_partial_banks() {
        let mut project = Project::new();
        project.tracks[0].steps.resize(20, None);
        let mut app = App::new(project, None);
        app.row = 1;
        app.step = 9;
        move_step_bank(&mut app, true);
        assert_eq!(app.step, 19);
        move_step_bank(&mut app, true);
        assert_eq!(app.step, 3);
    }

    #[test]
    fn sixty_four_step_track_renders_as_two_compact_rows_with_scroll_hint() {
        let mut project = Project::new();
        for track in &mut project.tracks {
            track.steps.resize(64, None);
        }
        let mut app = App::new(project, None);
        app.row = 1;
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Kick"));
        assert!(!screen.contains("LenRange"));
        assert!(screen.contains("01–32"));
        assert!(screen.contains("33–64"));
        assert!(screen.contains("more tracks"));
        assert!(screen.contains("Shift+D"));
    }
}
