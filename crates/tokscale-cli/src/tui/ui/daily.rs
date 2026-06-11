use chrono::{Local, Timelike};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, Table,
};
use std::collections::BTreeMap;

use super::mix::{
    compact_mix_summary_lines, embedded_mix_line_limit, has_mix_data, ranking_bar_line,
    token_profile_lines, MixRow,
};
use super::widgets::{
    format_cache_hit_rate, format_cost, format_cost_per_million, format_tokens,
    get_client_display_name, get_provider_display_name, get_provider_shade, scrollbar_state,
    truncate_ascii as truncate,
};
use crate::tui::app::{
    App, ClickAction, PeriodDetailKey, SortDirection, SortField, TimelineGranularity,
};
use crate::tui::data::{DailyUsage, HourlyModelInfo, HourlyUsage, TokenBreakdown};

const TIMELINE_INSPECTOR_MIN_WIDTH: u16 = 36;
const TIMELINE_INSPECTOR_MAX_WIDTH: u16 = 52;
const TIMELINE_WIDE_MIN_WIDTH: u16 = 104;

#[derive(Clone)]
struct TimelineMixRow {
    label: String,
    provider: String,
    color_key: String,
    tokens: TokenBreakdown,
    cost: f64,
    messages: u64,
}

struct TimelineRowData {
    label: String,
    period: Option<PeriodDetailKey>,
    cost: f64,
    tokens: TokenBreakdown,
    message_count: u32,
    turn_count: u32,
    source_count: usize,
    source_label: &'static str,
    is_current: bool,
    top_provider: String,
    top_model: String,
    provider_rows: Vec<TimelineMixRow>,
    model_rows: Vec<TimelineMixRow>,
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.is_daily_detail_active() {
        render_detail(frame, app, area);
        return;
    }

    if area.width >= TIMELINE_WIDE_MIN_WIDTH && area.height >= 12 {
        render_timeline_wide(frame, app, area);
        return;
    }

    if app.timeline_granularity == TimelineGranularity::Hour {
        super::hourly::render(frame, app, area);
        return;
    }

    render_timeline_table(frame, app, area);
}

