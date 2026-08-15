use super::{
    render::{ValueOrigin, displayed_parameter, fader_segments, render_centered},
    state::{
        App, ChordField, GeneratorDialog, LfoField, PatternPage, PresetAction, SidechainField,
        TriggerField,
    },
};
use crate::{
    generator::Target as GeneratorTarget,
    model::{
        ArpeggioRate, ArpeggioType, ChordShape, FmAlgorithm, FmOperatorField, FmRatio, LfoConfig,
        LfoDivision, LfoRate, LfoWaveform, ParameterId, ParameterValue, StepEvent,
        TriggerCondition,
    },
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use std::path::PathBuf;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn render_fm_operator_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    selected_operator: usize,
    selected_field: FmOperatorField,
) {
    let popup_area = compact_popup_rect(area, 116, 28);
    f.render_widget(Clear, popup_area);
    let track = a.row.saturating_sub(1);
    let algorithm = match displayed_parameter(a, track, a.step, ParameterId::FmAlgorithm) {
        Some((ParameterValue::FmAlgorithm(value), _)) => value,
        _ => FmAlgorithm::default(),
    };
    let panel = Block::bordered().title(format!(
        "FM Operators · {} · Step {} · {}",
        algorithm,
        a.step + 1,
        if a.scope == crate::reducer::Scope::Lock {
            "LOCK"
        } else {
            "BASE"
        },
    ));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);
    if inner.height < 8 {
        return;
    }
    render_centered(
        f,
        algorithm.diagram(),
        Rect { height: 1, ..inner },
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    );
    let overview = Rect {
        y: inner.y + 2,
        height: 7.min(inner.height.saturating_sub(5)),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(overview);
    for operator in 0..4 {
        render_fm_operator_column(
            f,
            columns[operator],
            a,
            track,
            algorithm,
            operator,
            selected_operator,
            selected_field,
        );
    }
    let footer_height = 2.min(inner.height);
    let detail_y = overview.y + overview.height + 1;
    let detail = Rect {
        x: inner.x + inner.width / 3,
        y: detail_y,
        width: inner.width / 3,
        height: inner
            .y
            .saturating_add(inner.height)
            .saturating_sub(detail_y)
            .saturating_sub(footer_height),
    };
    render_fm_operator_detail(f, detail, a, track, selected_operator, selected_field);
    f.render_widget(
        Paragraph::new(vec![
            Line::from("[←/→] operator  [Tab/BackTab] field  [↑/↓] adjust  [Shift+↑/↓] ±10%  [[/]] algorithm"),
            Line::from("[Shift+L] LFO  [Backspace/Del] remove lock  [o] audition  [Enter/Esc] close"),
        ])
        .alignment(Alignment::Center),
        Rect {
            y: inner.y + inner.height.saturating_sub(footer_height),
            height: footer_height,
            ..inner
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn render_fm_operator_column(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    track: usize,
    algorithm: FmAlgorithm,
    operator: usize,
    selected_operator: usize,
    selected_field: FmOperatorField,
) {
    let active = operator == selected_operator;
    let accent = if active {
        Color::Yellow
    } else {
        Color::LightCyan
    };
    let block = Block::bordered()
        .border_type(if active {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(accent))
        .title(format!("OP{} · {}", operator + 1, algorithm.role(operator)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    for (row, field) in FmOperatorField::ALL.into_iter().enumerate() {
        let parameter = ParameterId::fm_operator(operator, field).unwrap();
        let Some((value, origin)) = displayed_parameter(a, track, a.step, parameter) else {
            continue;
        };
        let value = match value {
            ParameterValue::FmRatio(value) => format!("{value}:1"),
            ParameterValue::Percent(value) => format!("{}%", value.get()),
            _ => continue,
        };
        let lfo = a.editor.project.tracks[track].lfos.get(parameter).is_some();
        let text = format!(
            "{} {:>5} {}{}",
            match field {
                FmOperatorField::Ratio => "RATIO",
                FmOperatorField::Level => "LEVEL",
                FmOperatorField::Feedback => "FDBK ",
            },
            value,
            if origin == ValueOrigin::Lock {
                "L"
            } else {
                "B"
            },
            if lfo { "~" } else { "" },
        );
        let selected = active && field == selected_field;
        render_centered(
            f,
            &text,
            Rect {
                y: inner.y + row as u16 * 2,
                height: 1,
                ..inner
            },
            Style::default()
                .fg(if origin == ValueOrigin::Lock {
                    Color::LightMagenta
                } else {
                    accent
                })
                .add_modifier(if selected {
                    Modifier::REVERSED | Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
    }
}

fn render_fm_operator_detail(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    track: usize,
    operator: usize,
    field: FmOperatorField,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let parameter = ParameterId::fm_operator(operator, field).unwrap();
    let Some((value, _)) = displayed_parameter(a, track, a.step, parameter) else {
        return;
    };
    let label = match field {
        FmOperatorField::Ratio => "Ratio",
        FmOperatorField::Level => "Level",
        FmOperatorField::Feedback => "Feedback",
    };
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!("OP{} {label}", operator + 1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    match value {
        ParameterValue::FmRatio(value) => {
            let choices = FmRatio::ALL.map(|ratio| format!("{ratio}:1"));
            let selected = FmRatio::ALL
                .iter()
                .position(|ratio| *ratio == value)
                .unwrap_or(0);
            render_lfo_selector(f, inner, &choices, selected, style);
        }
        ParameterValue::Percent(value) => render_lfo_fader(f, inner, value.get(), style),
        _ => {}
    }
}

pub(super) fn render_trigger_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    field: TriggerField,
) {
    let track = a.row.saturating_sub(1);
    let Ok(condition) = a.editor.trigger_condition_value(track, a.step) else {
        return;
    };
    let microtiming = a
        .editor
        .microtiming_value(track, a.step)
        .unwrap_or_default();
    let count = a.editor.retrigger_count_value(track, a.step).unwrap_or(1);
    let (cycle_position, cycle_length, chance) = match condition {
        TriggerCondition::Cycle { position, length } => (position, length, 50),
        TriggerCondition::Chance { probability } => (1, 2, probability.get()),
        TriggerCondition::Always => (1, 2, 50),
    };
    let popup_area = trigger_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let panel = Block::bordered().title(format!("Trigger · Step {}", a.step + 1));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);

    let controls_area = Rect {
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 6); 6])
        .split(controls_area);
    render_trigger_signed_fader(
        f,
        columns[0],
        "Microtime",
        microtiming.get(),
        field == TriggerField::Microtiming,
    );
    let mode_choices = ["Always", "Cycle", "Chance"];
    let mode_index = match condition {
        TriggerCondition::Always => 0,
        TriggerCondition::Cycle { .. } => 1,
        TriggerCondition::Chance { .. } => 2,
    };
    render_trigger_selector(
        f,
        columns[1],
        "Mode",
        &mode_choices,
        mode_index,
        field == TriggerField::Mode,
        false,
    );

    let phase_choices = (1..=cycle_length)
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    render_trigger_selector(
        f,
        columns[2],
        "Phase",
        &phase_choices,
        usize::from(cycle_position - 1),
        field == TriggerField::CyclePosition,
        !matches!(condition, TriggerCondition::Cycle { .. }),
    );

    let length_choices = ["2", "3", "4"];
    render_trigger_selector(
        f,
        columns[3],
        "Length",
        &length_choices,
        usize::from(cycle_length - 2),
        field == TriggerField::CycleLength,
        !matches!(condition, TriggerCondition::Cycle { .. }),
    );

    render_trigger_fader(
        f,
        columns[4],
        "Chance",
        chance,
        field == TriggerField::Chance,
        !matches!(condition, TriggerCondition::Chance { .. }),
    );

    let retrigger_choices = ["1", "2", "3", "4"];
    render_trigger_selector(
        f,
        columns[5],
        "Retrigger",
        &retrigger_choices,
        usize::from(count - 1),
        field == TriggerField::Retrigger,
        false,
    );
    f.render_widget(
        Paragraph::new(vec![
            Line::from("[←/→] select   [↑/↓] adjust   [Shift+↑/↓] ±10%"),
            Line::from("[Enter/Esc] close"),
        ])
        .alignment(Alignment::Center),
        Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(2),
            width: inner.width,
            height: 2.min(inner.height),
        },
    );
}

pub(super) fn render_sidechain_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    a: &App,
    selected: SidechainField,
) {
    let popup_area = sidechain_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let panel = Block::bordered().title("Ducking · Kick → Bass / Chord / Lead");
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);
    let controls_area = Rect {
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 3); 3])
        .split(controls_area);
    let sidechain = a.editor.project.globals.sidechain;
    for (index, field) in SidechainField::ALL.iter().enumerate() {
        let (label, value) = match field {
            SidechainField::Depth => {
                let value = if sidechain.depth == crate::model::Percent::ZERO {
                    "Off".to_string()
                } else {
                    format!("{}%\n{:.1} dB", sidechain.depth.get(), sidechain.depth_db())
                };
                ("Depth", value)
            }
            SidechainField::Attack => (
                "Attack",
                format!(
                    "{}%\n{:.2} ms",
                    sidechain.attack.get(),
                    sidechain.attack_ms()
                ),
            ),
            SidechainField::Release => (
                "Release",
                format!(
                    "{}%\n{:.0} ms",
                    sidechain.release.get(),
                    sidechain.release_ms()
                ),
            ),
        };
        let (content, style) =
            render_trigger_card(f, columns[index], label, selected == *field, false);
        f.render_widget(
            Paragraph::new(value)
                .alignment(Alignment::Center)
                .style(style),
            content,
        );
    }
    f.render_widget(
        Paragraph::new(vec![
            Line::from("[←/→] select   [↑/↓] adjust   [Shift+↑/↓] ±10%   [`/-/1–9/0] depth"),
            Line::from("[Enter/Esc] close · edits are immediate"),
        ])
        .alignment(Alignment::Center),
        Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(2),
            width: inner.width,
            height: 2.min(inner.height),
        },
    );
}

