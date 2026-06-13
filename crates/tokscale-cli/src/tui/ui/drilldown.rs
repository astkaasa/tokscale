use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation};
use std::collections::{BTreeMap, BTreeSet};

use super::mix::{ranking_bar_line, token_profile_lines};
use super::widgets::{
    format_cost, format_cost_per_million, format_tokens, get_client_display_name,
    get_provider_display_name, scrollbar_state, truncate_ascii as truncate,
};
use crate::tui::app::{
    App, ClickAction, DrilldownView, ModelDetailKey, ModelDetailPeriodRow, PeriodDetailKey,
    PeriodDetailModelRow, PeriodGranularity, SortDirection, SortField,
};
use crate::tui::data::TokenBreakdown;

struct SummaryItem {
    label: &'static str,
    compact_label: &'static str,
    value: String,
    compact_value: Option<String>,
}

struct PeriodTopModel {
    provider: String,
    model: String,
    color_key: String,
    tokens: TokenBreakdown,
    cost: f64,
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = app.drilldown_view().cloned() else {
        return;
    };

    match view {
        DrilldownView::Model(key) => render_model_detail(frame, app, area, &key),
        DrilldownView::Period(key) => render_period_detail(frame, app, area, &key),
    }
}

fn render_model_detail(frame: &mut Frame, app: &mut App, area: Rect, key: &ModelDetailKey) {
    let rows = app.get_sorted_model_detail_rows();
    let title = format!(" Model Detail  {} ", truncate(&key.model, 52));
    let inner = render_shell(frame, app, area, title, "Esc Back");

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if rows.is_empty() {
        render_empty(
            frame,
            app,
            inner,
            "No period breakdown found for this model.",
        );
        return;
    }

    let top_height = if inner.height >= 18 { 10 } else { 7 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_height), Constraint::Min(0)])
        .split(inner);
    if inner.width < 64 {
        render_model_summary(frame, app, chunks[0], key, &rows);
    } else if inner.width >= 104 {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(equal_column_constraints(3))
            .split(chunks[0]);

        render_model_summary(frame, app, top[0], key, &rows);
        render_model_mix(frame, app, top[1], &rows);
        render_model_top_periods(frame, app, top[2], key, &rows);
    } else {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(equal_column_constraints(2))
            .split(chunks[0]);

        render_model_summary(frame, app, top[0], key, &rows);
        render_model_mix(frame, app, top[1], &rows);
    }
    render_model_breakdown(frame, app, chunks[1], &rows);
}

fn render_period_detail(frame: &mut Frame, app: &mut App, area: Rect, key: &PeriodDetailKey) {
    let rows = app.get_sorted_period_detail_rows();
    let title = format!(
        " Period Detail  {}  {} ",
        key.label,
        key.granularity.label()
    );
    let inner = render_shell(frame, app, area, title, "Esc Back");

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if rows.is_empty() {
        render_empty(
            frame,
            app,
            inner,
            "No model breakdown found for this period.",
        );
        return;
    }

    let top_height = if inner.height >= 18 { 10 } else { 7 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_height), Constraint::Min(0)])
        .split(inner);
    let has_provider_mix = period_provider_count(&rows) > 1;
    if inner.width < 64 {
        render_period_summary(frame, app, chunks[0], key);
    } else if inner.width >= 112 && has_provider_mix {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(equal_column_constraints(4))
            .split(chunks[0]);

        render_period_summary(frame, app, top[0], key);
        render_period_mix(frame, app, top[1], &rows);
        render_period_token_mix(frame, app, top[2], &rows);
        render_period_top_models(frame, app, top[3], &rows);
    } else {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(equal_column_constraints(3))
            .split(chunks[0]);

        render_period_summary(frame, app, top[0], key);
        if has_provider_mix {
            render_period_mix(frame, app, top[1], &rows);
            render_period_token_mix(frame, app, top[2], &rows);
        } else {
            render_period_token_mix(frame, app, top[1], &rows);
            render_period_top_models(frame, app, top[2], &rows);
        }
    }
    render_period_breakdown(frame, app, chunks[1], &rows);
}

fn render_shell(frame: &mut Frame, app: &App, area: Rect, title: String, right: &str) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            title,
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(" {right} "),
                app.theme.subtle_text_style(),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn render_empty(frame: &mut Frame, app: &App, area: Rect, message: &str) {
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center),
        area,
    );
}

fn render_panel(frame: &mut Frame, app: &App, area: Rect, title: &str) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn equal_column_constraints(count: usize) -> Vec<Constraint> {
    vec![Constraint::Ratio(1, count as u32); count]
}