fn render_timeline_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Timeline ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height.saturating_sub(1) as usize;
    app.set_max_visible_items(visible_height);

    let daily = app.get_sorted_daily();
    if daily.is_empty() {
        let empty_msg = Paragraph::new("No daily usage data found. Press 'r' to refresh.")
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    let is_narrow = app.is_narrow();
    let is_very_narrow = app.is_very_narrow();
    let has_turn_data = daily.iter().any(|d| d.turn_count > 0);
    let sort_field = app.sort_field;
    let sort_direction = app.sort_direction;
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let theme_accent = app.theme.accent;
    let theme_selection = app.theme.selection;
    let metric_input_style = app.theme.metric_input_style();
    let metric_output_style = app.theme.metric_output_style();
    let metric_cache_read_style = app.theme.metric_cache_read_style();
    let metric_cache_write_style = app.theme.metric_cache_write_style();
    let current_row_style = app.theme.current_row_style();
    let striped_row_style = app.theme.striped_row_style();
    let today = Local::now().date_naive();

    // Date format adapts to *available* width, not just the narrow breakpoint.
    // In full mode the table can still be wider than the terminal, so the year
    // would otherwise get compressed to "2026-0". When the full layout doesn't
    // fit we drop the year (near-constant in a by-day list) to "%m-%d" and
    // shrink the date column, freeing 5 columns. `full_layout_width` is the
    // ideal full-mode total (Length(12) date + spacing); keep it in sync with
    // the `widths` block below.
    let full_layout_width: u16 = if has_turn_data { 112 } else { 105 };
    let compact_full_date = !is_narrow && !is_very_narrow && inner.width < full_layout_width;
    let date_col_width: u16 = if compact_full_date { 7 } else { 12 };
    let date_fmt: &str = if is_very_narrow {
        "%m/%d"
    } else if is_narrow || compact_full_date {
        "%m-%d"
    } else {
        "%Y-%m-%d"
    };

    let header_cells = if is_very_narrow {
        vec!["Date", "Cost"]
    } else if is_narrow {
        if has_turn_data {
            vec!["Date", "Turn", "Msgs", "Tokens", "Cost"]
        } else {
            vec!["Date", "Msgs", "Tokens", "Cost"]
        }
    } else if has_turn_data {
        vec![
            "Date", "Turn", "Msgs", "Input", "Output", "Cache R", "Cache W", "Cache×", "Total",
            "Cost", "Cost/1M",
        ]
    } else {
        vec![
            "Date", "Msgs", "Input", "Output", "Cache R", "Cache W", "Cache×", "Total", "Cost",
            "Cost/1M",
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
                    (8, false, false) if has_turn_data => sort_indicator(SortField::Tokens),
                    (7, false, false) if !has_turn_data => sort_indicator(SortField::Tokens),
                    (3, true, false) if has_turn_data => sort_indicator(SortField::Tokens),
                    (2, true, false) if !has_turn_data => sort_indicator(SortField::Tokens),
                    (9, false, false) if has_turn_data => sort_indicator(SortField::Cost),
                    (8, false, false) if !has_turn_data => sort_indicator(SortField::Cost),
                    (4, true, false) if has_turn_data => sort_indicator(SortField::Cost),
                    (3, true, false) if !has_turn_data => sort_indicator(SortField::Cost),
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

    let daily_len = daily.len();
    let start = scroll_offset.min(daily_len);
    let end = (start + visible_height).min(daily_len);

    if start >= daily_len {
        return;
    }

    let rows: Vec<Row> = daily[start..end]
        .iter()
        .enumerate()
        .map(|(i, day)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let is_today = day.date == today;

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(day.date.format(date_fmt).to_string()).style(if is_today {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    }),
                    Cell::from(format_cost(day.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else if is_narrow {
                let mut cells =
                    vec![
                        Cell::from(day.date.format(date_fmt).to_string()).style(if is_today {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        }),
                    ];
                if has_turn_data {
                    let turn_str = if day.turn_count > 0 {
                        day.turn_count.to_string()
                    } else {
                        "\u{2014}".to_string()
                    };
                    cells.push(Cell::from(turn_str));
                }
                cells.extend([
                    Cell::from(day.message_count.to_string()),
                    Cell::from(format_tokens(day.tokens.total())),
                    Cell::from(format_cost(day.cost)).style(Style::default().fg(Color::Green)),
                ]);
                cells
            } else {
                let mut cells =
                    vec![
                        Cell::from(day.date.format(date_fmt).to_string()).style(if is_today {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        }),
                    ];
                if has_turn_data {
                    let turn_str = if day.turn_count > 0 {
                        day.turn_count.to_string()
                    } else {
                        "\u{2014}".to_string()
                    };
                    cells.push(Cell::from(turn_str));
                }
                cells.extend([
                    Cell::from(day.message_count.to_string()),
                    Cell::from(format_tokens(day.tokens.input)).style(metric_input_style),
                    Cell::from(format_tokens(day.tokens.output)).style(metric_output_style),
                    Cell::from(format_tokens(day.tokens.cache_read)).style(metric_cache_read_style),
                    Cell::from(format_tokens(day.tokens.cache_write))
                        .style(metric_cache_write_style),
                    Cell::from(format_cache_hit_rate(
                        day.tokens.cache_read,
                        day.tokens.input,
                        day.tokens.cache_write,
                    ))
                    .style(Style::default().fg(Color::Cyan)),
                    Cell::from(format_tokens(day.tokens.total())),
                    Cell::from(format_cost(day.cost)).style(Style::default().fg(Color::Green)),
                    Cell::from(format_cost_per_million(day.cost, day.tokens.total()))
                        .style(Style::default().fg(Color::Rgb(150, 200, 150))),
                ]);
                cells
            };

            let row_style = if is_selected {
                Style::default().bg(theme_selection)
            } else if is_today {
                current_row_style
            } else if is_striped {
                striped_row_style
            } else {
                Style::default()
            };

            Row::new(cells).style(row_style).height(1)
        })
        .collect();
    let period_clicks = daily[start..end]
        .iter()
        .enumerate()
        .map(|(i, day)| {
            (
                Rect::new(
                    inner.x,
                    inner.y.saturating_add(1 + i as u16),
                    inner.width,
                    1,
                ),
                PeriodDetailKey::day(day.date),
            )
        })
        .collect::<Vec<_>>();

    let widths = if is_very_narrow {
        vec![Constraint::Percentage(60), Constraint::Percentage(40)]
    } else if is_narrow && has_turn_data {
        vec![
            Constraint::Percentage(30),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ]
    } else if is_narrow {
        vec![
            Constraint::Percentage(35),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
        ]
    } else if has_turn_data {
        vec![
            Constraint::Length(date_col_width),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Length(date_col_width),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);
    for (rect, key) in period_clicks {
        app.add_click_area(rect, ClickAction::OpenPeriodDetail(key));
    }

    if daily_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = scrollbar_state(daily_len, scroll_offset, visible_height);

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

fn render_timeline_wide(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.background)),
        area,
    );

    let inspector_width = ((area.width as f64) * 0.32).round() as u16;
    let inspector_width =
        inspector_width.clamp(TIMELINE_INSPECTOR_MIN_WIDTH, TIMELINE_INSPECTOR_MAX_WIDTH);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(64), Constraint::Length(inspector_width)])
        .split(area);

    render_timeline_list(frame, app, chunks[0]);
    render_timeline_inspector(frame, app, chunks[1]);
}

