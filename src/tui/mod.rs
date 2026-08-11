use crate::audio::Audio;
use crate::model::Project;
use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::stdout,
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(test)]
#[allow(unused_imports)]
use crate::{
    audio::{AudioCommand, ParameterSmoothing},
    generator::{ChordShapePool, Config as GeneratorConfig, Target as GeneratorTarget},
    model::{
        ArpeggioRate, ArpeggioType, CHORD_TRACK_INDEX, ChordShape, ChorusMode, DRUM_TRACK_COUNT,
        DelayDivision, GlobalParameterId, LEAD_TRACK_INDEX, LfoConfig, LfoDivision, LfoRate,
        LfoWaveform, MAX_STEP_COUNT, ParameterId, ParameterValue, Percent, RIMSHOT_TRACK_INDEX,
        STEP_BANK_SIZE, STEP_ROW_SIZE, SYNTH_TRACK_START, Scale, StepEvent, TRACK_COUNT, TrackKind,
        TriggerCondition, Waveform,
    },
    persistence,
    reducer::{Editor, Scope},
};
#[cfg(test)]
#[allow(unused_imports)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(test)]
#[allow(unused_imports)]
use ratatui::{
    layout::Rect,
    style::{Color, Modifier},
};

const DIRECT_PARAMETER_RAMP: Duration = Duration::from_millis(30);
const DIRECT_PERCENTAGE_HINT: &str = "[`/1–9/0] 0/10–90/100%";

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

mod controller;
mod controls;
mod input;
mod overlays;
mod render;
mod state;

#[cfg(test)]
use controls::GLOBAL_CONTROLS;
pub use state::App;
#[cfg(test)]
use state::FaderAnimation;
#[allow(unused_imports)]
pub(crate) use state::{
    ChordField, FileAction, GeneratorDialog, LfoField, Mode, ParameterBank, SidechainField,
    TriggerField,
};

#[cfg(test)]
#[allow(unused_imports)]
use controller::{
    adjusted_octave, change_octave, edit_global, enter_error, enter_global_edit, global_id,
    global_name, global_shortcut, handle_file_input, handle_global_key, handle_new_confirm,
    handle_open_confirm, handle_project_browser, handle_sidechain_key, handle_tempo_input,
    list_projects, move_global_editor, new_project, open_project, project_browser_mode,
    project_directory, project_path_for_name, request_new_project, reset_project_ui, save,
    save_as_mode, save_path_for_name, sync_project, sync_project_with_smoothing,
};
#[cfg(test)]
#[allow(unused_imports)]
use input::{
    active_parameter_shortcut, adjacent_pattern_in_count, apply, change_generator_value,
    clamp_parameter_percentage, clear_selected_track, coalesce_key, commit_pattern,
    duplicate_selected_track, enter_parameter_edit, flipped_waveform, generator_config,
    global_jump, handle_chord_key, handle_generator_dialog, handle_key, handle_lfo_key,
    handle_parameter_key, handle_pattern_dialog, handle_swing_key, handle_track_length_input,
    handle_track_probability_key, handle_trigger_key, is_clear_track_shortcut, lfo_choice_index,
    move_chord_editor_step, move_generator_field, move_generator_tab, move_parameter_editor,
    move_step, move_step_bank, move_step_page, move_step_vertical, normalize_cursor,
    open_chord_editor, open_lfo_editor, parameter_edit_passthrough, parameter_shortcut,
    parameter_supports_direct_percentage, parameter_upper_bound, pattern_edit_at, select_global,
    select_track, set_lfo_config, set_parameter, set_selected_track_length,
    switch_parameter_editor, toggle_parameter_bank, track_jump_index,
};
#[cfg(test)]
#[allow(unused_imports)]
use overlays::{
    generator_popup_rect, lfo_inactive_style, lfo_popup_rect, pattern_is_empty, popup, popup_at,
    popup_rect, probability_popup_rect, project_browser_popup_rect, quit_popup_rect,
    render_chord_control, render_chord_popup, render_generator_popup, render_lfo_control,
    render_lfo_fader, render_lfo_popup, render_lfo_selector, render_lfo_switch,
    render_pattern_popup, render_project_browser, render_trigger_popup, swing_popup_rect,
    tempo_popup_rect, trigger_popup_rect,
};
#[cfg(test)]
#[allow(unused_imports)]
use render::{
    ParameterDescriptor, ParameterGroup, ValueOrigin, articulation_title, displayed_parameter,
    displayed_parameter_for_recipe, draw_with_device, effect_descriptors, fader_segments,
    global_control_text, global_display_name, global_fader_segments, global_shortcut_text,
    global_value_text, help_available, lock_has_parameter, mode_name, parameter_descriptors,
    physical_parameter_readout, render_centered, render_global_cards, render_parameter_bank,
    render_pitch_lfo_card, scope_name, selected_accent, selected_chord_shape, step_cell,
    track_label,
};

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
        controller::refresh_audio_status(&mut app, audio);
        app.fader_animations
            .retain(|animation| !animation.is_complete(Instant::now()));
        terminal.draw(|f| render::draw(f, &app, audio))?;
        if event::poll(Duration::from_millis(8))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == event::KeyEventKind::Press {
                    input::handle_key(&mut app, audio, k)?
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
