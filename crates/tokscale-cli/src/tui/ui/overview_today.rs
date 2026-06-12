use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, Table,
};
use std::collections::BTreeMap;

use super::widgets::{
    format_cost, format_tokens, get_provider_display_name, get_provider_shade, scrollbar_state,
    truncate_ellipsis as truncate_string,
};
use crate::tui::app::{App, ClickAction, PeriodDetailKey, SortDirection, SortField};
use chrono::{Local, NaiveDateTime, Timelike};

struct TodaySummary {
    now: NaiveDateTime,
    cost: f64,
    tokens: u64,
    projected_cost: f64,
    projected_tokens: u64,
    usual_cost: Option<f64>,
    last_activity: Option<NaiveDateTime>,
    last_source: Option<String>,
    peak_hour: Option<(NaiveDateTime, f64)>,
    model_count: usize,
}

struct TodayHourBucket {
    hour: u32,
    cost: f64,
    tokens: u64,
    segments: Vec<TodayHourSegment>,
    projected: bool,
}

struct TodayHourSegment {
    cost: f64,
    color: Color,
}

struct TodaySignal {
    label: &'static str,
    detail: String,
    value: String,
    context: String,
    detail_style: Style,
    value_style: Style,
}

struct TodayMomentumRow {
    label: String,
    detail: String,
    value: String,
    detail_style: Style,
    value_style: Style,
}

struct TodayProviderMixRow {
    label: String,
    color: Color,
    provider: String,
    tokens: u64,
    cost: f64,
}

struct TodayModelRowData {
    label: String,
    provider: String,
    color_key: String,
    tokens_total: u64,
    cost: f64,
    first_hour: Option<NaiveDateTime>,
    last_hour: Option<NaiveDateTime>,
    peak_hour: Option<NaiveDateTime>,
    peak_cost: f64,
    signal: String,
}

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let now = Local::now().naive_local();

    if area.width < 88 || area.height < 22 {
        render_today_compact_dashboard(frame, app, area, now);
        return;
    }

    let signals_height = if area.height >= 28 { 7 } else { 6 };
    let min_table_height = 8;
    let top_height = area
        .height
        .saturating_sub(signals_height + min_table_height)
        .clamp(10, 16);
    let live_height = top_height.saturating_add(signals_height);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(live_height), Constraint::Min(0)])
        .split(area);

    let summary_width = ((area.width as f64) * 0.30).round() as u16;
    let summary_width = summary_width.clamp(34, 48);
    let live = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(48), Constraint::Length(summary_width)])
        .split(chunks[0]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_height),
            Constraint::Length(signals_height),
        ])
        .split(live[0]);

    let summary = today_summary(app, now);
    render_today_pace_panel(frame, app, left[0], &summary);
    render_today_signals_panel(frame, app, left[1], &summary);
    render_today_summary_panel(frame, app, live[1], &summary);
    render_today_models_table(frame, app, chunks[1], &summary);
}

fn render_today_compact_dashboard(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    now: NaiveDateTime,
) {
    let chart_height = if area.height >= 18 { 9 } else { 7 };
    let strip_height = if area.height >= 14 { 2 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chart_height),
            Constraint::Length(strip_height),
            Constraint::Min(0),
        ])
        .split(area);

    let summary = today_summary(app, now);
    render_today_pace_panel(frame, app, chunks[0], &summary);
    render_today_live_strip(frame, app, chunks[1], &summary);
    render_today_models_table(frame, app, chunks[2], &summary);
}

fn today_summary(app: &App, now: NaiveDateTime) -> TodaySummary {
    let today = app.overview_date();
    let today_usage = app.today_usage();
    let cost = today_usage.map(|day| day.cost).unwrap_or(0.0);
    let tokens = today_usage.map(|day| day.tokens.total()).unwrap_or(0);
    let elapsed_hours = if now.date() == today {
        (now.hour() as f64 + now.minute() as f64 / 60.0 + now.second() as f64 / 3600.0)
            .clamp(0.25, 24.0)
    } else {
        24.0
    };
    let projection_ratio = if elapsed_hours < 24.0 {
        24.0 / elapsed_hours
    } else {
        1.0
    };
    let projected_cost = cost * projection_ratio;
    let projected_tokens = (tokens as f64 * projection_ratio).round() as u64;

    let mut previous_costs: Vec<_> = app
        .data
        .daily
        .iter()
        .filter(|day| day.date < today && (day.cost > 0.0 || day.tokens.total() > 0))
        .map(|day| (day.date, day.cost))
        .collect::<Vec<_>>();
    previous_costs.sort_by_key(|(date, _)| *date);
    let mut previous_costs: Vec<f64> = previous_costs
        .into_iter()
        .rev()
        .take(7)
        .map(|(_, cost)| cost)
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .collect();
    previous_costs.sort_by(|a, b| a.total_cmp(b));
    let usual_cost = median_value(&previous_costs);

    let mut today_hours: Vec<_> = app
        .data
        .hourly
        .iter()
        .filter(|hour| hour.datetime.date() == today)
        .collect();
    today_hours.sort_by_key(|hour| hour.datetime);

    let last = today_hours
        .iter()
        .rev()
        .find(|hour| hour.cost > 0.0 || hour.tokens.total() > 0);
    let last_activity = last.map(|hour| hour.datetime);
    let last_source = last.and_then(|hour| hour.clients.iter().next().cloned());
    let peak_hour = today_hours
        .iter()
        .filter(|hour| hour.cost.is_finite() && hour.cost > 0.0)
        .max_by(|a, b| a.cost.total_cmp(&b.cost))
        .map(|hour| (hour.datetime, hour.cost));

    TodaySummary {
        now,
        cost,
        tokens,
        projected_cost,
        projected_tokens,
        usual_cost,
        last_activity,
        last_source,
        peak_hour,
        model_count: app.overview_model_len(),
    }
}

fn median_value(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }

    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

fn today_attention_color(app: &App) -> Color {
    app.theme.color(Color::Rgb(245, 158, 11))
}

