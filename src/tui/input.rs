use super::{
    controller::{
        change_octave, enter_error, enter_global_edit, enter_global_parameter_mode,
        finish_global_parameter_edit, global_id, global_shortcut, handle_default_preset_confirm,
        handle_file_input, handle_global_key, handle_global_parameter_key, handle_new_confirm,
        handle_open_confirm, handle_overwrite_confirm, handle_preset_browser, handle_preset_dialog,
        handle_preset_name_input, handle_preset_overwrite_confirm, handle_project_browser,
        handle_sidechain_key, handle_tempo_input, new_project, preset_dialog_mode,
        project_browser_mode, request_new_project, save, save_as_mode, sync_project,
        sync_project_with_smoothing,
    },
    controls::GLOBAL_CONTROLS,
    render::{
        is_recipe_parameter, parameter_descriptors, parameter_recipe, scope_name,
        selected_chord_shape, selected_drum_recipe, visible_parameter_descriptors,
    },
    state::{
        App, ChordField, GeneratorDialog, LfoField, Mode, ParameterBank, ParameterFocus,
        PatternPage, TriggerField,
    },
};
use crate::{
    audio::{Audio, AudioCommand},
    generator::{ChordShapePool, Config as GeneratorConfig, Target as GeneratorTarget},
    model::{
        ArpeggioRate, ArpeggioType, ChordShape, ChorusMode, DRUM_TRACK_COUNT, DrumRecipeSlot,
        FmAlgorithm, FmOperatorField, FmRatio, LfoConfig, LfoDivision, LfoRate, LfoWaveform,
        MAX_STEP_COUNT, ParameterId, ParameterValue, Percent, STEP_BANK_SIZE, STEP_ROW_SIZE,
        StepEvent, TRACK_COUNT, TrackKind, TriggerCondition, Waveform,
    },
    reducer::{Editor, Scope},
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    if k.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(k.code, KeyCode::Char('r' | 'R'))
        && matches!(
            a.mode,
            Mode::Navigation
                | Mode::ParameterEdit(_)
                | Mode::FmOperatorEdit { .. }
                | Mode::LfoEdit { .. }
                | Mode::ChordEdit { .. }
                | Mode::TriggerEdit { .. }
                | Mode::SwingEdit
                | Mode::TrackProbabilityEdit
                | Mode::GlobalParameterEdit(_)
                | Mode::GlobalEdit(_)
                | Mode::SidechainEdit { .. }
                | Mode::TempoInput { .. }
                | Mode::TrackLengthInput(_)
        )
    {
        toggle_recording(a, audio);
        return Ok(());
    }
    if matches!(a.mode, Mode::Error(_)) {
        if matches!(k.code, KeyCode::Esc | KeyCode::Enter) {
            a.mode = Mode::Navigation;
        }
        return Ok(());
    }
    if matches!(a.mode, Mode::OverwriteConfirm { .. }) {
        return handle_overwrite_confirm(a, audio, k);
    }
    if matches!(a.mode, Mode::PresetOverwriteConfirm { .. }) {
        handle_preset_overwrite_confirm(a, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::PresetDialog { .. }) {
        handle_preset_dialog(a, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::DefaultPresetConfirm { .. }) {
        handle_default_preset_confirm(a, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::FileInput(_, _)) {
        return handle_file_input(a, audio, k);
    }
    if matches!(a.mode, Mode::PresetNameInput { .. }) {
        handle_preset_name_input(a, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::ProjectBrowser { .. }) {
        handle_project_browser(a, audio, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::PresetBrowser { .. }) {
        handle_preset_browser(a, audio, k);
        return Ok(());
    }
    if matches!(a.mode, Mode::TempoInput { .. }) {
        return handle_tempo_input(a, audio, k);
    }
    if matches!(a.mode, Mode::SidechainEdit { .. }) {
        handle_sidechain_key(a, audio, k)?;
        return Ok(());
    }
    if matches!(a.mode, Mode::FmOperatorEdit { .. }) {
        if handle_fm_operator_key(a, audio, k)? || handle_shared_key(a, audio, k) {
            return Ok(());
        }
        return Ok(());
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
                    a.mode = save_as_mode()
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
    if is_preset_dialog_shortcut(&a.mode, k) {
        open_preset_dialog(a);
        return Ok(());
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        match k.code {
            KeyCode::Char('c' | 'C')
                if a.row > 0 && matches!(a.mode, Mode::Navigation | Mode::ParameterEdit(_)) =>
            {
                let (track, step) = (a.row - 1, a.step);
                match a.editor.copy_step(track, step) {
                    Ok(()) => a.status = format!("Copied step {}", step + 1),
                    Err(error) => a.status = error.to_string(),
                }
            }
            KeyCode::Char('x' | 'X')
                if a.row > 0 && matches!(a.mode, Mode::Navigation | Mode::ParameterEdit(_)) =>
            {
                cut_selected_step(a, audio);
            }
            KeyCode::Char('v' | 'V')
                if a.row > 0 && matches!(a.mode, Mode::Navigation | Mode::ParameterEdit(_)) =>
            {
                paste_selected_step(a, audio);
            }
            KeyCode::Char('p' | 'P') => {
                a.pattern_cursor = a.editor.pattern().min(a.editor.project.patterns.len() - 1);
                a.pattern_page = PatternPage::Patterns;
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
                a.mode = save_as_mode()
            }
            KeyCode::Char('s' | 'S') => {
                if a.path.is_some() {
                    save(a)?
                } else {
                    a.mode = save_as_mode()
                }
            }
            KeyCode::Char('o' | 'O') => match project_browser_mode() {
                Ok(mode) => a.mode = mode,
                Err(error) => enter_error(a, error.to_string()),
            },
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
    if matches!(k.code, KeyCode::Tab | KeyCode::BackTab)
        && matches!(
            a.mode,
            Mode::Navigation | Mode::ParameterEdit(_) | Mode::GlobalParameterEdit(_)
        )
    {
        if a.mode == Mode::Navigation {
            if a.row == 0 {
                enter_global_parameter_mode(a, global_id(a.global));
            } else {
                enter_parameter_mode(a);
            }
        } else if matches!(a.mode, Mode::GlobalParameterEdit(_)) {
            finish_global_parameter_edit(a);
        } else {
            finish_parameter_edit(a);
        }
        return Ok(());
    }
    if matches!(
        a.mode,
        Mode::Navigation | Mode::ParameterEdit(_) | Mode::GlobalParameterEdit(_)
    ) && global_jump(k)
    {
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
    if a.mode == Mode::TrackProbabilityEdit && handle_track_probability_key(a, audio, k)? {
        return Ok(());
    }
    if matches!(a.mode, Mode::ParameterEdit(_)) && handle_parameter_key(a, audio, k)? {
        return Ok(());
    }
    if matches!(a.mode, Mode::GlobalParameterEdit(_)) && handle_global_parameter_key(a, audio, k)? {
        return Ok(());
    }
    if matches!(a.mode, Mode::GlobalEdit(_)) && handle_global_key(a, audio, k)? {
        return Ok(());
    }
    if handle_shared_key(a, audio, k) {
        return Ok(());
    }
    if a.mode != Mode::Navigation {
        return Ok(());
    }
    match k.code {
        KeyCode::Char('=') if k.modifiers.is_empty() => {
            a.auto_advance = !a.auto_advance;
            a.status = if a.auto_advance {
                "Auto-advance on"
            } else {
                "Auto-advance off"
            }
            .into();
        }
        KeyCode::Char('p') if a.row > 0 => enter_lock_parameter_mode(a),
        KeyCode::Char('O')
            if a.row > 0 && a.editor.project.tracks[a.row - 1].kind == TrackKind::Fm =>
        {
            open_fm_operator_editor(a, false)
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
                chord_shapes: defaults.chord_shapes,
                ties: defaults.ties,
                accents: defaults.accents,
                slides: defaults.slides,
                field: 0,
            });
            a.status = "Generator ready".into();
        }
        KeyCode::Up if k.modifiers.contains(KeyModifiers::SHIFT) => {
            move_track_vertical(a, false);
        }
        KeyCode::Up => {
            move_step_vertical(a, false);
        }
        KeyCode::Down if k.modifiers.contains(KeyModifiers::SHIFT) => {
            move_track_vertical(a, true);
        }
        KeyCode::Down => {
            move_step_vertical(a, true);
        }
        KeyCode::Left => {
            if a.row == 0 {
                a.global = (a.global + GLOBAL_CONTROLS.len() - 1) % GLOBAL_CONTROLS.len()
            } else if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, false)
            } else {
                move_step(a, false)
            }
        }
        KeyCode::Right => {
            if a.row == 0 {
                a.global = (a.global + 1) % GLOBAL_CONTROLS.len()
            } else if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, true)
            } else {
                move_step(a, true)
            }
        }
        KeyCode::Enter if a.row == 0 => enter_global_edit(a, global_id(a.global)),
        KeyCode::Enter if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_event(track, step)) && sync_project(a, audio) {
                let inserted = a.editor.active_steps(track).unwrap()[step].is_some();
                if inserted && !a.playing {
                    let _ = audio.send(AudioCommand::AutoAudition {
                        track: track as u8,
                        step: step as u8,
                    });
                }
                if inserted {
                    advance_after_event_entry(a);
                }
            }
        }
        KeyCode::Backspace | KeyCode::Delete if is_clear_track_shortcut(&a.mode, a.row, k) => {
            clear_selected_track(a, audio)
        }
        KeyCode::Backspace | KeyCode::Delete if a.row > 0 => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.clear(track, step)) {
                sync_project(a, audio);
            }
        }
        KeyCode::Char('m') if a.row > 0 => {
            let ti = a.row - 1;
            if audio.available_commands() == 0 {
                a.status = "Audio command queue full; edit rejected".into();
                return Ok(());
            }
            let _ = a.editor.edit_track(ti, None, |p, _| {
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
        KeyCode::Char('a') if a.row > 0 => {
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
                        field: TriggerField::Microtiming,
                    };
                    a.status = "Editing microtiming, condition, and retrigger count".into();
                }
                Err(error) => a.status = error.to_string(),
            }
        }
        KeyCode::Char('S') if a.row > 0 => {
            a.mode = Mode::SwingEdit;
            a.status = format!("{} swing", a.editor.project.tracks[a.row - 1].name);
        }
        KeyCode::Char('Q') if a.row > 0 && k.modifiers.contains(KeyModifiers::SHIFT) => {
            a.editor.end_coalescing();
            a.mode = Mode::TrackProbabilityEdit;
            a.status = format!("{} probability", a.editor.project.tracks[a.row - 1].name);
        }
        KeyCode::Char('C')
            if a.row > 0 && a.editor.project.tracks[a.row - 1].kind.supports_voicing() =>
        {
            open_chord_editor(a)
        }
        KeyCode::Char(c @ '1'..='3')
            if a.row > 0 && k.modifiers.is_empty() && selected_recipe_shortcut(a, c).is_some() =>
        {
            apply_selected_recipe(a, audio, selected_recipe_shortcut(a, c).unwrap());
        }
        KeyCode::Char('0')
            if a.row > 0
                && k.modifiers.is_empty()
                && matches!(
                    a.editor.project.tracks[a.row - 1].kind,
                    TrackKind::Hat | TrackKind::Tom
                ) =>
        {
            reset_selected_recipe_overrides(a, audio);
        }
        KeyCode::Char(c) if a.row == 0 => {
            if let Some(id) = global_shortcut(c) {
                a.global = id as usize;
                enter_global_edit(a, id)
            }
        }
        KeyCode::Char('[') if is_pitched_track_row(a.row) => change_octave(a, audio, -1),
        KeyCode::Char(']') if is_pitched_track_row(a.row) => change_octave(a, audio, 1),
        KeyCode::Char('t') if is_pitched_track_row(a.row) => {
            let (track, step) = (a.row - 1, a.step);
            if apply(a, audio, |e| e.toggle_tie(track, step))
                && sync_project(a, audio)
                && a.editor.active_steps(track).unwrap()[step].is_some()
            {
                advance_after_event_entry(a);
            }
        }
        KeyCode::Char(c @ '1'..='8') if is_pitched_track_row(a.row) => {
            enter_selected_note(a, audio, c.to_digit(10).unwrap() as u8);
        }
        KeyCode::Esc => a.scope = Scope::Base,
        _ => {}
    }
    Ok(())
}