fn render_model_summary(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    key: &ModelDetailKey,
    rows: &[ModelDetailPeriodRow],
) {
    let inner = render_panel(frame, app, area, "Summary");
    let (tokens, cost, messages) = model_totals(rows);
    let active_days = rows
        .iter()
        .map(|row| row.date)
        .collect::<BTreeSet<_>>()
        .len();
    let sources = rows
        .iter()
        .map(|row| row.source.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let provider = get_provider_display_name(&key.provider);

    let coverage = format!("{active_days} days · {sources} sources");
    let compact_coverage = format!("{active_days}d / {sources}src");
    let lines = summary_lines(
        app,
        inner.width,
        vec![
            summary_item("Model", "Model", key.model.clone(), None),
            summary_item("Provider", "Prov", provider, None),
            summary_item("Cost", "Cost", format_cost(cost), None),
            summary_item("Tokens", "Tok", format_tokens(tokens.total()), None),
            summary_item("Messages", "Msgs", messages.to_string(), None),
            summary_item("Coverage", "Seen", coverage, Some(compact_coverage)),
            summary_item(
                "Efficiency",
                "$/1M",
                format_cost_per_million(cost, tokens.total()),
                None,
            ),
        ],
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_model_mix(frame: &mut Frame, app: &App, area: Rect, rows: &[ModelDetailPeriodRow]) {
    let inner = render_panel(frame, app, area, "Token Mix");
    let (tokens, _, _) = model_totals(rows);
    render_token_mix(frame, app, inner, &tokens);
}

fn render_model_top_periods(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    key: &ModelDetailKey,
    rows: &[ModelDetailPeriodRow],
) {
    let inner = render_panel(frame, app, area, "Top Periods");
    let total = rows
        .iter()
        .map(|row| row.cost.max(0.0))
        .sum::<f64>()
        .max(0.01);
    let mut top_rows = rows.iter().collect::<Vec<_>>();
    top_rows.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
    });

    let color = app.model_color_for(&key.provider, &key.color_key);
    let mut lines = Vec::new();
    for row in top_rows.into_iter().take(inner.height as usize) {
        lines.push(ranking_bar_line(
            &row.date.format("%m-%d").to_string(),
            &format_cost(row.cost),
            row.cost.max(0.0) / total,
            color,
            inner.width,
            app,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_period_summary(frame: &mut Frame, app: &App, area: Rect, key: &PeriodDetailKey) {
    let inner = render_panel(frame, app, area, "Summary");
    let days = app.period_days(key);
    let mut tokens = TokenBreakdown::default();
    let mut cost = 0.0;
    let mut messages = 0u64;
    let mut turns = 0u64;
    let mut sources = BTreeSet::new();
    for day in days {
        add_tokens(&mut tokens, &day.tokens);
        cost += day.cost.max(0.0);
        messages = messages.saturating_add(day.message_count as u64);
        turns = turns.saturating_add(day.turn_count as u64);
        sources.extend(day.source_breakdown.keys().cloned());
    }

    let range = format!("{}..{}", key.start.format("%m-%d"), key.end.format("%m-%d"));
    let compact_period = compact_period_label(key);
    let compact_range = compact_period_range(key);
    let lines = summary_lines(
        app,
        inner.width,
        vec![
            summary_item("Period", "When", key.label.clone(), Some(compact_period)),
            summary_item("Range", "Span", range, Some(compact_range)),
            summary_item("Cost", "Cost", format_cost(cost), None),
            summary_item("Tokens", "Tok", format_tokens(tokens.total()), None),
            summary_item("Messages", "Msgs", messages.to_string(), None),
            summary_item("Turns", "Turn", turns.to_string(), None),
            summary_item("Sources", "Src", sources.len().to_string(), None),
        ],
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

fn compact_period_label(key: &PeriodDetailKey) -> String {
    match key.granularity {
        PeriodGranularity::Day => key.start.format("%m-%d").to_string(),
        PeriodGranularity::Week | PeriodGranularity::Month => key.label.clone(),
    }
}

fn compact_period_range(key: &PeriodDetailKey) -> String {
    if key.start == key.end {
        return "1 day".to_string();
    }
    format!("{}..{}", key.start.format("%m-%d"), key.end.format("%m-%d"))
}

fn period_provider_count(rows: &[PeriodDetailModelRow]) -> usize {
    rows.iter()
        .map(|row| row.provider.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn render_period_mix(frame: &mut Frame, app: &App, area: Rect, rows: &[PeriodDetailModelRow]) {
    let inner = render_panel(frame, app, area, "Provider Mix");
    let mut by_provider: BTreeMap<&str, f64> = BTreeMap::new();
    for row in rows {
        *by_provider.entry(&row.provider).or_default() += row.cost.max(0.0);
    }
    let mut providers = by_provider.into_iter().collect::<Vec<_>>();
    providers.sort_by(|a, b| b.1.total_cmp(&a.1));

    let lines = provider_mix_lines(app, inner.width, inner.height as usize, providers);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_period_top_models(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    rows: &[PeriodDetailModelRow],
) {
    let inner = render_panel(frame, app, area, "Top Models");
    let top_models = period_top_models(rows);
    let total = top_models
        .iter()
        .map(|row| row.cost.max(0.0))
        .sum::<f64>()
        .max(0.01);
    let mut lines = Vec::new();
    for row in top_models.iter().take(inner.height as usize) {
        let color = app.model_color_for(&row.provider, &row.color_key);
        lines.push(ranking_bar_line(
            &row.model,
            &format_cost(row.cost),
            row.cost.max(0.0) / total,
            color,
            inner.width,
            app,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_period_token_mix(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    rows: &[PeriodDetailModelRow],
) {
    let inner = render_panel(frame, app, area, "Token Mix");
    let mut tokens = TokenBreakdown::default();
    for row in rows {
        add_tokens(&mut tokens, &row.tokens);
    }
    render_token_mix(frame, app, inner, &tokens);
}

fn render_token_mix(frame: &mut Frame, app: &App, area: Rect, tokens: &TokenBreakdown) {
    let lines = token_profile_lines(app, area.width, area.height as usize, tokens);
    frame.render_widget(Paragraph::new(lines), area);
}

fn provider_mix_lines(
    app: &App,
    width: u16,
    max_lines: usize,
    providers: Vec<(&str, f64)>,
) -> Vec<Line<'static>> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let providers = providers
        .into_iter()
        .filter(|(_, cost)| *cost > 0.0)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return vec![Line::from(Span::styled(
            truncate("No mix data", width as usize),
            app.theme.subtle_text_style(),
        ))];
    }

    let total = providers.iter().map(|(_, cost)| cost.max(0.0)).sum::<f64>();
    let visible = if providers.len() > max_lines && max_lines > 1 {
        max_lines - 1
    } else {
        max_lines
    };
    let mut lines = Vec::new();
    for (provider, cost) in providers.iter().take(visible) {
        let color = app
            .theme
            .color(super::widgets::get_provider_shade(provider, 0));
        let ratio = if total > 0.0 {
            cost.max(0.0) / total
        } else {
            0.0
        };
        lines.push(provider_mix_line(
            &get_provider_display_name(provider),
            *cost,
            ratio,
            color,
            width,
            app,
        ));
    }

    if providers.len() > visible && lines.len() < max_lines {
        let hidden_count = providers.len() - visible;
        let hidden_cost = providers[visible..]
            .iter()
            .map(|(_, cost)| cost.max(0.0))
            .sum::<f64>();
        let hidden_ratio = if total > 0.0 {
            hidden_cost / total
        } else {
            0.0
        };
        lines.push(provider_more_line(
            hidden_count,
            hidden_cost,
            hidden_ratio,
            width,
            app,
        ));
    }

    lines
}

fn provider_mix_line(
    label: &str,
    cost: f64,
    ratio: f64,
    color: Color,
    width: u16,
    app: &App,
) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    let value = format_cost(cost);
    let percent = format_share_percent(ratio);
    if width < 18 {
        return detail_value_line(label, &value, color, width, app);
    }

    let marker_width = usize::from(width >= 22) * 2;
    let value_width = value.chars().count().min(9);
    let percent_width = if width >= 24 {
        percent.chars().count().clamp(3, 4)
    } else {
        0
    };
    let reserved =
        marker_width + value_width + usize::from(percent_width > 0) * (percent_width + 1) + 1;

    if reserved >= width {
        return detail_value_line(label, &value, color, width, app);
    }

    let label_width = width - reserved;
    let mut spans = Vec::new();
    if marker_width > 0 {
        spans.push(Span::styled("●", Style::default().fg(color)));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        pad_exact(label, label_width),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    if percent_width > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            pad_exact(&percent, percent_width),
            app.theme.subtle_text_style(),
        ));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        pad_exact(&value, value_width),
        Style::default().fg(app.theme.foreground),
    ));

    Line::from(spans)
}

fn provider_more_line(
    hidden_count: usize,
    hidden_cost: f64,
    hidden_ratio: f64,
    width: u16,
    app: &App,
) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    let label = format!("+{hidden_count} more");
    let value = format!(
        "{} {}",
        format_share_percent(hidden_ratio),
        format_cost(hidden_cost)
    );
    detail_value_line(&label, &value, app.theme.muted, width, app)
}

fn detail_value_line(
    label: &str,
    value: &str,
    color: Color,
    width: usize,
    app: &App,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let value_width = value.chars().count();
    if width <= value_width + 1 {
        return Line::from(Span::styled(
            truncate(label, width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    let label_width = width.saturating_sub(value_width + 1);
    Line::from(vec![
        Span::styled(
            pad_exact(label, label_width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(app.theme.foreground)),
    ])
}

fn format_share_percent(ratio: f64) -> String {
    let percent = (ratio * 100.0).max(0.0);
    if percent > 0.0 && percent < 1.0 {
        "<1%".to_string()
    } else {
        format!("{percent:.0}%")
    }
}

fn render_model_breakdown(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    rows: &[ModelDetailPeriodRow],
) {
    let inner = render_panel(frame, app, area, "Breakdown");
    if inner.height == 0 {
        return;
    }
    let page_capacity = inner.height.saturating_sub(1).max(1) as usize;
    app.set_max_visible_items(page_capacity);

    render_model_breakdown_header(frame, app, inner);
    let start = app.scroll_offset.min(rows.len().saturating_sub(1));
    let end = (start + page_capacity).min(rows.len());
    let mut y = inner.y.saturating_add(1);
    for (offset, row) in rows[start..end].iter().enumerate() {
        let index = start + offset;
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        app.add_click_area(
            row_area,
            ClickAction::OpenPeriodDetail(PeriodDetailKey::day(row.date)),
        );
        render_model_breakdown_row(frame, app, row_area, row, index);
        y = y.saturating_add(1);
    }
    render_scrollbar(frame, app, area, rows.len(), page_capacity);
}

fn render_period_breakdown(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    rows: &[PeriodDetailModelRow],
) {
    let inner = render_panel(frame, app, area, "Model Breakdown");
    if inner.height == 0 {
        return;
    }
    let page_capacity = inner.height.saturating_sub(1).max(1) as usize;
    app.set_max_visible_items(page_capacity);

    render_period_breakdown_header(frame, app, inner);
    let start = app.scroll_offset.min(rows.len().saturating_sub(1));
    let end = (start + page_capacity).min(rows.len());
    let mut y = inner.y.saturating_add(1);
    for (offset, row) in rows[start..end].iter().enumerate() {
        let index = start + offset;
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        app.add_click_area(
            row_area,
            ClickAction::OpenModelDetail(ModelDetailKey {
                provider: row.provider.clone(),
                model: row.model.clone(),
                color_key: row.color_key.clone(),
            }),
        );
        render_period_breakdown_row(frame, app, row_area, row, index);
        y = y.saturating_add(1);
    }
    render_scrollbar(frame, app, area, rows.len(), page_capacity);
}

fn render_model_breakdown_header(frame: &mut Frame, app: &App, inner: Rect) {
    let style = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    spans.push(Span::styled(pad_right("#", 4), style));
    spans.push(Span::styled(
        pad_left(&format!("Date{}", sort_indicator(app, SortField::Date)), 12),
        style,
    ));
    spans.push(Span::styled(pad_left("Source", 14), style));
    spans.push(Span::styled(
        pad_right(&format!("Cost{}", sort_indicator(app, SortField::Cost)), 10),
        style,
    ));
    spans.push(Span::styled(
        pad_right(
            &format!("Tokens{}", sort_indicator(app, SortField::Tokens)),
            10,
        ),
        style,
    ));
    if inner.width >= 96 {
        spans.push(Span::styled(pad_right("Input", 10), style));
        spans.push(Span::styled(pad_right("Output", 10), style));
        spans.push(Span::styled(pad_right("Cache", 10), style));
        spans.push(Span::styled(pad_right("Msgs", 8), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

fn render_period_breakdown_header(frame: &mut Frame, app: &App, inner: Rect) {
    let style = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::BOLD);
    let model_width = period_breakdown_model_width(inner.width);
    let mut spans = Vec::new();
    spans.push(Span::styled(pad_right("#", 4), style));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(period_model_cell("Model", model_width), style));
    if period_breakdown_provider_visible(inner.width) {
        spans.push(Span::styled(pad_left("Provider", 14), style));
        spans.push(Span::styled(pad_left("Source", 12), style));
    }
    if period_breakdown_metrics_visible(inner.width) {
        spans.push(Span::styled(
            pad_right(&format!("Cost{}", sort_indicator(app, SortField::Cost)), 10),
            style,
        ));
        spans.push(Span::styled(
            pad_right(
                &format!("Tokens{}", sort_indicator(app, SortField::Tokens)),
                10,
            ),
            style,
        ));
    }
    if inner.width >= 116 {
        spans.push(Span::styled(pad_right("Input", 10), style));
        spans.push(Span::styled(pad_right("Output", 10), style));
        spans.push(Span::styled(pad_right("Cache", 10), style));
        spans.push(Span::styled(pad_right("Msgs", 8), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

fn render_model_breakdown_row(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    row: &ModelDetailPeriodRow,
    index: usize,
) {
    let selected = index == app.selected_index;
    let row_style = row_style(app, index, selected);
    frame.render_widget(Paragraph::new("").style(row_style), area);

    let mut spans = vec![
        Span::styled(
            pad_right(&row_marker(index, selected), 4),
            subtle_or_selected(app, selected),
        ),
        Span::styled(
            pad_left(&row.date.to_string(), 12),
            subtle_or_selected(app, selected),
        ),
        Span::styled(
            pad_left(&get_client_display_name(&row.source), 14),
            subtle_or_selected(app, selected),
        ),
        Span::styled(
            pad_right(&format_cost(row.cost), 10),
            metric_style(app, selected, Color::Green),
        ),
        Span::styled(
            pad_right(&format_tokens(row.tokens.total()), 10),
            subtle_or_selected(app, selected),
        ),
    ];
    if area.width >= 96 {
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.input), 10),
            metric_style(app, selected, Color::Rgb(96, 165, 250)),
        ));
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.output), 10),
            metric_style(app, selected, Color::Rgb(74, 222, 128)),
        ));
        let cache = row.tokens.cache_read.saturating_add(row.tokens.cache_write);
        spans.push(Span::styled(
            pad_right(&format_tokens(cache), 10),
            metric_style(app, selected, Color::Rgb(167, 139, 250)),
        ));
        spans.push(Span::styled(
            pad_right(&row.messages.to_string(), 8),
            subtle_or_selected(app, selected),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), area);
}

fn render_period_breakdown_row(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    row: &PeriodDetailModelRow,
    index: usize,
) {
    let selected = index == app.selected_index;
    let row_style = row_style(app, index, selected);
    frame.render_widget(Paragraph::new("").style(row_style), area);

    let color = app.model_color_for(&row.provider, &row.color_key);
    let model_width = period_breakdown_model_width(area.width);
    let mut spans = Vec::new();
    spans.push(Span::styled(
        pad_right(&row_marker(index, selected), 4),
        subtle_or_selected(app, selected),
    ));
    spans.push(Span::styled("● ", Style::default().fg(color)));
    spans.push(Span::styled(
        period_model_cell(&row.model, model_width),
        metric_style(app, selected, color).add_modifier(Modifier::BOLD),
    ));
    if period_breakdown_provider_visible(area.width) {
        spans.push(Span::styled(
            pad_left(&get_provider_display_name(&row.provider), 14),
            subtle_or_selected(app, selected),
        ));
        spans.push(Span::styled(
            pad_left(&get_client_display_name(&row.source), 12),
            subtle_or_selected(app, selected),
        ));
    }
    if period_breakdown_metrics_visible(area.width) {
        spans.push(Span::styled(
            pad_right(&format_cost(row.cost), 10),
            metric_style(app, selected, Color::Green),
        ));
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.total()), 10),
            subtle_or_selected(app, selected),
        ));
    }
    if area.width >= 116 {
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.input), 10),
            metric_style(app, selected, Color::Rgb(96, 165, 250)),
        ));
        spans.push(Span::styled(
            pad_right(&format_tokens(row.tokens.output), 10),
            metric_style(app, selected, Color::Rgb(74, 222, 128)),
        ));
        let cache = row.tokens.cache_read.saturating_add(row.tokens.cache_write);
        spans.push(Span::styled(
            pad_right(&format_tokens(cache), 10),
            metric_style(app, selected, Color::Rgb(167, 139, 250)),
        ));
        spans.push(Span::styled(
            pad_right(&row.messages.to_string(), 8),
            subtle_or_selected(app, selected),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), area);
}

fn render_scrollbar(frame: &mut Frame, app: &App, area: Rect, len: usize, page_capacity: usize) {
    if len <= page_capacity {
        return;
    }

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"))
        .track_symbol(Some("│"))
        .thumb_symbol("█");
    let mut state = scrollbar_state(len, app.scroll_offset, page_capacity);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

fn model_totals(rows: &[ModelDetailPeriodRow]) -> (TokenBreakdown, f64, u64) {
    let mut tokens = TokenBreakdown::default();
    let mut cost = 0.0;
    let mut messages = 0u64;
    for row in rows {
        add_tokens(&mut tokens, &row.tokens);
        if row.cost.is_finite() {
            cost += row.cost;
        }
        messages = messages.saturating_add(row.messages);
    }
    (tokens, cost, messages)
}

fn period_top_models(rows: &[PeriodDetailModelRow]) -> Vec<PeriodTopModel> {
    let mut by_model: BTreeMap<(String, String, String), PeriodTopModel> = BTreeMap::new();
    for row in rows {
        let entry = by_model
            .entry((
                row.provider.clone(),
                row.model.clone(),
                row.color_key.clone(),
            ))
            .or_insert_with(|| PeriodTopModel {
                provider: row.provider.clone(),
                model: row.model.clone(),
                color_key: row.color_key.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
            });
        if row.cost.is_finite() {
            entry.cost += row.cost.max(0.0);
        }
        add_tokens(&mut entry.tokens, &row.tokens);
    }

    let mut rows = by_model.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn add_tokens(target: &mut TokenBreakdown, source: &TokenBreakdown) {
    target.input = target.input.saturating_add(source.input);
    target.output = target.output.saturating_add(source.output);
    target.cache_read = target.cache_read.saturating_add(source.cache_read);
    target.cache_write = target.cache_write.saturating_add(source.cache_write);
    target.reasoning = target.reasoning.saturating_add(source.reasoning);
}

fn summary_item(
    label: &'static str,
    compact_label: &'static str,
    value: String,
    compact_value: Option<String>,
) -> SummaryItem {
    SummaryItem {
        label,
        compact_label,
        value,
        compact_value,
    }
}

fn summary_lines(app: &App, width: u16, items: Vec<SummaryItem>) -> Vec<Line<'static>> {
    items
        .into_iter()
        .map(|item| {
            summary_line(
                item.label,
                item.compact_label,
                &item.value,
                item.compact_value.as_deref(),
                width,
                app,
            )
        })
        .collect()
}

fn summary_line(
    label: &str,
    compact_label: &str,
    value: &str,
    compact_value: Option<&str>,
    width: u16,
    app: &App,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let compact = width < 36;
    let label = if compact { compact_label } else { label };
    let value = if compact {
        compact_value.unwrap_or(value)
    } else {
        value
    };
    let label_width = if compact { 7 } else { 12 };
    let label_width = label_width.min(width.saturating_sub(4) as usize).max(3);
    let value_width = width.saturating_sub((label_width + 1) as u16) as usize;

    Line::from(vec![
        Span::styled(pad_left(label, label_width), app.theme.subtle_text_style()),
        Span::styled(
            truncate(value, value_width),
            Style::default().fg(app.theme.foreground),
        ),
    ])
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

fn row_marker(index: usize, selected: bool) -> String {
    if selected {
        format!("▶{}", index + 1)
    } else {
        format!(" {}", index + 1)
    }
}

fn row_style(app: &App, index: usize, selected: bool) -> Style {
    if selected {
        Style::default()
            .bg(app.theme.selection)
            .fg(app.theme.foreground)
    } else if index % 2 == 1 {
        app.theme.striped_row_style()
    } else {
        Style::default()
    }
}

fn metric_style(app: &App, selected: bool, color: Color) -> Style {
    if selected {
        Style::default().fg(app.theme.foreground)
    } else {
        Style::default().fg(color)
    }
}

fn subtle_or_selected(app: &App, selected: bool) -> Style {
    if selected {
        Style::default().fg(app.theme.foreground)
    } else {
        app.theme.secondary_text_style()
    }
}

fn detail_model_width(width: u16) -> usize {
    if width >= 124 {
        32
    } else if width >= 92 {
        24
    } else {
        20
    }
}

fn period_breakdown_metrics_visible(width: u16) -> bool {
    width >= 52
}

fn period_breakdown_provider_visible(width: u16) -> bool {
    width >= 92
}

fn period_breakdown_model_width(width: u16) -> usize {
    let width = width as usize;
    let marker_width = 5;
    let bullet_width = 2;
    if width <= marker_width + bullet_width {
        return 1;
    }

    if !period_breakdown_metrics_visible(width as u16) {
        return width.saturating_sub(marker_width + bullet_width).max(1);
    }

    let provider_width = usize::from(period_breakdown_provider_visible(width as u16)) * (15 + 13);
    let metrics_width = 11 + 11;
    let separator_guard = 1;
    width
        .saturating_sub(
            marker_width + bullet_width + provider_width + metrics_width + separator_guard,
        )
        .min(detail_model_width(width as u16))
        .max(8)
}

fn period_model_cell(text: &str, width: usize) -> String {
    pad_exact(text, width)
}

fn pad_left(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:<width$} ")
}

fn pad_exact(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:<width$}")
}

fn pad_right(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:>width$} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiConfig;
    use crate::tui::data::{
        DailyModelInfo, DailySourceInfo, DailyUsage, TokenBreakdown, UsageData,
    };
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};
    use std::collections::BTreeMap;

    fn test_app() -> App {
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
        App::new_with_cached_data(config, Some(UsageData::default())).unwrap()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn char_position(line: &str, needle: &str) -> Option<usize> {
        line.find(needle)
            .map(|byte_index| line[..byte_index].chars().count())
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
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn daily_usage(date: NaiveDate) -> DailyUsage {
        daily_usage_with_model(date, "gpt-5")
    }

    fn daily_usage_with_model(date: NaiveDate, model: &str) -> DailyUsage {
        let tokens = TokenBreakdown {
            input: 10_000,
            output: 1_000,
            cache_read: 20_000,
            cache_write: 2_000,
            reasoning: 0,
        };
        let mut models = BTreeMap::new();
        models.insert(
            model.to_string(),
            DailyModelInfo {
                provider: "openai".to_string(),
                display_name: model.to_string(),
                color_key: model.to_string(),
                tokens: tokens.clone(),
                cost: 12.34,
                messages: 5,
            },
        );
        let mut source_breakdown = BTreeMap::new();
        source_breakdown.insert(
            "codex".to_string(),
            DailySourceInfo {
                tokens: tokens.clone(),
                cost: 12.34,
                models,
            },
        );
        DailyUsage {
            date,
            tokens,
            cost: 12.34,
            source_breakdown,
            message_count: 5,
            turn_count: 2,
        }
    }

    fn multi_provider_daily_usage(date: NaiveDate) -> DailyUsage {
        let mut usage = daily_usage_with_model(date, "gpt-5");
        let tokens = TokenBreakdown {
            input: 8_000,
            output: 900,
            cache_read: 4_000,
            cache_write: 700,
            reasoning: 0,
        };
        add_tokens(&mut usage.tokens, &tokens);
        usage.cost += 7.89;
        usage.message_count += 3;
        usage.turn_count += 1;

        let mut models = BTreeMap::new();
        models.insert(
            "claude-sonnet".to_string(),
            DailyModelInfo {
                provider: "anthropic".to_string(),
                display_name: "claude-sonnet".to_string(),
                color_key: "claude-sonnet".to_string(),
                tokens: tokens.clone(),
                cost: 7.89,
                messages: 3,
            },
        );
        usage.source_breakdown.insert(
            "cursor".to_string(),
            DailySourceInfo {
                tokens,
                cost: 7.89,
                models,
            },
        );
        usage
    }

    fn period_row(source: &str, provider: &str, model: &str, cost: f64) -> PeriodDetailModelRow {
        PeriodDetailModelRow {
            source: source.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            color_key: model.to_string(),
            tokens: TokenBreakdown {
                input: (cost * 1_000.0) as u64,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost,
            messages: 1,
        }
    }

    #[test]
    fn summary_line_uses_compact_text_without_overflow() {
        let app = test_app();
        let line = summary_line(
            "Coverage",
            "Seen",
            "11 days · 1 sources",
            Some("11d / 1src"),
            24,
            &app,
        );
        let text = line_text(&line);

        assert!(line.width() <= 24, "{text}");
        assert!(text.contains("Seen"), "{text}");
        assert!(text.contains("11d / 1src"), "{text}");
        assert!(!text.contains("Coverage"), "{text}");
    }

    #[test]
    fn ranking_bar_line_fits_available_width() {
        let app = test_app();
        for width in [1, 4, 8, 16, 24, 36, 72] {
            let line = ranking_bar_line(
                "claude-4-6-opus-high-thinking",
                "$62.43",
                0.68,
                Color::Green,
                width,
                &app,
            );
            let text = line_text(&line);

            assert!(line.width() <= width as usize, "{width}: {text}");
        }
    }

    #[test]
    fn compact_ranking_bar_keeps_value_when_it_fits() {
        let app = test_app();
        let line = ranking_bar_line("gpt-5.3-codex-xhigh", "$14.56", 0.16, Color::Cyan, 18, &app);
        let text = line_text(&line);

        assert!(line.width() <= 18, "{text}");
        assert!(text.contains("$14.56"), "{text}");
    }

    #[test]
    fn ranking_bar_line_uses_dotted_track() {
        let app = test_app();
        let line = ranking_bar_line("06-09", "$140.52", 0.42, Color::Green, 32, &app);
        let text = line_text(&line);

        assert!(line.width() <= 32, "{text}");
        assert!(text.contains("█"), "{text}");
        assert!(text.contains("·"), "{text}");
        assert!(!text.contains("░"), "{text}");
    }

    #[test]
    fn ranking_bar_line_uses_trace_mark_for_sub_cell_values() {
        let app = test_app();
        let line = ranking_bar_line("06-03", "$0.02", 0.001, Color::Green, 32, &app);
        let text = line_text(&line);

        assert!(line.width() <= 32, "{text}");
        assert!(text.contains("▏"), "{text}");
        assert!(text.contains("·"), "{text}");
        assert!(text.contains("$0.02"), "{text}");
    }

    #[test]
    fn provider_mix_lines_use_share_rows_without_bars() {
        let app = test_app();
        let lines = provider_mix_lines(
            &app,
            34,
            3,
            vec![("openai", 3.0), ("deepseek", 2.0), ("opencode-go", 1.0)],
        );
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("50%"), "{body}");
        assert!(body.contains("$3.00"), "{body}");
        assert!(!body.contains("█"), "{body}");
        assert!(!body.contains("▏"), "{body}");
        assert!(lines.iter().all(|line| line.width() <= 34), "{body}");
    }

    #[test]
    fn period_top_models_aggregate_sources_and_sort_by_cost() {
        let rows = vec![
            period_row("codex", "openai", "small-model", 2.0),
            period_row("codex", "openai", "gpt-5", 1.0),
            period_row("cursor", "openai", "gpt-5", 20.0),
        ];

        let top_models = period_top_models(&rows);

        assert_eq!(top_models.len(), 2);
        assert_eq!(top_models[0].model, "gpt-5");
        assert_eq!(top_models[0].provider, "openai");
        assert_eq!(top_models[0].cost, 21.0);
        assert_eq!(top_models[0].tokens.input, 21_000);
        assert_eq!(top_models[1].model, "small-model");
    }

    #[test]
    fn very_narrow_period_detail_keeps_top_area_to_summary_only() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.data.daily = vec![daily_usage(date)];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 28, 24);

        assert!(body.contains("Summary"), "{body}");
        assert!(body.contains("When"), "{body}");
        assert!(body.contains("04-17"), "{body}");
        assert!(body.contains("1 day"), "{body}");
        assert!(!body.contains("Provider Mix"), "{body}");
        assert!(!body.contains("Top Models"), "{body}");
    }

    #[test]
    fn medium_period_detail_single_provider_prefers_token_mix_and_top_models() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.data.daily = vec![daily_usage(date)];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 96, 24);

        assert!(body.contains("Token Mix"), "{body}");
        assert!(body.contains("Top Models"), "{body}");
        assert!(!body.contains("Provider Mix"), "{body}");
    }

    #[test]
    fn period_breakdown_header_shows_effective_cost_sort_for_inherited_date_sort() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.sort_field = SortField::Date;
        app.sort_direction = SortDirection::Descending;
        app.data.daily = vec![daily_usage(date)];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 96, 24);

        assert!(body.contains("Cost ▼"), "{body}");
    }

    #[test]
    fn period_breakdown_header_aligns_with_row_columns() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.data.daily = vec![multi_provider_daily_usage(date)];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 120, 24);
        let header = body
            .lines()
            .find(|line| line.contains("#") && line.contains("Model") && line.contains("Provider"))
            .expect("period breakdown header");
        let row = body
            .lines()
            .find(|line| line.contains("▶1"))
            .expect("selected period breakdown row");

        assert_eq!(
            char_position(header, "Provider"),
            char_position(row, "OpenAI"),
            "{body}"
        );
        assert_eq!(
            char_position(header, "Source"),
            char_position(row, "Codex"),
            "{body}"
        );
        assert_eq!(
            char_position(header, "Cost"),
            char_position(row, "$12.34"),
            "{body}"
        );
    }

    #[test]
    fn medium_period_detail_multi_provider_keeps_provider_and_token_mix_visible() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.data.daily = vec![multi_provider_daily_usage(date)];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 96, 24);

        assert!(body.contains("Provider Mix"), "{body}");
        assert!(body.contains("Token Mix"), "{body}");
        assert!(body.contains("%"), "{body}");
        assert!(body.contains("Cache hit"), "{body}");
        assert!(!body.contains("Top Models"), "{body}");
        assert!(
            !body.contains("█"),
            "detail mix panels should use compact rows, not bar charts\n{body}"
        );
    }

    #[test]
    fn very_narrow_period_breakdown_hides_metric_columns() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let mut app = test_app();
        app.data.daily = vec![daily_usage_with_model(
            date,
            "claude-4-6-opus-high-thinking",
        )];
        app.open_period_detail(PeriodDetailKey::day(date));

        let body = render_body(&mut app, 28, 24);
        let selected_row = body
            .lines()
            .find(|line| line.contains("▶1"))
            .expect("selected breakdown row");

        assert!(selected_row.contains("claude-4-6-opu..."), "{body}");
        assert!(
            !selected_row.contains("$12.34"),
            "narrow row should hide cost column\n{body}"
        );
        assert!(
            !selected_row.contains("33K"),
            "narrow row should hide token column\n{body}"
        );
    }
}