fn render_timeline_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Timeline ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(timeline_title_status(app, area).right_aligned())
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let page_capacity = inner.height.saturating_sub(1).max(1) as usize;
    app.set_max_visible_items(page_capacity);

    let rows_data = timeline_rows(app);
    if rows_data.is_empty() {
        let empty_msg = Paragraph::new("No timeline data found. Press 'r' to refresh.")
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    render_timeline_header(frame, app, inner);

    let rows_len = rows_data.len();
    let start = app.scroll_offset.min(rows_len.saturating_sub(1));
    let end = (start + page_capacity).min(rows_len);

    let mut y = inner.y.saturating_add(1);
    for (i, row) in rows_data[start..end].iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }

        let idx = i + start;
        let is_selected = idx == app.selected_index;
        let is_current = row.is_current;
        let is_striped = idx % 2 == 1;
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        if let Some(period) = row.period.clone() {
            app.add_click_area(row_area, ClickAction::OpenPeriodDetail(period));
        }
        let row_style = if is_selected {
            Style::default()
                .bg(app.theme.selection)
                .fg(app.theme.foreground)
        } else if is_current {
            app.theme.current_row_style()
        } else if is_striped {
            app.theme.striped_row_style()
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new("").style(row_style), row_area);

        let time_style = if is_selected {
            Style::default()
                .fg(app.theme.foreground)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            app.theme.secondary_text_style()
        };

        let mut spans = Vec::new();
        let marker = if is_selected { "▶" } else { " " };
        spans.push(Span::styled(
            pad_right(&format!("{marker}{}", idx + 1), 4),
            Style::default().fg(if is_selected {
                app.theme.foreground
            } else {
                app.theme.muted
            }),
        ));
        spans.push(Span::styled(
            pad_left(&row.label, timeline_time_width(app)),
            time_style,
        ));
        spans.push(Span::styled(
            pad_right(&format_cost(row.cost), 10),
            Style::default()
                .fg(if is_selected {
                    app.theme.foreground
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.total()), 10),
            app.theme.secondary_text_style(),
        ));
        if inner.width >= 76 {
            spans.push(Span::styled(
                pad_left(&row.top_provider, 14),
                app.theme.secondary_text_style(),
            ));
        }
        let reserved = if inner.width >= 94 { 21 } else { 13 };
        let fixed = Line::from(spans.clone()).width();
        let model_width = (inner.width as usize)
            .saturating_sub(fixed)
            .saturating_sub(reserved)
            .max(8);
        spans.push(Span::styled(
            pad_left(&row.top_model, model_width),
            app.theme.secondary_text_style(),
        ));
        if inner.width >= 94 {
            spans.push(Span::styled(
                pad_right(&row.message_count.to_string(), 7),
                app.theme.secondary_text_style(),
            ));
        }
        spans.push(Span::styled(
            pad_right(&row.source_count.to_string(), 8),
            app.theme.subtle_text_style(),
        ));

        frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), row_area);
        y = y.saturating_add(1);
    }

    if rows_len > page_capacity {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");
        let mut state = scrollbar_state(rows_len, app.scroll_offset, page_capacity);
        frame.render_stateful_widget(scrollbar, inner, &mut state);
    }
}

fn render_timeline_header(frame: &mut Frame, app: &App, inner: Rect) {
    if inner.height == 0 {
        return;
    }

    let header_style = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(pad_right("#", 4), header_style),
        Span::styled(
            pad_left(
                &format!(
                    "{}{}",
                    app.timeline_granularity.title_label(),
                    sort_indicator(app, SortField::Date)
                ),
                timeline_time_width(app),
            ),
            header_style,
        ),
        Span::styled(
            pad_right(&format!("Cost{}", sort_indicator(app, SortField::Cost)), 10),
            header_style,
        ),
        Span::styled(
            pad_right(
                &format!("Tokens{}", sort_indicator(app, SortField::Tokens)),
                10,
            ),
            header_style,
        ),
    ];
    if inner.width >= 76 {
        spans.push(Span::styled(pad_left("Top Provider", 14), header_style));
    }
    let fixed = Line::from(spans.clone()).width();
    let reserved = if inner.width >= 94 { 21 } else { 13 };
    let model_width = (inner.width as usize)
        .saturating_sub(fixed)
        .saturating_sub(reserved)
        .max(8);
    spans.push(Span::styled(
        pad_left("Top Model", model_width),
        header_style,
    ));
    if inner.width >= 94 {
        spans.push(Span::styled(pad_right("Msgs", 7), header_style));
    }
    spans.push(Span::styled(
        pad_right(timeline_source_label(app), 8),
        header_style,
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

fn timeline_title_status(app: &mut App, area: Rect) -> Line<'static> {
    let granularities = [TimelineGranularity::Day, TimelineGranularity::Hour];
    let status = format!(
        "  •  {} {}  •  {} ",
        timeline_item_count(app),
        timeline_item_label(app),
        timeline_sort_label(app)
    );
    let selector_width = granularities
        .iter()
        .map(|granularity| Line::from(format!(" {} ", granularity.short_label()).as_str()).width())
        .sum::<usize>() as u16;
    let status_width = Line::from(status.as_str()).width() as u16;
    let mut click_x = area
        .right()
        .saturating_sub(selector_width.saturating_add(status_width));

    let mut spans = Vec::new();
    for granularity in granularities {
        let selected = app.timeline_granularity == granularity;
        let style = if selected {
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.color(Color::Rgb(30, 64, 175)))
                .add_modifier(Modifier::BOLD)
        } else {
            app.theme.subtle_text_style()
        };
        let label = format!(" {} ", granularity.short_label());
        let width = Line::from(label.as_str()).width() as u16;
        spans.push(Span::styled(label, style));
        app.add_click_area(
            Rect::new(click_x, area.y, width, 1),
            ClickAction::TimelineGranularity(granularity),
        );
        click_x = click_x.saturating_add(width);
    }
    spans.push(Span::styled(status, app.theme.subtle_text_style()));
    Line::from(spans)
}

