use super::{
    render::GLOBAL_IDS,
    state::{App, FileAction, Mode, ParameterBank, SidechainField},
};
use crate::{
    audio::{Audio, AudioCommand, ParameterSmoothing},
    model::{DelayDivision, GlobalParameterId, Percent, Project, Scale, TRACK_COUNT},
    persistence,
    reducer::Scope,
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{path::PathBuf, sync::atomic::Ordering};

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
    let project = a.editor.synchronized_project();
    if audio
        .send(Audio::snapshot_with_smoothing_and_map(
            &project,
            smoothing,
            pattern_map,
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
    let changed = a.editor.edit(None, |p| {
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
        let project = a.editor.synchronized_project();
        match persistence::save_atomic(&path, &project) {
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

pub(super) fn resolved_path(input: &str) -> Result<PathBuf> {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
pub(super) fn handle_file_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::FileInput(action, mut input) = a.mode.clone() else {
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
                        let project = a.editor.synchronized_project();
                        match persistence::save_atomic(&path, &project) {
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
                    FileAction::Open => {
                        if a.editor.is_dirty() {
                            a.mode = Mode::OpenConfirm(path)
                        } else {
                            open_project(a, audio, path)
                        }
                    }
                },
                Err(e) => enter_error(a, e.to_string()),
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn handle_open_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
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

pub(super) fn handle_new_confirm(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Char('s' | 'S') => {
            if a.path.is_none() {
                a.pending_new = true;
                a.mode = Mode::FileInput(FileAction::SaveAs, String::new())
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
    [
        GlobalParameterId::Tempo,
        GlobalParameterId::DelayDivision,
        GlobalParameterId::DelayFeedback,
        GlobalParameterId::ReverbTime,
        GlobalParameterId::ReverbTone,
        GlobalParameterId::ReverbPreDelay,
        GlobalParameterId::Ducking,
        GlobalParameterId::Key,
        GlobalParameterId::Scale,
    ][index % GLOBAL_IDS.len()]
}
pub(super) fn global_shortcut(c: char) -> Option<GlobalParameterId> {
    match c {
        't' => Some(GlobalParameterId::Tempo),
        'y' => Some(GlobalParameterId::DelayDivision),
        'f' => Some(GlobalParameterId::DelayFeedback),
        'r' => Some(GlobalParameterId::ReverbTime),
        'b' => Some(GlobalParameterId::ReverbTone),
        'p' => Some(GlobalParameterId::ReverbPreDelay),
        'd' => Some(GlobalParameterId::Ducking),
        'k' => Some(GlobalParameterId::Key),
        's' => Some(GlobalParameterId::Scale),
        _ => None,
    }
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
        (a.global + 1) % GLOBAL_IDS.len()
    } else {
        (a.global + GLOBAL_IDS.len() - 1) % GLOBAL_IDS.len()
    };
    a.editor.end_coalescing();
    a.global = next;
    enter_global_edit(a, global_id(next));
}

pub(super) fn global_name(id: GlobalParameterId) -> &'static str {
    match id {
        GlobalParameterId::Tempo => "tempo",
        GlobalParameterId::DelayDivision => "delay division",
        GlobalParameterId::DelayFeedback => "delay feedback",
        GlobalParameterId::ReverbTime => "reverb time",
        GlobalParameterId::ReverbTone => "reverb tone",
        GlobalParameterId::ReverbPreDelay => "reverb pre-delay",
        GlobalParameterId::Ducking => "ducking",
        GlobalParameterId::Key => "key",
        GlobalParameterId::Scale => "scale",
    }
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
                GlobalParameterId::Key => edit_global(a, audio, id, move |g| {
                    g.key = g.key.shifted(selector_direction)
                }),
                GlobalParameterId::Scale => edit_global(a, audio, id, |g| {
                    g.scale = if g.scale == Scale::Major {
                        Scale::NaturalMinor
                    } else {
                        Scale::Major
                    }
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
pub(super) fn refresh_audio_status(a: &mut App, audio: &Audio) {
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
    a.callback_overruns = audio.status.callback_overruns.load(Ordering::Relaxed);
    a.max_callback_duration_ns = audio
        .status
        .max_callback_duration_ns
        .load(Ordering::Relaxed);
    a.max_callback_load_per_mille = audio
        .status
        .max_callback_load_per_mille
        .load(Ordering::Relaxed);
    if audio.status.failed.load(Ordering::Acquire) {
        a.status = "Audio stream failed; editing and saving remain available".into()
    } else if audio.status.non_finite.swap(false, Ordering::AcqRel) {
        a.status = "Audio DSP produced a non-finite value; output was silenced".into()
    }
}