fn today_attention_style(app: &App) -> Style {
    Style::default()
        .fg(today_attention_color(app))
        .add_modifier(Modifier::BOLD)
}

fn today_positive_color(app: &App) -> Color {
    app.theme.color(Color::Rgb(74, 222, 128))
}

fn today_positive_style(app: &App) -> Style {
    Style::default()
        .fg(today_positive_color(app))
        .add_modifier(Modifier::BOLD)
}

fn today_live_color(app: &App) -> Color {
    app.theme.color(Color::Rgb(45, 212, 191))
}

fn today_blue_color(app: &App) -> Color {
    app.theme.color(Color::Rgb(96, 165, 250))
}

fn today_delta_style(app: &App, delta: f64) -> Style {
    if delta >= 25.0 {
        today_attention_style(app)
    } else if delta <= -25.0 {
        today_positive_style(app)
    } else {
        app.theme.secondary_text_style()
    }
}

fn render_today_pace_panel(frame: &mut Frame, app: &mut App, area: Rect, summary: &TodaySummary) {
    if area.height < 3 || area.width < 18 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Today · 24h Pace ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(
                    " Now {} · current pace {} ",
                    format_time(summary.now),
                    format_cost(summary.projected_cost)
                ),
                app.theme.subtle_text_style(),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_today_hour_chart(frame, app, inner, summary);
}

fn render_today_hour_chart(frame: &mut Frame, app: &mut App, area: Rect, summary: &TodaySummary) {
    if area.height < 5 || area.width < 28 {
        let text = Line::from(vec![
            Span::styled("Today ", app.theme.subtle_text_style()),
            Span::styled(
                format_cost(summary.cost),
                Style::default()
                    .fg(today_live_color(app))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · current pace ", app.theme.subtle_text_style()),
            Span::styled(
                format_cost(summary.projected_cost),
                today_attention_style(app),
            ),
        ]);
        frame.render_widget(Paragraph::new(text), area);
        return;
    }

    let buckets = today_hour_buckets(app, summary);
    let has_data = buckets
        .iter()
        .any(|bucket| bucket.cost > 0.0 || bucket.tokens > 0);
    if !has_data {
        let empty = if today_is_loading(app) {
            "Scanning session data...\nToday will populate when data loads."
        } else {
            "No hourly activity yet"
        };
        let empty = Paragraph::new(empty)
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, area);
        return;
    }

    let legend_height = if area.height >= 9 { 1 } else { 0 };
    let y_label_width: u16 = if area.width < 52 { 6 } else { 7 };
    let title_y = area.y;
    let plot_y = area.y.saturating_add(1);
    let label_y = area.y + area.height - 1 - legend_height;
    let plot_height = label_y.saturating_sub(plot_y).max(1);
    let plot_x = area.x.saturating_add(y_label_width);
    let plot_width = area.width.saturating_sub(y_label_width);

    let scale_max = buckets
        .iter()
        .map(|bucket| bucket.cost)
        .fold(0.0_f64, |a, b| a.max(b))
        .max(1.0);

    let buf = frame.buffer_mut();
    write_buf_text(
        buf,
        area.x,
        title_y,
        "Cost by hour",
        Style::default()
            .fg(app.theme.foreground)
            .add_modifier(Modifier::BOLD),
        area.width as usize,
    );
    if summary.projected_cost > summary.cost && area.width >= 58 {
        write_buf_text(
            buf,
            area.x.saturating_add(14),
            title_y,
            "dotted bars show current pace",
            app.theme.subtle_text_style(),
            area.width.saturating_sub(14) as usize,
        );
    }

    let mid_row = plot_height / 2;
    for row_from_top in 0..plot_height {
        let y = plot_y + row_from_top;
        let is_top = row_from_top == 0;
        let is_mid = row_from_top == mid_row && plot_height >= 4;
        let label = if is_top {
            format_cost(scale_max)
        } else if is_mid {
            format_cost(scale_max / 2.0)
        } else {
            String::new()
        };
        let padded = format!(
            "{:>width$}│",
            label,
            width = y_label_width.saturating_sub(1) as usize
        );
        write_buf_text(
            buf,
            area.x,
            y,
            &padded,
            Style::default().fg(app.theme.muted),
            y_label_width as usize,
        );
        let grid_char = if is_top || is_mid { '┈' } else { ' ' };
        for x in plot_x..plot_x.saturating_add(plot_width) {
            buf[(x, y)]
                .set_char(grid_char)
                .set_style(Style::default().fg(app.theme.muted));
        }
    }

    let axis_label = format!(
        "{:>width$}│",
        "$0",
        width = y_label_width.saturating_sub(1) as usize
    );
    write_buf_text(
        buf,
        area.x,
        label_y,
        &axis_label,
        Style::default().fg(app.theme.muted),
        y_label_width as usize,
    );
    for x in plot_x..plot_x.saturating_add(plot_width) {
        buf[(x, label_y)]
            .set_char('─')
            .set_style(Style::default().fg(app.theme.muted));
    }

    let slot_width = ((plot_width as usize) / 24).max(1);
    let bar_width = if slot_width >= 4 {
        3
    } else if slot_width >= 2 {
        slot_width - 1
    } else {
        1
    };
    let now_hour = if summary.now.date() == app.overview_date() {
        summary.now.hour().min(23) as usize
    } else {
        23
    };

    for bucket in &buckets {
        let offset = (bucket.hour as usize)
            .saturating_mul(slot_width)
            .saturating_add(slot_width.saturating_sub(bar_width) / 2);
        let x_start = plot_x.saturating_add(offset as u16);
        if x_start >= plot_x.saturating_add(plot_width) {
            continue;
        }

        let filled_height = ((bucket.cost / scale_max) * plot_height as f64)
            .round()
            .clamp(0.0, plot_height as f64) as u16;
        if filled_height == 0 {
            continue;
        }

        if bucket.projected {
            draw_today_bar_segment(
                buf,
                x_start,
                bar_width as u16,
                plot_y + plot_height - filled_height,
                filled_height,
                '░',
                Style::default().fg(app.theme.muted),
                plot_x + plot_width,
            );
            continue;
        }

        let mut segment_y_end = plot_y + plot_height;
        let mut remaining = filled_height;
        let mut segments = bucket.segments.iter().collect::<Vec<_>>();
        segments.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        if segments.is_empty() {
            draw_today_bar_segment(
                buf,
                x_start,
                bar_width as u16,
                plot_y + plot_height - filled_height,
                filled_height,
                '█',
                Style::default().fg(app.theme.highlight),
                plot_x + plot_width,
            );
            continue;
        }

        for (index, segment) in segments.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            let segment_height = if index == segments.len() - 1 {
                remaining
            } else {
                ((segment.cost / bucket.cost.max(0.01)) * filled_height as f64)
                    .round()
                    .clamp(1.0, remaining as f64) as u16
            };
            segment_y_end = segment_y_end.saturating_sub(segment_height);
            draw_today_bar_segment(
                buf,
                x_start,
                bar_width as u16,
                segment_y_end,
                segment_height,
                '█',
                Style::default().fg(segment.color),
                plot_x + plot_width,
            );
            remaining = remaining.saturating_sub(segment_height);
        }
    }

    let now_x = plot_x
        .saturating_add((now_hour.saturating_mul(slot_width)) as u16)
        .saturating_add((slot_width / 2) as u16)
        .min(plot_x.saturating_add(plot_width.saturating_sub(1)));
    for y in plot_y..plot_y.saturating_add(plot_height) {
        buf[(now_x, y)]
            .set_char('┆')
            .set_style(today_attention_style(app));
    }
    for hour in [0_u32, 4, 8, 12, 16, 20, 23] {
        let offset = (hour as usize)
            .saturating_mul(slot_width)
            .saturating_add(slot_width / 2);
        let x = plot_x.saturating_add(offset as u16);
        if x + 1 >= area.x + area.width {
            continue;
        }
        write_buf_text(
            buf,
            x.saturating_sub(1),
            label_y,
            &format!("{hour:02}"),
            Style::default().fg(app.theme.muted),
            2,
        );
    }

    if legend_height > 0 {
        render_today_chart_legend(
            frame,
            app,
            Rect::new(area.x, label_y + 1, area.width, 1),
            summary,
        );
    }

    if let Some(day_date) = app.today_usage().map(|day| day.date) {
        for bucket in buckets.iter().filter(|bucket| !bucket.projected) {
            let offset = (bucket.hour as usize).saturating_mul(slot_width);
            let width = slot_width.max(1) as u16;
            app.add_click_area(
                Rect::new(
                    plot_x.saturating_add(offset as u16),
                    plot_y,
                    width,
                    plot_height.saturating_add(1),
                ),
                ClickAction::OpenPeriodDetail(PeriodDetailKey::day(day_date)),
            );
        }
    }
}

