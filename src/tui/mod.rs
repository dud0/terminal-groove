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
    generator::{Config as GeneratorConfig, Target as GeneratorTarget},
    model::{
        ArpeggioRate, ArpeggioType, ChordShape, ChorusMode, DelayDivision, GlobalParameterId,
        LfoConfig, LfoDivision, LfoRate, LfoWaveform, MAX_STEP_COUNT, ParameterId, ParameterValue,
        Percent, STEP_BANK_SIZE, STEP_ROW_SIZE, Scale, StepEvent, TRACK_COUNT, TrackKind,
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
mod input;
mod overlays;
mod render;
mod state;

pub use state::App;
#[cfg(test)]
use state::FaderAnimation;
#[allow(unused_imports)]
pub(crate) use state::{
    ChordField, FileAction, GeneratorDialog, LfoField, Mode, ParameterBank, TriggerField,
};

#[cfg(test)]
#[allow(unused_imports)]
use controller::{
    adjusted_octave, change_octave, edit_global, enter_error, enter_global_edit, global_id,
    global_name, global_shortcut, handle_file_input, handle_global_key, handle_new_confirm,
    handle_open_confirm, handle_tempo_input, move_global_editor, new_project, open_project,
    refresh_audio_status, request_new_project, reset_project_ui, resolved_path, save, sync_project,
    sync_project_with_smoothing,
};
#[cfg(test)]
#[allow(unused_imports)]
use input::{
    active_parameter_shortcut, adjacent_pattern_in_count, apply, change_generator_value,
    coalesce_key, commit_pattern, duplicate_selected_track, enter_parameter_edit, flipped_waveform,
    global_jump, handle_chord_key, handle_generator_dialog, handle_key, handle_lfo_key,
    handle_parameter_key, handle_pattern_dialog, handle_swing_key, handle_track_length_input,
    handle_trigger_key, lfo_choice_index, move_chord_editor_step, move_generator_field,
    move_parameter_editor, move_step, move_step_bank, move_step_page, move_step_vertical,
    normalize_cursor, open_chord_editor, open_lfo_editor, parameter_edit_passthrough,
    parameter_shortcut, parameter_supports_direct_percentage, pattern_edit_at, select_global,
    select_track, set_lfo_config, set_parameter, set_selected_track_length,
    switch_parameter_editor, toggle_parameter_bank, track_jump_index,
};
#[cfg(test)]
#[allow(unused_imports)]
use overlays::{
    generator_popup_rect, lfo_inactive_style, lfo_popup_rect, pattern_is_empty, popup, popup_at,
    popup_rect, quit_popup_rect, render_chord_control, render_chord_popup, render_generator_popup,
    render_lfo_control, render_lfo_fader, render_lfo_popup, render_lfo_selector, render_lfo_switch,
    render_pattern_popup, render_trigger_popup, swing_popup_rect, tempo_popup_rect,
    trigger_popup_rect,
};
#[cfg(test)]
#[allow(unused_imports)]
use render::{
    GLOBAL_IDS, ParameterDescriptor, ParameterGroup, ValueOrigin, articulation_title,
    displayed_parameter, draw_with_device, effect_descriptors, fader_segments, global_control_text,
    global_display_name, global_fader_segments, global_shortcut_text, global_value_text,
    help_available, lock_has_parameter, mode_name, parameter_descriptors,
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

    fn rendered_lines(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_with_device(frame, app, "null"))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
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
        assert_eq!(synth.len(), 17);
        assert_eq!(synth[4].id, ParameterId::OscillatorMix);
        assert_eq!(synth[4].group, ParameterGroup::Instrument);
        assert_eq!(synth[5].id, ParameterId::PulseWidth);
        assert_eq!(synth[7].id, ParameterId::Chorus);
        assert_eq!(synth[9].id, ParameterId::Pitch);
        assert_eq!(synth[9].shortcut, "i");
        assert_eq!(synth[10].group, ParameterGroup::Filter);
        assert_eq!(synth[11].shortcut, "R");
        assert_eq!(synth[13].group, ParameterGroup::Envelope);
        let lead = parameter_descriptors(TrackKind::Lead);
        assert_eq!(
            lead.iter()
                .map(|descriptor| descriptor.id)
                .collect::<Vec<_>>(),
            vec![
                ParameterId::Level,
                ParameterId::DelaySend,
                ParameterId::ReverbSend,
                ParameterId::Pan,
                ParameterId::OscillatorMix,
                ParameterId::PulseWidth,
                ParameterId::SubOscillator,
                ParameterId::Pitch,
                ParameterId::Cutoff,
                ParameterId::Resonance,
                ParameterId::FilterEnvelope,
                ParameterId::Attack,
                ParameterId::Decay,
                ParameterId::Sustain,
                ParameterId::Release,
            ]
        );
        assert_eq!(lead[7].group, ParameterGroup::Instrument);
        assert_eq!(lead[8].group, ParameterGroup::Filter);
        assert_eq!(lead[11].group, ParameterGroup::Envelope);
        assert_ne!(
            ParameterGroup::Mixer.color(),
            ParameterGroup::Filter.color()
        );
        let effects = effect_descriptors();
        assert_eq!(effects.len(), 7);
        assert_eq!(effects[0].id, ParameterId::DistortionDrive);
        assert_eq!(effects[2].shortcut, "x");
        assert_eq!(effects[3].id, ParameterId::PhaserRate);
        assert_eq!(effects[6].shortcut, "M");
    }

    #[test]
    fn tab_switches_parameter_bank_and_selects_first_control_while_editing() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.mode = Mode::ParameterEdit(ParameterId::Level);
        toggle_parameter_bank(&mut app);
        assert_eq!(app.parameter_bank, ParameterBank::Effects);
        assert_eq!(app.mode, Mode::ParameterEdit(ParameterId::DistortionDrive));
        toggle_parameter_bank(&mut app);
        assert_eq!(app.parameter_bank, ParameterBank::Params);
        assert_eq!(app.mode, Mode::ParameterEdit(ParameterId::Level));
    }

    #[test]
    fn effects_bank_does_not_claim_pitched_note_keys() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.parameter_bank = ParameterBank::Effects;

        for key in '1'..='8' {
            assert_eq!(active_parameter_shortcut(&app, key), None);
        }
        assert_eq!(
            active_parameter_shortcut(&app, 'd'),
            Some(ParameterId::DistortionDrive)
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
    fn pattern_delete_normalizes_step_for_the_new_active_pattern() {
        let mut project = Project::new();
        let mut short_pattern = project.patterns[0].clone();
        short_pattern.tracks[0].steps.resize(4, None);
        project.patterns.push(short_pattern);
        let mut app = App::new(project, None);
        app.row = 1;
        app.step = 15;
        app.pattern_cursor = 0;

        app.editor.delete_pattern_at(0).unwrap();
        normalize_cursor(&mut app);

        assert_eq!(app.step, 3);
        let _ = rendered(&app, 120, 34);
    }

    #[test]
    fn pattern_dialog_renders_horizontal_plain_numbered_states() {
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns.push(project.patterns[0].clone());
        project.patterns[1].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.mode = Mode::PatternDialog;
        app.pattern_cursor = 2;
        app.active_pattern = 0;
        app.queued_pattern = Some(1);

        let screen = rendered(&app, 120, 34);
        assert!(screen.lines().next().unwrap().contains("Patterns (3)"));
        assert!(screen.contains("Patterns (3)"));
        assert!(screen.contains("1") && screen.contains("2") && screen.contains("3"));
        assert!(screen.contains("▶") && screen.contains("⏭"));
        assert!(screen.contains("Enter select (queue while playing)"));
        assert!(!screen.contains("P001"));
        assert!(!screen.contains("used"));
    }

    #[test]
    fn generator_dialog_field_arrows_clamp_and_values_change_horizontally() {
        assert_eq!(move_generator_field(0, true), 0);
        assert_eq!(move_generator_field(0, false), 1);
        assert_eq!(move_generator_field(4, false), 5);
        assert_eq!(move_generator_field(5, false), 5);
        assert_eq!(move_generator_field(5, true), 4);

        let mut dialog = GeneratorDialog {
            target: GeneratorTarget::WholePattern,
            track: 0,
            seed: "123".into(),
            density: Percent::new(48).unwrap(),
            ties: Percent::new(18).unwrap(),
            accents: Percent::new(24).unwrap(),
            field: 3,
        };
        change_generator_value(&mut dialog, true);
        assert_eq!(dialog.density, Percent::new(53).unwrap());
        change_generator_value(&mut dialog, false);
        assert_eq!(dialog.density, Percent::new(48).unwrap());

        dialog.field = 4;
        change_generator_value(&mut dialog, false);
        assert_eq!(dialog.ties, Percent::new(13).unwrap());
        dialog.field = 5;
        dialog.accents = Percent::new(100).unwrap();
        change_generator_value(&mut dialog, true);
        assert_eq!(dialog.accents, Percent::new(100).unwrap());

        dialog.field = 0;
        change_generator_value(&mut dialog, true);
        assert_eq!(dialog.target, GeneratorTarget::Track(0));
        dialog.field = 1;
        change_generator_value(&mut dialog, true);
        assert_eq!(dialog.track, 1);
        assert_eq!(dialog.target, GeneratorTarget::Track(1));
    }

    #[test]
    fn generator_dialog_highlights_the_selected_field() {
        let mut app = App::new(Project::new(), None);
        app.mode = Mode::GeneratorDialog(GeneratorDialog {
            target: GeneratorTarget::WholePattern,
            track: 0,
            seed: "123".into(),
            density: Percent::new(48).unwrap(),
            ties: Percent::new(18).unwrap(),
            accents: Percent::new(24).unwrap(),
            field: 4,
        });

        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("> Ties"));
        assert!(screen.contains("[↑/↓]/[Tab] field"));
        assert!(!screen.contains("> Target"));
    }

    #[test]
    fn trigger_dialog_renders_horizontal_cards() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: TriggerCondition::Cycle {
                position: 1,
                length: 2,
            },
            retrigger_count: 1,
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.row = 1;
        app.mode = Mode::TriggerEdit {
            field: TriggerField::CycleLength,
        };

        let lines = rendered_lines(&app, 120, 34);
        assert!(lines.iter().any(|line| {
            ["Mode", "Phase", "Length", "Chance", "Retrigger"]
                .iter()
                .all(|label| line.contains(label))
        }));
        let screen = lines.join("");
        assert!(screen.contains("Trigger · Step 1"));
        assert!(screen.contains("[←/→] select"));
        assert!(screen.contains("50%"));
        assert!(screen.contains("···"));
    }

    #[test]
    fn trigger_selector_arrows_follow_the_visual_order() {
        let choices = ["Always", "Cycle", "Chance"];
        assert_eq!(
            choices[lfo_choice_index(1, choices.len(), KeyCode::Up)],
            "Always"
        );
        assert_eq!(
            choices[lfo_choice_index(1, choices.len(), KeyCode::Down)],
            "Chance"
        );
        assert_eq!(lfo_choice_index(0, choices.len(), KeyCode::Up), 0);
        assert_eq!(lfo_choice_index(2, choices.len(), KeyCode::Down), 2);
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
    fn tilde_global_jump_selects_the_global_row() {
        assert!(global_jump(KeyEvent::new(
            KeyCode::Char('~'),
            KeyModifiers::SHIFT
        )));
        assert!(!global_jump(KeyEvent::new(
            KeyCode::Char('`'),
            KeyModifiers::SHIFT
        )));

        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.scope = Scope::Lock;
        app.mode = Mode::ParameterEdit(ParameterId::Cutoff);
        select_global(&mut app);

        assert_eq!(app.row, 0);
        assert_eq!(app.scope, Scope::Base);
        assert_eq!(app.mode, Mode::Navigation);
    }

    #[test]
    fn track_jump_clamps_step_and_replaces_incompatible_parameter() {
        let mut project = Project::new();
        project.patterns[0].tracks[0].steps.resize(4, None);
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
    fn direct_percentage_entry_excludes_discrete_parameters() {
        assert!(parameter_supports_direct_percentage(ParameterId::Level));
        assert!(!parameter_supports_direct_percentage(ParameterId::Waveform));
        assert!(!parameter_supports_direct_percentage(ParameterId::Chorus));
        assert!(!parameter_supports_direct_percentage(ParameterId::Spread));
        assert!(!parameter_supports_direct_percentage(ParameterId::Pitch));
    }

    #[test]
    fn pitch_lfo_parameter_card_advertises_assignment_removal() {
        let mut project = Project::new();
        project.tracks[4].lfos.pitch = Some(LfoConfig::default());
        let mut app = App::new(project, None);
        app.row = 5;
        app.mode = Mode::ParameterEdit(ParameterId::Pitch);

        let screen = rendered(&app, 220, 34);
        assert!(screen.contains("[Backspace/Del] remove LFO"));
    }

    #[test]
    fn pitch_lfo_card_is_visible_for_chord_and_lead_only() {
        let mut project = Project::new();
        project.tracks[4].lfos.pitch = Some(LfoConfig {
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        });
        let mut app = App::new(project, None);
        app.row = 5;
        let chord = rendered(&app, 220, 34);
        assert!(chord.contains("Pitch"));
        assert!(chord.contains("[i]"));
        assert!(chord.contains("100%"));
        assert!(chord.contains("±2st"));

        app.row = 6;
        let lead = rendered(&app, 220, 34);
        assert!(lead.contains("Pitch"));
        app.row = 1;
        let kick = rendered(&app, 220, 34);
        assert!(!kick.contains("Pitch"));
    }

    #[test]
    fn help_hint_is_limited_to_supported_modes() {
        assert!(help_available(&Mode::Navigation));
        assert!(help_available(&Mode::ParameterEdit(ParameterId::Level)));
        assert!(help_available(&Mode::LfoEdit {
            parameter: ParameterId::Level,
            field: LfoField::Depth,
        }));
        assert!(help_available(&Mode::ChordEdit {
            shape: ChordShape::TriadRoot,
        }));
        assert!(!help_available(&Mode::PatternDialog));
        assert!(!help_available(&Mode::GlobalEdit(GlobalParameterId::Key)));
        assert!(!help_available(&Mode::TempoInput(String::new())));
    }

    #[test]
    fn accent_readout_resolves_ties_to_their_source_note() {
        let mut app = App::new(Project::new(), None);
        app.row = 4;
        app.editor.set_note(3, 0, 1).unwrap();
        app.editor.toggle_accent(3, 0).unwrap();
        assert_eq!(
            articulation_title(&app, 3),
            "[A] Accent on · [Shift+G] Slide off"
        );
        app.editor.toggle_tie(3, 1).unwrap();
        assert_eq!(selected_accent(&app, 3), Some((true, None)));
        app.step = 1;
        assert_eq!(selected_accent(&app, 3), Some((true, Some(0))));
        assert_eq!(articulation_title(&app, 3), "Accent on from step 1");
    }

    #[test]
    fn empty_accent_readout_shows_the_track_default() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.editor.toggle_accent(0, 0).unwrap();

        assert_eq!(selected_accent(&app, 0), Some((true, None)));
        assert_eq!(articulation_title(&app, 0), "[A] Accent default on");
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
    fn global_faders_normalize_to_their_model_bounds() {
        let mut globals = Project::new().globals;
        globals.reverb_time_seconds = 0.2;
        assert_eq!(
            global_fader_segments(&globals, GlobalParameterId::ReverbTime),
            Some(0)
        );
        globals.reverb_time_seconds = 10.0;
        assert_eq!(
            global_fader_segments(&globals, GlobalParameterId::ReverbTime),
            Some(10)
        );
        globals.reverb_pre_delay_ms = 0;
        assert_eq!(
            global_fader_segments(&globals, GlobalParameterId::ReverbPreDelay),
            Some(0)
        );
        globals.reverb_pre_delay_ms = 200;
        assert_eq!(
            global_fader_segments(&globals, GlobalParameterId::ReverbPreDelay),
            Some(10)
        );
    }

    #[test]
    fn screen_renders_fader_bank_and_local_shortcuts_at_minimum_size() {
        let backend = TestBackend::new(120, 34);
        let mut project = Project::new();
        project.tracks[3].input_octave = Some(4);
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[3].steps[1] = Some(StepEvent::Note {
            degree: 2,
            octave: 4,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
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
        assert!(screen.contains("[?] Help"));

        let mut modal = App::new(Project::new(), None);
        modal.mode = Mode::TempoInput(String::new());
        let modal_screen = rendered(&modal, 119, 34);
        assert!(!modal_screen.contains("[?] Help"));
    }

    #[test]
    fn help_overlay_groups_contextual_shortcuts_and_direct_percentage_mapping() {
        let mut app = App::new(Project::new(), None);
        app.mode = Mode::Help;
        let screen = rendered(&app, 240, 60);

        assert!(screen.contains("PATTERNS  Ctrl+P open dialog"));
        assert!(screen.contains("NAVIGATION  ↑/↓ rows"));
        assert!(screen.contains("EVENTS & TRACKS  p BASE/LOCK"));
        assert!(screen.contains("PARAMETERS  v level"));
        assert!(screen.contains("GLOBAL  t tempo"));
        assert!(screen.contains("Ctrl+Shift+S save as"));
        assert!(screen.contains("o audition selected step"));
        assert!(screen.contains("[`/1–9/0] percent"));
        assert!(screen.contains("Backspace/Delete remove lock/LFO"));
        assert!(screen.contains("Esc close help"));

        let minimum_screen = rendered(&app, 120, 34);
        assert!(minimum_screen.contains("GLOBAL  t tempo"));
        assert!(minimum_screen.contains("Esc in navigation resets scope to BASE"));
    }

    #[test]
    fn metadata_header_is_one_line_without_persistent_shortcut_hints() {
        let app = App::new(Project::new(), None);
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("terminal-groove"));
        assert!(screen.contains("audio: null"));
        assert!(!screen.contains("[Ctrl+P] Patterns"));
        assert!(!screen.contains("[Space] Play/Pause"));
    }

    #[test]
    fn persistent_audio_overload_badge_preserves_transient_status() {
        let clean = App::new(Project::new(), None);
        assert!(!rendered(&clean, 120, 34).contains("audio overload"));

        let mut overloaded = App::new(Project::new(), None);
        overloaded.callback_overruns = 3;
        overloaded.max_callback_load_per_mille = 1_270;
        overloaded.status = "Edit feedback remains visible".into();
        let screen = rendered(&overloaded, 120, 34);
        assert!(screen.contains("⚠ audio overload: 3, max 127%"));
        assert!(screen.contains("Edit feedback remains visible"));
    }

    #[test]
    fn parameter_editor_hints_match_scope_and_parameter_capabilities() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.mode = Mode::ParameterEdit(ParameterId::Level);
        let base = rendered(&app, 220, 34);
        assert!(base.contains(DIRECT_PERCENTAGE_HINT));
        assert!(base.contains("[Shift+L] LFO"));
        assert!(!base.contains("remove lock"));

        app.scope = Scope::Lock;
        let locked = rendered(&app, 220, 34);
        assert!(locked.contains("[Backspace/Del] remove lock"));

        app.row = 5;
        app.scope = Scope::Base;
        app.mode = Mode::ParameterEdit(ParameterId::Spread);
        let spread = rendered(&app, 220, 34);
        assert!(!spread.contains(DIRECT_PERCENTAGE_HINT));
        assert!(!spread.contains("[Shift+L] LFO"));
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
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::SeventhFirstInversion),
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        let mut app = App::new(project, None);
        app.row = 5;
        let title_screen = rendered(&app, 120, 34);
        assert!(title_screen.contains("[C] Chord trigger 3-5-7-1"));
        app.mode = Mode::ChordEdit {
            shape: ChordShape::SeventhFirstInversion,
        };
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Chord · Step 1"));
        assert!(screen.contains("Shape"));
        assert!(screen.contains("3-5-7-1"));
        assert!(screen.contains("[←/→] select"));
    }

    #[test]
    fn chord_shape_editor_page_navigation_follows_each_step() {
        let mut project = Project::new();
        project.patterns[0].tracks[4].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[0].tracks[4].steps[1] = Some(StepEvent::Note {
            degree: 2,
            octave: 3,
            accent: false,
            chord_shape: Some(ChordShape::Sus4SecondInversion),
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
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
    fn lfo_modal_is_lower_and_capped_on_large_terminals() {
        assert_eq!(
            lfo_popup_rect(Rect::new(0, 0, 120, 34)),
            Rect::new(14, 9, 92, 20)
        );
        assert_eq!(
            lfo_popup_rect(Rect::new(0, 0, 200, 50)),
            Rect::new(54, 25, 92, 20)
        );
    }

    #[test]
    fn compact_dialog_rectangles_fit_their_content() {
        let area = Rect::new(0, 0, 120, 34);
        assert_eq!(trigger_popup_rect(area), Rect::new(14, 9, 92, 20));
        assert_eq!(generator_popup_rect(area), Rect::new(31, 11, 58, 12));
        assert_eq!(swing_popup_rect(area), Rect::new(36, 14, 48, 6));

        assert_eq!(
            trigger_popup_rect(Rect::new(0, 0, 200, 50)),
            Rect::new(54, 25, 92, 20)
        );
        assert_eq!(
            generator_popup_rect(Rect::new(0, 0, 200, 50)),
            Rect::new(71, 19, 58, 12)
        );
        assert_eq!(
            swing_popup_rect(Rect::new(0, 0, 200, 50)),
            Rect::new(76, 22, 48, 6)
        );
    }

    #[test]
    fn compact_swing_and_generator_dialogs_render_without_wrapping() {
        let mut app = App::new(Project::new(), None);
        app.row = 1;
        app.mode = Mode::SwingEdit;
        let swing = rendered_lines(&app, 120, 34);
        assert!(
            swing
                .iter()
                .any(|line| line.contains("0–75% · applies to offbeat sixteenths"))
        );

        app.mode = Mode::GeneratorDialog(GeneratorDialog {
            target: GeneratorTarget::WholePattern,
            track: 0,
            seed: "123".into(),
            density: Percent::new(48).unwrap(),
            ties: Percent::new(18).unwrap(),
            accents: Percent::new(24).unwrap(),
            field: 3,
        });
        let generator = rendered_lines(&app, 120, 34);
        assert!(
            generator
                .iter()
                .any(|line| line.contains("[↑/↓]/[Tab] field  [←→] change  type seed"))
        );
    }

    #[test]
    fn quit_popup_fits_confirmation_prompt() {
        assert_eq!(
            quit_popup_rect(Rect::new(0, 0, 120, 34)),
            Rect::new(41, 15, 37, 3)
        );
    }

    #[test]
    fn tempo_popup_fits_its_content() {
        assert_eq!(
            tempo_popup_rect(Rect::new(0, 0, 120, 34)),
            Rect::new(21, 15, 77, 4)
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
    fn pitch_lfo_modal_shows_physical_depth_range() {
        let mut project = Project::new();
        project.tracks[4].lfos.pitch = Some(LfoConfig {
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        });
        let mut app = App::new(project, None);
        app.row = 5;
        app.mode = Mode::LfoEdit {
            parameter: ParameterId::Pitch,
            field: LfoField::Depth,
        };
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("Track LFO · pitch"));
        assert!(screen.contains("100% · ±2.0 st"));
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
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
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
        project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
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
    fn kick_decay_readout_matches_exponential_dsp_mapping() {
        let mut app = App::new(Project::new(), None);
        app.editor.project.tracks[0]
            .set_parameter(ParameterId::Decay, ParameterValue::Percent(Percent::ZERO));
        assert_eq!(
            physical_parameter_readout(&app, 0, 0, ParameterId::Decay),
            "80 ms · BASE"
        );

        app.editor.project.tracks[0].set_parameter(
            ParameterId::Decay,
            ParameterValue::Percent(Percent::new(50).unwrap()),
        );
        assert_eq!(
            physical_parameter_readout(&app, 0, 0, ParameterId::Decay),
            "310 ms · BASE"
        );

        app.editor.project.tracks[0].set_parameter(
            ParameterId::Decay,
            ParameterValue::Percent(Percent::new(100).unwrap()),
        );
        assert_eq!(
            physical_parameter_readout(&app, 0, 0, ParameterId::Decay),
            "1200 ms · BASE"
        );
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
        project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            arpeggio: crate::model::ArpeggioConfig::default(),
            condition: Default::default(),
            retrigger_count: 1,
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
        let mut app = App::new(Project::new(), None);
        app.mode = Mode::GlobalEdit(GlobalParameterId::Tempo);
        let screen = rendered(&app, 120, 34);
        for key in ["[t]", "[y]", "[f]", "[r]", "[b]", "[p]", "[k]", "[s]"] {
            assert!(screen.contains(key), "missing {key}");
        }
        for value in ["Tempo", "1/8", "30%", "2.5 s", "40%", "20 ms", "C", "Major"] {
            assert!(screen.contains(value), "missing {value}");
        }
        assert!(screen.contains("███"));
        assert!(screen.contains("● 1/8"));
        assert!(screen.contains("● C"));
        assert!(screen.contains("● Major"));

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
                .enumerate()
                .any(|(index, cell)| {
                    index / 120 >= 14 && cell.modifier.contains(Modifier::REVERSED)
                })
        );
    }

    #[test]
    fn global_navigation_row_omits_local_shortcuts() {
        let text = global_control_text(&Project::new().globals);
        assert!(text.iter().all(|control| !control.starts_with('[')));
        assert!(text.iter().all(|control| !control.contains("] ")));
    }

    #[test]
    fn global_shortcuts_enter_editing_for_every_control() {
        let mut app = App::new(Project::new(), None);
        for id in GLOBAL_IDS {
            enter_global_edit(&mut app, id);
            match id {
                GlobalParameterId::Tempo => assert_eq!(app.mode, Mode::TempoInput(String::new())),
                _ => assert_eq!(app.mode, Mode::GlobalEdit(id)),
            }
        }
    }

    #[test]
    fn global_editor_left_and_right_cycle_controls() {
        let mut app = App::new(Project::new(), None);
        app.global = 0;
        app.mode = Mode::GlobalEdit(GlobalParameterId::Tempo);

        move_global_editor(&mut app, false);
        assert_eq!(app.global, GLOBAL_IDS.len() - 1);
        assert_eq!(app.mode, Mode::GlobalEdit(GlobalParameterId::Scale));

        move_global_editor(&mut app, true);
        assert_eq!(app.global, 0);
        assert_eq!(app.mode, Mode::TempoInput(String::new()));
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
                arpeggio: crate::model::ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
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
                arpeggio: crate::model::ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
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
        assert_eq!(mode_name(&Mode::NewConfirm), "Unsaved confirmation");
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
    fn new_project_requests_confirmation_only_when_dirty() {
        let mut app = App::new(Project::new(), None);
        assert!(request_new_project(&mut app));
        assert_eq!(app.mode, Mode::Navigation);

        app.editor
            .edit(None, |project| {
                project.tracks[0].muted = !project.tracks[0].muted;
                Ok(())
            })
            .unwrap();
        assert!(!request_new_project(&mut app));
        assert_eq!(app.mode, Mode::NewConfirm);
    }

    #[test]
    fn entering_error_clears_pending_project_continuations() {
        let mut app = App::new(Project::new(), None);
        app.pending_open = Some("next.groove.json".into());
        app.pending_new = true;
        app.pending_quit = true;

        enter_error(&mut app, "save failed");

        assert!(matches!(app.mode, Mode::Error(ref message) if message == "save failed"));
        assert!(app.pending_open.is_none());
        assert!(!app.pending_new);
        assert!(!app.pending_quit);
    }

    #[test]
    fn new_project_confirmation_renders_save_discard_and_cancel() {
        let mut app = App::new(Project::new(), None);
        app.mode = Mode::NewConfirm;
        let screen = rendered(&app, 120, 34);
        assert!(screen.contains("New project"));
        assert!(screen.contains("Save [S]"));
        assert!(screen.contains("Discard [D]"));
        assert!(screen.contains("Cancel [Esc]"));
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
        project.patterns[0].tracks[0].steps.resize(64, None);
        project.patterns[0].tracks[1].steps.resize(20, None);
        project.patterns[0].tracks[2].steps.resize(40, None);
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
        project.patterns[0].tracks[0].steps.resize(20, None);
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
        for track in &mut project.patterns[0].tracks {
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