pub(super) fn is_preset_dialog_shortcut(mode: &Mode, k: KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL)
        && !k.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(k.code, KeyCode::Char('l'))
        && matches!(mode, Mode::Navigation | Mode::ParameterEdit(_))
}

pub(super) fn open_preset_dialog(a: &mut App) {
    if a.row == 0 {
        a.status = "Select a track to manage presets".into();
        return;
    }
    match preset_dialog_mode(a, a.row - 1) {
        Ok(mode) => a.mode = mode,
        Err(error) => enter_error(a, error.to_string()),
    }
}

fn toggle_recording(a: &mut App, audio: &mut Audio) {
    match audio.recording_state() {
        crate::audio::RecordingState::Idle => {
            match audio.start_project_recording(a.path.as_deref()) {
                Ok(path) => {
                    a.recording_state = crate::audio::RecordingState::Recording;
                    a.status = format!("Recording to {}", path.display());
                }
                Err(error) => a.status = format!("Could not start recording: {error}"),
            }
        }
        crate::audio::RecordingState::Recording => match audio.stop_recording() {
            Ok(()) => {
                a.recording_state = crate::audio::RecordingState::Finalizing;
                a.status = audio.recording_path().map_or_else(
                    || "Finalizing WAV".into(),
                    |path| format!("Finalizing WAV {}", path.display()),
                );
            }
            Err(error) => a.status = format!("Could not stop recording: {error}"),
        },
        crate::audio::RecordingState::Finalizing => {
            a.status = "WAV finalization is still in progress".into()
        }
    }
}