fn write_buf_text(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, max_width: usize) {
    for (i, ch) in text.chars().take(max_width).enumerate() {
        buf[(x.saturating_add(i as u16), y)]
            .set_char(ch)
            .set_style(style);
    }
}

fn draw_today_bar_segment(
    buf: &mut Buffer,
    x_start: u16,
    width: u16,
    y_start: u16,
    height: u16,
    symbol: char,
    style: Style,
    x_limit: u16,
) {
    for y in y_start..y_start.saturating_add(height) {
        for dx in 0..width {
            let x = x_start.saturating_add(dx);
            if x < x_limit {
                buf[(x, y)].set_char(symbol).set_style(style);
            }
        }
    }
}

fn today_hour_buckets(app: &App, summary: &TodaySummary) -> Vec<TodayHourBucket> {
    let today = app.overview_date();
    let current_hour = if summary.now.date() == today {
        summary.now.hour().min(23)
    } else {
        23
    };
    let elapsed_hours = if summary.now.date() == today {
        (summary.now.hour() as f64
            + summary.now.minute() as f64 / 60.0
            + summary.now.second() as f64 / 3600.0)
            .clamp(0.25, 24.0)
    } else {
        24.0
    };
    let projected_cost_per_hour = if summary.cost > 0.0 && elapsed_hours < 24.0 {
        summary.cost / elapsed_hours
    } else {
        0.0
    };
    let projected_tokens_per_hour = if summary.tokens > 0 && elapsed_hours < 24.0 {
        summary.tokens as f64 / elapsed_hours
    } else {
        0.0
    };

    let mut buckets: Vec<TodayHourBucket> = (0..24)
        .map(|hour| TodayHourBucket {
            hour,
            cost: 0.0,
            tokens: 0,
            segments: Vec::new(),
            projected: false,
        })
        .collect();
    let mut segment_maps: Vec<BTreeMap<String, (f64, Color)>> =
        (0..24).map(|_| BTreeMap::new()).collect();

    for hour in app
        .data
        .hourly
        .iter()
        .filter(|hour| hour.datetime.date() == today)
    {
        let index = hour.datetime.hour().min(23) as usize;
        let bucket = &mut buckets[index];
        bucket.cost += hour.cost.max(0.0);
        bucket.tokens = bucket.tokens.saturating_add(hour.tokens.total());

        for info in hour.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let entry = segment_maps[index]
                .entry(provider.clone())
                .or_insert_with(|| (0.0, app.theme.color(get_provider_shade(&provider, 0))));
            entry.0 += info.cost.max(0.0);
        }
    }

    for (index, segment_map) in segment_maps.into_iter().enumerate() {
        buckets[index].segments = segment_map
            .into_values()
            .map(|(cost, color)| TodayHourSegment { cost, color })
            .collect();
    }

    if projected_cost_per_hour > 0.0 {
        for hour in (current_hour + 1)..24 {
            let bucket = &mut buckets[hour as usize];
            bucket.cost = projected_cost_per_hour;
            bucket.tokens = projected_tokens_per_hour.round() as u64;
            bucket.projected = true;
        }
    }

    buckets
}