fn render_timeline_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            format!(" Selected {} ", app.timeline_granularity.title_label()),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows_data = timeline_rows(app);
    if rows_data.is_empty() {
        let empty = Paragraph::new("No timeline item selected")
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let selected = app.selected_index.min(rows_data.len().saturating_sub(1));
    let row = &rows_data[selected];
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        if row.is_current {
            format!("{}  Current", row.label)
        } else {
            row.label.clone()
        },
        Style::default()
            .fg(if row.is_current {
                Color::Yellow
            } else {
                app.theme.foreground
            })
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(section_line("Summary", app));
    lines.push(kv_line("Cost", &format_cost(row.cost), app));
    lines.push(kv_line("Tokens", &format_tokens(row.tokens.total()), app));
    lines.push(kv_line("Messages", &row.message_count.to_string(), app));
    if row.turn_count > 0 {
        lines.push(kv_line("Turns", &row.turn_count.to_string(), app));
    }
    lines.push(kv_line(
        row.source_label,
        &row.source_count.to_string(),
        app,
    ));

    let provider_rows = row
        .provider_rows
        .iter()
        .map(|provider| {
            MixRow::cost(
                provider.label.clone(),
                provider.cost,
                get_provider_shade(&provider.provider, 0),
            )
        })
        .collect::<Vec<_>>();
    if provider_rows.len() > 1 {
        append_mix_section(
            &mut lines,
            "Provider Mix",
            &provider_rows,
            6,
            None,
            inner,
            app,
        );
    }

    let model_rows = row
        .model_rows
        .iter()
        .map(|model| {
            MixRow::cost(
                model.label.clone(),
                model.cost,
                app.model_color_for(&model.provider, &model.color_key),
            )
        })
        .collect::<Vec<_>>();
    append_ranking_section(&mut lines, "Top Models", &model_rows, 6, inner, app);

    append_token_profile_section(&mut lines, "Token Mix", &row.tokens, 7, inner, app);

    if lines.len() > inner.height as usize {
        lines.truncate(inner.height as usize);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn append_mix_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    rows: &[MixRow],
    preferred_lines: usize,
    footer: Option<String>,
    area: Rect,
    app: &App,
) {
    if !has_mix_data(rows) {
        return;
    }

    let Some(body_slots) = section_body_slots(lines, area) else {
        return;
    };
    let limit = embedded_mix_line_limit(area.height, preferred_lines).min(body_slots);
    if limit == 0 {
        return;
    }
    let body = compact_mix_summary_lines(app, area.width, limit, rows, footer);
    if body.is_empty() {
        return;
    }

    lines.push(Line::from(""));
    lines.push(section_line(title, app));
    lines.extend(body);
}

fn append_token_profile_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    tokens: &TokenBreakdown,
    preferred_lines: usize,
    area: Rect,
    app: &App,
) {
    if tokens.total() == 0 {
        return;
    }

    let Some(body_slots) = section_body_slots(lines, area) else {
        return;
    };
    let limit = preferred_lines.min(body_slots);
    if limit == 0 {
        return;
    }
    let body = token_profile_lines(app, area.width, limit, tokens);
    if body.is_empty() {
        return;
    }

    lines.push(Line::from(""));
    lines.push(section_line(title, app));
    lines.extend(body);
}

fn append_ranking_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    rows: &[MixRow],
    preferred_lines: usize,
    area: Rect,
    app: &App,
) {
    if !has_mix_data(rows) {
        return;
    }

    let Some(body_slots) = section_body_slots(lines, area) else {
        return;
    };
    let limit = embedded_mix_line_limit(area.height, preferred_lines).min(body_slots);
    if limit == 0 {
        return;
    }

    let positive_rows = rows
        .iter()
        .filter(|row| row.amount > 0.0)
        .collect::<Vec<_>>();
    let total = positive_rows
        .iter()
        .map(|row| row.amount.max(0.0))
        .sum::<f64>();
    let visible = if positive_rows.len() > limit && limit > 1 {
        limit - 1
    } else {
        limit
    };
    let mut body = positive_rows
        .iter()
        .take(visible)
        .map(|row| {
            let ratio = if total > 0.0 {
                row.amount.max(0.0) / total
            } else {
                0.0
            };
            ranking_bar_line(&row.label, &row.value, ratio, row.color, area.width, app)
        })
        .collect::<Vec<_>>();
    if positive_rows.len() > visible && body.len() < limit {
        let hidden = &positive_rows[visible..];
        let hidden_amount = hidden.iter().map(|row| row.amount.max(0.0)).sum::<f64>();
        body.push(Line::from(Span::styled(
            truncate(
                &format!("+{} more {}", hidden.len(), format_cost(hidden_amount)),
                area.width as usize,
            ),
            app.theme.subtle_text_style(),
        )));
    }
    if body.is_empty() {
        return;
    }

    lines.push(Line::from(""));
    lines.push(section_line(title, app));
    lines.extend(body);
}