fn handle_shared_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> bool {
    match k.code {
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            true
        }
        KeyCode::Char(' ') => {
            if audio.send(AudioCommand::PlayPause).is_ok() {
                a.playing = !a.playing;
                a.paused = !a.playing;
                a.status = if a.playing { "Playing" } else { "Paused" }.into();
            } else {
                a.status = "Audio command queue full".into();
            }
            true
        }
        KeyCode::Char('.') => {
            if audio.send(AudioCommand::Stop).is_ok() {
                a.playing = false;
                a.paused = false;
                a.playheads = [None; TRACK_COUNT];
                a.status = "Stopped and reset".into();
            } else {
                a.status = "Audio command queue full".into();
            }
            true
        }
        KeyCode::Char('o') if a.row > 0 => {
            if audio
                .send(AudioCommand::Audition {
                    track: (a.row - 1) as u8,
                    step: a.step as u8,
                })
                .is_ok()
            {
                a.status = "Auditioning selection".into();
            } else {
                a.status = "Audio command queue full".into();
            }
            true
        }
        _ => false,
    }
}

pub(super) fn move_step(a: &mut App, forward: bool) {
    let track = a.row - 1;
    let length = a.editor.active_steps(track).unwrap().len();
    a.step = if forward {
        (a.step + 1) % length
    } else {
        (a.step + length - 1) % length
    };
}

fn advance_after_event_entry(a: &mut App) {
    if !a.auto_advance || a.row == 0 {
        return;
    }
    if matches!(a.mode, Mode::ChordEdit { .. }) {
        move_chord_editor_step(a, true);
    } else {
        move_step(a, true);
        refresh_lock_recipe(a);
    }
}

pub(super) fn is_pitched_track_row(row: usize) -> bool {
    row > DRUM_TRACK_COUNT
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
        '&' => Some(6),
        '*' => Some(7),
        '(' => Some(8),
        ')' => Some(9),
        _ => None,
    };
    if shifted_symbol.is_some() {
        return shifted_symbol;
    }
    if k.modifiers.contains(KeyModifiers::SHIFT) {
        return c
            .to_digit(10)
            .filter(|&track| track <= 9)
            .map(|track| if track == 0 { 9 } else { track as usize - 1 });
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
    a.mode = Mode::Navigation;
}

pub(super) fn select_track(a: &mut App, track: usize) {
    a.editor.end_coalescing();
    a.row = track + 1;
    a.step = a.step.min(a.editor.active_steps(track).unwrap().len() - 1);
    if a.parameter_recipe.get() > a.editor.project.tracks[track].drum_recipe_count() {
        a.parameter_recipe = DrumRecipeSlot::ONE;
    }

    if matches!(a.mode, Mode::ParameterEdit(_)) {
        restore_parameter_edit(a, remembered_parameter(a, track, a.parameter_bank));
    }

    if a.scope == Scope::Lock
        && let Mode::ParameterEdit(parameter) = a.mode
        && is_recipe_parameter(a.editor.project.tracks[track].kind, parameter)
    {
        a.parameter_recipe = selected_drum_recipe(a, track);
    }
}

pub(super) fn commit_pattern(a: &mut App, audio: &mut Audio, pattern: usize) -> bool {
    let previous = a.editor.pattern();
    if pattern >= a.editor.project.patterns.len() || audio.available_commands() < 1 {
        a.status = "Audio command queue full; pattern switch rejected".into();
        return false;
    }
    if !a.playing && pattern != previous && !a.editor.select_pattern(pattern) {
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
    if a.pattern_page == PatternPage::Song {
        return handle_song_dialog(a, audio, k);
    }
    match k.code {
        KeyCode::Esc | KeyCode::Char('p' | 'P') => a.mode = Mode::Navigation,
        KeyCode::Tab => a.pattern_page = PatternPage::Song,
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
            |e, cursor| e.insert_pattern(cursor),
            "Inserted pattern",
        ),
        KeyCode::Char('d') | KeyCode::Char('D') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.duplicate_pattern(cursor),
            "Duplicated pattern",
        ),
        KeyCode::Char('c' | 'C') => {
            if a.editor.copy_pattern(a.pattern_cursor) {
                a.status = format!("Copied pattern {}", a.pattern_cursor + 1);
            }
        }
        KeyCode::Char('x' | 'X') => {
            pattern_edit_at(a, audio, |e, cursor| e.cut_pattern(cursor), "Cut pattern")
        }
        KeyCode::Char('v' | 'V') => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.paste_pattern(cursor),
            "Pasted pattern",
        ),
        KeyCode::Delete | KeyCode::Backspace => pattern_edit_at(
            a,
            audio,
            |e, cursor| e.delete_pattern(cursor),
            "Deleted pattern",
        ),
        _ => {}
    }
    Ok(())
}

fn commit_song(a: &mut App, audio: &mut Audio, entry: usize) -> bool {
    if entry >= a.editor.project.song.len() || audio.available_commands() < 1 {
        a.status = "Audio command queue full; song switch rejected".into();
        return false;
    }
    if audio
        .send(AudioCommand::SelectSong { entry: entry as u8 })
        .is_err()
    {
        a.status = "Audio command queue full; song switch rejected".into();
        return false;
    }
    if !a.playing {
        let pattern = usize::from(a.editor.project.song[entry].pattern - 1);
        a.editor.select_pattern(pattern);
        a.active_pattern = pattern;
        a.active_song = entry;
        a.song_bar = 0;
        a.song_mode = true;
    }
    a.status = if a.playing {
        format!("Song entry {} queued for next bar", entry + 1)
    } else {
        format!("Song entry {} selected", entry + 1)
    };
    true
}

fn song_edit_at<F>(a: &mut App, audio: &mut Audio, f: F, message: &str)
where
    F: FnOnce(&mut Editor, usize) -> Result<bool, crate::reducer::EditError>,
{
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; song edit rejected".into();
        return;
    }
    match f(&mut a.editor, a.song_cursor) {
        Ok(true) if sync_project(a, audio) => {
            a.song_cursor = a.song_cursor.min(a.editor.project.song.len() - 1);
            a.status = message.into();
        }
        Ok(false) => a.status = "Song entry unchanged".into(),
        Err(error) => a.status = error.to_string(),
        _ => {}
    }
}

