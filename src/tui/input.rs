use super::{
    controller::{
        change_octave, enter_global_edit, global_id, global_shortcut, handle_file_input,
        handle_global_key, handle_new_confirm, handle_open_confirm, handle_tempo_input,
        new_project, request_new_project, save, sync_project, sync_project_with_smoothing,
    },
    render::{
        GLOBAL_IDS, parameter_descriptors, scope_name, selected_chord_shape,
        visible_parameter_descriptors,
    },
    state::{
        App, ChordField, FileAction, GeneratorDialog, LfoField, Mode, ParameterBank, TriggerField,
    },
};
use crate::{
    audio::{Audio, AudioCommand},
    generator::{Config as GeneratorConfig, Target as GeneratorTarget},
    model::{
        ArpeggioRate, ArpeggioType, ChordShape, ChorusMode, LfoConfig, LfoDivision, LfoRate,
        LfoWaveform, MAX_STEP_COUNT, ParameterId, ParameterValue, Percent, STEP_BANK_SIZE,
        STEP_ROW_SIZE, StepEvent, TRACK_COUNT, TrackKind, TriggerCondition, Waveform,
    },
    reducer::{Editor, Scope},
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
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
    if a.mode == Mode::NewConfirm {
        return handle_new_confirm(a, audio, k);
    }
    if a.mode == Mode::PatternDialog {
        return handle_pattern_dialog(a, audio, k);
    }
    if matches!(a.mode, Mode::GeneratorDialog(_)) {
        return handle_generator_dialog(a, audio, k);
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
            KeyCode::Char('n' | 'N') => {
                if request_new_project(a) {
                    new_project(a, audio)
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
        && a.row > 0
        && matches!(k.code, KeyCode::Tab | KeyCode::BackTab)
    {
        toggle_parameter_bank(a);
        return Ok(());
    }
    if matches!(a.mode, Mode::Navigation | Mode::ParameterEdit(_)) && global_jump(k) {
        select_global(a);
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
    if matches!(a.mode, Mode::TriggerEdit { .. }) && handle_trigger_key(a, audio, k)? {
        return Ok(());
    }
    if a.mode == Mode::SwingEdit && handle_swing_key(a, audio, k)? {
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
        KeyCode::Char('g') => {
            let track = a.row.saturating_sub(1).min(TRACK_COUNT - 1);
            let defaults = GeneratorConfig::default();
            a.mode = Mode::GeneratorDialog(GeneratorDialog {
                target: if a.row == 0 {
                    GeneratorTarget::WholePattern
                } else {
                    GeneratorTarget::Track(track)
                },
                track,
                seed: defaults.seed.to_string(),
                density: defaults.density,
                range_low: defaults.range_low,
                range_high: defaults.range_high,
                ties: defaults.ties,
                accents: defaults.accents,
                field: 0,
            });
            a.status = "Generator ready".into();
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
        KeyCode::Char('m') if a.row > 0 && a.parameter_bank == ParameterBank::Params => {
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
        KeyCode::Delete if is_clear_track_shortcut(&a.mode, a.row, k) => {
            clear_selected_track(a, audio)
        }
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
        KeyCode::Char('T') if a.row > 0 => {
            let track = a.row - 1;
            match a.editor.trigger_condition_value(track, a.step) {
                Ok(_) => {
                    a.mode = Mode::TriggerEdit {
                        field: TriggerField::Mode,
                    };
                    a.status = "Editing trigger condition and retrigger count".into();
                }
                Err(error) => a.status = error.to_string(),
            }
        }
        KeyCode::Char('S') if a.row > 0 => {
            a.mode = Mode::SwingEdit;
            a.status = format!("{} swing", a.editor.project.tracks[a.row - 1].name);
        }
        KeyCode::Char('C') if a.row == 5 => open_chord_editor(a),
        KeyCode::Char(c) if a.row == 0 => {
            if let Some(id) = global_shortcut(c) {
                a.global = id as usize;
                enter_global_edit(a, id)
            }
        }
        KeyCode::Char('[') if a.row > 3 => change_octave(a, audio, -1),
        KeyCode::Char(']') if a.row > 3 => change_octave(a, audio, 1),
        KeyCode::Char(c) if a.row > 0 && active_parameter_shortcut(a, c).is_some() => {
            if let Some(parameter) = active_parameter_shortcut(a, c) {
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

pub(super) fn move_step(a: &mut App, forward: bool) {
    let track = a.row - 1;
    let length = a.editor.project.tracks[track].steps.len();
    a.step = if forward {
        (a.step + 1) % length
    } else {
        (a.step + length - 1) % length
    };
}

pub(super) fn track_jump_index(k: KeyEvent) -> Option<usize> {
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

pub(super) fn global_jump(k: KeyEvent) -> bool {
    matches!(k.code, KeyCode::Char('~'))
}

pub(super) fn select_global(a: &mut App) {
    a.editor.end_coalescing();
    a.row = 0;
    a.scope = Scope::Base;
    a.parameter_bank = ParameterBank::Params;
    a.mode = Mode::Navigation;
}

pub(super) fn select_track(a: &mut App, track: usize) {
    a.editor.end_coalescing();
    a.row = track + 1;
    a.step = a.step.min(a.editor.project.tracks[track].steps.len() - 1);

    match a.mode {
        Mode::ParameterEdit(parameter)
            if !parameter.is_valid_for(a.editor.project.tracks[track].kind) =>
        {
            let replacement = visible_parameter_descriptors(
                a.parameter_bank,
                a.editor.project.tracks[track].kind,
            )[0]
            .id;
            enter_parameter_edit(a, replacement);
        }
        _ => {}
    }
}

pub(super) fn commit_pattern(a: &mut App, audio: &mut Audio, pattern: usize) -> bool {
    let previous = a.editor.pattern();
    if pattern >= a.editor.project.patterns.len() || audio.available_commands() < 2 {
        a.status = "Audio command queue full; pattern switch rejected".into();
        return false;
    }
    if pattern != previous && !a.editor.select_pattern(pattern) {
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

pub(super) fn adjacent_pattern_in_count(pattern: usize, forward: bool, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if forward {
        (pattern + 1) % count
    } else {
        (pattern + count - 1) % count
    }
}

pub(super) fn handle_pattern_dialog(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
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

pub(super) fn handle_generator_dialog(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let Mode::GeneratorDialog(dialog) = &mut a.mode else {
        return Ok(());
    };
    match k.code {
        KeyCode::Esc => a.mode = Mode::Navigation,
        KeyCode::Tab => dialog.field = move_generator_tab(dialog.field, false),
        KeyCode::BackTab => dialog.field = move_generator_tab(dialog.field, true),
        KeyCode::Up | KeyCode::Down => {
            dialog.field = move_generator_field(dialog.field, k.code == KeyCode::Up);
        }
        KeyCode::Left | KeyCode::Right if dialog.field == 0 => {
            change_generator_value(dialog, k.code == KeyCode::Right);
        }
        KeyCode::Left | KeyCode::Right if dialog.field == 1 => {
            change_generator_value(dialog, k.code == KeyCode::Right);
        }
        KeyCode::Char(c) if dialog.field == 2 && c.is_ascii_digit() => {
            if dialog.seed.len() < 20 {
                dialog.seed.push(c);
            }
        }
        KeyCode::Backspace if dialog.field == 2 => {
            dialog.seed.pop();
        }
        KeyCode::Left if dialog.field == 2 => {
            dialog.seed.pop();
        }
        KeyCode::Right if dialog.field == 2 => {}
        KeyCode::Left | KeyCode::Right if (4..=5).contains(&dialog.field) => {
            change_generator_value(dialog, k.code == KeyCode::Right);
        }
        KeyCode::Left | KeyCode::Right if (3..=7).contains(&dialog.field) => {
            change_generator_value(dialog, k.code == KeyCode::Right);
        }
        KeyCode::Enter => {
            let config = generator_config(dialog);
            let seed = config.seed;
            if audio.available_commands() == 0 {
                a.status = "Audio command queue full; generation rejected".into();
            } else {
                match a.editor.generate_pattern(config) {
                    Ok(0) => a.status = format!("Seed {seed}: no empty steps available"),
                    Ok(count) => {
                        sync_project(a, audio);
                        a.status = format!("Generated {count} events · seed {seed}");
                        a.mode = Mode::Navigation;
                    }
                    Err(error) => a.status = error.to_string(),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn move_generator_field(field: usize, up: bool) -> usize {
    if up {
        field.saturating_sub(1)
    } else {
        field.saturating_add(1).min(GENERATOR_FIELD_COUNT - 1)
    }
}

const GENERATOR_FIELD_COUNT: usize = 8;

pub(super) fn move_generator_tab(field: usize, backward: bool) -> usize {
    if backward {
        (field + GENERATOR_FIELD_COUNT - 1) % GENERATOR_FIELD_COUNT
    } else {
        (field + 1) % GENERATOR_FIELD_COUNT
    }
}

pub(super) fn generator_config(dialog: &GeneratorDialog) -> GeneratorConfig {
    GeneratorConfig {
        target: dialog.target,
        seed: dialog
            .seed
            .parse::<u64>()
            .unwrap_or(GeneratorConfig::default().seed),
        density: dialog.density,
        range_low: dialog.range_low,
        range_high: dialog.range_high,
        ties: dialog.ties,
        accents: dialog.accents,
    }
}

pub(super) fn change_generator_value(dialog: &mut GeneratorDialog, right: bool) {
    match dialog.field {
        0 => {
            dialog.target = match dialog.target {
                GeneratorTarget::WholePattern => GeneratorTarget::Track(dialog.track),
                GeneratorTarget::Track(_) => GeneratorTarget::WholePattern,
            };
        }
        1 => {
            let delta = if right { 1 } else { TRACK_COUNT - 1 };
            dialog.track = (dialog.track + delta) % TRACK_COUNT;
            if matches!(dialog.target, GeneratorTarget::Track(_)) {
                dialog.target = GeneratorTarget::Track(dialog.track);
            }
        }
        4 => {
            if right {
                dialog.range_low = dialog.range_low.saturating_add(1).min(dialog.range_high);
            } else {
                dialog.range_low = dialog.range_low.saturating_sub(1);
            }
        }
        5 => {
            if right {
                dialog.range_high = dialog.range_high.saturating_add(1).min(7);
            } else {
                dialog.range_high = dialog.range_high.saturating_sub(1).max(dialog.range_low);
            }
        }
        3 | 6 | 7 => {
            let delta = if right { 5 } else { -5 };
            match dialog.field {
                3 => dialog.density = dialog.density.saturating_add(delta),
                6 => dialog.ties = dialog.ties.saturating_add(delta),
                7 => dialog.accents = dialog.accents.saturating_add(delta),
                _ => unreachable!(),
            }
        }
        _ => {}
    }
}

pub(super) fn pattern_edit_at<F>(a: &mut App, audio: &mut Audio, f: F, message: &str)
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
            normalize_cursor(a);
            a.status = format!("{message}: {}", cursor + 1);
        }
        Ok((true, _)) => {}
        Ok((false, _)) => a.status = "No change".into(),
        Err(e) => a.status = e.to_string(),
    }
}

pub(super) fn move_step_page(a: &mut App, forward: bool) {
    a.editor.end_coalescing();
    move_step(a, forward);
}

pub(super) fn move_step_bank(a: &mut App, forward: bool) {
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

pub(super) fn move_step_vertical(a: &mut App, down: bool) {
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

pub(super) fn normalize_cursor(a: &mut App) {
    a.pattern_cursor = a
        .pattern_cursor
        .min(a.editor.project.patterns.len().saturating_sub(1));
    if a.row > 0 {
        a.step = a
            .step
            .min(a.editor.project.tracks[a.row - 1].steps.len() - 1);
    }
}

pub(super) fn set_selected_track_length(
    a: &mut App,
    audio: &mut Audio,
    length: usize,
    coalesce: bool,
) {
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

pub(super) fn duplicate_selected_track(a: &mut App, audio: &mut Audio) {
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

pub(super) fn is_clear_track_shortcut(mode: &Mode, row: usize, k: KeyEvent) -> bool {
    *mode == Mode::Navigation
        && row > 0
        && k.code == KeyCode::Delete
        && k.modifiers.contains(KeyModifiers::SHIFT)
}

pub(super) fn clear_selected_track(a: &mut App, audio: &mut Audio) {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; track clear rejected".into();
        return;
    }
    let track = a.row - 1;
    let track_name = a.editor.project.tracks[track].name.clone();
    a.editor.end_coalescing();
    match a.editor.clear_track(track) {
        Ok(0) => a.status = format!("{track_name}: no events to clear"),
        Ok(count) if sync_project(a, audio) => {
            a.status = format!("{track_name}: cleared {count} event(s)");
        }
        Ok(_) => {}
        Err(error) => a.status = error.to_string(),
    }
}

pub(super) fn handle_track_length_input(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
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

pub(super) fn handle_parameter_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
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
            let maximum = parameter_upper_bound(parameter);
            let value = ParameterValue::Percent(
                crate::model::Percent::new(
                    (current.get() as i16 + delta).clamp(0, maximum as i16) as u8
                )
                .unwrap(),
            );
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
            if let Some(next) = active_parameter_shortcut(a, c) {
                switch_parameter_editor(a, next);
                return Ok(true);
            }
            if let Some(value) = crate::reducer::percentage_key(c) {
                if parameter_supports_direct_percentage(parameter) {
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::Percent(clamp_parameter_percentage(parameter, value)),
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
        KeyCode::Backspace | KeyCode::Delete if parameter == ParameterId::Pitch => {
            let has_lfo = a.editor.lfo(track, parameter).ok().flatten().is_some();
            if has_lfo {
                if set_lfo_config(a, audio, parameter, None, None) {
                    a.editor.end_coalescing();
                    a.mode = Mode::Navigation;
                    a.status = format!("Removed {} LFO", parameter.display_name());
                }
            } else {
                a.mode = Mode::Navigation;
                a.status = format!("No {} LFO to remove", parameter.display_name());
            }
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

pub(super) fn parameter_supports_direct_percentage(parameter: ParameterId) -> bool {
    !matches!(
        parameter,
        ParameterId::Waveform | ParameterId::Chorus | ParameterId::Spread | ParameterId::Pitch
    )
}

pub(super) fn clamp_parameter_percentage(parameter: ParameterId, value: Percent) -> Percent {
    Percent::new(value.get().min(parameter_upper_bound(parameter))).unwrap()
}

pub(super) const fn parameter_upper_bound(parameter: ParameterId) -> u8 {
    match parameter {
        ParameterId::PhaserFeedback | ParameterId::FlangerFeedback => 90,
        _ => 100,
    }
}

pub(super) fn parameter_edit_passthrough(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::Char(' ')
            | KeyCode::Char('.')
            | KeyCode::Char('o')
            | KeyCode::Char('A')
            | KeyCode::Char('G')
    )
}

pub(super) fn open_lfo_editor(a: &mut App, audio: &mut Audio, parameter: ParameterId) {
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

pub(super) fn open_chord_editor(a: &mut App) {
    if a.row != 5 {
        a.status = "Chord editor is available on the Chord track only".into();
        return;
    }
    let track = &a.editor.project.tracks[4];
    let shape = match track.steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => chord_shape.unwrap_or_default(),
        Some(StepEvent::Tie { .. }) => selected_chord_shape(a, 4).unwrap_or_default(),
        None => track.input_chord_shape.unwrap_or_default(),
        _ => return,
    };
    let shape = a.editor.chord_shape_value(4, a.step).unwrap_or(shape);
    a.chord_field = ChordField::Shape;
    a.mode = Mode::ChordEdit { shape };
    a.status = if matches!(track.steps[a.step], Some(StepEvent::Tie { .. })) {
        "Chord settings are inherited; edit the note trigger".into()
    } else if track.steps[a.step].is_none() {
        "Editing Chord input defaults".into()
    } else {
        "Editing Chord trigger settings".into()
    };
}

pub(super) fn handle_chord_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::ChordEdit { shape } = a.mode else {
        return Ok(false);
    };
    let field = a.chord_field;
    let track = a.row.saturating_sub(1);
    let read_only = matches!(
        a.editor.project.tracks[track].steps[a.step],
        Some(StepEvent::Tie { .. })
    );
    match k.code {
        KeyCode::Char('o') => {
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
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('C') => {
            a.mode = Mode::Navigation;
            a.status = "Chord trigger editing finished".into();
        }
        KeyCode::Char('?') => a.mode = Mode::Help,
        KeyCode::Left | KeyCode::Right => {
            let index = ChordField::ALL
                .iter()
                .position(|value| *value == field)
                .unwrap();
            let next = if k.code == KeyCode::Right {
                (index + 1) % ChordField::ALL.len()
            } else {
                (index + ChordField::ALL.len() - 1) % ChordField::ALL.len()
            };
            a.chord_field = ChordField::ALL[next];
        }
        KeyCode::PageUp | KeyCode::PageDown => {
            move_chord_editor_step(a, k.code == KeyCode::PageDown);
            a.status = format!("Editing Chord at step {}", a.step + 1);
        }
        KeyCode::Up | KeyCode::Down => {
            let step = a.step;
            match field {
                ChordField::Shape => {
                    let index = ChordShape::ALL
                        .iter()
                        .position(|value| *value == shape)
                        .unwrap();
                    let next = lfo_choice_index(index, ChordShape::ALL.len(), k.code);
                    let next_shape = ChordShape::ALL[next];
                    if read_only {
                        a.status = "Chord settings are inherited; edit the note trigger".into();
                    } else if next_shape != shape
                        && apply(a, audio, |editor| {
                            editor.set_chord_shape(track, step, next_shape)
                        })
                    {
                        sync_project(a, audio);
                        a.mode = Mode::ChordEdit { shape: next_shape };
                        a.status = format!("Chord shape set to {next_shape}");
                    }
                }
                ChordField::Arp => {
                    if read_only {
                        a.status = "Chord settings are inherited; edit the note trigger".into();
                    } else if apply(a, audio, |editor| {
                        editor.set_arpeggio_enabled(
                            track,
                            step,
                            !editor
                                .arpeggio_config_value(track, step)
                                .unwrap_or_default()
                                .enabled,
                        )
                    }) {
                        sync_project(a, audio);
                    }
                }
                ChordField::Type => {
                    let config = a
                        .editor
                        .arpeggio_config_value(track, step)
                        .unwrap_or_default();
                    if !config.enabled {
                        a.status = "Arpeggio type is disabled while Arp is off".into();
                    } else if read_only {
                        a.status = "Chord settings are inherited; edit the note trigger".into();
                    } else {
                        let index = ArpeggioType::ALL
                            .iter()
                            .position(|v| *v == config.r#type)
                            .unwrap();
                        let next = lfo_choice_index(index, ArpeggioType::ALL.len(), k.code);
                        if next != index
                            && apply(a, audio, |editor| {
                                editor.set_arpeggio_type(track, step, ArpeggioType::ALL[next])
                            })
                        {
                            sync_project(a, audio);
                        }
                    }
                }
                ChordField::Rate => {
                    let config = a
                        .editor
                        .arpeggio_config_value(track, step)
                        .unwrap_or_default();
                    if !config.enabled {
                        a.status = "Arpeggio rate is disabled while Arp is off".into();
                    } else if read_only {
                        a.status = "Chord settings are inherited; edit the note trigger".into();
                    } else {
                        let index = ArpeggioRate::ALL
                            .iter()
                            .position(|v| *v == config.rate)
                            .unwrap();
                        let next = lfo_choice_index(index, ArpeggioRate::ALL.len(), k.code);
                        if next != index
                            && apply(a, audio, |editor| {
                                editor.set_arpeggio_rate(track, step, ArpeggioRate::ALL[next])
                            })
                        {
                            sync_project(a, audio);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(true)
}

pub(super) fn move_chord_editor_step(a: &mut App, forward: bool) {
    move_step_page(a, forward);
    let shape = a
        .editor
        .chord_shape_value(4, a.step)
        .unwrap_or_else(|_| selected_chord_shape(a, a.row - 1).unwrap_or_default());
    a.mode = Mode::ChordEdit { shape };
}

pub(super) fn handle_trigger_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::TriggerEdit { field } = a.mode else {
        return Ok(false);
    };
    let track = a.row.saturating_sub(1);
    let step = a.step;
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => return Ok(false),
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            return Ok(true);
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('T') => {
            a.mode = Mode::Navigation;
            a.status = "Trigger editing finished".into();
            return Ok(true);
        }
        KeyCode::Left | KeyCode::Right => {
            let index = TriggerField::ALL
                .iter()
                .position(|value| *value == field)
                .unwrap();
            let next = if k.code == KeyCode::Right {
                (index + 1) % TriggerField::ALL.len()
            } else {
                (index + TriggerField::ALL.len() - 1) % TriggerField::ALL.len()
            };
            a.mode = Mode::TriggerEdit {
                field: TriggerField::ALL[next],
            };
            return Ok(true);
        }
        _ => {}
    }
    let condition = match a.editor.trigger_condition_value(track, step) {
        Ok(value) => value,
        Err(error) => {
            a.status = error.to_string();
            a.mode = Mode::Navigation;
            return Ok(true);
        }
    };
    let mut changed = false;
    match field {
        TriggerField::Mode if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
            let modes = [
                TriggerCondition::Always,
                TriggerCondition::Cycle {
                    position: 1,
                    length: 2,
                },
                TriggerCondition::Chance {
                    probability: Percent::new(50).unwrap(),
                },
            ];
            let index = match condition {
                TriggerCondition::Always => 0,
                TriggerCondition::Cycle { .. } => 1,
                TriggerCondition::Chance { .. } => 2,
            };
            let next = lfo_choice_index(index, modes.len(), k.code);
            changed = apply(a, audio, |editor| {
                editor.set_trigger_condition(track, step, modes[next])
            });
        }
        TriggerField::CyclePosition if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
            if let TriggerCondition::Cycle { position, length } = condition {
                let index = usize::from(position - 1);
                let next = lfo_choice_index(index, usize::from(length), k.code);
                let position = next as u8 + 1;
                changed = apply(a, audio, |editor| {
                    editor.set_trigger_condition(
                        track,
                        step,
                        TriggerCondition::Cycle { position, length },
                    )
                });
            }
        }
        TriggerField::CycleLength if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
            if let TriggerCondition::Cycle { position, length } = condition {
                let index = usize::from(length - 2);
                let next = lfo_choice_index(index, 3, k.code);
                let length = next as u8 + 2;
                changed = apply(a, audio, |editor| {
                    editor.set_trigger_condition(
                        track,
                        step,
                        TriggerCondition::Cycle {
                            position: position.min(length),
                            length,
                        },
                    )
                });
            }
        }
        TriggerField::Chance => {
            if let TriggerCondition::Chance { probability } = condition {
                let direction: i16 = if k.code == KeyCode::Down { -1 } else { 1 };
                let value = match k.code {
                    KeyCode::Up | KeyCode::Down => probability.saturating_add(direction),
                    KeyCode::Char(c) => crate::reducer::percentage_key(c).unwrap_or(probability),
                    _ => probability,
                };
                if value != probability {
                    changed = apply(a, audio, |editor| {
                        editor.set_trigger_condition(
                            track,
                            step,
                            TriggerCondition::Chance { probability: value },
                        )
                    });
                }
            }
        }
        TriggerField::Retrigger if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
            let count = a.editor.retrigger_count_value(track, step).unwrap_or(1);
            let index = usize::from(count - 1);
            let next = lfo_choice_index(index, 4, k.code);
            let count = next as u8 + 1;
            changed = apply(a, audio, |editor| {
                editor.set_retrigger_count(track, step, count)
            });
        }
        _ => {}
    }
    if changed {
        sync_project(a, audio);
    }
    Ok(true)
}

pub(super) fn handle_swing_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let track = a.row.saturating_sub(1);
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => Ok(false),
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            Ok(true)
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('S') => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation;
            a.status = "Swing editing finished".into();
            Ok(true)
        }
        KeyCode::Up | KeyCode::Down => {
            let delta = if k.modifiers.contains(KeyModifiers::SHIFT) {
                10
            } else {
                1
            };
            let delta = if k.code == KeyCode::Down {
                -delta
            } else {
                delta
            };
            let current = a.editor.project.tracks[track].swing;
            let value = Percent::new((current.get() as i16 + delta).clamp(0, 75) as u8).unwrap();
            if apply(a, audio, |editor| {
                editor.set_track_swing(track, value, None)
            }) {
                sync_project(a, audio);
            }
            Ok(true)
        }
        _ => Ok(true),
    }
}

pub(super) fn handle_lfo_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
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

pub(super) fn lfo_choice_index(current: usize, len: usize, key: KeyCode) -> usize {
    match key {
        KeyCode::Up => current.saturating_sub(1),
        KeyCode::Down => current.saturating_add(1).min(len.saturating_sub(1)),
        _ => current,
    }
}

pub(super) fn set_lfo_config(
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

pub(super) fn parameter_shortcut(kind: TrackKind, c: char) -> Option<ParameterId> {
    parameter_descriptors(kind)
        .iter()
        .find(|descriptor| descriptor.shortcut.starts_with(c))
        .map(|descriptor| descriptor.id)
}

pub(super) fn active_parameter_shortcut(a: &App, c: char) -> Option<ParameterId> {
    if a.parameter_bank == ParameterBank::Params {
        return parameter_shortcut(a.editor.project.tracks[a.row - 1].kind, c);
    }
    visible_parameter_descriptors(a.parameter_bank, a.editor.project.tracks[a.row - 1].kind)
        .iter()
        .find(|descriptor| descriptor.shortcut.starts_with(c))
        .map(|descriptor| descriptor.id)
}

pub(super) fn toggle_parameter_bank(a: &mut App) {
    a.editor.end_coalescing();
    a.parameter_bank = match a.parameter_bank {
        ParameterBank::Params => ParameterBank::Effects,
        ParameterBank::Effects => ParameterBank::Params,
    };
    if let Mode::ParameterEdit(_) = a.mode {
        let parameter = visible_parameter_descriptors(
            a.parameter_bank,
            a.editor.project.tracks[a.row - 1].kind,
        )[0]
        .id;
        enter_parameter_edit(a, parameter);
    }
    a.status = match a.parameter_bank {
        ParameterBank::Params => "PARAMS bank".into(),
        ParameterBank::Effects => "EFFECTS bank".into(),
    };
}

pub(super) fn enter_parameter_edit(a: &mut App, parameter: ParameterId) {
    a.mode = Mode::ParameterEdit(parameter);
    a.status = format!("Editing {}", parameter.display_name());
}

pub(super) fn switch_parameter_editor(a: &mut App, parameter: ParameterId) {
    a.editor.end_coalescing();
    enter_parameter_edit(a, parameter);
}

pub(super) fn move_parameter_editor(a: &mut App, forward: bool) {
    let Mode::ParameterEdit(current) = a.mode else {
        return;
    };
    let descriptors =
        visible_parameter_descriptors(a.parameter_bank, a.editor.project.tracks[a.row - 1].kind);
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

pub(super) fn flipped_waveform(waveform: Waveform) -> Waveform {
    match waveform {
        Waveform::Square => Waveform::Saw,
        Waveform::Saw => Waveform::Square,
    }
}

pub(super) fn set_parameter(
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

pub(super) fn coalesce_key(
    track: usize,
    step: usize,
    parameter: ParameterId,
) -> crate::reducer::CoalesceKey {
    crate::reducer::CoalesceKey(track, step, parameter as u8)
}

pub(super) fn apply<F: FnOnce(&mut Editor) -> Result<bool, crate::reducer::EditError>>(
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
