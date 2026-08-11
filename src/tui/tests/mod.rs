use super::*;
use super::{controller::*, controls::GLOBAL_CONTROLS, input::*, overlays::*, render::*, state::*};
use crate::{
    generator::{ChordShapePool, Target as GeneratorTarget},
    model::{
        CHORD_TRACK_INDEX, ChordShape, DRUM_TRACK_COUNT, GlobalParameterId, LEAD_TRACK_INDEX,
        LfoConfig, LfoDivision, LfoRate, Microtiming, ParameterId, ParameterValue, Percent,
        RIMSHOT_TRACK_INDEX, STEP_BANK_SIZE, SYNTH_TRACK_START, StepEvent, TRACK_COUNT, TrackKind,
        TriggerCondition, Waveform,
    },
    reducer::Scope,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier},
};

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
fn parameter_banks_are_contextual_and_model_compatible() {
    let cases = [
        (TrackKind::Kick, 7),
        (TrackKind::Snare, 7),
        (TrackKind::Hat, 8),
        (TrackKind::Tom, 13),
        (TrackKind::Cymbal, 7),
        (TrackKind::Rimshot, 7),
        (TrackKind::Bass, 9),
        (TrackKind::Chord, 18),
        (TrackKind::Lead, 19),
    ];
    let common = [
        ParameterId::Level,
        ParameterId::DelaySend,
        ParameterId::ReverbSend,
        ParameterId::Pan,
    ];

    for (kind, expected_len) in cases {
        let descriptors = parameter_descriptors(kind);
        assert_eq!(descriptors.len(), expected_len, "{kind:?}");
        assert_eq!(
            descriptors
                .iter()
                .take(common.len())
                .map(|descriptor| descriptor.id)
                .collect::<Vec<_>>(),
            common,
            "{kind:?}",
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| descriptor.id.is_valid_for(kind)
                    || descriptor.id == ParameterId::Pitch && descriptor.id.supports_lfo(kind)),
            "{kind:?}",
        );
    }

    let effects = effect_descriptors();
    assert_eq!(effects.len(), 12);
    assert!(effects.iter().all(|descriptor| {
        descriptor.id.is_valid_for(TrackKind::Kick) && !descriptor.id.supports_lfo(TrackKind::Kick)
    }));
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
    app.row = SYNTH_TRACK_START + 1;
    app.parameter_bank = ParameterBank::Effects;

    for key in '1'..='8' {
        assert_eq!(active_parameter_shortcut(&app, key), None);
    }
    assert_eq!(
        active_parameter_shortcut(&app, 'd'),
        Some(ParameterId::DistortionDrive)
    );
    assert_eq!(
        active_parameter_shortcut(&app, 'q'),
        Some(ParameterId::FlangerDelay)
    );
    assert_eq!(parameter_upper_bound(ParameterId::FlangerFeedback), 90);
}

#[test]
fn flanger_readouts_show_physical_units() {
    let mut app = App::new(Project::new(), None);
    app.row = 1;
    assert_eq!(
        physical_parameter_readout(&app, 0, 0, ParameterId::FlangerDelay),
        "2.0 ms center · 0.1–3.8 ms range · BASE"
    );
    assert_eq!(
        physical_parameter_readout(&app, 0, 0, ParameterId::FlangerDepth),
        "±1.9 ms effective · 0.1–3.8 ms range · BASE"
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

    app.row = SYNTH_TRACK_START + 1;
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

    app.editor.delete_pattern(0).unwrap();
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
        recipe: crate::model::DrumRecipeSlot::ONE,
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
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
fn song_page_renders_transport_progress_and_inline_controls() {
    let mut project = Project::new();
    project.patterns.push(project.patterns[0].clone());
    project.song = vec![
        crate::model::SongEntry {
            pattern: 1,
            bars: 2,
        },
        crate::model::SongEntry {
            pattern: 2,
            bars: 3,
        },
    ];
    let mut app = App::new(project, None);
    app.mode = Mode::PatternDialog;
    app.pattern_page = PatternPage::Song;
    app.song_cursor = 1;
    app.song_mode = true;
    app.active_song = 0;
    app.song_bar = 1;

    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("Song (2) · SONG"));
    assert!(screen.contains("P001×02") && screen.contains("P002×03"));
    assert!(screen.contains("Playing entry 1 · bar 2/2"));
    assert!(screen.contains("[/] pattern"));
}

#[test]
fn generator_dialog_field_arrows_clamp_and_values_change_horizontally() {
    assert_eq!(move_generator_field(0, true), 0);
    assert_eq!(move_generator_field(0, false), 1);
    assert_eq!(move_generator_field(4, false), 5);
    assert_eq!(move_generator_field(8, false), 9);
    assert_eq!(move_generator_field(9, false), 9);
    assert_eq!(move_generator_field(9, true), 8);
    assert_eq!(move_generator_tab(9, false), 0);
    assert_eq!(move_generator_tab(0, true), 9);

    let mut dialog = GeneratorDialog {
        target: GeneratorTarget::WholePattern,
        track: 0,
        seed: "123".into(),
        density: Percent::new(48).unwrap(),
        range_low: 2,
        range_high: 6,
        chord_shapes: ChordShapePool::AllShapes,
        ties: Percent::new(18).unwrap(),
        accents: Percent::new(24).unwrap(),
        slides: Percent::new(18).unwrap(),
        field: 3,
    };
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.density, Percent::new(53).unwrap());
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.density, Percent::new(48).unwrap());

    dialog.field = 4;
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.range_low, 3);
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.range_low, 2);
    dialog.range_low = 0;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.range_low, 0);
    dialog.range_low = 6;
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.range_low, 6);

    dialog.field = 5;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.range_high, 6);
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.range_high, 7);
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.range_high, 7);
    dialog.range_high = 2;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.range_high, 6);

    dialog.range_low = 2;
    dialog.range_high = 6;
    dialog.field = 6;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.chord_shapes, ChordShapePool::RootShapes);
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.chord_shapes, ChordShapePool::Default);
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.chord_shapes, ChordShapePool::Default);
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.chord_shapes, ChordShapePool::RootShapes);

    dialog.field = 7;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.ties, Percent::new(13).unwrap());
    dialog.field = 8;
    dialog.accents = Percent::new(100).unwrap();
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.accents, Percent::new(100).unwrap());
    dialog.field = 9;
    dialog.slides = Percent::ZERO;
    change_generator_value(&mut dialog, false);
    assert_eq!(dialog.slides, Percent::ZERO);
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.slides, Percent::new(5).unwrap());

    dialog.field = 0;
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.target, GeneratorTarget::Track(0));
    dialog.field = 1;
    change_generator_value(&mut dialog, true);
    assert_eq!(dialog.track, 1);
    assert_eq!(dialog.target, GeneratorTarget::Track(1));
}

