use chrono::{Local, Timelike};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
};

use super::widgets::{format_cost, format_tokens, ratatui_scrollbar_position};
use crate::tui::app::{App, SortDirection, SortField};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let peak_cost = app
        .data
        .hourly
        .iter()
        .max_by(|a, b| a.cost.total_cmp(&b.cost));
    let busy_hours = app.data.hourly.len();
    let title_line = if busy_hours == 0 {
        Line::from(vec![Span::styled(
            " Hourly Usage (local) ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )])
    } else if let Some(p) = peak_cost {
        Line::from(vec![
            Span::styled(
                " Hourly Usage (local) ",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("·", Style::default().fg(app.theme.muted)),
            Span::styled(
                format!(" {}h ", busy_hours),
                Style::default().fg(app.theme.muted),
            ),
            Span::styled("peak", Style::default().fg(app.theme.muted)),
            Span::raw(" "),
            Span::styled(
                format_cost(p.cost),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" @ {} ", p.hour_start.format("%m/%d %H:00")),
                Style::default().fg(app.theme.muted),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            " Hourly Usage (local) ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(title_line)
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height.saturating_sub(1) as usize;
    app.max_visible_items = visible_height;

    let hourly = app.get_sorted_hourly();
    if hourly.is_empty() {
        let empty_msg = Paragraph::new(
            "No hourly usage (messages need parseable timestamps). Press 'r' to refresh.",
        )
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    let is_narrow = app.is_narrow();
    let is_very_narrow = app.is_very_narrow();
    let sort_field = app.sort_field;
    let sort_direction = app.sort_direction;
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let theme_accent = app.theme.accent;
    let theme_selection = app.theme.selection;
    let now = Local::now().naive_local();
    let current_hour = now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0));

    let header_cells = if is_very_narrow {
        vec!["Hour", "Cost"]
    } else if is_narrow {
        vec!["Hour (local)", "Tokens", "Cost"]
    } else {
        vec![
            "Hour (local)",
            "Input",
            "Output",
            "Cache Read",
            "Cache Write",
            "Total",
            "Cost",
        ]
    };

    let sort_indicator = |field: SortField| -> &'static str {
        if sort_field == field {
            match sort_direction {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            }
        } else {
            ""
        }
    };

    let header = Row::new(
        header_cells
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let indicator = match (i, is_narrow, is_very_narrow) {
                    (0, _, _) => sort_indicator(SortField::Date),
                    (5, false, false) => sort_indicator(SortField::Tokens),
                    (1, true, false) => sort_indicator(SortField::Tokens),
                    (6, false, false) => sort_indicator(SortField::Cost),
                    (2, true, false) => sort_indicator(SortField::Cost),
                    (1, _, true) => sort_indicator(SortField::Cost),
                    _ => "",
                };
                Cell::from(format!("{}{}", h, indicator))
            })
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme_accent)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let hourly_len = hourly.len();
    let start = scroll_offset.min(hourly_len);
    let end = (start + visible_height).min(hourly_len);

    if start >= hourly_len {
        return;
    }

    let rows: Vec<Row> = hourly[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let is_current = current_hour == Some(row.hour_start);

            let label = if is_very_narrow {
                row.hour_start.format("%m/%d %H").to_string()
            } else if is_narrow {
                row.hour_start.format("%m/%d %H:00").to_string()
            } else {
                row.hour_start.format("%Y-%m-%d %H:00").to_string()
            };

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(label).style(if is_current {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    }),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else if is_narrow {
                vec![
                    Cell::from(label).style(if is_current {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    }),
                    Cell::from(format_tokens(row.tokens.total())),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else {
                vec![
                    Cell::from(label).style(if is_current {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().add_modifier(Modifier::BOLD)
                    }),
                    Cell::from(format_tokens(row.tokens.input))
                        .style(Style::default().fg(Color::Rgb(100, 200, 100))),
                    Cell::from(format_tokens(row.tokens.output))
                        .style(Style::default().fg(Color::Rgb(200, 100, 100))),
                    Cell::from(format_tokens(row.tokens.cache_read))
                        .style(Style::default().fg(Color::Rgb(100, 150, 200))),
                    Cell::from(format_tokens(row.tokens.cache_write))
                        .style(Style::default().fg(Color::Rgb(200, 150, 100))),
                    Cell::from(format_tokens(row.tokens.total())),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            };

            let row_style = if is_selected {
                Style::default().bg(theme_selection)
            } else if is_current {
                Style::default().bg(Color::Rgb(28, 42, 34))
            } else if is_striped {
                Style::default().bg(Color::Rgb(20, 24, 30))
            } else {
                Style::default()
            };

            Row::new(cells).style(row_style).height(1)
        })
        .collect();

    let widths = if is_very_narrow {
        vec![Constraint::Percentage(60), Constraint::Percentage(40)]
    } else if is_narrow {
        vec![
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ]
    } else {
        vec![
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);

    if hourly_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(hourly_len)
            .position(ratatui_scrollbar_position(
                scroll_offset,
                hourly_len,
                visible_height,
            ))
            .viewport_content_length(visible_height);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut scrollbar_state,
        );
    }
}