fn render_trigger_card(
    f: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    active: bool,
    disabled: bool,
) -> (Rect, Style) {
    let accent = if disabled {
        Color::DarkGray
    } else {
        Color::LightCyan
    };
    let style = if active {
        Style::default()
            .fg(accent)
            .reversed()
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
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
    (content, style)
}

fn render_trigger_selector<T: AsRef<str>>(
    f: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    choices: &[T],
    selected: usize,
    active: bool,
    disabled: bool,
) {
    let (content, style) = render_trigger_card(f, area, label, active, disabled);
    render_lfo_selector(f, content, choices, selected, style);
}

fn render_trigger_fader(
    f: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    value: u8,
    active: bool,
    disabled: bool,
) {
    let (content, style) = render_trigger_card(f, area, label, active, disabled);
    render_centered(
        f,
        &format!("{value}%"),
        Rect {
            height: 1.min(content.height),
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
        value,
        style,
    );
}

fn render_trigger_signed_fader(
    f: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    value: i8,
    active: bool,
) {
    let (content, style) = render_trigger_card(f, area, label, active, false);
    render_centered(
        f,
        &if value > 0 {
            format!("+{value}%")
        } else {
            format!("{value}%")
        },
        Rect {
            height: 1.min(content.height),
            ..content
        },
        style,
    );
    let segment_area = Rect {
        y: content.y + 1,
        height: content.height.saturating_sub(1),
        ..content
    };
    let height = segment_area.height.min(10);
    if height == 0 {
        return;
    }
    let start_y = segment_area.y + segment_area.height.saturating_sub(height) / 2;
    let amount = ((i16::from(value.abs()) * 5 + 25) / 50).min(5) as u16;
    let center = height / 2;
    for row in 0..height {
        let filled = if value > 0 {
            row + amount >= center && row < center
        } else if value < 0 {
            row >= center && row < center + amount
        } else {
            false
        };
        let text = if filled {
            "███"
        } else if row == center {
            "───"
        } else {
            "···"
        };
        render_centered(
            f,
            text,
            Rect {
                x: segment_area.x,
                y: start_y + row,
                width: segment_area.width,
                height: 1,
            },
            if filled {
                style
            } else {
                lfo_inactive_style(style)
            },
        );
    }
}

pub(super) fn render_chord_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    selected: ChordShape,
    a: &App,
) {
    let step = a.step;
    let popup_area = lfo_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let track = a.row.saturating_sub(1);
    let origin = match a.editor.active_steps(track).unwrap()[step] {
        Some(StepEvent::Note { .. }) => "TRIGGER",
        Some(StepEvent::Tie { .. }) => "INHERITED",
        None => "INPUT",
        _ => "",
    };
    let track_name = &a.editor.project.tracks[track].name;
    let mode = if selected == ChordShape::Single {
        "MONO"
    } else {
        "CHORD"
    };
    let panel = Block::bordered().title(format!(
        "Voicing · {track_name} · Step {} · {origin} · {mode}",
        step + 1
    ));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);
    let config = a
        .editor
        .arpeggio_config_value(track, step)
        .unwrap_or_default();
    let controls_area = Rect {
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(controls_area);
    for (index, field) in ChordField::ALL.iter().enumerate() {
        render_chord_control(
            f,
            columns[index],
            selected,
            config,
            *field,
            a.chord_field == *field,
            !config.enabled && matches!(field, ChordField::Type | ChordField::Rate),
        );
    }
    f.render_widget(
        Paragraph::new(vec![
            Line::from("[←/→] select   [↑/↓] adjust   [PageUp/Down] step"),
            Line::from("[Enter/Esc] close   ties inherit note-trigger settings"),
        ])
        .alignment(Alignment::Center),
        Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(2),
            width: inner.width,
            height: 2.min(inner.height),
        },
    );
}

