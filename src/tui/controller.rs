use super::{
    controls::{GLOBAL_CONTROLS, global_control},
    state::{App, FileAction, Mode, ParameterBank, PatternPage, SidechainField},
};
use crate::{
    audio::{Audio, AudioCommand, ParameterSmoothing},
    model::{DelayDivision, GlobalParameterId, Percent, Project, TRACK_COUNT, TrackKind},
    persistence,
    reducer::Scope,
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub(super) const PROJECT_DIRECTORY_NAME: &str = ".projects";
pub(super) const PROJECT_EXTENSION: &str = ".groove.json";
pub(super) const PRESET_DIRECTORY_NAME: &str = ".presets";
pub(super) const PRESET_EXTENSION: &str = ".preset.json";

fn is_atomic_save_temporary(name: &str) -> bool {
    let Some(name) = name.strip_prefix('.') else {
        return false;
    };
    let Some(name) = name.strip_suffix(".tmp") else {
        return false;
    };
    let Some((project_name, pid)) = name.rsplit_once('.') else {
        return false;
    };
    project_name.ends_with(PROJECT_EXTENSION)
        && !pid.is_empty()
        && pid.chars().all(|character| character.is_ascii_digit())
}

pub(super) fn project_directory() -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(PROJECT_DIRECTORY_NAME))
}

pub(super) fn preset_directory(kind: TrackKind) -> Result<PathBuf> {
    Ok(std::env::current_dir()?
        .join(PRESET_DIRECTORY_NAME)
        .join(preset_kind_name(kind)))
}

fn preset_kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Kick => "kick",
        TrackKind::Snare => "snare",
        TrackKind::Hat => "hat",
        TrackKind::Tom => "tom",
        TrackKind::Cymbal => "cymbal",
        TrackKind::Rimshot => "rimshot",
        TrackKind::Bass => "bass",
        TrackKind::Chord => "chord",
        TrackKind::Lead => "lead",
    }
}

pub(super) fn save_as_mode() -> Mode {
    Mode::FileInput(FileAction::SaveAs, String::new())
}

pub(super) fn list_projects(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut projects = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let is_temporary = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_atomic_save_temporary);
        if !is_temporary {
            projects.push(path);
        }
    }
    projects.sort_by(|left, right| {
        left.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .cmp(&right.file_name().unwrap_or_default().to_string_lossy())
    });
    Ok(projects)
}

pub(super) fn project_browser_mode() -> Result<Mode> {
    let entries = list_projects(&project_directory()?)?;
    Ok(Mode::ProjectBrowser {
        entries,
        selected: 0,
    })
}

pub(super) fn list_presets(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut entries = list_projects(directory)?;
    entries.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(PRESET_EXTENSION))
    });
    Ok(entries)
}

pub(super) fn preset_browser_mode(track: usize, kind: TrackKind) -> Result<Mode> {
    let entries = list_presets(&preset_directory(kind)?)?;
    Ok(Mode::PresetBrowser {
        track,
        entries,
        selected: 0,
    })
}

pub(super) fn project_path_for_name(directory: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        anyhow::bail!("Project name cannot be empty")
    }
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        anyhow::bail!("Project name must be a single file name")
    }
    let mut base_name = name;
    while let Some(stripped) = base_name.strip_suffix(PROJECT_EXTENSION) {
        base_name = stripped;
    }
    if base_name.is_empty() {
        anyhow::bail!("Project name cannot be empty")
    }
    Ok(directory.join(format!("{base_name}{PROJECT_EXTENSION}")))
}

pub(super) fn save_path_for_name(name: &str) -> Result<PathBuf> {
    project_path_for_name(&project_directory()?, name)
}

pub(super) fn preset_path_for_name(track: TrackKind, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        anyhow::bail!("Preset name cannot be empty")
    }
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        anyhow::bail!("Preset name must be a single file name")
    }
    let mut base_name = name;
    while let Some(stripped) = base_name.strip_suffix(PRESET_EXTENSION) {
        base_name = stripped;
    }
    if base_name.is_empty() {
        anyhow::bail!("Preset name cannot be empty")
    }
    Ok(preset_directory(track)?.join(format!("{base_name}{PRESET_EXTENSION}")))
}

pub(super) fn save_as_needs_overwrite_confirmation(destination: &Path) -> io::Result<bool> {
    destination.try_exists()
}