#[test]
fn generator_dialog_submission_preserves_octave_bounds() {
    let dialog = GeneratorDialog {
        target: GeneratorTarget::WholePattern,
        track: 0,
        seed: "123".into(),
        density: Percent::new(48).unwrap(),
        range_low: 1,
        range_high: 5,
        chord_shapes: ChordShapePool::RootShapes,
        ties: Percent::new(18).unwrap(),
        accents: Percent::new(24).unwrap(),
        slides: Percent::new(35).unwrap(),
        field: 4,
    };
    let config = generator_config(&dialog);
    assert_eq!(config.range_low, 1);
    assert_eq!(config.range_high, 5);
    assert_eq!(config.seed, 123);
    assert_eq!(config.chord_shapes, ChordShapePool::RootShapes);
    assert_eq!(config.slides, Percent::new(35).unwrap());
}

#[test]
fn generator_dialog_highlights_the_selected_field() {
    let mut app = App::new(Project::new(), None);
    app.mode = Mode::GeneratorDialog(GeneratorDialog {
        target: GeneratorTarget::WholePattern,
        track: 0,
        seed: "123".into(),
        density: Percent::new(48).unwrap(),
        range_low: 2,
        range_high: 6,
        chord_shapes: ChordShapePool::AllShapes,
        ties: Percent::new(18).unwrap(),
        accents: Percent::new(24).unwrap(),
        slides: Percent::new(18).unwrap(),
        field: 4,
    });

    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("> Low octave O2"));
    assert!(screen.contains("High octave O6"));
    assert!(screen.contains("[↑/↓]/[Tab] field"));
    assert!(!screen.contains("> Target"));
}

#[test]
fn generator_dialog_marks_track_specific_fields_inapplicable() {
    let project = Project::new();
    let mut dialog = GeneratorDialog {
        target: GeneratorTarget::Track(0),
        track: 0,
        seed: "123".into(),
        density: Percent::new(48).unwrap(),
        range_low: 2,
        range_high: 6,
        chord_shapes: ChordShapePool::AllShapes,
        ties: Percent::new(18).unwrap(),
        accents: Percent::new(24).unwrap(),
        slides: Percent::new(18).unwrap(),
        field: 6,
    };

    assert!(!dialog.field_is_applicable(&project, 6));
    assert!(!dialog.field_is_applicable(&project, 9));
    dialog.target = GeneratorTarget::Track(SYNTH_TRACK_START);
    assert!(!dialog.field_is_applicable(&project, 6));
    assert!(dialog.field_is_applicable(&project, 9));
    dialog.target = GeneratorTarget::Track(CHORD_TRACK_INDEX);
    assert!(dialog.field_is_applicable(&project, 6));
    assert!(!dialog.field_is_applicable(&project, 9));
    dialog.target = GeneratorTarget::Track(LEAD_TRACK_INDEX);
    assert!(!dialog.field_is_applicable(&project, 6));
    assert!(dialog.field_is_applicable(&project, 9));
    dialog.target = GeneratorTarget::WholePattern;
    assert!(dialog.field_is_applicable(&project, 6));
    assert!(dialog.field_is_applicable(&project, 9));

    let mut app = App::new(project, None);
    dialog.target = GeneratorTarget::Track(0);
    app.mode = Mode::GeneratorDialog(dialog);
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("> Chord shapes All shapes  (n/a)"));
    assert!(screen.contains("Slides     18%  (n/a)"));
}