pub(super) fn render_chord_control(
    f: &mut ratatui::Frame,
    area: Rect,
    shape: ChordShape,
    config: crate::model::ArpeggioConfig,
    field: ChordField,
    active: bool,
    disabled: bool,
) {
    let accent = if disabled {
        Color::DarkGray
    } else {
        Color::LightCyan
    };
    let style = if active {
        Style::default()
            .fg(if disabled { Color::DarkGray } else { accent })
            .reversed()
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    };
    let label = match field {
        ChordField::Shape => "Shape",
        ChordField::Arp => "Arp",
        ChordField::Type => "Type",
        ChordField::Rate => "Rate",
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
        ChordField::Shape => {
            let choices = ChordShape::ALL.map(|value| {
                if value == ChordShape::Single {
                    "1 (Mono)".into()
                } else {
                    value.to_string()
                }
            });
            let current = ChordShape::ALL
                .iter()
                .position(|value| *value == shape)
                .unwrap_or(0);
            render_lfo_selector(f, content, &choices, current, style);
        }
        ChordField::Arp => render_lfo_switch(f, content, "ON", "OFF", config.enabled, style),
        ChordField::Type => {
            let choices = ArpeggioType::ALL.map(|value| value.to_string());
            let current = ArpeggioType::ALL
                .iter()
                .position(|value| *value == config.r#type)
                .unwrap_or(0);
            render_lfo_selector(f, content, &choices, current, style);
        }
        ChordField::Rate => {
            let choices = ArpeggioRate::ALL.map(|value| value.to_string());
            let current = ArpeggioRate::ALL
                .iter()
                .position(|value| *value == config.rate)
                .unwrap_or(0);
            render_lfo_selector(f, content, &choices, current, style);
        }
    }
}

pub(super) fn render_pattern_popup(f: &mut ratatui::Frame, area: Rect, a: &App) {
    if a.pattern_page == PatternPage::Song {
        render_song_popup(f, area, a);
        return;
    }
    let height = 7.min(area.height.saturating_sub(4));
    let width = area.width.saturating_sub(8).max(24);
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
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
            Line::from("▶ playing  ⏭ next  ·  ←/→ Home End  ·  Enter select (queue while playing)"),
            Line::from(
                "N insert · D duplicate · C copy · X cut · V paste · Delete/Backspace remove · Esc close",
            ),
        ]),
        Rect {
            y: help_y,
            height: inner.height.saturating_sub(help_y.saturating_sub(inner.y)),
            ..inner
        },
    );
}