fn section_body_slots(lines: &[Line<'static>], area: Rect) -> Option<usize> {
    let remaining = (area.height as usize).saturating_sub(lines.len());
    if remaining < 3 {
        return None;
    }
    Some(remaining - 2)
}

fn timeline_sort_label(app: &App) -> String {
    let field = match app.sort_field {
        SortField::Date => "Date",
        SortField::Cost => "Cost",
        SortField::Tokens => "Tokens",
    };
    let direction = match app.sort_direction {
        SortDirection::Ascending => "asc",
        SortDirection::Descending => "desc",
    };
    format!("{field} {direction}")
}

fn sort_indicator(app: &App, field: SortField) -> &'static str {
    if app.sort_field == field {
        match app.sort_direction {
            SortDirection::Ascending => " ▲",
            SortDirection::Descending => " ▼",
        }
    } else {
        ""
    }
}

fn timeline_rows(app: &App) -> Vec<TimelineRowData> {
    match app.timeline_granularity {
        TimelineGranularity::Day => app
            .get_sorted_daily()
            .into_iter()
            .map(timeline_day_row)
            .collect(),
        TimelineGranularity::Hour => app
            .get_sorted_hourly()
            .into_iter()
            .map(|hour| timeline_hour_row(hour))
            .collect(),
    }
}

fn timeline_day_row(day: &DailyUsage) -> TimelineRowData {
    let provider_rows = provider_rows(day);
    let model_rows = model_rows(day);
    TimelineRowData {
        label: day.date.format("%Y-%m-%d").to_string(),
        period: Some(PeriodDetailKey::day(day.date)),
        cost: day.cost,
        tokens: day.tokens.clone(),
        message_count: day.message_count,
        turn_count: day.turn_count,
        source_count: day.source_breakdown.len(),
        source_label: "Sources",
        is_current: day.date == Local::now().date_naive(),
        top_provider: provider_rows
            .first()
            .map(|row| row.label.clone())
            .unwrap_or_else(|| "—".to_string()),
        top_model: model_rows
            .first()
            .map(|row| row.label.clone())
            .unwrap_or_else(|| "—".to_string()),
        provider_rows,
        model_rows,
    }
}

fn timeline_hour_row(hour: &HourlyUsage) -> TimelineRowData {
    let provider_rows = hourly_provider_rows(hour);
    let model_rows = hourly_model_rows(&hour.models);
    let now = Local::now().naive_local();
    let current_hour = now.date().and_hms_opt(now.hour(), 0, 0).unwrap_or(now);
    TimelineRowData {
        label: hour.datetime.format("%m-%d %H:00").to_string(),
        period: None,
        cost: hour.cost,
        tokens: hour.tokens.clone(),
        message_count: hour.message_count,
        turn_count: hour.turn_count,
        source_count: hour.clients.len(),
        source_label: "Clients",
        is_current: hour.datetime == current_hour,
        top_provider: provider_rows
            .first()
            .map(|row| row.label.clone())
            .unwrap_or_else(|| "—".to_string()),
        top_model: model_rows
            .first()
            .map(|row| row.label.clone())
            .unwrap_or_else(|| "—".to_string()),
        provider_rows,
        model_rows,
    }
}

fn timeline_item_count(app: &App) -> usize {
    match app.timeline_granularity {
        TimelineGranularity::Day => app.data.daily.len(),
        TimelineGranularity::Hour => app.data.hourly.len(),
    }
}

fn timeline_item_label(app: &App) -> &'static str {
    match app.timeline_granularity {
        TimelineGranularity::Day => "days",
        TimelineGranularity::Hour => "hours",
    }
}

fn timeline_time_width(app: &App) -> usize {
    match app.timeline_granularity {
        TimelineGranularity::Day => 12,
        TimelineGranularity::Hour => 13,
    }
}

fn timeline_source_label(app: &App) -> &'static str {
    match app.timeline_granularity {
        TimelineGranularity::Day => "Sources",
        TimelineGranularity::Hour => "Clients",
    }
}

fn provider_rows(day: &DailyUsage) -> Vec<TimelineMixRow> {
    let mut rows: BTreeMap<String, TimelineMixRow> = BTreeMap::new();
    for source in day.source_breakdown.values() {
        for info in source.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let row = rows
                .entry(provider.clone())
                .or_insert_with(|| TimelineMixRow {
                    label: get_provider_display_name(&provider).to_string(),
                    provider: provider.clone(),
                    color_key: provider.clone(),
                    tokens: TokenBreakdown::default(),
                    cost: 0.0,
                    messages: 0,
                });
            add_tokens(&mut row.tokens, &info.tokens);
            if info.cost.is_finite() {
                row.cost += info.cost;
            }
            row.messages = row.messages.saturating_add(info.messages);
        }
    }

    sort_mix_rows(rows.into_values().collect())
}