fn handle_song_dialog(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<()> {
    let count = a.editor.project.song.len();
    match k.code {
        KeyCode::Esc | KeyCode::Char('p' | 'P') => a.mode = Mode::Navigation,
        KeyCode::Tab | KeyCode::BackTab => a.pattern_page = PatternPage::Patterns,
        KeyCode::Left => a.song_cursor = adjacent_pattern_in_count(a.song_cursor, false, count),
        KeyCode::Right => a.song_cursor = adjacent_pattern_in_count(a.song_cursor, true, count),
        KeyCode::Home => a.song_cursor = 0,
        KeyCode::End => a.song_cursor = count - 1,
        KeyCode::Up => song_edit_at(
            a,
            audio,
            |e, cursor| e.change_song_bars(cursor, 1),
            "Increased song bars",
        ),
        KeyCode::Down => song_edit_at(
            a,
            audio,
            |e, cursor| e.change_song_bars(cursor, -1),
            "Decreased song bars",
        ),
        KeyCode::Char('[') => song_edit_at(
            a,
            audio,
            |e, cursor| e.change_song_pattern(cursor, -1),
            "Previous song pattern",
        ),
        KeyCode::Char(']') => song_edit_at(
            a,
            audio,
            |e, cursor| e.change_song_pattern(cursor, 1),
            "Next song pattern",
        ),
        KeyCode::Enter => {
            if commit_song(a, audio, a.song_cursor) {
                a.mode = Mode::Navigation;
            }
        }
        KeyCode::Char('n' | 'N') => song_edit_at(
            a,
            audio,
            |e, cursor| e.insert_song_entry(cursor),
            "Inserted song entry",
        ),
        KeyCode::Char('d' | 'D') => song_edit_at(
            a,
            audio,
            |e, cursor| e.duplicate_song_entry(cursor),
            "Duplicated song entry",
        ),
        KeyCode::Char('c' | 'C') => {
            if a.editor.copy_song_entry(a.song_cursor) {
                a.status = format!("Copied song entry {}", a.song_cursor + 1);
            }
        }
        KeyCode::Char('x' | 'X') => song_edit_at(
            a,
            audio,
            |e, cursor| e.cut_song_entry(cursor),
            "Cut song entry",
        ),
        KeyCode::Char('v' | 'V') => song_edit_at(
            a,
            audio,
            |e, cursor| e.paste_song_entry(cursor),
            "Pasted song entry",
        ),
        KeyCode::Delete | KeyCode::Backspace => song_edit_at(
            a,
            audio,
            |e, cursor| e.delete_song_entry(cursor),
            "Deleted song entry",
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
        KeyCode::Left | KeyCode::Right
            if (3..=9).contains(&dialog.field)
                && dialog.field_is_applicable(&a.editor.project, dialog.field) =>
        {
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

const GENERATOR_FIELD_COUNT: usize = 10;

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
        chord_shapes: dialog.chord_shapes,
        ties: dialog.ties,
        accents: dialog.accents,
        slides: dialog.slides,
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
        6 => {
            let index = ChordShapePool::ALL
                .iter()
                .position(|value| *value == dialog.chord_shapes)
                .unwrap_or_default();
            let next = if right {
                index.saturating_add(1).min(ChordShapePool::ALL.len() - 1)
            } else {
                index.saturating_sub(1)
            };
            dialog.chord_shapes = ChordShapePool::ALL[next];
        }
        3 | 7 | 8 | 9 => {
            let delta = if right { 5 } else { -5 };
            match dialog.field {
                3 => dialog.density = dialog.density.saturating_add(delta),
                7 => dialog.ties = dialog.ties.saturating_add(delta),
                8 => dialog.accents = dialog.accents.saturating_add(delta),
                9 => dialog.slides = dialog.slides.saturating_add(delta),
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
    refresh_lock_recipe(a);
}

fn refresh_lock_recipe(a: &mut App) {
    if a.scope == Scope::Lock
        && let Mode::ParameterEdit(parameter) = a.mode
        && is_recipe_parameter(a.editor.project.tracks[a.row - 1].kind, parameter)
    {
        a.parameter_recipe = selected_drum_recipe(a, a.row - 1);
    }
}

pub(super) fn move_step_bank(a: &mut App, forward: bool) {
    let track = a.row - 1;
    let length = a.editor.active_steps(track).unwrap().len();
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
    refresh_lock_recipe(a);
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
    let length = a.editor.active_steps(track).unwrap().len();
    if down {
        if step_row + 1 < length.div_ceil(STEP_ROW_SIZE) {
            a.step = ((step_row + 1) * STEP_ROW_SIZE + column).min(length - 1);
        } else if track + 1 < TRACK_COUNT {
            let destination = track + 1;
            a.row = destination + 1;
            a.step = column.min(a.editor.active_steps(destination).unwrap().len() - 1);
        }
    } else if step_row > 0 {
        a.step -= STEP_ROW_SIZE;
    } else if track == 0 {
        a.row = 0;
        a.scope = Scope::Base;
    } else {
        let destination = track - 1;
        let destination_length = a.editor.active_steps(destination).unwrap().len();
        a.row = destination + 1;
        a.step = ((destination_length - 1) / STEP_ROW_SIZE * STEP_ROW_SIZE + column)
            .min(destination_length - 1);
    }
}

pub(super) fn move_track_vertical(a: &mut App, down: bool) {
    if a.row == 0 {
        if down {
            a.row = 1;
            a.step = 0;
            a.scope = Scope::Base;
        }
        return;
    }

    let track = a.row - 1;
    let Some(destination) = (if down {
        track.checked_add(1).filter(|&next| next < TRACK_COUNT)
    } else {
        track.checked_sub(1)
    }) else {
        return;
    };
    let column = a.step % STEP_ROW_SIZE;
    a.row = destination + 1;
    a.step = column.min(a.editor.active_steps(destination).unwrap().len() - 1);
    refresh_lock_recipe(a);
}

pub(super) fn normalize_cursor(a: &mut App) {
    a.pattern_cursor = a
        .pattern_cursor
        .min(a.editor.project.patterns.len().saturating_sub(1));
    if a.row > 0 {
        a.step = a
            .step
            .min(a.editor.active_steps(a.row - 1).unwrap().len() - 1);
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
    let old_length = a.editor.active_steps(track).unwrap().len();
    let old_events = a
        .editor
        .active_steps(track)
        .unwrap()
        .iter()
        .flatten()
        .count();
    let key = coalesce.then_some(crate::reducer::CoalesceKey(track, usize::MAX, u8::MAX));
    match a.editor.set_track_length(track, length, key) {
        Ok(true) => {
            normalize_cursor(a);
            if sync_project(a, audio) {
                let new_events = a
                    .editor
                    .active_steps(track)
                    .unwrap()
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
    let old_length = a.editor.active_steps(track).unwrap().len();
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
        && matches!(k.code, KeyCode::Backspace | KeyCode::Delete)
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
            let current = a.editor.active_steps(a.row - 1).unwrap().len();
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
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, k.code == KeyCode::PageDown);
            } else {
                move_step_page(a, k.code == KeyCode::PageDown);
            }
            Ok(true)
        }
        KeyCode::Left => {
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                select_parameter_bank(a, ParameterBank::Params);
            } else {
                move_parameter_editor(a, false);
            }
            Ok(true)
        }
        KeyCode::Right => {
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                select_parameter_bank(a, ParameterBank::Effects);
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
            let current = match editor_parameter_value(a, track, parameter) {
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
                Ok(ParameterValue::LeadSubMode(mode)) => {
                    use crate::model::LeadSubMode;
                    let next = match (mode, k.code) {
                        (LeadSubMode::OneOctaveSquare, KeyCode::Up) => LeadSubMode::TwoOctaveSquare,
                        (LeadSubMode::TwoOctaveSquare, KeyCode::Up) => {
                            LeadSubMode::TwoOctaveNarrowPulse
                        }
                        (LeadSubMode::TwoOctaveNarrowPulse, KeyCode::Down) => {
                            LeadSubMode::TwoOctaveSquare
                        }
                        (LeadSubMode::TwoOctaveSquare, KeyCode::Down) => {
                            LeadSubMode::OneOctaveSquare
                        }
                        _ => mode,
                    };
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::LeadSubMode(next),
                        true,
                        false,
                    );
                    return Ok(true);
                }
                Ok(ParameterValue::FmAlgorithm(mode)) => {
                    let index = FmAlgorithm::ALL
                        .iter()
                        .position(|choice| *choice == mode)
                        .unwrap_or(0);
                    let next = if k.code == KeyCode::Up {
                        (index + 1).min(FmAlgorithm::ALL.len() - 1)
                    } else {
                        index.saturating_sub(1)
                    };
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::FmAlgorithm(FmAlgorithm::ALL[next]),
                        true,
                        false,
                    );
                    return Ok(true);
                }
                Ok(ParameterValue::FmRatio(mode)) => {
                    let index = FmRatio::ALL
                        .iter()
                        .position(|choice| *choice == mode)
                        .unwrap_or(0);
                    let next = if k.code == KeyCode::Up {
                        (index + 1).min(FmRatio::ALL.len() - 1)
                    } else {
                        index.saturating_sub(1)
                    };
                    set_parameter(
                        a,
                        audio,
                        parameter,
                        ParameterValue::FmRatio(FmRatio::ALL[next]),
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
        KeyCode::Char('C') if a.editor.project.tracks[track].kind.supports_voicing() => {
            a.mode = Mode::Navigation;
            open_chord_editor(a);
            Ok(true)
        }
        KeyCode::Char('O') if a.editor.project.tracks[track].kind == TrackKind::Fm => {
            open_fm_operator_editor(a, true);
            Ok(true)
        }
        KeyCode::Enter if parameter.fm_operator_field().is_some() => {
            open_fm_operator_editor(a, true);
            Ok(true)
        }
        KeyCode::Char('p') => {
            a.scope = if a.scope == Scope::Base {
                Scope::Lock
            } else {
                Scope::Base
            };
            if a.scope == Scope::Lock
                && is_recipe_parameter(a.editor.project.tracks[track].kind, parameter)
            {
                a.parameter_recipe = selected_drum_recipe(a, track);
            }
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
            finish_parameter_edit(a);
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
    parameter.is_percentage() && parameter != ParameterId::Pitch
}

pub(super) fn clamp_parameter_percentage(parameter: ParameterId, value: Percent) -> Percent {
    Percent::new(value.get().min(parameter_upper_bound(parameter))).unwrap()
}

pub(super) const fn parameter_upper_bound(parameter: ParameterId) -> u8 {
    parameter.upper_bound()
}

pub(super) fn parameter_edit_passthrough(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o')
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
    if a.row == 0 || !a.editor.project.tracks[a.row - 1].kind.supports_voicing() {
        a.status = "Voicing editor is available on Chord and FM tracks".into();
        return;
    }
    let track_index = a.row - 1;
    let track = &a.editor.project.tracks[track_index];
    let steps = a.editor.active_steps(track_index).unwrap();
    let shape = match steps[a.step].as_ref() {
        Some(StepEvent::Note { chord_shape, .. }) => {
            chord_shape.unwrap_or_else(|| track.default_voicing_shape().unwrap())
        }
        Some(StepEvent::Tie { .. }) => selected_chord_shape(a, track_index)
            .unwrap_or_else(|| track.default_voicing_shape().unwrap()),
        None => track.input_voicing_shape().unwrap(),
        _ => return,
    };
    let shape = a
        .editor
        .chord_shape_value(track_index, a.step)
        .unwrap_or(shape);
    a.chord_field = ChordField::Shape;
    a.mode = Mode::ChordEdit { shape };
    a.status = if matches!(steps[a.step], Some(StepEvent::Tie { .. })) {
        "Voicing is inherited; edit the note trigger".into()
    } else if steps[a.step].is_none() {
        "Editing voicing input defaults".into()
    } else {
        "Editing trigger voicing".into()
    };
}

pub(super) fn handle_chord_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::ChordEdit { shape } = a.mode else {
        return Ok(false);
    };
    let field = a.chord_field;
    let track = a.row.saturating_sub(1);
    let read_only = matches!(
        a.editor.active_steps(track).unwrap()[a.step],
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
            a.status = "Voicing editing finished".into();
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
            a.status = format!("Editing voicing at step {}", a.step + 1);
        }
        KeyCode::Char(c @ '1'..='8') if k.modifiers.is_empty() => {
            enter_selected_note(a, audio, c.to_digit(10).unwrap() as u8);
        }
        KeyCode::Char('[') if k.modifiers.is_empty() => change_octave(a, audio, -1),
        KeyCode::Char(']') if k.modifiers.is_empty() => change_octave(a, audio, 1),
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
                        a.status = "Voicing is inherited; edit the note trigger".into();
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
                        a.status = "Voicing is inherited; edit the note trigger".into();
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
                        a.status = "Voicing is inherited; edit the note trigger".into();
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
                        a.status = "Voicing is inherited; edit the note trigger".into();
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
    refresh_chord_editor_shape(a);
}

fn refresh_chord_editor_shape(a: &mut App) {
    let track = a.row - 1;
    let shape = a
        .editor
        .chord_shape_value(track, a.step)
        .unwrap_or_else(|_| {
            selected_chord_shape(a, track).unwrap_or_else(|| {
                a.editor.project.tracks[track]
                    .default_voicing_shape()
                    .unwrap()
            })
        });
    a.mode = Mode::ChordEdit { shape };
}

fn enter_selected_note(a: &mut App, audio: &mut Audio, degree: u8) {
    let (track, step) = (a.row - 1, a.step);
    if apply(a, audio, |editor| editor.set_note(track, step, degree)) && sync_project(a, audio) {
        let chord_editor = matches!(a.mode, Mode::ChordEdit { .. });
        if chord_editor {
            refresh_chord_editor_shape(a);
        }
        if !a.playing {
            let _ = audio.send(AudioCommand::AutoAudition {
                track: track as u8,
                step: step as u8,
            });
        }
        advance_after_event_entry(a);
    }
}

pub(super) fn finish_trigger_edit(a: &mut App) {
    a.editor.end_coalescing();
    a.mode = Mode::Navigation;
    a.status = "Trigger editing finished".into();
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
            a.editor.end_coalescing();
            a.mode = Mode::Help;
            return Ok(true);
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('T') => {
            finish_trigger_edit(a);
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
            a.editor.end_coalescing();
            a.status = error.to_string();
            a.mode = Mode::Navigation;
            return Ok(true);
        }
    };
    let mut changed = false;
    match field {
        TriggerField::Microtiming if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
            let current = a.editor.microtiming_value(track, step).unwrap_or_default();
            let direction = if k.code == KeyCode::Down { -1 } else { 1 };
            let amount = if k.modifiers.contains(KeyModifiers::SHIFT) {
                direction * 10
            } else {
                direction
            };
            let value = current.saturating_add(amount);
            if value != current {
                changed = apply(a, audio, |editor| {
                    editor.set_microtiming(
                        track,
                        step,
                        value,
                        Some(crate::reducer::CoalesceKey(track, step, 0xfd)),
                    )
                });
            }
        }
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

pub(super) fn handle_track_probability_key(
    a: &mut App,
    audio: &mut Audio,
    k: KeyEvent,
) -> Result<bool> {
    let track = a.row.saturating_sub(1);
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => Ok(false),
        KeyCode::Char('?') => {
            a.mode = Mode::Help;
            Ok(true)
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('Q') => {
            a.editor.end_coalescing();
            a.mode = Mode::Navigation;
            a.status = "Track probability editing finished".into();
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
            let current = a.editor.project.tracks[track].probability;
            let value = current.saturating_add(delta);
            if apply(a, audio, |editor| {
                editor.set_track_probability(
                    track,
                    value,
                    Some(crate::reducer::CoalesceKey(track, 0, 0xfe)),
                )
            }) {
                sync_project(a, audio);
            }
            Ok(true)
        }
        _ => Ok(true),
    }
}

pub(super) fn open_fm_operator_editor(a: &mut App, return_to_parameter: bool) {
    if a.row == 0 || a.editor.project.tracks[a.row - 1].kind != TrackKind::Fm {
        a.status = "FM operator editor is available on the FM track only".into();
        return;
    }
    if let Mode::ParameterEdit(parameter) = a.mode
        && let Some((operator, _)) = parameter.fm_operator_field()
    {
        a.fm_operator = operator;
    }
    a.editor.end_coalescing();
    a.mode = Mode::FmOperatorEdit {
        operator: a.fm_operator,
        field: a.fm_operator_field,
        return_to_parameter,
    };
    a.status = format!("Editing FM operator {}", a.fm_operator + 1);
}

pub(super) fn handle_fm_operator_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::FmOperatorEdit {
        operator,
        field,
        return_to_parameter,
    } = a.mode
    else {
        return Ok(false);
    };
    let track = a.row.saturating_sub(1);
    let parameter = ParameterId::fm_operator(operator, field).unwrap();
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => return Ok(false),
        KeyCode::Enter | KeyCode::Esc => {
            a.editor.end_coalescing();
            a.fm_operator = operator;
            a.fm_operator_field = field;
            a.mode = if return_to_parameter {
                Mode::ParameterEdit(
                    ParameterId::fm_operator(operator, FmOperatorField::Level).unwrap(),
                )
            } else {
                Mode::Navigation
            };
            a.status = "FM operator editing finished".into();
        }
        KeyCode::Left | KeyCode::Right => {
            a.editor.end_coalescing();
            let next = if k.code == KeyCode::Right {
                (operator + 1).min(3)
            } else {
                operator.saturating_sub(1)
            };
            a.fm_operator = next;
            a.mode = Mode::FmOperatorEdit {
                operator: next,
                field,
                return_to_parameter,
            };
        }
        KeyCode::Tab | KeyCode::BackTab => {
            a.editor.end_coalescing();
            let index = FmOperatorField::ALL
                .iter()
                .position(|candidate| *candidate == field)
                .unwrap_or(0);
            let next = if k.code == KeyCode::Tab {
                (index + 1) % FmOperatorField::ALL.len()
            } else {
                (index + FmOperatorField::ALL.len() - 1) % FmOperatorField::ALL.len()
            };
            a.fm_operator_field = FmOperatorField::ALL[next];
            a.mode = Mode::FmOperatorEdit {
                operator,
                field: a.fm_operator_field,
                return_to_parameter,
            };
        }
        KeyCode::PageUp | KeyCode::PageDown => {
            if k.modifiers.contains(KeyModifiers::SHIFT) {
                move_step_bank(a, k.code == KeyCode::PageDown);
            } else {
                move_step_page(a, k.code == KeyCode::PageDown);
            }
        }
        KeyCode::Char('[' | ']') => {
            let Ok(ParameterValue::FmAlgorithm(current)) =
                editor_parameter_value(a, track, ParameterId::FmAlgorithm)
            else {
                return Ok(true);
            };
            let index = FmAlgorithm::ALL
                .iter()
                .position(|candidate| *candidate == current)
                .unwrap_or(0);
            let next = if k.code == KeyCode::Char(']') {
                (index + 1).min(FmAlgorithm::ALL.len() - 1)
            } else {
                index.saturating_sub(1)
            };
            set_parameter(
                a,
                audio,
                ParameterId::FmAlgorithm,
                ParameterValue::FmAlgorithm(FmAlgorithm::ALL[next]),
                true,
                false,
            );
        }
        KeyCode::Up | KeyCode::Down => {
            let current = match editor_parameter_value(a, track, parameter) {
                Ok(value) => value,
                Err(error) => {
                    a.status = error.to_string();
                    return Ok(true);
                }
            };
            let value = match current {
                ParameterValue::FmRatio(current) => {
                    let index = FmRatio::ALL
                        .iter()
                        .position(|candidate| *candidate == current)
                        .unwrap_or(0);
                    let next = if k.code == KeyCode::Up {
                        (index + 1).min(FmRatio::ALL.len() - 1)
                    } else {
                        index.saturating_sub(1)
                    };
                    ParameterValue::FmRatio(FmRatio::ALL[next])
                }
                ParameterValue::Percent(current) => {
                    let delta = if k.modifiers.contains(KeyModifiers::SHIFT) {
                        10
                    } else {
                        1
                    };
                    let delta = if k.code == KeyCode::Up { delta } else { -delta };
                    ParameterValue::Percent(current.saturating_add(delta))
                }
                _ => return Ok(true),
            };
            set_parameter(a, audio, parameter, value, true, false);
        }
        KeyCode::Char('L')
            if matches!(field, FmOperatorField::Level | FmOperatorField::Feedback) =>
        {
            a.fm_lfo_return = Some((operator, field, return_to_parameter));
            open_lfo_editor(a, audio, parameter);
            if !matches!(a.mode, Mode::LfoEdit { .. }) {
                a.fm_lfo_return = None;
            }
        }
        KeyCode::Char(c) if matches!(field, FmOperatorField::Level | FmOperatorField::Feedback) => {
            if let Some(value) = crate::reducer::percentage_key(c) {
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
        KeyCode::Backspace | KeyCode::Delete if a.scope == Scope::Lock => {
            if audio.available_commands() == 0 {
                a.status = "Audio command queue full; edit rejected".into();
            } else {
                match a.editor.clear_parameter_lock(track, a.step, parameter) {
                    Ok(true) => {
                        sync_project(a, audio);
                        a.status = format!("Removed {} lock", parameter.display_name());
                    }
                    Ok(false) => a.status = "No lock to clear".into(),
                    Err(error) => a.status = error.to_string(),
                }
            }
        }
        KeyCode::Char('?') => a.mode = Mode::Help,
        _ => {}
    }
    Ok(true)
}

pub(super) fn finish_lfo_editor(a: &mut App, parameter: ParameterId, status: String) {
    if let Some((operator, field, return_to_parameter)) = a.fm_lfo_return.take() {
        a.fm_operator = operator;
        a.fm_operator_field = field;
        a.mode = Mode::FmOperatorEdit {
            operator,
            field,
            return_to_parameter,
        };
    } else {
        a.mode = Mode::ParameterEdit(parameter);
    }
    a.status = status;
}

pub(super) fn abandon_lfo_editor_for_help(a: &mut App) {
    a.editor.end_coalescing();
    a.fm_lfo_return = None;
    a.mode = Mode::Help;
}

pub(super) fn handle_lfo_key(a: &mut App, audio: &mut Audio, k: KeyEvent) -> Result<bool> {
    let Mode::LfoEdit { parameter, field } = a.mode.clone() else {
        return Ok(false);
    };
    let track = a.row.saturating_sub(1);
    match k.code {
        KeyCode::Char(' ') | KeyCode::Char('.') | KeyCode::Char('o') => return Ok(false),
        KeyCode::Char('?') => {
            abandon_lfo_editor_for_help(a);
            return Ok(true);
        }
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('L') => {
            a.editor.end_coalescing();
            finish_lfo_editor(a, parameter, "LFO editing finished".into());
            return Ok(true);
        }
        KeyCode::Backspace | KeyCode::Delete => {
            if set_lfo_config(a, audio, parameter, None, None) {
                finish_lfo_editor(
                    a,
                    parameter,
                    format!("Removed {} LFO", parameter.display_name()),
                );
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
        finish_lfo_editor(a, parameter, "LFO assignment is unavailable".into());
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
                LfoField::TriggerReset => {
                    let current = usize::from(!config.reset_on_trigger);
                    config.reset_on_trigger = lfo_choice_index(current, 2, k.code) == 0;
                }
                LfoField::StartPhase => {
                    config.start_phase = config.start_phase.saturating_add(percent_delta);
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
            LfoField::StartPhase => {
                config.start_phase = value;
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

fn remembered_parameter(a: &App, track: usize, bank: ParameterBank) -> ParameterFocus {
    let kind = a.editor.project.tracks[track].kind;
    let descriptors = visible_parameter_descriptors(bank, kind);
    a.remembered_parameters[track][bank.index()]
        .filter(|focus| {
            descriptors.iter().enumerate().any(|(index, descriptor)| {
                descriptor.id == focus.parameter && parameter_recipe(kind, index) == focus.recipe
            })
        })
        .unwrap_or(ParameterFocus {
            parameter: descriptors[0].id,
            recipe: parameter_recipe(kind, 0),
        })
}

pub(super) fn enter_parameter_mode(a: &mut App) {
    if a.row == 0 {
        a.status = "Select a track to edit parameters".into();
        return;
    }
    restore_parameter_edit(a, remembered_parameter(a, a.row - 1, a.parameter_bank));
}

pub(super) fn enter_lock_parameter_mode(a: &mut App) {
    if a.row == 0 {
        a.status = "Select a track to lock parameters".into();
        return;
    }
    a.scope = Scope::Lock;
    restore_parameter_edit(a, remembered_parameter(a, a.row - 1, a.parameter_bank));
}

pub(super) fn finish_parameter_edit(a: &mut App) {
    a.editor.end_coalescing();
    a.mode = Mode::Navigation;
    a.status = "Parameter editing finished".into();
}

pub(super) fn select_parameter_bank(a: &mut App, bank: ParameterBank) {
    a.editor.end_coalescing();
    a.parameter_bank = bank;
    a.parameter_recipe = DrumRecipeSlot::ONE;
    if let Mode::ParameterEdit(_) = a.mode {
        restore_parameter_edit(a, remembered_parameter(a, a.row - 1, a.parameter_bank));
        refresh_lock_recipe(a);
    }
    a.status = match a.parameter_bank {
        ParameterBank::Params => "PARAMS bank".into(),
        ParameterBank::Effects => "EFFECTS bank".into(),
    };
}

pub(super) fn enter_parameter_edit(a: &mut App, parameter: ParameterId) {
    let track = a.row.saturating_sub(1);
    let kind = a.editor.project.tracks[track].kind;
    let recipe = if is_recipe_parameter(kind, parameter) {
        if a.scope == Scope::Lock {
            selected_drum_recipe(a, track)
        } else if matches!(a.mode, Mode::ParameterEdit(_)) {
            a.parameter_recipe
        } else {
            DrumRecipeSlot::ONE
        }
    } else {
        DrumRecipeSlot::ONE
    };
    enter_parameter_focus(a, ParameterFocus { parameter, recipe });
}

fn restore_parameter_edit(a: &mut App, focus: ParameterFocus) {
    let track = a.row - 1;
    let kind = a.editor.project.tracks[track].kind;
    let recipe = if a.scope == Scope::Lock && is_recipe_parameter(kind, focus.parameter) {
        selected_drum_recipe(a, track)
    } else {
        focus.recipe
    };
    enter_parameter_focus(
        a,
        ParameterFocus {
            parameter: focus.parameter,
            recipe,
        },
    );
}

fn enter_parameter_focus(a: &mut App, focus: ParameterFocus) {
    let track = a.row.saturating_sub(1);
    let kind = a.editor.project.tracks[track].kind;
    if a.row > 0
        && visible_parameter_descriptors(a.parameter_bank, kind)
            .iter()
            .enumerate()
            .any(|(index, descriptor)| {
                descriptor.id == focus.parameter && parameter_recipe(kind, index) == focus.recipe
            })
    {
        a.remembered_parameters[track][a.parameter_bank.index()] = Some(focus);
    }
    a.parameter_recipe = focus.recipe;
    a.mode = Mode::ParameterEdit(focus.parameter);
    a.status = format!("Editing {}", focus.parameter.display_name());
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
    let kind = a.editor.project.tracks[a.row - 1].kind;
    let index = descriptors
        .iter()
        .enumerate()
        .position(|(index, descriptor)| {
            descriptor.id == current
                && (!is_recipe_parameter(kind, current)
                    || parameter_recipe(kind, index) == a.parameter_recipe)
        })
        .unwrap_or(0);
    let mut next = index;
    loop {
        next = if forward {
            (next + 1) % descriptors.len()
        } else {
            (next + descriptors.len() - 1) % descriptors.len()
        };
        let recipe = parameter_recipe(kind, next);
        if a.scope == Scope::Base
            || !is_recipe_parameter(kind, descriptors[next].id)
            || recipe == selected_drum_recipe(a, a.row - 1)
        {
            a.parameter_recipe = recipe;
            break;
        }
    }
    switch_parameter_editor(a, descriptors[next].id);
}

fn editor_parameter_value(
    a: &App,
    track: usize,
    parameter: ParameterId,
) -> Result<ParameterValue, crate::reducer::EditError> {
    let kind = a.editor.project.tracks[track].kind;
    if is_recipe_parameter(kind, parameter) {
        a.editor
            .drum_recipe_parameter_value(track, a.step, a.scope, a.parameter_recipe, parameter)
    } else {
        a.editor.parameter_value(track, a.step, a.scope, parameter)
    }
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
        .then(|| editor_parameter_value(a, track, parameter).ok())
        .flatten()
        .and_then(|value| match value {
            ParameterValue::Percent(value) => Some(value),
            ParameterValue::Waveform(_) => None,
            ParameterValue::Chorus(_) => None,
            ParameterValue::LeadSubMode(_) => None,
            ParameterValue::FmAlgorithm(_) | ParameterValue::FmRatio(_) => None,
        });
    let key = keep_editing.then_some(coalesce_key(track, step, parameter, a.parameter_recipe));
    let kind = a.editor.project.tracks[track].kind;
    let changed = if is_recipe_parameter(kind, parameter) {
        a.editor.set_drum_recipe_parameter(
            track,
            step,
            a.scope,
            (a.parameter_recipe, parameter),
            value,
            key,
        )
    } else {
        a.editor
            .set_parameter(track, step, a.scope, parameter, value, key)
    };
    match changed {
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
    recipe: DrumRecipeSlot,
) -> crate::reducer::CoalesceKey {
    crate::reducer::CoalesceKey(
        track,
        step,
        parameter as u8 + (recipe.get() - 1) * ParameterId::ALL.len() as u8,
    )
}

fn selected_recipe_shortcut(a: &App, key: char) -> Option<DrumRecipeSlot> {
    let track = a.editor.project.tracks.get(a.row.checked_sub(1)?)?;
    let recipe = DrumRecipeSlot::new(key.to_digit(10)? as u8)?;
    (matches!(track.kind, TrackKind::Hat | TrackKind::Tom)
        && recipe.get() <= track.drum_recipe_count())
    .then_some(recipe)
}

fn apply_selected_recipe(a: &mut App, audio: &mut Audio, recipe: DrumRecipeSlot) {
    let required = if a.playing { 1 } else { 2 };
    if audio.available_commands() < required {
        a.status = "Audio command queue full; recipe edit rejected".into();
        return;
    }
    let (track, step) = (a.row - 1, a.step);
    match a.editor.set_drum_recipe(track, step, recipe) {
        Ok(changed) if !changed || sync_project(a, audio) => {
            a.parameter_recipe = recipe;
            a.status = if changed {
                format!("Recipe {} applied", recipe.get())
            } else {
                format!("Recipe {} auditioned", recipe.get())
            };
            if !a.playing {
                let _ = audio.send(AudioCommand::AutoAudition {
                    track: track as u8,
                    step: step as u8,
                });
            }
            if changed {
                advance_after_event_entry(a);
            }
        }
        Ok(_) => {}
        Err(error) => a.status = error.to_string(),
    }
}

fn reset_selected_recipe_overrides(a: &mut App, audio: &mut Audio) {
    let required = if a.playing { 1 } else { 2 };
    if audio.available_commands() < required {
        a.status = "Audio command queue full; recipe reset rejected".into();
        return;
    }
    let (track, step) = (a.row - 1, a.step);
    match a.editor.clear_drum_recipe_overrides(track, step) {
        Ok(changed) if !changed || sync_project(a, audio) => {
            a.status = if changed {
                "Recipe overrides cleared".into()
            } else {
                "Recipe auditioned without overrides".into()
            };
            if !a.playing {
                let _ = audio.send(AudioCommand::AutoAudition {
                    track: track as u8,
                    step: step as u8,
                });
            }
        }
        Ok(_) => {}
        Err(error) => a.status = error.to_string(),
    }
}

fn cut_selected_step(a: &mut App, audio: &mut Audio) {
    if audio.available_commands() == 0 {
        a.status = "Audio command queue full; cut rejected".into();
        return;
    }
    let (track, step) = (a.row - 1, a.step);
    match a.editor.cut_step(track, step) {
        Ok(true) if sync_project(a, audio) => {
            a.mode = Mode::Navigation;
            a.status = format!("Cut step {}", step + 1);
        }
        Ok(true) => {}
        Ok(false) => a.status = format!("Copied empty step {}", step + 1),
        Err(error) => a.status = error.to_string(),
    }
}

fn paste_selected_step(a: &mut App, audio: &mut Audio) {
    let required = if a.playing { 1 } else { 2 };
    if audio.available_commands() < required {
        a.status = "Audio command queue full; paste rejected".into();
        return;
    }
    let (track, step) = (a.row - 1, a.step);
    match a.editor.paste_step(track, step) {
        Ok(true) if sync_project(a, audio) => {
            a.mode = Mode::Navigation;
            a.status = format!("Pasted step {}", step + 1);
            if !a.playing && a.editor.active_steps(track).unwrap()[step].is_some() {
                let _ = audio.send(AudioCommand::AutoAudition {
                    track: track as u8,
                    step: step as u8,
                });
            }
        }
        Ok(true) => {}
        Ok(false) => a.status = "No change".into(),
        Err(error) => a.status = error.to_string(),
    }
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