fn render_song_popup(f: &mut ratatui::Frame, area: Rect, a: &App) {
    let height = 8.min(area.height.saturating_sub(4));
    let width = area.width.saturating_sub(8).max(24);
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    f.render_widget(Clear, popup_area);
    let block = Block::bordered().title(format!(
        "Song ({}) · {}",
        a.editor.project.song.len(),
        if a.song_mode { "SONG" } else { "DIRECT" }
    ));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    let cell_width = 14usize;
    let cursor_x = a.song_cursor.saturating_mul(cell_width);
    let visible_width = usize::from(inner.width);
    let scroll_x = cursor_x
        .saturating_add(cell_width)
        .saturating_sub(visible_width)
        .min(cursor_x)
        .min(usize::from(u16::MAX));
    let cells = a
        .editor
        .project
        .song
        .iter()
        .enumerate()
        .flat_map(|(index, entry)| {
            let cursor = index == a.song_cursor;
            let active = a.song_mode && index == a.active_song;
            let queued = a.queued_song == Some(index);
            let marker = match (active, queued) {
                (true, true) => "▶⏭",
                (true, false) => "▶ ",
                (false, true) => "⏭ ",
                (false, false) => "  ",
            };
            let style = if cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
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
                Span::styled(
                    format!("{:>2} P{:03}×{:02} ", index + 1, entry.pattern, entry.bars),
                    style,
                ),
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
    let progress = if a.song_mode {
        format!(
            "Playing entry {} · bar {}/{}",
            a.active_song + 1,
            a.song_bar + 1,
            a.editor.project.song[a.active_song].bars
        )
    } else {
        "Direct pattern transport".into()
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(progress),
            Line::from("▶ playing  ⏭ next  ·  ←/→ Home End select · ↑/↓ bars · [/] pattern"),
            Line::from("Enter play from entry · N insert · D duplicate · C copy · X cut · V paste · Delete/Backspace remove · Tab patterns · Esc close"),
        ]),
        Rect { y: inner.y.saturating_add(2), height: inner.height.saturating_sub(2), ..inner },
    );
}