fn render_today_chart_legend(frame: &mut Frame, app: &App, area: Rect, summary: &TodaySummary) {
    let providers = today_provider_mix(app, summary);
    if providers.is_empty() {
        return;
    }

    let mut spans = Vec::new();
    for (index, provider) in providers.iter().take(4).enumerate() {
        if index > 0 {
            spans.push(Span::styled("  · ", Style::default().fg(app.theme.muted)));
        }
        spans.push(Span::styled("●", Style::default().fg(provider.color)));
        spans.push(Span::raw(format!(
            " {}",
            truncate_string(&provider.label, if app.is_narrow() { 9 } else { 14 })
        )));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn today_provider_mix(app: &App, summary: &TodaySummary) -> Vec<TodayProviderMixRow> {
    let mut by_provider: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    for model in today_live_model_rows(app, summary) {
        let provider = crate::tui::colors::provider_color_key(&model.provider, &model.color_key);
        let entry = by_provider.entry(provider).or_insert((0, 0.0));
        entry.0 = entry.0.saturating_add(model.tokens_total);
        entry.1 += model.cost.max(0.0);
    }

    let mut rows = by_provider
        .into_iter()
        .map(|(provider, (tokens, cost))| TodayProviderMixRow {
            label: get_provider_display_name(&provider),
            color: app.theme.color(get_provider_shade(&provider, 0)),
            provider,
            tokens,
            cost,
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.cmp(&a.tokens))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn render_today_summary_panel(frame: &mut Frame, app: &App, area: Rect, summary: &TodaySummary) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Live Summary ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let baseline = summary
        .usual_cost
        .map(|usual| {
            let delta = if usual > 0.0 {
                ((summary.projected_cost / usual) - 1.0) * 100.0
            } else {
                0.0
            };
            format_delta_percent(delta)
        })
        .unwrap_or_else(|| "no baseline".to_string());
    let baseline_style = summary
        .usual_cost
        .map(|usual| {
            let delta = if usual > 0.0 {
                ((summary.projected_cost / usual) - 1.0) * 100.0
            } else {
                0.0
            };
            if delta >= 25.0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if delta <= -25.0 {
                Style::default()
                    .fg(today_positive_color(app))
                    .add_modifier(Modifier::BOLD)
            } else {
                app.theme.secondary_text_style()
            }
        })
        .unwrap_or_else(|| app.theme.secondary_text_style());

    let summary_lines = vec![
        today_metric_line(
            app,
            "Today",
            &format_cost(summary.cost),
            &format_tokens(summary.tokens),
            Style::default()
                .fg(today_live_color(app))
                .add_modifier(Modifier::BOLD),
        ),
        today_metric_line(
            app,
            "Current pace",
            &format_cost(summary.projected_cost),
            &format_tokens(summary.projected_tokens),
            today_attention_style(app),
        ),
        Line::from(vec![
            Span::styled("vs usual  ", app.theme.subtle_text_style()),
            Span::styled(baseline, baseline_style),
            Span::styled("  7d median", app.theme.subtle_text_style()),
        ]),
        Line::from(vec![
            Span::styled("Last      ", app.theme.subtle_text_style()),
            Span::styled(
                summary
                    .last_activity
                    .map(format_time)
                    .unwrap_or_else(|| "--:--".to_string()),
                Style::default().fg(app.theme.foreground),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                summary
                    .last_source
                    .as_deref()
                    .map(|source| truncate_string(source, 14))
                    .unwrap_or_else(|| "no activity".to_string()),
                app.theme.secondary_text_style(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Peak hour ", app.theme.subtle_text_style()),
            Span::styled(
                summary
                    .peak_hour
                    .map(|(hour, _)| format_hour(hour))
                    .unwrap_or_else(|| "--:00".to_string()),
                Style::default().fg(app.theme.foreground),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                summary
                    .peak_hour
                    .map(|(_, cost)| format_cost(cost))
                    .unwrap_or_else(|| "$0.00".to_string()),
                Style::default().fg(today_positive_color(app)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Models    ", app.theme.subtle_text_style()),
            Span::styled(
                summary.model_count.to_string(),
                Style::default().fg(app.theme.foreground),
            ),
        ]),
    ];

    if inner.height < 11 {
        frame.render_widget(Paragraph::new(summary_lines), inner);
        return;
    }

    let summary_height = summary_lines.len() as u16;
    frame.render_widget(
        Paragraph::new(summary_lines),
        Rect::new(inner.x, inner.y, inner.width, summary_height),
    );

    let momentum_heading_y = inner.y.saturating_add(summary_height + 1);
    if momentum_heading_y >= inner.y.saturating_add(inner.height) {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Momentum",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect::new(inner.x, momentum_heading_y, inner.width, 1),
    );

    let rows_y = momentum_heading_y.saturating_add(1);
    let rows_height = inner.y.saturating_add(inner.height).saturating_sub(rows_y);
    if rows_height == 0 {
        return;
    }

    let rows = today_momentum_rows(app, summary);
    if rows.is_empty() {
        let empty = if today_is_loading(app) {
            "Scanning session data..."
        } else {
            "No hourly momentum yet"
        };
        frame.render_widget(
            Paragraph::new(empty).style(Style::default().fg(app.theme.muted)),
            Rect::new(inner.x, rows_y, inner.width, rows_height),
        );
        return;
    }

    render_today_momentum_rows(
        frame,
        app,
        Rect::new(inner.x, rows_y, inner.width, rows_height),
        &rows[..rows.len().min(rows_height as usize)],
    );
}

fn today_metric_line(
    app: &App,
    label: &str,
    primary: &str,
    secondary: &str,
    primary_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<13}"), app.theme.subtle_text_style()),
        Span::styled(primary.to_string(), primary_style),
        Span::styled("  ", Style::default()),
        Span::styled(secondary.to_string(), app.theme.secondary_text_style()),
    ])
}

fn render_today_live_strip(frame: &mut Frame, app: &App, area: Rect, summary: &TodaySummary) {
    let line = Line::from(vec![
        Span::styled("Today ", app.theme.subtle_text_style()),
        Span::styled(
            format_cost(summary.cost),
            Style::default()
                .fg(today_live_color(app))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · current pace ", app.theme.subtle_text_style()),
        Span::styled(
            format_cost(summary.projected_cost),
            today_attention_style(app),
        ),
        Span::styled(" · models ", app.theme.subtle_text_style()),
        Span::styled(
            summary.model_count.to_string(),
            Style::default().fg(app.theme.foreground),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_today_signals_panel(frame: &mut Frame, app: &App, area: Rect, summary: &TodaySummary) {
    if area.height < 3 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Today's Signals ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let signals = today_signals(app, summary);
    if signals.is_empty() {
        let empty = if today_is_loading(app) {
            "Scanning session data..."
        } else {
            "No signals yet"
        };
        frame.render_widget(
            Paragraph::new(empty).style(Style::default().fg(app.theme.muted)),
            inner,
        );
        return;
    }

    let limit = inner.height as usize;
    render_today_signal_rows(frame, app, inner, &signals[..signals.len().min(limit)]);
}

fn today_signals(app: &App, summary: &TodaySummary) -> Vec<TodaySignal> {
    let mut signals = Vec::new();
    let top_model = today_live_model_rows(app, summary)
        .into_iter()
        .max_by(|a, b| a.cost.total_cmp(&b.cost));
    let top_provider = today_provider_mix(app, summary).into_iter().next();
    let top_source = today_top_source(app);

    if let Some(model) = top_model.as_ref() {
        signals.push(TodaySignal {
            label: "Cost leader",
            detail: signal_cost_leader_detail(model, top_source.as_ref(), top_provider.as_ref()),
            value: format_cost(model.cost),
            context: format!(
                "{} of cost",
                format_percent(model.cost / summary.cost.max(0.01))
            ),
            detail_style: Style::default()
                .fg(app.model_color_for(&model.provider, &model.color_key))
                .add_modifier(Modifier::BOLD),
            value_style: today_positive_style(app),
        });
    }

    if let Some((tokens, from, to)) = today_biggest_jump(app) {
        signals.push(TodaySignal {
            label: "Token surge",
            detail: format!("{from}->{to}"),
            value: format!("+{}", format_tokens(tokens)),
            context: format!(
                "{} of tokens",
                format_percent(tokens as f64 / summary.tokens.max(1) as f64)
            ),
            detail_style: app.theme.secondary_text_style(),
            value_style: today_attention_style(app),
        });
    }

    if let Some(model) = top_model.as_ref() {
        let provider_count = today_provider_mix(app, summary).len();
        let source_count = active_today_source_count(app);
        signals.push(TodaySignal {
            label: "Mix",
            detail: format!(
                "{} · {}",
                plural_count(provider_count, "provider"),
                plural_count(source_count, "source")
            ),
            value: plural_count(summary.model_count, "model"),
            context: format!(
                "{} leads {}",
                truncate_string(&model.label, 12),
                format_percent(model.cost / summary.cost.max(0.01))
            ),
            detail_style: app.theme.secondary_text_style(),
            value_style: Style::default()
                .fg(today_blue_color(app))
                .add_modifier(Modifier::BOLD),
        });
    }

    signals
}

fn signal_cost_leader_detail(
    model: &TodayModelRowData,
    top_source: Option<&(String, f64)>,
    top_provider: Option<&TodayProviderMixRow>,
) -> String {
    let mut parts = vec![model.label.clone()];
    if let Some((source, _)) = top_source {
        parts.push(source.clone());
    }
    if let Some(provider) = top_provider {
        if !parts.iter().any(|part| part == &provider.label) {
            parts.push(provider.label.clone());
        }
    }
    parts.join(" · ")
}

fn render_today_signal_rows(frame: &mut Frame, app: &App, area: Rect, signals: &[TodaySignal]) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label_width = if area.width >= 72 { 14 } else { 12 };
    let value_width = if area.width >= 44 { 12 } else { 8 };
    let context_width = if area.width >= 96 {
        22
    } else if area.width >= 76 {
        20
    } else if area.width >= 64 {
        16
    } else {
        0
    };
    let fixed_width = label_width + value_width + context_width + 3;
    let available_name_width = (area.width as usize).saturating_sub(fixed_width).max(6);
    let detail_width = available_name_width.min(if area.width >= 96 { 42 } else { 30 });
    let show_header = area.height as usize > signals.len();
    let row_start = usize::from(show_header);

    if show_header {
        let header_style = app.theme.subtle_text_style().add_modifier(Modifier::BOLD);
        let mut spans = vec![
            Span::styled(
                format!("{:<width$} ", "Signal", width = label_width),
                header_style,
            ),
            Span::styled(
                format!("{:<width$} ", "Detail", width = detail_width),
                header_style,
            ),
            Span::styled(
                format!("{:>width$} ", "Value", width = value_width),
                header_style,
            ),
        ];
        if context_width > 0 {
            spans.push(Span::styled(
                format!("{:<width$}", "Context", width = context_width),
                header_style,
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    for (index, signal) in signals.iter().enumerate() {
        let y_offset = index + row_start;
        if y_offset as u16 >= area.height {
            break;
        }
        let mut spans = Vec::new();
        spans.push(Span::styled(
            format!(
                "{:<width$} ",
                truncate_string(signal.label, label_width),
                width = label_width
            ),
            app.theme.subtle_text_style(),
        ));
        spans.push(Span::styled(
            format!(
                "{:<width$} ",
                truncate_string(&signal.detail, detail_width),
                width = detail_width
            ),
            signal.detail_style,
        ));
        spans.push(Span::styled(
            format!(
                "{:>width$}",
                truncate_string(&signal.value, value_width),
                width = value_width
            ),
            signal.value_style,
        ));
        spans.push(Span::raw(" "));
        if context_width > 0 {
            spans.push(Span::styled(
                format!(
                    "{:<width$}",
                    truncate_string(&signal.context, context_width),
                    width = context_width
                ),
                app.theme.secondary_text_style(),
            ));
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y + y_offset as u16, area.width, 1),
        );
    }
}

fn today_momentum_rows(app: &App, summary: &TodaySummary) -> Vec<TodayMomentumRow> {
    let today = app.overview_date();
    let mut hours = app
        .data
        .hourly
        .iter()
        .filter(|hour| {
            hour.datetime.date() == today && (hour.cost > 0.0 || hour.tokens.total() > 0)
        })
        .collect::<Vec<_>>();
    hours.sort_by_key(|hour| hour.datetime);

    let Some(last) = hours.last() else {
        return Vec::new();
    };

    let active_hour_count = hours.len().max(1) as f64;
    let average_cost = hours.iter().map(|hour| hour.cost.max(0.0)).sum::<f64>() / active_hour_count;
    let delta = if average_cost > 0.0 {
        ((last.cost / average_cost) - 1.0) * 100.0
    } else {
        0.0
    };

    let mut rows = vec![
        TodayMomentumRow {
            label: "Last hour".to_string(),
            detail: format_hour(last.datetime),
            value: format!(
                "{} / {}",
                format_cost(last.cost),
                format_tokens(last.tokens.total())
            ),
            detail_style: app.theme.secondary_text_style(),
            value_style: Style::default()
                .fg(today_live_color(app))
                .add_modifier(Modifier::BOLD),
        },
        TodayMomentumRow {
            label: "vs avg hour".to_string(),
            detail: format_cost(average_cost),
            value: format_delta_percent(delta),
            detail_style: app.theme.secondary_text_style(),
            value_style: today_delta_style(app, delta),
        },
    ];

    let latest_model = last
        .models
        .values()
        .max_by(|a, b| a.cost.total_cmp(&b.cost));
    let (latest_model, latest_model_style) = latest_model
        .map(|model| {
            (
                model.display_name.clone(),
                Style::default()
                    .fg(app.model_color_for(&model.provider, &model.color_key))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .unwrap_or_else(|| {
            (
                "unknown model".to_string(),
                app.theme.secondary_text_style(),
            )
        });
    let latest_source = last
        .clients
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| "unknown source".to_string());
    let recent = summary.now.date() == today
        && summary.now.signed_duration_since(last.datetime).num_hours() <= 1;

    rows.push(TodayMomentumRow {
        label: if recent {
            "Active now".to_string()
        } else {
            "Latest model".to_string()
        },
        detail: latest_model,
        value: latest_source,
        detail_style: latest_model_style,
        value_style: app.theme.secondary_text_style(),
    });

    rows
}

fn render_today_momentum_rows(frame: &mut Frame, app: &App, area: Rect, rows: &[TodayMomentumRow]) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label_width = if area.width >= 34 { 12 } else { 10 };
    let value_width = if area.width >= 38 { 15 } else { 12 };
    let fixed_width = label_width + value_width + 2;
    let detail_width = (area.width as usize).saturating_sub(fixed_width).max(6);

    for (index, row) in rows.iter().enumerate() {
        if index as u16 >= area.height {
            break;
        }

        let mut spans = Vec::new();
        spans.push(Span::styled(
            format!(
                "{:<width$} ",
                truncate_string(&row.label, label_width),
                width = label_width
            ),
            app.theme.subtle_text_style(),
        ));
        spans.push(Span::styled(
            format!(
                "{:<width$} ",
                truncate_string(&row.detail, detail_width),
                width = detail_width
            ),
            row.detail_style,
        ));
        spans.push(Span::styled(
            format!(
                "{:>width$}",
                truncate_string(&row.value, value_width),
                width = value_width
            ),
            row.value_style,
        ));

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y + index as u16, area.width, 1),
        );
    }
}

fn table_text_cell(text: impl Into<String>, style: Style) -> Cell<'static> {
    Cell::from(Span::styled(text.into(), style))
}

fn table_right_cell(text: impl Into<String>, style: Style) -> Cell<'static> {
    Cell::from(Line::from(Span::styled(text.into(), style)).right_aligned())
}

fn format_percent(ratio: f64) -> String {
    let percent = (ratio.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0);
    let percent = if percent >= 99.5 {
        "100".to_string()
    } else if percent >= 10.0 || percent == 0.0 {
        format!("{percent:.0}")
    } else {
        format!("{percent:.1}")
    };
    format!("{percent}%")
}

fn plural_count(count: usize, singular: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {singular}{suffix}")
}

fn today_top_source(app: &App) -> Option<(String, f64)> {
    let day = app.today_usage()?;
    day.source_breakdown
        .iter()
        .map(|(source, info)| (source.clone(), info.cost))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn active_today_source_count(app: &App) -> usize {
    app.today_usage()
        .map(|day| {
            day.source_breakdown
                .values()
                .filter(|source| source.cost > 0.0 || source.tokens.total() > 0)
                .count()
        })
        .unwrap_or(0)
}

fn today_biggest_jump(app: &App) -> Option<(u64, String, String)> {
    let today = app.overview_date();
    let mut hours: Vec<_> = app
        .data
        .hourly
        .iter()
        .filter(|hour| hour.datetime.date() == today)
        .collect();
    hours.sort_by_key(|hour| hour.datetime);

    hours
        .windows(2)
        .filter_map(|window| {
            let prev = window[0].tokens.total();
            let current = window[1].tokens.total();
            let jump = current.checked_sub(prev)?;
            Some((
                jump,
                format_hour(window[0].datetime),
                format_hour(window[1].datetime),
            ))
        })
        .max_by_key(|(jump, _, _)| *jump)
}

fn render_today_models_table(frame: &mut Frame, app: &mut App, area: Rect, summary: &TodaySummary) {
    if area.height < 3 || area.width < 20 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Today's Models ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(
                    " Sort: {} {} ",
                    today_sort_label(app.sort_field),
                    today_sort_direction_label(app.sort_direction)
                ),
                app.theme.subtle_text_style(),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = today_live_model_rows(app, summary);
    let page_capacity = inner.height.saturating_sub(1).max(1) as usize;
    app.set_max_visible_items(page_capacity);

    if rows.is_empty() {
        let empty = if today_is_loading(app) {
            "Scanning session data...\nToday's models will appear after load."
        } else {
            "No model activity today"
        };
        let empty = Paragraph::new(empty)
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let show_provider = inner.width >= 70;
    let show_times = inner.width >= 92;
    let show_signal = inner.width >= 112;
    let rank_width = if inner.width >= 42 { 4 } else { 0 };
    let provider_width = if show_provider { 14 } else { 0 };
    let cost_width = 10usize;
    let tokens_width = if inner.width >= 52 { 9 } else { 7 };
    let time_width = if show_times { 7 } else { 0 };
    let peak_width = if show_times { 14 } else { 0 };
    let signal_width = if show_signal { 18 } else { 0 };

    let header_style = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::BOLD);
    let mut widths = Vec::new();
    let mut header_cells = Vec::new();
    if rank_width > 0 {
        widths.push(Constraint::Length(rank_width as u16));
        header_cells.push(table_right_cell("#", header_style));
    }
    widths.push(Constraint::Min(8));
    header_cells.push(table_text_cell("Model", header_style));
    if show_provider {
        widths.push(Constraint::Length(provider_width as u16));
        header_cells.push(table_text_cell("Provider", header_style));
    }
    widths.push(Constraint::Length(cost_width as u16));
    header_cells.push(table_right_cell("Cost", header_style));
    widths.push(Constraint::Length(tokens_width as u16));
    header_cells.push(table_right_cell("Tokens", header_style));
    if show_times {
        widths.push(Constraint::Length(time_width as u16));
        header_cells.push(table_right_cell("First", header_style));
        widths.push(Constraint::Length(time_width as u16));
        header_cells.push(table_right_cell("Last", header_style));
        widths.push(Constraint::Length(peak_width as u16));
        header_cells.push(table_right_cell("Peak", header_style));
    }
    if show_signal {
        widths.push(Constraint::Length(signal_width as u16));
        header_cells.push(table_text_cell("Signal", header_style));
    }

    let start = app.scroll_offset.min(rows.len());
    let end = (start + page_capacity).min(rows.len());
    let table_rows = rows[start..end]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let idx = start + offset;
            let selected = idx == app.selected_index;
            let row_style = if selected {
                Style::default()
                    .bg(app.theme.selection)
                    .fg(app.theme.foreground)
            } else if offset % 2 == 1 {
                Style::default().bg(app.theme.color(Color::Rgb(10, 16, 22)))
            } else {
                Style::default()
            };

            let color = app.model_color_for(&row.provider, &row.color_key);
            let mut cells = Vec::new();
            if rank_width > 0 {
                let marker = if selected { "▶" } else { " " };
                cells.push(table_right_cell(
                    format!("{marker}{}", idx + 1),
                    Style::default().fg(if selected {
                        app.theme.foreground
                    } else {
                        app.theme.muted
                    }),
                ));
            }
            cells.push(Cell::from(Line::from(vec![
                Span::styled("● ".to_string(), Style::default().fg(color)),
                Span::styled(
                    row.label.clone(),
                    Style::default()
                        .fg(if selected {
                            app.theme.foreground
                        } else {
                            color
                        })
                        .add_modifier(Modifier::BOLD),
                ),
            ])));
            if show_provider {
                let provider =
                    crate::tui::colors::provider_color_key(&row.provider, &row.color_key);
                cells.push(table_text_cell(
                    get_provider_display_name(&provider),
                    app.theme.secondary_text_style(),
                ));
            }
            cells.push(table_right_cell(
                format_cost(row.cost),
                Style::default()
                    .fg(if selected {
                        app.theme.foreground
                    } else {
                        today_positive_color(app)
                    })
                    .add_modifier(Modifier::BOLD),
            ));
            cells.push(table_right_cell(
                format_tokens(row.tokens_total),
                app.theme.secondary_text_style(),
            ));
            if show_times {
                cells.push(table_right_cell(
                    format_optional_time(row.first_hour),
                    app.theme.secondary_text_style(),
                ));
                cells.push(table_right_cell(
                    format_optional_time(row.last_hour),
                    app.theme.secondary_text_style(),
                ));
                cells.push(table_right_cell(
                    format_peak(row.peak_hour, row.peak_cost),
                    today_attention_style(app),
                ));
            }
            if show_signal {
                cells.push(table_text_cell(
                    truncate_string(&row.signal, signal_width),
                    signal_style(app, &row.signal),
                ));
            }

            Row::new(cells).style(row_style).height(1)
        })
        .collect::<Vec<_>>();

    let table = Table::new(table_rows, widths).header(Row::new(header_cells));
    frame.render_widget(table, inner);

    for (offset, row) in rows[start..end].iter().enumerate() {
        let y = inner.y.saturating_add(1 + offset as u16);
        if y >= inner.y + inner.height {
            break;
        }

        app.add_click_area(
            Rect::new(inner.x, y, inner.width, 1),
            ClickAction::OpenModelDetail(crate::tui::app::ModelDetailKey {
                provider: row.provider.clone(),
                model: row.label.clone(),
                color_key: row.color_key.clone(),
            }),
        );
    }

    if rows.len() > page_capacity {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        let mut scrollbar_state = scrollbar_state(rows.len(), app.scroll_offset, page_capacity);
        frame.render_stateful_widget(
            scrollbar,
            inner.inner(Margin {
                horizontal: 0,
                vertical: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn today_live_model_rows(app: &App, summary: &TodaySummary) -> Vec<TodayModelRowData> {
    let today = app.overview_date();
    let mut rows_by_key: BTreeMap<(String, String, String), TodayModelRowData> = BTreeMap::new();

    for hour in app
        .data
        .hourly
        .iter()
        .filter(|hour| hour.datetime.date() == today)
    {
        for info in hour.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let key = (
                provider.clone(),
                info.display_name.clone(),
                info.color_key.clone(),
            );
            let row = rows_by_key.entry(key).or_insert_with(|| TodayModelRowData {
                label: info.display_name.clone(),
                provider,
                color_key: info.color_key.clone(),
                tokens_total: 0,
                cost: 0.0,
                first_hour: None,
                last_hour: None,
                peak_hour: None,
                peak_cost: 0.0,
                signal: String::new(),
            });

            row.tokens_total = row.tokens_total.saturating_add(info.tokens.total());
            row.cost += info.cost.max(0.0);
            row.first_hour = Some(match row.first_hour {
                Some(existing) => existing.min(hour.datetime),
                None => hour.datetime,
            });
            row.last_hour = Some(match row.last_hour {
                Some(existing) => existing.max(hour.datetime),
                None => hour.datetime,
            });
            if info.cost > row.peak_cost {
                row.peak_cost = info.cost;
                row.peak_hour = Some(hour.datetime);
            }
        }
    }

    let mut rows: Vec<TodayModelRowData> = rows_by_key.into_values().collect();
    if rows.is_empty() {
        rows = today_daily_model_rows(app);
    }

    apply_today_model_signals(summary, &mut rows);
    sort_today_model_rows(app, &mut rows);
    rows
}

fn today_daily_model_rows(app: &App) -> Vec<TodayModelRowData> {
    let Some(day) = app.today_usage() else {
        return Vec::new();
    };

    let mut rows_by_key: BTreeMap<(String, String, String), TodayModelRowData> = BTreeMap::new();
    for source in day.source_breakdown.values() {
        for info in source.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let key = (
                provider.clone(),
                info.display_name.clone(),
                info.color_key.clone(),
            );
            let row = rows_by_key.entry(key).or_insert_with(|| TodayModelRowData {
                label: info.display_name.clone(),
                provider,
                color_key: info.color_key.clone(),
                tokens_total: 0,
                cost: 0.0,
                first_hour: None,
                last_hour: None,
                peak_hour: None,
                peak_cost: 0.0,
                signal: "daily total".to_string(),
            });
            row.tokens_total = row.tokens_total.saturating_add(info.tokens.total());
            row.cost += info.cost.max(0.0);
            row.peak_cost = row.peak_cost.max(info.cost.max(0.0));
        }
    }

    rows_by_key.into_values().collect()
}

fn apply_today_model_signals(summary: &TodaySummary, rows: &mut [TodayModelRowData]) {
    let max_cost = rows
        .iter()
        .map(|row| row.cost)
        .fold(0.0_f64, |a, b| a.max(b));
    let usual_ratio = summary
        .usual_cost
        .filter(|usual| *usual > 0.0)
        .map(|usual| summary.projected_cost / usual);
    for row in rows {
        if row.signal == "daily total" {
            continue;
        }

        let recent = row
            .last_hour
            .map(|last| summary.now.signed_duration_since(last).num_hours() <= 2)
            .unwrap_or(false);
        let overnight = row
            .first_hour
            .zip(row.last_hour)
            .map(|(first, last)| first.hour() < 6 && last.hour() <= 10)
            .unwrap_or(false);
        let dominant = max_cost > 0.0 && row.cost >= max_cost * 0.9;

        row.signal = if dominant && usual_ratio.is_some_and(|ratio| ratio >= 1.25) {
            "above usual pace".to_string()
        } else if recent && row.peak_cost >= row.cost * 0.35 {
            "recent spike".to_string()
        } else if overnight {
            "overnight work".to_string()
        } else if recent {
            "active now".to_string()
        } else {
            "steady".to_string()
        };
    }
}

fn sort_today_model_rows(app: &App, rows: &mut [TodayModelRowData]) {
    let tie_breaker = |a: &TodayModelRowData, b: &TodayModelRowData| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.color_key.cmp(&b.color_key))
    };

    match (app.sort_field, app.sort_direction) {
        (SortField::Cost, SortDirection::Descending) => {
            rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Cost, SortDirection::Ascending) => {
            rows.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Tokens, SortDirection::Descending) => rows.sort_by(|a, b| {
            b.tokens_total
                .cmp(&a.tokens_total)
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Tokens, SortDirection::Ascending) => rows.sort_by(|a, b| {
            a.tokens_total
                .cmp(&b.tokens_total)
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Date, SortDirection::Descending) => rows.sort_by(|a, b| {
            b.last_hour
                .cmp(&a.last_hour)
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Date, SortDirection::Ascending) => rows.sort_by(|a, b| {
            a.last_hour
                .cmp(&b.last_hour)
                .then_with(|| tie_breaker(a, b))
        }),
    }
}

fn today_sort_label(sort_field: SortField) -> &'static str {
    match sort_field {
        SortField::Cost => "Cost",
        SortField::Tokens => "Tokens",
        SortField::Date => "Last",
    }
}

fn today_sort_direction_label(sort_direction: SortDirection) -> &'static str {
    match sort_direction {
        SortDirection::Ascending => "asc",
        SortDirection::Descending => "desc",
    }
}

fn today_is_loading(app: &App) -> bool {
    app.data.loading || (app.background_loading && !today_has_activity(app))
}

fn today_has_activity(app: &App) -> bool {
    app.today_usage()
        .map(|day| day.cost > 0.0 || day.tokens.total() > 0)
        .unwrap_or(false)
        || app.data.hourly.iter().any(|hour| {
            hour.datetime.date() == app.overview_date()
                && (hour.cost > 0.0 || hour.tokens.total() > 0)
        })
}

fn format_delta_percent(delta: f64) -> String {
    if delta.abs() < 0.5 {
        "flat".to_string()
    } else if delta > 0.0 {
        format!("+{delta:.0}%")
    } else {
        format!("{delta:.0}%")
    }
}

fn format_time(time: NaiveDateTime) -> String {
    time.format("%H:%M").to_string()
}

fn format_hour(time: NaiveDateTime) -> String {
    time.format("%H:00").to_string()
}

fn format_optional_time(time: Option<NaiveDateTime>) -> String {
    time.map(format_time).unwrap_or_else(|| "--:--".to_string())
}

fn format_peak(time: Option<NaiveDateTime>, cost: f64) -> String {
    match time {
        Some(time) => format!("{} {}", format_hour(time), format_cost(cost)),
        None => format_cost(cost),
    }
}

fn signal_style(app: &App, signal: &str) -> Style {
    if signal.contains("above") || signal.contains("spike") {
        today_attention_style(app)
    } else if signal.contains("active") {
        Style::default().fg(today_live_color(app))
    } else {
        app.theme.secondary_text_style()
    }
}