fn hourly_provider_rows(hour: &HourlyUsage) -> Vec<TimelineMixRow> {
    let mut rows: BTreeMap<String, TimelineMixRow> = BTreeMap::new();
    for info in hour.models.values() {
        let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
        let row = rows
            .entry(provider.clone())
            .or_insert_with(|| TimelineMixRow {
                label: get_provider_display_name(&provider).to_string(),
                provider: provider.clone(),
                color_key: provider.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                messages: 0,
            });
        add_tokens(&mut row.tokens, &info.tokens);
        if info.cost.is_finite() {
            row.cost += info.cost;
        }
    }

    sort_mix_rows(rows.into_values().collect())
}

fn hourly_model_rows(models: &BTreeMap<String, HourlyModelInfo>) -> Vec<TimelineMixRow> {
    let mut rows: BTreeMap<(String, String), TimelineMixRow> = BTreeMap::new();
    for info in models.values() {
        let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
        let key = (provider.clone(), info.display_name.clone());
        let row = rows.entry(key).or_insert_with(|| TimelineMixRow {
            label: info.display_name.clone(),
            provider: provider.clone(),
            color_key: info.color_key.clone(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            messages: 0,
        });
        add_tokens(&mut row.tokens, &info.tokens);
        if info.cost.is_finite() {
            row.cost += info.cost;
        }
    }

    sort_mix_rows(rows.into_values().collect())
}

fn model_rows(day: &DailyUsage) -> Vec<TimelineMixRow> {
    let mut rows: BTreeMap<(String, String), TimelineMixRow> = BTreeMap::new();
    for source in day.source_breakdown.values() {
        for info in source.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let key = (provider.clone(), info.display_name.clone());
            let row = rows.entry(key).or_insert_with(|| TimelineMixRow {
                label: info.display_name.clone(),
                provider: provider.clone(),
                color_key: info.color_key.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                messages: 0,
            });
            add_tokens(&mut row.tokens, &info.tokens);
            if info.cost.is_finite() {
                row.cost += info.cost;
            }
            row.messages = row.messages.saturating_add(info.messages);
        }
    }

    sort_mix_rows(rows.into_values().collect())
}

fn sort_mix_rows(mut rows: Vec<TimelineMixRow>) -> Vec<TimelineMixRow> {
    rows.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn add_tokens(total: &mut TokenBreakdown, tokens: &TokenBreakdown) {
    total.input = total.input.saturating_add(tokens.input);
    total.output = total.output.saturating_add(tokens.output);
    total.cache_read = total.cache_read.saturating_add(tokens.cache_read);
    total.cache_write = total.cache_write.saturating_add(tokens.cache_write);
    total.reasoning = total.reasoning.saturating_add(tokens.reasoning);
}

fn section_line(label: &str, app: &App) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(app.theme.foreground)
            .add_modifier(Modifier::BOLD),
    ))
}

fn kv_line(label: &str, value: &str, app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), app.theme.subtle_text_style()),
        Span::styled(value.to_string(), app.theme.secondary_text_style()),
    ])
}

fn pad_left(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:<width$} ")
}

fn pad_right(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:>width$} ")
}

fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .daily_detail_date()
        .map(|date| format!(" Daily Detail: {} ", date))
        .unwrap_or_else(|| " Daily Detail ".to_string());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            title,
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height.saturating_sub(1) as usize;
    app.set_max_visible_items(visible_height);

    let rows_data = app.get_sorted_daily_detail_rows();
    if rows_data.is_empty() {
        let empty_msg =
            Paragraph::new("No model details found for this day. Press Esc to go back.")
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
    let theme_muted = app.theme.muted;
    let theme_selection = app.theme.selection;
    let metric_input_style = app.theme.metric_input_style();
    let metric_output_style = app.theme.metric_output_style();
    let metric_cache_read_style = app.theme.metric_cache_read_style();
    let metric_cache_write_style = app.theme.metric_cache_write_style();
    let striped_row_style = app.theme.striped_row_style();

    let header_cells = if is_very_narrow {
        vec!["Model", "Cost"]
    } else if is_narrow {
        vec!["Model", "Source", "Msgs", "Tokens", "Cost"]
    } else {
        vec![
            "#", "Model", "Provider", "Source", "Msgs", "Input", "Output", "Cache R", "Cache W",
            "Cache×", "Total", "Cost",
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
                    (10, false, false) => sort_indicator(SortField::Tokens),
                    (11, false, false) => sort_indicator(SortField::Cost),
                    (3, true, false) => sort_indicator(SortField::Tokens),
                    (4, true, false) => sort_indicator(SortField::Cost),
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

    let detail_len = rows_data.len();
    let start = scroll_offset.min(detail_len);
    let end = (start + visible_height).min(detail_len);

    if start >= detail_len {
        return;
    }

    let rows: Vec<Row> = rows_data[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let model_color = app.model_color_for(row.provider, row.color_key);

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(truncate(row.model, 18)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else if is_narrow {
                vec![
                    Cell::from(truncate(row.model, 24)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(get_client_display_name(row.source))
                        .style(Style::default().fg(theme_muted)),
                    Cell::from(row.messages.to_string()),
                    Cell::from(format_tokens(row.tokens.total())),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else {
                vec![
                    Cell::from(format!("{}", idx + 1)).style(Style::default().fg(theme_muted)),
                    Cell::from(truncate(row.model, 30)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(get_provider_display_name(row.provider)),
                    Cell::from(get_client_display_name(row.source))
                        .style(Style::default().fg(theme_muted)),
                    Cell::from(row.messages.to_string()),
                    Cell::from(format_tokens(row.tokens.input)).style(metric_input_style),
                    Cell::from(format_tokens(row.tokens.output)).style(metric_output_style),
                    Cell::from(format_tokens(row.tokens.cache_read)).style(metric_cache_read_style),
                    Cell::from(format_tokens(row.tokens.cache_write))
                        .style(metric_cache_write_style),
                    Cell::from(format_cache_hit_rate(
                        row.tokens.cache_read,
                        row.tokens.input,
                        row.tokens.cache_write,
                    ))
                    .style(Style::default().fg(Color::Cyan)),
                    Cell::from(format_tokens(row.tokens.total())),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            };

            let row_style = if is_selected {
                Style::default().bg(theme_selection)
            } else if is_striped {
                striped_row_style
            } else {
                Style::default()
            };

            Row::new(cells).style(row_style).height(1)
        })
        .collect();

    let widths = if is_very_narrow {
        vec![Constraint::Percentage(70), Constraint::Percentage(30)]
    } else if is_narrow {
        vec![
            Constraint::Percentage(42),
            Constraint::Percentage(18),
            Constraint::Percentage(12),
            Constraint::Percentage(15),
            Constraint::Percentage(13),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(20),
            Constraint::Length(16),
            Constraint::Length(14),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);

    if detail_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = scrollbar_state(detail_len, scroll_offset, visible_height);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Tab, TuiConfig};
    use crate::tui::data::{DailyModelInfo, DailySourceInfo, DailyUsage, TokenBreakdown};
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};
    use std::collections::BTreeMap;

    fn day(date: NaiveDate, cost: f64) -> DailyUsage {
        let tokens = TokenBreakdown {
            input: 10_000,
            output: 2_000,
            cache_read: 40_000,
            cache_write: 1_000,
            reasoning: 0,
        };
        let mut models = BTreeMap::new();
        models.insert(
            "gpt-5".to_string(),
            DailyModelInfo {
                provider: "openai".to_string(),
                display_name: "gpt-5".to_string(),
                color_key: "gpt-5".to_string(),
                tokens: tokens.clone(),
                cost,
                messages: 10,
            },
        );
        let mut source_breakdown = BTreeMap::new();
        source_breakdown.insert(
            "codex".to_string(),
            DailySourceInfo {
                tokens: tokens.clone(),
                cost,
                models,
            },
        );

        DailyUsage {
            date,
            tokens,
            cost,
            source_breakdown,
            message_count: 10,
            turn_count: 3,
        }
    }

    fn multi_provider_day(date: NaiveDate) -> DailyUsage {
        let mut usage = day(date, 3.0);
        let tokens = TokenBreakdown {
            input: 6_000,
            output: 1_000,
            cache_read: 12_000,
            cache_write: 500,
            reasoning: 0,
        };
        usage.tokens.input += tokens.input;
        usage.tokens.output += tokens.output;
        usage.tokens.cache_read += tokens.cache_read;
        usage.tokens.cache_write += tokens.cache_write;
        usage.cost += 2.0;
        usage.message_count += 4;
        usage.turn_count += 1;

        let mut models = BTreeMap::new();
        models.insert(
            "claude-sonnet".to_string(),
            DailyModelInfo {
                provider: "anthropic".to_string(),
                display_name: "claude-sonnet".to_string(),
                color_key: "claude-sonnet".to_string(),
                tokens: tokens.clone(),
                cost: 2.0,
                messages: 4,
            },
        );
        usage.source_breakdown.insert(
            "cursor".to_string(),
            DailySourceInfo {
                tokens,
                cost: 2.0,
                models,
            },
        );
        usage
    }

    fn make_app(width: u16) -> App {
        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: None,
        };
        let mut app = App::new_with_cached_data(config, None).unwrap();
        app.terminal_width = width;
        app.current_tab = Tab::Daily;
        app.sort_field = SortField::Date;
        app.sort_direction = SortDirection::Descending;
        app.data.daily = vec![
            day(NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(), 3.0),
            day(NaiveDate::from_ymd_opt(2026, 5, 28).unwrap(), 2.0),
        ];
        app
    }

    fn render_body(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, app, Rect::new(0, 0, width, height)))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn wide_terminal_keeps_year() {
        let mut app = make_app(130);
        let body = render_body(&mut app, 130, 12);
        assert!(
            body.contains("2026-05-29"),
            "a layout that fits should keep the full date\n{body}"
        );
    }

    #[test]
    fn full_mode_drops_year_when_layout_does_not_fit() {
        // 100 cols is full mode (>= 80) but narrower than the ~112-col full
        // layout, so the year is dropped — the date stays readable as "05-29"
        // instead of being compressed to "2026-0".
        let mut app = make_app(100);
        let body = render_body(&mut app, 100, 12);
        assert!(
            !body.contains("2026-05-29"),
            "year should be dropped when the full layout does not fit\n{body}"
        );
        assert!(body.contains("05-29"), "expected compact date\n{body}");
    }

    #[test]
    fn wide_timeline_renders_selected_day_inspector() {
        let mut app = make_app(140);
        let body = render_body(&mut app, 140, 24);

        assert!(body.contains("Selected Day"), "expected inspector\n{body}");
        assert!(body.contains("Summary"), "expected day summary\n{body}");
        assert!(body.contains("Token Mix"), "expected token mix\n{body}");
        assert!(body.contains("Cache hit"), "expected token profile\n{body}");
    }

    #[test]
    fn timeline_inspector_top_models_use_dotted_ranking_bars() {
        let mut app = make_app(140);
        app.data.daily[0] = multi_provider_day(NaiveDate::from_ymd_opt(2026, 5, 29).unwrap());
        let body = render_body(&mut app, 140, 24);

        assert!(body.contains("Top Models"), "{body}");
        assert!(body.contains("█"), "{body}");
        assert!(body.contains("·"), "{body}");
    }

    #[test]
    fn timeline_inspector_token_mix_uses_profile_rows() {
        let app = make_app(140);
        let mut lines = Vec::new();
        let tokens = TokenBreakdown {
            input: 490_000,
            output: 78_000,
            cache_read: 14_100_000,
            cache_write: 0,
            reasoning: 0,
        };

        append_token_profile_section(
            &mut lines,
            "Token Mix",
            &tokens,
            6,
            Rect::new(0, 0, 34, 24),
            &app,
        );
        let body = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("Cache read"), "{body}");
        assert!(body.contains("Cache write"), "{body}");
        assert!(body.contains("14.1M"), "{body}");
        assert!(body.contains("Cache hit"), "{body}");
        assert!(body.contains("28.8x"), "{body}");
        assert!(!body.contains("█"), "{body}");
        assert!(!body.contains("·"), "{body}");
        assert!(!body.contains("●"), "{body}");
    }

    #[test]
    fn timeline_inspector_token_profile_never_adds_orphan_title() {
        let app = make_app(140);
        let mut lines = vec![Line::from("filled"); 8];
        let tokens = TokenBreakdown {
            input: 490_000,
            output: 78_000,
            cache_read: 14_100_000,
            cache_write: 0,
            reasoning: 0,
        };

        append_token_profile_section(
            &mut lines,
            "Token Mix",
            &tokens,
            6,
            Rect::new(0, 0, 34, 10),
            &app,
        );

        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| {
            !line
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("Token Mix"))
        }));
    }

    #[test]
    fn timeline_inspector_token_profile_fits_remaining_space() {
        let app = make_app(140);
        let mut lines = vec![Line::from("filled"); 7];
        let tokens = TokenBreakdown {
            input: 490_000,
            output: 78_000,
            cache_read: 14_100_000,
            cache_write: 0,
            reasoning: 0,
        };

        append_token_profile_section(
            &mut lines,
            "Token Mix",
            &tokens,
            6,
            Rect::new(0, 0, 34, 10),
            &app,
        );

        let body = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(lines.len(), 10);
        assert!(body.contains("Token Mix"), "{body}");
        assert!(body.contains("Input"), "{body}");
        assert!(!body.contains("Cache read"), "{body}");
    }

    #[test]
    fn medium_timeline_omits_single_provider_mix_from_inspector() {
        let mut app = make_app(110);
        let body = render_body(&mut app, 110, 24);

        assert!(body.contains("Selected Day"), "expected inspector\n{body}");
        assert!(
            !body.contains("Provider Mix"),
            "single provider mix should be omitted\n{body}"
        );
        assert!(body.contains("Top Models"), "expected top models\n{body}");
        assert!(body.contains("Token Mix"), "expected token mix\n{body}");
    }

    #[test]
    fn medium_timeline_keeps_multi_provider_mix_in_inspector() {
        let mut app = make_app(110);
        app.data.daily[0] = multi_provider_day(NaiveDate::from_ymd_opt(2026, 5, 29).unwrap());
        let body = render_body(&mut app, 110, 24);

        assert!(body.contains("Selected Day"), "expected inspector\n{body}");
        assert!(
            body.contains("Provider Mix"),
            "expected provider mix\n{body}"
        );
        assert!(body.contains("OpenAI"), "expected first provider\n{body}");
        assert!(
            body.contains("Anthropic"),
            "expected second provider\n{body}"
        );
    }

    #[test]
    fn timeline_inspector_skips_empty_mix_sections() {
        let mut app = make_app(140);
        app.data.daily = vec![DailyUsage {
            date: NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            source_breakdown: BTreeMap::new(),
            message_count: 0,
            turn_count: 0,
        }];
        let body = render_body(&mut app, 140, 24);

        assert!(body.contains("Selected Day"), "expected inspector\n{body}");
        assert!(
            !body.contains("Provider Mix"),
            "empty provider mix should be omitted\n{body}"
        );
        assert!(
            !body.contains("Token Mix"),
            "empty token mix should be omitted\n{body}"
        );
    }
}