pub(super) fn render_generator_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    dialog: &GeneratorDialog,
    a: &App,
) {
    let target = match dialog.target {
        GeneratorTarget::WholePattern => "Whole pattern".to_string(),
        GeneratorTarget::Track(track) => format!(
            "Track {} ({})",
            track + 1,
            a.editor.project.tracks[track].name
        ),
    };
    let fields = [
        (Some(0), format!("Target     {target}")),
        (
            Some(1),
            format!("Track      {}", a.editor.project.tracks[dialog.track].name),
        ),
        (
            Some(2),
            format!(
                "Seed       {}",
                if dialog.seed.is_empty() {
                    "0"
                } else {
                    &dialog.seed
                }
            ),
        ),
        (Some(3), format!("Density    {}", dialog.density)),
        (Some(4), format!("Low octave O{}", dialog.range_low)),
        (Some(5), format!("High octave O{}", dialog.range_high)),
        (Some(6), format!("Voicings   {}", dialog.chord_shapes)),
        (Some(7), format!("Ties       {}", dialog.ties)),
        (Some(8), format!("Accents    {}", dialog.accents)),
        (Some(9), format!("Slides     {}", dialog.slides)),
    ];
    let lines = fields
        .into_iter()
        .map(|(index, field)| {
            let active = index == Some(dialog.field);
            let applicable = index
                .map(|index| dialog.field_is_applicable(&a.editor.project, index))
                .unwrap_or(true);
            let marker = if active { "> " } else { "  " };
            let style = if !applicable {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM)
            } else if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let suffix = if applicable { "" } else { "  (n/a)" };
            Line::from(Span::styled(format!("{marker}{field}{suffix}"), style))
        })
        .chain([
            Line::from(""),
            Line::from("[↑/↓]/[Tab] field  [←→] change  type seed"),
            Line::from("[Enter] apply  [Esc] cancel"),
        ])
        .collect::<Vec<_>>();
    let popup_area = generator_popup_rect(area);
    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Pattern idea generator [g]"),
        ),
        popup_area,
    );
}

pub(super) fn pattern_is_empty(pattern: &crate::model::Pattern) -> bool {
    pattern
        .tracks
        .iter()
        .all(|track| track.steps.iter().all(Option::is_none))
}

pub(super) fn render_lfo_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    parameter: ParameterId,
    config: LfoConfig,
    selected: LfoField,
    tempo_bpm: u16,
) {
    let popup_area = lfo_popup_rect(area);
    f.render_widget(Clear, popup_area);
    let panel = Block::bordered().title(format!("Track LFO · {} · ~", parameter.display_name()));
    let inner = panel.inner(popup_area);
    f.render_widget(panel, popup_area);

    let controls_area = Rect {
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 7); 7])
        .split(controls_area);
    for (index, field) in LfoField::ALL.iter().enumerate() {
        render_lfo_control(
            f,
            columns[index],
            config,
            parameter,
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
            Line::from("[←/→] select   [↑/↓] adjust   [Shift+↑/↓] ±10% fields"),
            Line::from(
                "[`/-/1–9/0] phase/free rate/depth   [Backspace/Del] remove   [Enter/Esc] close",
            ),
        ])
        .alignment(Alignment::Center),
        help_area,
    );
}