pub(super) fn sync_project(a: &mut App, audio: &mut Audio) -> bool {
    sync_project_with_smoothing(a, audio, false)
}

pub(super) fn sync_project_with_smoothing(
    a: &mut App,
    audio: &mut Audio,
    direct_entry: bool,
) -> bool {
    let smoothing = if direct_entry {
        ParameterSmoothing::Fader
    } else {
        ParameterSmoothing::Default
    };
    let pattern_map = a.editor.take_pattern_map();
    let song_map = a.editor.take_song_map();
    if audio
        .send(Audio::snapshot_with_smoothing_and_maps(
            a.editor.project(),
            smoothing,
            pattern_map,
            song_map,
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
pub(super) fn change_octave(a: &mut App, audio: &mut Audio, d: i8) {
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
    let changed = a.editor.edit(None, |p, _| {
        p.tracks[ti].input_octave = Some(new);
        Ok(())
    });
    if matches!(changed, Ok(true)) && sync_project(a, audio) {
        a.status = format!("{track_name} input octave: {new}");
    }
}

pub(super) fn adjusted_octave(octave: u8, delta: i8) -> u8 {
    (octave as i8 + delta).clamp(0, 7) as u8
}

pub(super) fn enter_error(a: &mut App, message: impl Into<String>) {
    a.pending_open = None;
    a.pending_new = false;
    a.pending_quit = false;
    a.mode = Mode::Error(message.into());
}

pub(super) fn save(a: &mut App) -> Result<()> {
    if let Some(path) = a.path.clone() {
        match persistence::save_atomic(&path, a.editor.project()) {
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

fn complete_save_as(a: &mut App, audio: &mut Audio, path: PathBuf) {
    match persistence::save_atomic(&path, a.editor.project()) {
        Ok(()) => {
            a.path = Some(path.clone());
            a.editor.mark_saved();
            a.status = format!("Saved {}", path.display());
            a.mode = Mode::Navigation;
            if let Some(open) = a.pending_open.take() {
                open_project(a, audio, open)
            } else if a.pending_new {
                a.pending_new = false;
                new_project(a, audio)
            } else if a.pending_quit {
                a.quit = true
            }
        }
        Err(e) => enter_error(a, e.to_string()),
    }
}

pub(super) fn handle_overwrite_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::OverwriteConfirm { path, input } = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Enter | KeyCode::Char('o' | 'O') => complete_save_as(a, audio, path),
        KeyCode::Esc => a.mode = Mode::FileInput(FileAction::SaveAs, input),
        _ => {}
    }
    Ok(())
}

pub(super) fn handle_file_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::FileInput(FileAction::SaveAs, mut input) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Esc => {
            a.pending_open = None;
            a.pending_new = false;
            a.pending_quit = false;
            a.mode = Mode::Navigation
        }
        KeyCode::Backspace => {
            input.pop();
            a.mode = Mode::FileInput(FileAction::SaveAs, input)
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            input.push(c);
            a.mode = Mode::FileInput(FileAction::SaveAs, input)
        }
        KeyCode::Enter => {
            let path = match save_path_for_name(&input) {
                Ok(path) => path,
                Err(error) => {
                    a.status = error.to_string();
                    return Ok(());
                }
            };
            match save_as_needs_overwrite_confirmation(&path) {
                Ok(true) => a.mode = Mode::OverwriteConfirm { path, input },
                Ok(false) => complete_save_as(a, audio, path),
                Err(error) => enter_error(
                    a,
                    format!("Could not check whether {} exists: {error}", path.display()),
                ),
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn handle_project_browser(a: &mut App, audio: &mut Audio, k: KeyEvent) {
    let Mode::ProjectBrowser { entries, selected } = a.mode.clone() else {
        return;
    };
    match k.code {
        KeyCode::Esc => a.mode = Mode::Navigation,
        KeyCode::Up => {
            a.mode = Mode::ProjectBrowser {
                entries,
                selected: selected.saturating_sub(1),
            }
        }
        KeyCode::Down => {
            a.mode = Mode::ProjectBrowser {
                selected: selected
                    .saturating_add(1)
                    .min(entries.len().saturating_sub(1)),
                entries,
            }
        }
        KeyCode::Home => {
            a.mode = Mode::ProjectBrowser {
                entries,
                selected: 0,
            }
        }
        KeyCode::End => {
            a.mode = Mode::ProjectBrowser {
                selected: entries.len().saturating_sub(1),
                entries,
            }
        }
        KeyCode::Enter => {
            if let Some(path) = entries.get(selected).cloned() {
                if a.editor.is_dirty() {
                    a.mode = Mode::OpenConfirm(path)
                } else {
                    open_project(a, audio, path)
                }
            } else {
                a.status = "No projects found in .projects".into();
            }
        }
        _ => a.mode = Mode::ProjectBrowser { entries, selected },
    }
}

pub(super) fn handle_preset_browser(a: &mut App, audio: &mut Audio, k: KeyEvent) {
    let Mode::PresetBrowser {
        track,
        entries,
        selected,
    } = a.mode.clone()
    else {
        return;
    };
    match k.code {
        KeyCode::Esc => a.mode = Mode::Navigation,
        KeyCode::Up => {
            a.mode = Mode::PresetBrowser {
                track,
                entries,
                selected: selected.saturating_sub(1),
            }
        }
        KeyCode::Down => {
            a.mode = Mode::PresetBrowser {
                track,
                selected: selected
                    .saturating_add(1)
                    .min(entries.len().saturating_sub(1)),
                entries,
            }
        }
        KeyCode::Home => {
            a.mode = Mode::PresetBrowser {
                track,
                entries,
                selected: 0,
            }
        }
        KeyCode::End => {
            a.mode = Mode::PresetBrowser {
                track,
                selected: entries.len().saturating_sub(1),
                entries,
            }
        }
        KeyCode::Enter => {
            if let Some(path) = entries.get(selected) {
                load_preset(a, audio, track, path);
            } else {
                a.status = format!("No {} presets found", a.editor.project.tracks[track].name);
            }
        }
        _ => {
            a.mode = Mode::PresetBrowser {
                track,
                entries,
                selected,
            }
        }
    }
}

pub(super) fn handle_preset_name_input(a: &mut App, k: KeyEvent) {
    let Mode::PresetNameInput { track, mut input } = a.mode.clone() else {
        return;
    };
    match k.code {
        KeyCode::Esc => a.mode = Mode::Navigation,
        KeyCode::Backspace => {
            input.pop();
            a.mode = Mode::PresetNameInput { track, input };
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            input.push(c);
            a.mode = Mode::PresetNameInput { track, input };
        }
        KeyCode::Enter => {
            let kind = a.editor.project.tracks[track].kind;
            match preset_path_for_name(kind, &input) {
                Ok(path) => match save_as_needs_overwrite_confirmation(&path) {
                    Ok(true) => a.mode = Mode::PresetOverwriteConfirm { path, track, input },
                    Ok(false) => save_preset(a, track, path),
                    Err(error) => enter_error(
                        a,
                        format!("Could not check whether {} exists: {error}", path.display()),
                    ),
                },
                Err(error) => a.status = error.to_string(),
            }
        }
        _ => {}
    }
}

pub(super) fn handle_preset_overwrite_confirm(a: &mut App, k: KeyEvent) {
    let Mode::PresetOverwriteConfirm { track, path, input } = a.mode.clone() else {
        return;
    };
    match k.code {
        KeyCode::Enter | KeyCode::Char('o' | 'O') => save_preset(a, track, path),
        KeyCode::Esc => a.mode = Mode::PresetNameInput { track, input },
        _ => {}
    }
}

fn save_preset(a: &mut App, track: usize, path: PathBuf) {
    let preset = persistence::TrackPreset::from_track(a.editor.project.tracks[track].clone());
    match persistence::save_track_preset_atomic(&path, &preset) {
        Ok(()) => {
            a.status = format!(
                "Saved {} preset {}",
                a.editor.project.tracks[track].name,
                path.display()
            );
            a.mode = Mode::Navigation;
        }
        Err(error) => enter_error(a, error.to_string()),
    }
}

fn load_preset(a: &mut App, audio: &mut Audio, track: usize, path: &Path) {
    let preset = match persistence::load_track_preset(path) {
        Ok(preset) => preset,
        Err(error) => {
            enter_error(a, error.to_string());
            return;
        }
    };
    if preset.track.kind != a.editor.project.tracks[track].kind {
        enter_error(
            a,
            format!(
                "Preset {} is incompatible with {}",
                path.display(),
                a.editor.project.tracks[track].name
            ),
        );
        return;
    }
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; preset load rejected".into();
        return;
    }
    match a.editor.edit(None, |project, _| {
        project.tracks[track] = preset.track;
        Ok(())
    }) {
        Ok(true) if sync_project(a, audio) => {
            a.scope = Scope::Base;
            a.mode = Mode::Navigation;
            a.status = format!("Loaded preset {}", path.display());
        }
        Ok(false) => {
            a.scope = Scope::Base;
            a.mode = Mode::Navigation;
            a.status = "Preset already matches this track".into();
        }
        Ok(true) => {}
        Err(error) => enter_error(a, error.to_string()),
    }
}
pub(super) fn handle_open_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::OpenConfirm(path) = a.mode.clone() else {
        return Ok(());
    };
    match k.code {
        KeyCode::Char('s' | 'S') => {
            if a.path.is_none() {
                a.pending_open = Some(path);
                a.mode = save_as_mode()
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

pub(super) fn handle_new_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Char('s' | 'S') => {
            if a.path.is_none() {
                a.pending_new = true;
                a.mode = save_as_mode()
            } else {
                save(a)?;
                if !a.editor.is_dirty() {
                    new_project(a, audio)
                }
            }
        }
        KeyCode::Char('d' | 'D') => new_project(a, audio),
        KeyCode::Esc | KeyCode::Char('c' | 'C') => a.mode = Mode::Navigation,
        _ => {}
    }
    Ok(())
}

pub(super) fn request_new_project(a: &mut App) -> bool {
    if a.editor.is_dirty() {
        a.mode = Mode::NewConfirm;
        false
    } else {
        true
    }
}

pub(super) fn reset_project_ui(a: &mut App) {
    a.row = 0;
    a.global = 0;
    a.step = 0;
    a.scope = Scope::Base;
    a.parameter_bank = ParameterBank::Params;
    a.playing = false;
    a.paused = false;
    a.pattern_cursor = 0;
    a.active_pattern = 0;
    a.queued_pattern = None;
    a.pattern_page = PatternPage::Patterns;
    a.song_cursor = 0;
    a.song_mode = false;
    a.active_song = 0;
    a.queued_song = None;
    a.song_bar = 0;
    a.playheads = [None; TRACK_COUNT];
    a.pending_open = None;
    a.pending_new = false;
    a.pending_quit = false;
    a.fader_animations.clear();
    a.mode = Mode::Navigation;
}

pub(super) fn new_project(a: &mut App, audio: &mut Audio) {
    if audio.available_commands() < 2 {
        enter_error(a, "Audio command queue full; new project was not created");
        return;
    }
    let project = Project::new();
    let _ = audio.send(AudioCommand::Stop);
    if audio.send(Audio::snapshot(&project)).is_err() {
        enter_error(a, "Audio command queue full; new project was not created");
        return;
    }
    a.editor.replace_loaded(project);
    a.path = None;
    reset_project_ui(a);
    a.status = "New project".into();
}

pub(super) fn open_project(a: &mut App, audio: &mut Audio, path: PathBuf) {
    match persistence::load(&path) {
        Ok(project) => {
            if audio.available_commands() < 2 {
                enter_error(a, "Audio command queue full; project was not opened");
                return;
            }
            let _ = audio.send(AudioCommand::Stop);
            if audio.send(Audio::snapshot(&project)).is_err() {
                enter_error(a, "Audio command queue full; project was not opened");
                return;
            }
            a.editor.replace_loaded(project);
            a.path = Some(path.clone());
            reset_project_ui(a);
            a.status = format!("Opened {}", path.display());
        }
        Err(e) => enter_error(a, e.to_string()),
    }
}

pub(super) fn global_id(index: usize) -> GlobalParameterId {
    GLOBAL_CONTROLS[index % GLOBAL_CONTROLS.len()].id
}
pub(super) fn global_shortcut(c: char) -> Option<GlobalParameterId> {
    GLOBAL_CONTROLS
        .iter()
        .find(|control| control.shortcut.starts_with(c))
        .map(|control| control.id)
}
pub(super) fn enter_global_edit(a: &mut App, id: GlobalParameterId) {
    a.mode = if id == GlobalParameterId::Tempo {
        Mode::TempoInput(String::new())
    } else if id == GlobalParameterId::Ducking {
        Mode::SidechainEdit {
            field: SidechainField::Depth,
        }
    } else {
        Mode::GlobalEdit(id)
    };
    a.status = format!("Editing {}", global_name(id));
}

pub(super) fn move_global_editor(a: &mut App, forward: bool) {
    let next = if forward {
        (a.global + 1) % GLOBAL_CONTROLS.len()
    } else {
        (a.global + GLOBAL_CONTROLS.len() - 1) % GLOBAL_CONTROLS.len()
    };
    a.editor.end_coalescing();
    a.global = next;
    enter_global_edit(a, global_id(next));
}

pub(super) fn global_name(id: GlobalParameterId) -> &'static str {
    global_control(id).name
}
pub(super) fn edit_global<F: FnOnce(&mut crate::model::Globals)>(
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
            |p, _| {
                f(&mut p.globals);
                Ok(())
            },
        )
        .unwrap_or(false);
    if changed && sync_project(a, audio) {
        a.status = format!("{} updated", global_name(id))
    }
}
pub(super) fn handle_global_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::GlobalEdit(id) = a.mode else {
        return Ok(false);
    };
    match k.code {
        KeyCode::Esc | KeyCode::Enter => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation
        }
        KeyCode::Left => move_global_editor(a, false),
        KeyCode::Right => move_global_editor(a, true),
        KeyCode::Char(c) => {
            if let Some(next) = global_shortcut(c) {
                a.global = next as usize;
                enter_global_edit(a, next)
            } else if matches!(
                id,
                GlobalParameterId::DelayFeedback
                    | GlobalParameterId::ReverbTone
                    | GlobalParameterId::ReverbReturn
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
                    } else if id == GlobalParameterId::ReverbTone {
                        edit_global(a, audio, id, move |g| g.reverb_tone = v);
                    } else {
                        edit_global(a, audio, id, move |g| g.reverb_return = v);
                    }
                    a.mode = Mode::Navigation
                }
            }
        }
        KeyCode::Up | KeyCode::Down => {
            let direction = if k.code == KeyCode::Up { 1 } else { -1 };
            // Selectors are rendered top-to-bottom, so Up selects the previous
            // value while faders continue to increase on Up.
            let selector_direction = -direction;
            match id {
                GlobalParameterId::DelayDivision => edit_global(a, audio, id, move |g| {
                    let i = DelayDivision::ALL
                        .iter()
                        .position(|x| *x == g.delay_division)
                        .unwrap() as i32;
                    g.delay_division =
                        DelayDivision::ALL[(i + selector_direction).clamp(0, 10) as usize]
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
                GlobalParameterId::ReverbReturn => {
                    let d: i16 = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        10
                    } else {
                        1
                    };
                    edit_global(a, audio, id, move |g| {
                        g.reverb_return = g.reverb_return.saturating_add(direction as i16 * d)
                    })
                }
                GlobalParameterId::Key => edit_global(a, audio, id, move |g| {
                    g.key = g.key.shifted(selector_direction)
                }),
                GlobalParameterId::Scale => edit_global(a, audio, id, move |g| {
                    g.scale = g.scale.shifted(selector_direction)
                }),
                GlobalParameterId::Tempo => {}
                GlobalParameterId::Ducking => {}
            }
        }
        _ => {}
    }
    Ok(true)
}

pub(super) fn handle_sidechain_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::SidechainEdit { field } = a.mode else {
        return Ok(false);
    };
    match k.code {
        KeyCode::Esc | KeyCode::Enter => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation;
        }
        KeyCode::Left | KeyCode::Right => {
            let direction = if k.code == KeyCode::Right { 1 } else { -1 };
            let index = (field as i16 + direction).clamp(0, 2) as usize;
            a.editor.end_coalescing();
            a.mode = Mode::SidechainEdit {
                field: SidechainField::ALL[index],
            };
        }
        KeyCode::Char(c) if field == SidechainField::Depth => {
            if let Some(value) = crate::reducer::percentage_key(c) {
                edit_global(a, audio, GlobalParameterId::Ducking, move |g| {
                    g.sidechain.depth = value;
                });
            }
        }
        KeyCode::Up | KeyCode::Down => {
            let direction = if k.code == KeyCode::Up { 1 } else { -1 };
            let amount = if k.modifiers.contains(KeyModifiers::SHIFT) {
                10
            } else {
                1
            };
            edit_global(a, audio, GlobalParameterId::Ducking, move |g| {
                let value = match field {
                    SidechainField::Depth => &mut g.sidechain.depth,
                    SidechainField::Attack => &mut g.sidechain.attack,
                    SidechainField::Release => &mut g.sidechain.release,
                };
                *value = value.saturating_add(direction * amount);
            });
        }
        _ => {}
    }
    Ok(true)
}
pub(super) fn handle_tempo_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
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
        KeyCode::Left => move_global_editor(a, false),
        KeyCode::Right => move_global_editor(a, true),
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
pub(super) fn refresh_audio_status(a: &mut App, audio: &mut Audio) {
    let recording_event = audio.poll_recording_event();
    a.recording_state = audio.recording_state();
    let recording_status = recording_event.map(|event| {
        a.recording_state = crate::audio::RecordingState::Idle;
        match event.result {
            Ok(()) if audio.status.failed.load(Ordering::Acquire) => format!(
                "Audio stream failed; retained {} stereo frames at {}; details logged to {}",
                event.frames,
                event.path.display(),
                audio.audio_log_path().display()
            ),
            Ok(()) => format!(
                "Recorded {} stereo frames to {}",
                event.frames,
                event.path.display()
            ),
            Err(error) if audio.status.failed.load(Ordering::Acquire) => format!(
                "Recording stopped: {error}; partial take retained at {}; audio details logged to {}",
                event.path.display(),
                audio.audio_log_path().display()
            ),
            Err(error) => format!(
                "Recording stopped: {error}; partial take retained at {}",
                event.path.display()
            ),
        }
    });
    a.playing = audio.status.running.load(Ordering::Acquire);
    a.paused = audio.status.paused.load(Ordering::Acquire);
    a.active_pattern = usize::from(audio.status.active_pattern.load(Ordering::Acquire))
        .min(a.editor.project.patterns.len() - 1);
    if a.editor.pattern() != a.active_pattern {
        a.editor.select_pattern(a.active_pattern);
        if a.row > 0 {
            let length = a
                .editor
                .active_steps(a.row - 1)
                .map_or(1, |steps| steps.len());
            a.step = a.step.min(length.saturating_sub(1));
        }
    }
    let queued = audio.status.queued_pattern.load(Ordering::Acquire);
    a.queued_pattern =
        (queued != u8::MAX).then_some(usize::from(queued).min(a.editor.project.patterns.len() - 1));
    a.song_mode = audio.status.song_mode.load(Ordering::Acquire);
    a.active_song = usize::from(audio.status.active_song.load(Ordering::Acquire))
        .min(a.editor.project.song.len().saturating_sub(1));
    let queued_song = audio.status.queued_song.load(Ordering::Acquire);
    a.queued_song = (queued_song != u16::MAX)
        .then_some(usize::from(queued_song).min(a.editor.project.song.len().saturating_sub(1)));
    a.song_bar = audio.status.song_bar.load(Ordering::Acquire);
    for track in 0..TRACK_COUNT {
        let step = audio.status.playheads[track].load(Ordering::Acquire);
        a.playheads[track] =
            (step < a.editor.active_steps(track).unwrap().len() as u8).then_some(step as usize);
    }
    a.callback_overruns = audio.status.callback_overruns.load(Ordering::Relaxed);
    a.max_callback_load_per_mille = audio
        .status
        .max_callback_load_per_mille
        .load(Ordering::Relaxed);
    let diagnostics = audio.log_pending_diagnostics();
    if audio.status.failed.load(Ordering::Acquire) {
        a.status = match diagnostics {
            Ok(_) => format!(
                "Audio stream failed; details logged to {}",
                audio.audio_log_path().display()
            ),
            Err(error) => format!(
                "Audio stream failed; could not write {}: {error}",
                audio.audio_log_path().display()
            ),
        };
    } else if let Ok(diagnostics) = diagnostics {
        if diagnostics.non_finite {
            a.status = format!(
                "Audio DSP produced a non-finite value; details logged to {}",
                audio.audio_log_path().display()
            )
        }
    }
    if let Some(recording_status) = recording_status {
        a.status = recording_status;
    }
}