#[test]
fn trigger_dialog_renders_horizontal_cards() {
    assert_eq!(TriggerField::ALL[0], TriggerField::Microtiming);
    let mut project = Project::new();
    project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
        accent: false,
        recipe: crate::model::DrumRecipeSlot::ONE,
        condition: TriggerCondition::Cycle {
            position: 1,
            length: 2,
        },
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    let mut app = App::new(project, None);
    app.row = 1;
    app.mode = Mode::TriggerEdit {
        field: TriggerField::CycleLength,
    };

    let lines = rendered_lines(&app, 120, 34);
    assert!(lines.iter().any(|line| {
        [
            "Microtime",
            "Mode",
            "Phase",
            "Length",
            "Chance",
            "Retrigger",
        ]
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
fn closing_trigger_editor_ends_microtiming_coalescing() {
    let mut app = App::new(Project::new(), None);
    app.editor.toggle_event(0, 0).unwrap();
    app.editor.end_coalescing();
    let key = Some(crate::reducer::CoalesceKey(0, 0, 0xfd));

    app.editor
        .set_microtiming(0, 0, Microtiming::new(1).unwrap(), key)
        .unwrap();
    app.mode = Mode::TriggerEdit {
        field: TriggerField::Microtiming,
    };
    finish_trigger_edit(&mut app);
    app.editor
        .set_microtiming(0, 0, Microtiming::new(2).unwrap(), key)
        .unwrap();

    assert!(app.editor.undo());
    assert_eq!(
        app.editor.microtiming_value(0, 0).unwrap(),
        Microtiming::new(1).unwrap()
    );
    assert!(app.editor.undo());
    assert_eq!(
        app.editor.microtiming_value(0, 0).unwrap(),
        Microtiming::ZERO
    );
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
    for (key, track) in ('1'..='9').zip(0..TRACK_COUNT) {
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
    assert_eq!(
        track_jump_index(KeyEvent::new(KeyCode::Char('('), KeyModifiers::SHIFT)),
        Some(8)
    );
}

#[test]
fn pitched_navigation_starts_after_all_drum_rows() {
    assert!(!input::is_pitched_track_row(DRUM_TRACK_COUNT));
    assert!(input::is_pitched_track_row(DRUM_TRACK_COUNT + 1));
}

#[test]
fn chord_editor_uses_the_shifted_chord_row() {
    let mut app = App::new(Project::new(), None);
    app.row = CHORD_TRACK_INDEX + 1;
    open_chord_editor(&mut app);
    assert!(matches!(app.mode, Mode::ChordEdit { .. }));

    app.mode = Mode::Navigation;
    app.row = DRUM_TRACK_COUNT;
    open_chord_editor(&mut app);
    assert_eq!(app.mode, Mode::Navigation);
    assert!(app.status.contains("Chord track only"));
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
    app.row = SYNTH_TRACK_START + 1;
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
    app.row = SYNTH_TRACK_START + 1;
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
    assert!(!parameter_supports_direct_percentage(
        ParameterId::LeadSubMode
    ));
    assert!(!parameter_supports_direct_percentage(ParameterId::Pitch));
}

#[test]
fn direct_feedback_percentage_entry_clamps_zero_key_to_ninety() {
    let zero_key = crate::reducer::percentage_key('0').unwrap();
    assert_eq!(
        clamp_parameter_percentage(ParameterId::PhaserFeedback, zero_key),
        Percent::new(90).unwrap()
    );
    assert_eq!(
        clamp_parameter_percentage(ParameterId::FlangerFeedback, zero_key),
        Percent::new(90).unwrap()
    );
}

#[test]
fn pitch_lfo_parameter_card_advertises_assignment_removal() {
    let mut project = Project::new();
    project.tracks[CHORD_TRACK_INDEX]
        .lfos
        .set(ParameterId::Pitch, Some(LfoConfig::default()));
    let mut app = App::new(project, None);
    app.row = CHORD_TRACK_INDEX + 1;
    app.mode = Mode::ParameterEdit(ParameterId::Pitch);

    let screen = rendered(&app, 220, 34);
    assert!(screen.contains("[Backspace/Del] remove LFO"));
}

#[test]
fn pitch_lfo_card_is_visible_for_chord_and_lead_only() {
    let mut project = Project::new();
    project.tracks[CHORD_TRACK_INDEX].lfos.set(
        ParameterId::Pitch,
        Some(LfoConfig {
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        }),
    );
    let mut app = App::new(project, None);
    app.row = CHORD_TRACK_INDEX + 1;
    let chord = rendered(&app, 220, 34);
    assert!(chord.contains("Pitch"));
    assert!(chord.contains("[i]"));
    assert!(chord.contains("100%"));
    assert!(chord.contains("±2st"));

    app.row = TRACK_COUNT;
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
    app.row = SYNTH_TRACK_START + 1;
    app.editor.set_note(SYNTH_TRACK_START, 0, 1).unwrap();
    app.editor.toggle_accent(SYNTH_TRACK_START, 0).unwrap();
    assert_eq!(
        articulation_title(&app, SYNTH_TRACK_START),
        "Accent on · Slide off"
    );
    app.editor.toggle_tie(SYNTH_TRACK_START, 1).unwrap();
    assert_eq!(selected_accent(&app, SYNTH_TRACK_START), Some((true, None)));
    app.step = 1;
    assert_eq!(
        selected_accent(&app, SYNTH_TRACK_START),
        Some((true, Some(0)))
    );
    assert_eq!(
        articulation_title(&app, SYNTH_TRACK_START),
        "Accent on from step 1"
    );
}

#[test]
fn lead_slide_has_the_same_visible_articulation_state_as_bass_slide() {
    let mut app = App::new(Project::new(), None);
    app.row = LEAD_TRACK_INDEX + 1;
    app.editor.set_note(LEAD_TRACK_INDEX, 0, 1).unwrap();
    app.editor.toggle_slide(LEAD_TRACK_INDEX, 0).unwrap();

    assert_eq!(
        articulation_title(&app, LEAD_TRACK_INDEX),
        "Accent off · Slide on"
    );
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("Slide on"));
}

#[test]
fn empty_accent_readout_shows_the_track_default() {
    let mut app = App::new(Project::new(), None);
    app.row = 1;
    app.editor.toggle_accent(0, 0).unwrap();

    assert_eq!(selected_accent(&app, 0), Some((true, None)));
    assert_eq!(articulation_title(&app, 0), "Accent default on");
}

#[test]
fn articulation_titles_label_only_hat_and_tom_recipes() {
    let mut app = App::new(Project::new(), None);
    app.editor.toggle_event(0, 0).unwrap();
    assert_eq!(articulation_title(&app, 0), "Accent off");

    app.editor.toggle_event(2, 0).unwrap();
    assert_eq!(articulation_title(&app, 2), "Accent off · CLOSED");
}

#[test]
fn fader_animation_interpolates_and_reaches_its_target() {
    let started = Instant::now();
    let animation = FaderAnimation {
        track: 0,
        step: 0,
        scope: Scope::Base,
        parameter: ParameterId::Level,
        recipe: crate::model::DrumRecipeSlot::ONE,
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
    globals.reverb_return = Percent::ZERO;
    assert_eq!(
        global_fader_segments(&globals, GlobalParameterId::ReverbReturn),
        Some(0)
    );
    globals.reverb_return = Percent::new(100).unwrap();
    assert_eq!(
        global_fader_segments(&globals, GlobalParameterId::ReverbReturn),
        Some(10)
    );
}

#[test]
fn screen_renders_fader_bank_and_local_shortcuts_at_minimum_size() {
    let backend = TestBackend::new(120, 34);
    let mut project = Project::new();
    project.tracks[SYNTH_TRACK_START].input_octave = Some(4);
    project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Note {
        degree: 1,
        octave: 3,
        accent: false,
        chord_shape: None,
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    project.patterns[0].tracks[SYNTH_TRACK_START].steps[1] = Some(StepEvent::Note {
        degree: 2,
        octave: 4,
        accent: false,
        chord_shape: None,
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: crate::model::ParameterLocks::from_pairs([(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(50).unwrap()),
        )]),
    });
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
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
    assert!(screen.contains("Shift+Delete clear selected track"));
    assert!(screen.contains("EVENTS & TRACKS  p BASE/LOCK"));
    assert!(screen.contains("PARAMETERS  v level"));
    assert!(screen.contains("GLOBAL  t tempo"));
    assert!(screen.contains("m reverb return"));
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
fn shifted_delete_is_limited_to_navigation_track_rows() {
    let clear = KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT);
    assert!(is_clear_track_shortcut(&Mode::Navigation, 1, clear));
    assert!(!is_clear_track_shortcut(&Mode::Navigation, 0, clear));
    assert!(!is_clear_track_shortcut(
        &Mode::ParameterEdit(ParameterId::Level),
        1,
        clear,
    ));
    assert!(!is_clear_track_shortcut(
        &Mode::Navigation,
        1,
        KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    ));
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

    app.row = CHORD_TRACK_INDEX + 1;
    app.scope = Scope::Base;
    app.mode = Mode::ParameterEdit(ParameterId::Spread);
    let spread = rendered(&app, 220, 34);
    assert!(!spread.contains(DIRECT_PERCENTAGE_HINT));
    assert!(!spread.contains("[Shift+L] LFO"));

    app.mode = Mode::ParameterEdit(ParameterId::Noise);
    let noise = rendered(&app, 220, 34);
    assert!(!noise.contains("[Shift+L] LFO"));

    app.row = LEAD_TRACK_INDEX + 1;
    app.mode = Mode::ParameterEdit(ParameterId::KeyboardTracking);
    let tracking = rendered(&app, 220, 34);
    assert!(!tracking.contains("[Shift+L] LFO"));
}

#[test]
fn lfo_modal_and_fader_badge_render_at_minimum_size() {
    let mut project = Project::new();
    project.tracks[SYNTH_TRACK_START].lfos.set(
        ParameterId::Cutoff,
        Some(LfoConfig {
            reset_on_trigger: true,
            start_phase: Percent::new(25).unwrap(),
            ..Default::default()
        }),
    );
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
    app.mode = Mode::LfoEdit {
        parameter: ParameterId::Cutoff,
        field: LfoField::Depth,
    };
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("Track LFO · cutoff"));
    assert!(screen.contains("● Sine"));
    assert!(screen.contains("● ON"));
    assert!(screen.contains("25% · 90°"));
    assert!(screen.contains("±10 pp"));
    assert!(screen.contains("███"));
    assert!(screen.contains("║"));
    assert!(screen.contains("~"));
}

#[test]
fn chord_shape_modal_and_title_render_at_minimum_size() {
    let mut project = Project::new();
    project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
        degree: 1,
        octave: 3,
        accent: false,
        chord_shape: Some(ChordShape::DyadThird),
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    let mut app = App::new(project, None);
    app.row = CHORD_TRACK_INDEX + 1;
    let title_screen = rendered(&app, 120, 34);
    assert!(title_screen.contains("Chord trigger 1-3"));
    app.mode = Mode::ChordEdit {
        shape: ChordShape::DyadThird,
    };
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("Chord · Step 1"));
    assert!(screen.contains("Shape"));
    assert!(screen.contains("○ 1"));
    assert!(screen.contains("● 1-3"));
    assert!(screen.contains("○ 1-5"));
    assert!(screen.contains("[←/→] select"));
}

#[test]
fn chord_shape_editor_page_navigation_follows_each_step() {
    let mut project = Project::new();
    project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[0] = Some(StepEvent::Note {
        degree: 1,
        octave: 3,
        accent: false,
        chord_shape: None,
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    project.patterns[0].tracks[CHORD_TRACK_INDEX].steps[1] = Some(StepEvent::Note {
        degree: 2,
        octave: 3,
        accent: false,
        chord_shape: Some(ChordShape::Sus4SecondInversion),
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    let mut app = App::new(project, None);
    app.row = CHORD_TRACK_INDEX + 1;
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
        Rect::new(2, 9, 116, 20)
    );
    assert_eq!(
        lfo_popup_rect(Rect::new(0, 0, 200, 50)),
        Rect::new(42, 25, 116, 20)
    );
}

#[test]
fn compact_dialog_rectangles_fit_their_content() {
    let area = Rect::new(0, 0, 120, 34);
    assert_eq!(trigger_popup_rect(area), Rect::new(14, 9, 92, 20));
    assert_eq!(generator_popup_rect(area), Rect::new(31, 9, 58, 15));
    assert_eq!(swing_popup_rect(area), Rect::new(36, 14, 48, 6));
    assert_eq!(probability_popup_rect(area), Rect::new(36, 14, 48, 6));
    assert_eq!(
        overwrite_popup_rect(area, "/tmp/existing.groove.json"),
        Rect::new(22, 14, 76, 5)
    );
    let long_destination = "x".repeat(150);
    assert_eq!(
        overwrite_popup_rect(area, &long_destination),
        Rect::new(22, 13, 76, 7)
    );

    assert_eq!(
        trigger_popup_rect(Rect::new(0, 0, 200, 50)),
        Rect::new(54, 25, 92, 20)
    );
    assert_eq!(
        generator_popup_rect(Rect::new(0, 0, 200, 50)),
        Rect::new(71, 17, 58, 15)
    );
    assert_eq!(
        generator_popup_rect(Rect::new(0, 0, 40, 10)),
        Rect::new(0, 0, 40, 10)
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
        range_low: 2,
        range_high: 6,
        chord_shapes: ChordShapePool::AllShapes,
        ties: Percent::new(18).unwrap(),
        accents: Percent::new(24).unwrap(),
        slides: Percent::new(18).unwrap(),
        field: 3,
    });
    let generator = rendered_lines(&app, 120, 34);
    assert!(
        generator
            .iter()
            .any(|line| line.contains("[↑/↓]/[Tab] field  [←→] change  type seed"))
    );
    let _ = rendered_lines(&app, 40, 10);

    app.mode = Mode::TrackProbabilityEdit;
    let probability = rendered_lines(&app, 120, 34);
    assert!(
        probability
            .iter()
            .any(|line| line.contains("0–100% · evaluated after event conditions"))
    );
    let _ = rendered_lines(&app, 40, 10);
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
    project.tracks[SYNTH_TRACK_START]
        .lfos
        .set(ParameterId::Cutoff, Some(LfoConfig::default()));
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
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

    app.editor.project.tracks[SYNTH_TRACK_START].lfos.set(
        ParameterId::Cutoff,
        Some(LfoConfig {
            rate: LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            },
            ..Default::default()
        }),
    );
    let free = rendered(&app, 120, 34);
    assert!(free.contains("100%"));
    assert!(free.contains("20.000 Hz"));
}

#[test]
fn pitch_lfo_modal_shows_physical_depth_range() {
    let mut project = Project::new();
    project.tracks[CHORD_TRACK_INDEX].lfos.set(
        ParameterId::Pitch,
        Some(LfoConfig {
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        }),
    );
    let mut app = App::new(project, None);
    app.row = CHORD_TRACK_INDEX + 1;
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
    project.tracks[SYNTH_TRACK_START]
        .lfos
        .set(ParameterId::Cutoff, Some(LfoConfig::default()));
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
    app.mode = Mode::LfoEdit {
        parameter: ParameterId::Cutoff,
        field: LfoField::Enabled,
    };
    let screen = rendered(&app, 120, 34);
    let enabled = screen.rfind("Enabled").unwrap();
    let waveform = screen.rfind("Waveform").unwrap();
    let trigger_reset = screen.rfind("Trigger Reset").unwrap();
    let start_phase = screen.rfind("Start Phase").unwrap();
    let rate_mode = screen.rfind("Rate Mode").unwrap();
    let rate = screen.rfind("Rate").unwrap();
    let depth = screen.rfind("Depth").unwrap();
    assert!(enabled < waveform);
    assert!(waveform < trigger_reset);
    assert!(trigger_reset < start_phase);
    assert!(start_phase < rate_mode);
    assert!(rate_mode < rate);
    assert!(rate < depth);
}

#[test]
fn lock_scope_labels_explicit_and_inherited_values() {
    let mut project = Project::new();
    project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Note {
        degree: 1,
        octave: 3,
        accent: false,
        chord_shape: None,
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: crate::model::ParameterLocks::from_pairs([(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(50).unwrap()),
        )]),
    });
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
    app.scope = Scope::Lock;
    let (_, cutoff_origin) =
        displayed_parameter(&app, SYNTH_TRACK_START, 0, ParameterId::Cutoff).unwrap();
    let (_, level_origin) =
        displayed_parameter(&app, SYNTH_TRACK_START, 0, ParameterId::Level).unwrap();
    assert_eq!(cutoff_origin, ValueOrigin::Lock);
    assert_eq!(level_origin, ValueOrigin::Base);
}

#[test]
fn lock_values_remain_displayed_after_track_navigation() {
    let mut project = Project::new();
    project.patterns[0].tracks[0].steps[0] = Some(StepEvent::Trigger {
        accent: false,
        recipe: crate::model::DrumRecipeSlot::ONE,
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: crate::model::ParameterLocks::from_pairs([(
            ParameterId::Level,
            ParameterValue::Percent(Percent::new(25).unwrap()),
        )]),
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
    app.row = SYNTH_TRACK_START + 1;
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
fn rimshot_readouts_show_reference_modes_and_longest_decay() {
    let app = App::new(Project::new(), None);
    assert_eq!(
        physical_parameter_readout(&app, RIMSHOT_TRACK_INDEX, 0, ParameterId::Tune),
        "222/500/1000 Hz modes · BASE"
    );
    assert_eq!(
        physical_parameter_readout(&app, RIMSHOT_TRACK_INDEX, 0, ParameterId::Decay),
        "45 ms longest mode · BASE"
    );
}

#[test]
fn lock_parameter_editing_has_a_prominent_banner() {
    let mut app = App::new(Project::new(), None);
    app.row = SYNTH_TRACK_START + 1;
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
    app.row = SYNTH_TRACK_START + 1;
    app.mode = Mode::ParameterEdit(ParameterId::Cutoff);
    let screen = rendered(&app, 120, 34);
    assert!(!screen.contains("LOCK PARAMETER EDITING"));
}

#[test]
fn locked_badge_uses_a_distinct_color() {
    let mut project = Project::new();
    project.patterns[0].tracks[SYNTH_TRACK_START].steps[0] = Some(StepEvent::Note {
        degree: 1,
        octave: 3,
        accent: false,
        chord_shape: None,
        arpeggio: crate::model::ArpeggioConfig::default(),
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: crate::model::ParameterLocks::from_pairs([(
            ParameterId::Cutoff,
            ParameterValue::Percent(Percent::new(50).unwrap()),
        )]),
    });
    let mut app = App::new(project, None);
    app.row = SYNTH_TRACK_START + 1;
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
    for key in [
        "[t]", "[y]", "[f]", "[r]", "[b]", "[p]", "[m]", "[d]", "[k]", "[s]",
    ] {
        assert!(screen.contains(key), "missing {key}");
    }
    for value in [
        "Tempo", "Dly div", "Rev time", "Pre-dly", "1/8", "30%", "2.5 s", "40%", "20 ms", "Return",
        "Off", "C", "Major",
    ] {
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
fn global_cards_use_semantic_groups_and_track_card_geometry() {
    let app = App::new(Project::new(), None);
    let lines = rendered_lines(&app, 120, 34);

    assert!(lines[17].contains("CLOCK"));
    assert!(lines[17].contains("DELAY"));
    assert!(lines[17].contains("REVERB"));
    assert!(lines[17].contains("DUCKING"));
    assert!(lines[17].contains("SCALE"));
    assert!(!lines[18].contains("┌"));
    assert!(!lines[18].contains("┬"));
    assert!(lines[29].contains("Tempo"));
    assert!(lines[30].contains("[t]"));

    let backend = TestBackend::new(120, 34);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| draw_with_device(frame, &app, "null"))
        .unwrap();
    let group_heading = &terminal.backend().buffer().content[17 * 120..18 * 120];
    assert!(
        group_heading[10..20]
            .iter()
            .any(|cell| cell.symbol() == "C" && cell.fg == Color::Cyan)
    );
    assert!(
        group_heading[20..40]
            .iter()
            .any(|cell| cell.symbol() == "D" && cell.fg == Color::LightBlue)
    );
    assert!(
        group_heading[40..80]
            .iter()
            .any(|cell| cell.symbol() == "R" && cell.fg == Color::Magenta)
    );
    assert!(
        group_heading[80..90]
            .iter()
            .any(|cell| cell.symbol() == "D" && cell.fg == Color::Red)
    );
    assert!(
        group_heading[90..110]
            .iter()
            .any(|cell| cell.symbol() == "S" && cell.fg == Color::Green)
    );
}

#[test]
fn global_controls_keep_related_groups_contiguous() {
    use super::controls::GlobalParameterGroup;

    let groups = GLOBAL_CONTROLS
        .iter()
        .map(|control| control.group)
        .collect::<Vec<_>>();
    assert_eq!(
        groups,
        vec![
            GlobalParameterGroup::Clock,
            GlobalParameterGroup::Delay,
            GlobalParameterGroup::Delay,
            GlobalParameterGroup::Reverb,
            GlobalParameterGroup::Reverb,
            GlobalParameterGroup::Reverb,
            GlobalParameterGroup::Reverb,
            GlobalParameterGroup::Ducking,
            GlobalParameterGroup::Scale,
            GlobalParameterGroup::Scale,
        ]
    );
}

#[test]
fn global_navigation_row_omits_local_shortcuts() {
    let text = global_control_text(&Project::new().globals);
    assert!(text.iter().all(|control| !control.starts_with('[')));
    assert!(text.iter().all(|control| !control.contains("] ")));
}

#[test]
fn sidechain_editor_renders_off_state_and_physical_readouts() {
    let mut app = App::new(Project::new(), None);
    app.mode = Mode::SidechainEdit {
        field: SidechainField::Depth,
    };
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("Ducking"));
    assert!(screen.contains("Off"));
    assert!(screen.contains("1.13 ms"));
    assert!(screen.contains("Kick"));
    assert!(screen.contains("Enter/Esc"));
}

#[test]
fn global_shortcuts_enter_editing_for_every_control() {
    let mut app = App::new(Project::new(), None);
    for id in GLOBAL_CONTROLS.map(|control| control.id) {
        enter_global_edit(&mut app, id);
        match id {
            GlobalParameterId::Tempo => assert_eq!(app.mode, Mode::TempoInput(String::new())),
            GlobalParameterId::Ducking => assert_eq!(
                app.mode,
                Mode::SidechainEdit {
                    field: SidechainField::Depth
                }
            ),
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
    assert_eq!(app.global, GLOBAL_CONTROLS.len() - 1);
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
            microtiming: crate::model::Microtiming::ZERO,
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
            microtiming: crate::model::Microtiming::ZERO,
            locks: crate::model::ParameterLocks::from_pairs([(
                ParameterId::Cutoff,
                ParameterValue::Percent(Percent::new(50).unwrap()),
            )]),
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
        mode_name(&Mode::ProjectBrowser {
            entries: Vec::new(),
            selected: 0,
        }),
        "Project browser"
    );
    assert_eq!(mode_name(&Mode::Error("bad".into())), "Error dialog");
    assert_eq!(mode_name(&Mode::Help), "Help");
    assert_eq!(mode_name(&Mode::NewConfirm), "Unsaved confirmation");
    assert_eq!(
        mode_name(&Mode::OverwriteConfirm {
            path: PathBuf::from(".projects/song.groove.json"),
            input: "song".into(),
        }),
        "Overwrite confirmation"
    );
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
fn save_as_uses_the_gitignored_default_project_path() {
    assert_eq!(
        save_as_mode(),
        Mode::FileInput(FileAction::SaveAs, String::new())
    );

    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        project_path_for_name(directory.path(), "lead").unwrap(),
        directory.path().join("lead.groove.json")
    );
    assert_eq!(
        project_path_for_name(directory.path(), "lead.groove.json").unwrap(),
        directory.path().join("lead.groove.json")
    );
    assert!(project_path_for_name(directory.path(), "bad/name").is_err());
    assert!(project_path_for_name(directory.path(), "").is_err());
}

#[test]
fn save_as_overwrite_check_applies_to_every_existing_destination() {
    let directory = tempfile::tempdir().unwrap();
    let existing = directory.path().join("existing.groove.json");
    let missing = directory.path().join("missing.groove.json");
    std::fs::write(&existing, b"existing").unwrap();

    assert!(save_as_needs_overwrite_confirmation(&existing).unwrap());
    assert!(!save_as_needs_overwrite_confirmation(&missing).unwrap());
}

#[test]
fn overwrite_confirmation_renders_destination_and_controls() {
    let mut app = App::new(Project::new(), None);
    app.mode = Mode::OverwriteConfirm {
        path: PathBuf::from(".projects/existing.groove.json"),
        input: "existing".into(),
    };

    let screen = rendered(&app, 120, 34);

    assert!(screen.contains("Overwrite existing project?"));
    assert!(screen.contains(".projects/existing.groove.json"));
    assert!(screen.contains("Overwrite [Enter/O]"));
    assert!(screen.contains("Cancel [Esc]"));
}

#[test]
fn overwrite_confirmation_keeps_controls_visible_for_long_destinations() {
    let mut app = App::new(Project::new(), None);
    app.mode = Mode::OverwriteConfirm {
        path: PathBuf::from(format!("/{}", "x".repeat(5_000))),
        input: "long".into(),
    };

    let screen = rendered(&app, 120, 34);

    assert!(screen.contains("Overwrite [Enter/O]"));
    assert!(screen.contains("Cancel [Esc]"));
}

#[test]
fn project_browser_lists_sorted_regular_non_temporary_files() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("zeta"), b"legacy").unwrap();
    std::fs::write(directory.path().join("alpha.groove.json"), b"current").unwrap();
    std::fs::write(
        directory.path().join(".song.groove.json.1234.tmp"),
        b"temporary",
    )
    .unwrap();
    std::fs::create_dir(directory.path().join("nested")).unwrap();

    let entries = list_projects(directory.path()).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["alpha.groove.json", "zeta"]
    );
}

#[test]
fn project_browser_render_shows_entries_and_selection() {
    let mut app = App::new(Project::new(), None);
    app.mode = Mode::ProjectBrowser {
        entries: vec![
            PathBuf::from(".projects/alpha.groove.json"),
            PathBuf::from(".projects/beta"),
        ],
        selected: 1,
    };

    let screen = rendered(&app, 120, 34);

    assert!(screen.contains("Open project"));
    assert!(screen.contains("alpha.groove.json"));
    assert!(screen.contains("beta"));
    assert!(screen.contains("[Enter] open"));
}

#[test]
fn new_project_requests_confirmation_only_when_dirty() {
    let mut app = App::new(Project::new(), None);
    assert!(request_new_project(&mut app));
    assert_eq!(app.mode, Mode::Navigation);

    app.editor
        .edit(None, |project, _| {
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
fn global_shortcuts_select_all_controls() {
    assert_eq!(global_shortcut('t'), Some(GlobalParameterId::Tempo));
    assert_eq!(global_shortcut('y'), Some(GlobalParameterId::DelayDivision));
    assert_eq!(global_shortcut('f'), Some(GlobalParameterId::DelayFeedback));
    assert_eq!(global_shortcut('r'), Some(GlobalParameterId::ReverbTime));
    assert_eq!(global_shortcut('b'), Some(GlobalParameterId::ReverbTone));
    assert_eq!(
        global_shortcut('p'),
        Some(GlobalParameterId::ReverbPreDelay)
    );
    assert_eq!(global_shortcut('m'), Some(GlobalParameterId::ReverbReturn));
    assert_eq!(global_shortcut('d'), Some(GlobalParameterId::Ducking));
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
    app.step = app.editor.active_steps(TRACK_COUNT - 1).unwrap().len() - 1;
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
fn bank_navigation_refreshes_the_lock_recipe() {
    let mut project = Project::new();
    project.patterns[0].tracks[3]
        .steps
        .resize(STEP_BANK_SIZE * 2, None);
    project.patterns[0].tracks[3].steps[STEP_BANK_SIZE] = Some(StepEvent::Trigger {
        accent: false,
        recipe: crate::model::DrumRecipeSlot::TWO,
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    let mut app = App::new(project, None);
    app.row = 4;
    app.scope = Scope::Lock;
    app.mode = Mode::ParameterEdit(ParameterId::Tune);
    app.parameter_recipe = crate::model::DrumRecipeSlot::ONE;

    move_step_bank(&mut app, true);

    assert_eq!(app.step, STEP_BANK_SIZE);
    assert_eq!(app.parameter_recipe, crate::model::DrumRecipeSlot::TWO);
}

#[test]
fn sixty_four_step_track_renders_as_two_compact_rows_without_shortcut_hints() {
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
    assert!(!screen.contains("more tracks"));
    assert!(!screen.contains("Shift+D"));
}

#[test]
fn main_layout_hides_non_parameter_shortcut_hints() {
    let mut app = App::new(Project::new(), None);
    app.row = 1;
    let screen = rendered(&app, 120, 34);

    assert!(screen.contains("[v]"));
    for hint in [
        "[←→] select",
        "[Enter] edit",
        "[Tab] bank",
        "[p]",
        "[m]",
        "[o]",
        "[Shift+Delete]",
        "[Shift+D]",
    ] {
        assert!(
            !screen.contains(hint),
            "unexpected main-layout hint: {hint}"
        );
    }
}

#[test]
fn drum_recipe_cards_render_inline_and_arrow_navigation_reaches_duplicates() {
    let mut app = App::new(Project::new(), None);
    app.row = 4;
    let screen = rendered(&app, 120, 34);
    assert!(screen.contains("LOW"));
    assert!(screen.contains("MEDIUM"));
    assert!(screen.contains("HIGH"));

    app.mode = Mode::ParameterEdit(ParameterId::Tune);
    app.parameter_recipe = crate::model::DrumRecipeSlot::ONE;
    move_parameter_editor(&mut app, true);
    move_parameter_editor(&mut app, true);
    move_parameter_editor(&mut app, true);
    assert_eq!(app.mode, Mode::ParameterEdit(ParameterId::Tune));
    assert_eq!(app.parameter_recipe, crate::model::DrumRecipeSlot::TWO);

    app.row = 3;
    let hat = rendered(&app, 120, 34);
    assert!(hat.contains("CLOSED"));
    assert!(hat.contains("OPEN"));
}

#[test]
fn lock_scope_exposes_only_the_selected_drum_recipe_group() {
    let mut project = Project::new();
    project.patterns[0].tracks[3].steps[0] = Some(StepEvent::Trigger {
        accent: false,
        recipe: crate::model::DrumRecipeSlot::TWO,
        condition: Default::default(),
        retrigger_count: 1,
        microtiming: crate::model::Microtiming::ZERO,
        locks: Default::default(),
    });
    let mut app = App::new(project, None);
    app.row = 4;
    app.scope = Scope::Lock;
    assert!(
        displayed_parameter_for_recipe(
            &app,
            3,
            0,
            ParameterId::Tune,
            crate::model::DrumRecipeSlot::ONE
        )
        .is_some()
    );
    assert!(
        displayed_parameter_for_recipe(
            &app,
            3,
            0,
            ParameterId::Tune,
            crate::model::DrumRecipeSlot::TWO
        )
        .is_some()
    );
    assert_eq!(
        step_cell(app.editor.active_steps(3).unwrap()[0].as_ref()),
        "x2 "
    );
}