pub(super) fn render_lfo_control(
    f: &mut ratatui::Frame,
    area: Rect,
    config: LfoConfig,
    parameter: ParameterId,
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
        LfoField::TriggerReset => "Trigger Reset",
        LfoField::StartPhase => "Start Phase",
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
        LfoField::TriggerReset => {
            render_lfo_switch(f, content, "ON", "OFF", config.reset_on_trigger, style)
        }
        LfoField::StartPhase => {
            render_centered(
                f,
                &format!(
                    "{}% · {:.0}°",
                    config.start_phase.get(),
                    config.start_phase.get() as f32 * 3.6
                ),
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
                config.start_phase.get(),
                style,
            );
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
            let depth_label = if parameter == ParameterId::Pitch {
                format!(
                    "{}% · ±{:.1} st",
                    config.depth.get(),
                    config.depth.get() as f32 * 0.02
                )
            } else {
                format!("±{} pp", config.depth.get())
            };
            render_centered(
                f,
                &depth_label,
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

pub(super) fn render_lfo_fader(f: &mut ratatui::Frame, area: Rect, value: u8, active_style: Style) {
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

pub(super) fn render_lfo_switch(
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

pub(super) fn render_lfo_selector<T: AsRef<str>>(
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

pub(super) fn lfo_inactive_style(active_style: Style) -> Style {
    if active_style.add_modifier.contains(Modifier::REVERSED) {
        active_style
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub(super) fn popup(f: &mut ratatui::Frame, area: Rect, title: &str, text: &str) {
    popup_at(f, popup_rect(area), title, text);
}

pub(super) fn popup_at(f: &mut ratatui::Frame, r: Rect, title: &str, text: &str) {
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        r,
    )
}

pub(super) fn render_project_browser(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[PathBuf],
    selected: usize,
) {
    render_file_browser(
        f,
        area,
        entries,
        selected,
        "Open project",
        "No projects in your Terminal Groove Projects folder",
    );
}

pub(super) fn render_preset_browser(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[super::state::PresetBrowserEntry],
    selected: usize,
) {
    use super::state::PresetBrowserEntry;
    let popup_area = compact_popup_rect(area, 68, 24);
    f.render_widget(Clear, popup_area);
    let block = Block::bordered()
        .title("Load track preset")
        .title_bottom("[↑/↓] select  [Home/End] jump  [Enter] load  [Esc] close");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    if entries.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "User",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "  No user presets for this track kind",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            inner,
        );
        return;
    }
    let mut lines = Vec::new();
    let mut selected_line = 0;
    let mut built = false;
    let mut user = false;
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            PresetBrowserEntry::BuiltIn { name, .. } => {
                if !built {
                    lines.push(Line::styled(
                        "Built-in",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                    built = true;
                }
                if index == selected {
                    selected_line = lines.len();
                }
                lines.push(Line::styled(
                    format!("{} {name}", if index == selected { "▶" } else { " " }),
                    if index == selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    },
                ));
            }
            PresetBrowserEntry::User(path) => {
                if !user {
                    lines.push(Line::styled(
                        "User",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                    user = true;
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if index == selected {
                    selected_line = lines.len();
                }
                lines.push(Line::styled(
                    format!("{} {name}", if index == selected { "▶" } else { " " }),
                    if index == selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    },
                ));
            }
        }
    }
    if !user {
        lines.push(Line::styled(
            "User",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::styled(
            "  No user presets",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let description = match entries.get(selected) {
        Some(PresetBrowserEntry::BuiltIn { description, .. }) => Some(description.as_str()),
        _ => None,
    };
    let description_rows = u16::from(description.is_some()) * 2;
    let list_height = inner.height.saturating_sub(description_rows).max(1);
    let start = selected_line
        .saturating_sub(usize::from(list_height) / 2)
        .min(lines.len().saturating_sub(usize::from(list_height)));
    f.render_widget(
        Paragraph::new(lines).scroll((start as u16, 0)),
        Rect {
            height: list_height,
            ..inner
        },
    );
    if let Some(description) = description {
        f.render_widget(
            Paragraph::new(Line::styled(description, Style::default().fg(Color::Gray)))
                .wrap(Wrap { trim: true }),
            Rect {
                y: inner.y.saturating_add(list_height),
                height: description_rows,
                ..inner
            },
        );
    }
}

pub(super) fn render_preset_dialog(
    f: &mut ratatui::Frame,
    area: Rect,
    track_name: &str,
    selected: PresetAction,
    has_default: bool,
) {
    let popup_area = compact_popup_rect(area, 48, 9);
    f.render_widget(Clear, popup_area);
    let block = Block::bordered()
        .title(format!("Track presets · {track_name}"))
        .title_bottom("[↑/↓] select  [Enter] continue  [Esc] close");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let lines = PresetAction::ALL
        .into_iter()
        .map(|action| {
            let disabled = action == PresetAction::ClearDefault && !has_default;
            let active = action == selected;
            let marker = if active { "▶ " } else { "  " };
            let suffix = if disabled { " (unavailable)" } else { "" };
            let style = if disabled {
                Style::default().fg(Color::DarkGray)
            } else if active {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!("{marker}{}{}", action.label(), suffix),
                style,
            ))
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_file_browser(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[PathBuf],
    selected: usize,
    title: &str,
    empty_message: &str,
) {
    let popup_area = project_browser_popup_rect(area, entries.len());
    f.render_widget(Clear, popup_area);
    let block = Block::bordered()
        .title(title)
        .title_bottom("[↑/↓] select  [Home/End] jump  [Enter] open  [Esc] cancel");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if entries.is_empty() {
        f.render_widget(Paragraph::new(empty_message), inner);
        return;
    }

    let visible = usize::from(inner.height);
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(entries.len().saturating_sub(visible));
    let lines = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, path)| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "<unnamed>".into());
            let marker = if index == selected { "▶ " } else { "  " };
            let style = if index == selected {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![Span::styled(marker, style), Span::styled(name, style)])
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn project_browser_popup_rect(area: Rect, entry_count: usize) -> Rect {
    let list_height = entry_count.min(14) as u16;
    compact_popup_rect(area, 76, list_height.saturating_add(4).max(6))
}

pub(super) fn quit_popup_rect(area: Rect) -> Rect {
    const WIDTH: u16 = 37;
    const HEIGHT: u16 = 3;
    let width = WIDTH.min(area.width);
    let height = HEIGHT.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn overwrite_popup_rect(area: Rect, destination: &str) -> Rect {
    const WIDTH: u16 = 76;
    const FIXED_HEIGHT: u16 = 4;
    let width = WIDTH.min(area.width);
    let inner_width = usize::from(width.saturating_sub(2));
    let destination_lines = wrapped_line_count(destination, inner_width);
    let height = destination_lines
        .saturating_add(FIXED_HEIGHT)
        .min(area.height);
    compact_popup_rect(area, width, height)
}

pub(super) fn overwrite_destination(destination: &str, popup_area: Rect) -> String {
    let inner_width = usize::from(popup_area.width.saturating_sub(2));
    let destination_lines = usize::from(popup_area.height.saturating_sub(4).max(1));
    truncate_to_width(destination, inner_width.saturating_mul(destination_lines))
}

fn wrapped_line_count(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 1;
    }
    text.lines()
        .map(|line| UnicodeWidthStr::width(line).div_ceil(width).max(1))
        .sum::<usize>()
        .min(usize::from(u16::MAX)) as u16
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let ellipsis_width = UnicodeWidthChar::width('…').unwrap_or(1);
    let limit = width.saturating_sub(ellipsis_width);
    let mut result = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used.saturating_add(character_width) > limit {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

pub(super) fn tempo_popup_rect(area: Rect) -> Rect {
    const WIDTH: u16 = 77;
    const HEIGHT: u16 = 4;
    let width = WIDTH.min(area.width);
    let height = HEIGHT.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn lfo_popup_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(116);
    let height = area.height.saturating_sub(4).min(20);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height + 5),
        width,
        height,
    }
}

fn compact_popup_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn trigger_popup_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(92);
    let height = area.height.saturating_sub(4).min(20);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height + 5),
        width,
        height,
    }
}

pub(super) fn generator_popup_rect(area: Rect) -> Rect {
    compact_popup_rect(area, 58, 15)
}

pub(super) fn swing_popup_rect(area: Rect) -> Rect {
    compact_popup_rect(area, 48, 6)
}

pub(super) fn probability_popup_rect(area: Rect) -> Rect {
    compact_popup_rect(area, 48, 6)
}

pub(super) fn sidechain_popup_rect(area: Rect) -> Rect {
    compact_popup_rect(area, 72, 10)
}

pub(super) fn popup_rect(area: Rect) -> Rect {
    Rect {
        x: area.x + 10,
        y: area.y + 5,
        width: area.width - 20,
        height: (area.height - 10).max(5),
    }
}
