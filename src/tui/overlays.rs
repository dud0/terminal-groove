use super::{
    render::{fader_segments, render_centered},
    state::{App, ChordField, GeneratorDialog, LfoField, TriggerField},
};
use crate::{
    generator::Target as GeneratorTarget,
    model::{
        ArpeggioRate, ArpeggioType, ChordShape, LfoConfig, LfoDivision, LfoRate, LfoWaveform,
        ParameterId, StepEvent, TriggerCondition,
    },
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

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
    let count = a.editor.retrigger_count_value(track, a.step).unwrap_or(1);
    let (cycle_position, cycle_length, chance) = match condition {
        TriggerCondition::Cycle { position, length } => (position, length, 50),
        TriggerCondition::Chance { probability } => (1, 2, probability.get()),
        TriggerCondition::Always => (1, 2, 50),
    };
    let mode = match condition {
        TriggerCondition::Always => "Always",
        TriggerCondition::Cycle { .. } => "Cycle",
        TriggerCondition::Chance { .. } => "Chance",
    };
    let fields = [
        (TriggerField::Mode, format!("Mode: {mode}"), false),
        (
            TriggerField::CyclePosition,
            format!("Phase: {cycle_position}"),
            !matches!(condition, TriggerCondition::Cycle { .. }),
        ),
        (
            TriggerField::CycleLength,
            format!("Length: {cycle_length}"),
            !matches!(condition, TriggerCondition::Cycle { .. }),
        ),
        (
            TriggerField::Chance,
            format!("Chance: {chance}%"),
            !matches!(condition, TriggerCondition::Chance { .. }),
        ),
        (
            TriggerField::Retrigger,
            format!("Retrigger: {count}"),
            false,
        ),
    ];
    let text = fields
        .into_iter()
        .map(|(candidate, value, disabled)| {
            let marker = if candidate == field { "> " } else { "  " };
            let suffix = if disabled { " (inactive)" } else { "" };
            Line::from(Span::styled(
                format!("{marker}{value}{suffix}"),
                if disabled {
                    Style::default().fg(Color::DarkGray)
                } else if candidate == field {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    let popup_area = lfo_popup_rect(area);
    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new(text)
            .block(Block::bordered().title(format!("Trigger · Step {}", a.step + 1))),
        popup_area,
    );
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
    let origin = match a.editor.project.tracks[track].steps[step] {
        Some(StepEvent::Note { .. }) => "TRIGGER",
        Some(StepEvent::Tie { .. }) => "INHERITED",
        None => "INPUT",
        _ => "",
    };
    let panel = Block::bordered().title(format!("Chord · Step {} · {origin}", step + 1));
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
            let choices = ChordShape::ALL.map(|value| value.to_string());
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
    let text = format!(
        "Target     {target}\nTrack      {}\nSeed       {}\nDensity    {}\nRange      O2–O6\nTies       {}\nAccents    {}\n\n[Tab/↑↓] field  [←→] change  type seed  [Enter] apply  [Esc] cancel",
        a.editor.project.tracks[dialog.track].name,
        if dialog.seed.is_empty() {
            "0"
        } else {
            &dialog.seed
        },
        dialog.density,
        dialog.ties,
        dialog.accents,
    );
    popup(f, area, "Pattern idea generator [g]", &text);
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
            Line::from("[`/1–9/0] free rate/depth   [Backspace/Del] remove   [Enter/Esc] close"),
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

pub(super) fn lfo_popup_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(92);
    let height = area.height.saturating_sub(4).min(20);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height + 5),
        width,
        height,
    }
}

pub(super) fn popup_rect(area: Rect) -> Rect {
    Rect {
        x: area.x + 10,
        y: area.y + 5,
        width: area.width - 20,
        height: (area.height - 10).max(5),
    }
}
