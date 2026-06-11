use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation};
use std::collections::BTreeMap;

use super::bar_chart::{render_stacked_bar_chart, ModelSegment, StackedBarData};
use super::mix::{render_stacked_mix_summary, MixRow};
use super::widgets::scrollbar_state;
use super::widgets::{
    format_cost, format_tokens, get_provider_display_name, get_provider_shade,
    light_ratio_bar_spans, truncate_ellipsis as truncate_string,
};
use crate::tui::app::{
    App, ChartGranularity, ClickAction, OverviewMode, PeriodDetailKey, SortDirection, SortField,
};
use crate::tui::data::TokenBreakdown;
use chrono::Datelike;
use tokscale_core::GroupBy;

struct OverviewSummary {
    today_cost: f64,
    today_tokens: u64,
    total_cost: f64,
    total_tokens: u64,
    active_days: u32,
    model_count: usize,
}

struct ProviderMixRow {
    provider: String,
    label: String,
    color: Color,
    tokens: u64,
    cost: f64,
}

struct ModelRowData {
    label: String,
    provider: String,
    color_key: String,
    tokens_total: u64,
    tokens_input: u64,
    tokens_output: u64,
    tokens_cache_read: u64,
    tokens_cache_write: u64,
    cost: f64,
}

#[derive(Debug, Clone, Copy)]
struct MetricBarScale {
    display_max: f64,
    actual_max: f64,
    focus_max: f64,
    compressed: bool,
}

fn overview_model_label(group_by: &GroupBy, model: &str, workspace_label: Option<&str>) -> String {
    if *group_by == GroupBy::WorkspaceModel {
        format!(
            "{} / {}",
            workspace_label.unwrap_or("Unknown workspace"),
            model
        )
    } else {
        model.to_string()
    }
}

fn overview_color_key<'a>(group_by: &GroupBy, model: &'a str) -> &'a str {
    if *group_by == GroupBy::WorkspaceModel {
        model
            .rsplit_once(" / ")
            .map(|(_, base_model)| base_model)
            .unwrap_or(model)
    } else {
        model
    }
}

fn overview_model_rows(app: &App) -> Vec<ModelRowData> {
    match app.overview_mode {
        OverviewMode::All => all_model_rows(app),
        OverviewMode::Today => today_model_rows(app),
    }
}

fn all_model_rows(app: &App) -> Vec<ModelRowData> {
    let group_by = app.group_by.borrow().clone();
    app.get_sorted_models()
        .iter()
        .map(|m| ModelRowData {
            label: overview_model_label(&group_by, &m.model, m.workspace_label.as_deref()),
            provider: m.provider.clone(),
            color_key: overview_color_key(&group_by, &m.model).to_string(),
            tokens_total: m.tokens.total(),
            tokens_input: m.tokens.input,
            tokens_output: m.tokens.output,
            tokens_cache_read: m.tokens.cache_read,
            tokens_cache_write: m.tokens.cache_write,
            cost: m.cost,
        })
        .collect()
}

fn today_model_rows(app: &App) -> Vec<ModelRowData> {
    let Some(day) = app.today_usage() else {
        return Vec::new();
    };

    let mut rows_by_key: BTreeMap<(String, String), ModelRowData> = BTreeMap::new();
    for source_info in day.source_breakdown.values() {
        for info in source_info.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let key = (provider.clone(), info.display_name.clone());
            let row = rows_by_key.entry(key).or_insert_with(|| ModelRowData {
                label: info.display_name.clone(),
                provider,
                color_key: info.color_key.clone(),
                tokens_total: 0,
                tokens_input: 0,
                tokens_output: 0,
                tokens_cache_read: 0,
                tokens_cache_write: 0,
                cost: 0.0,
            });

            add_tokens(row, &info.tokens);
            if info.cost.is_finite() {
                row.cost += info.cost;
            }
        }
    }

    let mut rows: Vec<ModelRowData> = rows_by_key.into_values().collect();
    sort_model_rows(app, &mut rows);
    rows
}

fn add_tokens(row: &mut ModelRowData, tokens: &TokenBreakdown) {
    row.tokens_total = row.tokens_total.saturating_add(tokens.total());
    row.tokens_input = row.tokens_input.saturating_add(tokens.input);
    row.tokens_output = row.tokens_output.saturating_add(tokens.output);
    row.tokens_cache_read = row.tokens_cache_read.saturating_add(tokens.cache_read);
    row.tokens_cache_write = row.tokens_cache_write.saturating_add(tokens.cache_write);
}

fn sort_model_rows(app: &App, rows: &mut [ModelRowData]) {
    let tie_breaker = |a: &ModelRowData, b: &ModelRowData| {
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
        (SortField::Date, _) => rows.sort_by(tie_breaker),
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // Pre-fill entire overview area with theme background so that chart and
    // legend cells (which only set fg via direct buffer writes) don't fall
    // through to the terminal's default background color.
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.background)),
        area,
    );

    if area.width >= 96 && area.height >= 18 {
        render_wide_dashboard(frame, app, area);
    } else {
        render_compact_dashboard(frame, app, area);
    }
}

fn render_wide_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let legend_height = 1u16;
    let min_list_height = 7u16;
    let top_capacity = area.height.saturating_sub(legend_height + min_list_height);
    let top_height = top_capacity.clamp(10, 16);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_height),
            Constraint::Length(legend_height),
            Constraint::Min(0),
        ])
        .split(area);

    let side_width = ((area.width as f64) * 0.32).round() as u16;
    let side_width = side_width.clamp(38, 58);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(42), Constraint::Length(side_width)])
        .split(chunks[0]);

    let list_area_height = chunks[2].height.saturating_sub(2);
    let items_per_page = list_area_height.saturating_sub(1) as usize;
    let items_per_page = items_per_page.max(1);
    app.set_max_visible_items(items_per_page);

    render_chart_panel(frame, app, top[0]);
    render_overview_sidebar(frame, app, top[1]);
    render_legend(frame, app, chunks[1]);
    render_top_models(frame, app, chunks[2], items_per_page);
}

fn render_compact_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let safe_height = area.height.max(12) as usize;
    let chart_height = (safe_height as f64 * 0.34).floor().max(6.0) as u16;
    let summary_height = if area.height >= 13 { 1 } else { 0 };
    let legend_height = 1u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chart_height),
            Constraint::Length(summary_height),
            Constraint::Length(legend_height),
            Constraint::Min(0),
        ])
        .split(area);

    let list_area_height = chunks[3].height.saturating_sub(2);
    let items_per_page = list_area_height.saturating_sub(1) as usize;
    let items_per_page = items_per_page.max(1);
    app.set_max_visible_items(items_per_page);

    render_chart_panel(frame, app, chunks[0]);
    if summary_height > 0 {
        render_summary_strip(frame, app, chunks[1]);
    }
    render_legend(frame, app, chunks[2]);
    render_top_models(frame, app, chunks[3], items_per_page);
}

fn render_chart_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 3 || area.width < 12 {
        render_chart(frame, app, area);
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            format!(" {} ", app.overview_title()),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(chart_granularity_selector(app, area).right_aligned())
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_chart(frame, app, inner);
}

fn chart_granularity_selector(app: &mut App, area: Rect) -> Line<'static> {
    if app.overview_mode == OverviewMode::Today {
        return Line::from(Span::styled(" Today ", app.theme.subtle_text_style()));
    }
    if area.width < 34 {
        return Line::from(Span::styled(" All ", app.theme.subtle_text_style()));
    }

    let granularities = [
        ChartGranularity::Daily,
        ChartGranularity::Weekly,
        ChartGranularity::Monthly,
    ];
    let total_width = granularities
        .iter()
        .map(|granularity| Line::from(format!(" {} ", granularity.short_label()).as_str()).width())
        .sum::<usize>() as u16
        + 2;
    let mut click_x = area.right().saturating_sub(total_width).saturating_add(1);
    let mut spans = vec![Span::styled(" ", app.theme.subtle_text_style())];
    for granularity in granularities {
        let selected = app.chart_granularity == granularity;
        let style = if selected {
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.color(Color::Rgb(30, 64, 175)))
                .add_modifier(Modifier::BOLD)
        } else {
            app.theme.subtle_text_style()
        };
        let label = format!(" {} ", granularity.short_label());
        spans.push(Span::styled(label.clone(), style));
        let width = Line::from(label.as_str()).width() as u16;
        app.add_click_area(
            Rect::new(click_x, area.y, width, 1),
            ClickAction::OverviewChartGranularity(granularity),
        );
        click_x = click_x.saturating_add(width);
    }
    spans.push(Span::styled(" ", app.theme.subtle_text_style()));
    Line::from(spans)
}

fn render_chart(frame: &mut Frame, app: &mut App, area: Rect) {
    let group_by = app.group_by.borrow().clone();

    let data: Vec<StackedBarData> = if app.overview_mode == OverviewMode::Today {
        let today = app.overview_date();
        app.data
            .hourly
            .iter()
            .filter(|h| h.datetime.date() == today)
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|h| {
                let models: Vec<ModelSegment> = h
                    .models
                    .values()
                    .map(|info| ModelSegment {
                        model_id: info.display_name.clone(),
                        tokens: info.tokens.total(),
                        color: app.model_color_for(&info.provider, &info.color_key),
                    })
                    .collect();

                StackedBarData {
                    date: h.datetime.format("%H:%M").to_string(),
                    period: None,
                    models,
                    total: h.tokens.total(),
                }
            })
            .collect()
    } else {
        match app.chart_granularity {
            ChartGranularity::Daily => daily_chart_bars(app, &group_by),
            ChartGranularity::Weekly => weekly_chart_bars(app, &group_by),
            ChartGranularity::Monthly => monthly_chart_bars(app, &group_by),
        }
    };

    render_stacked_bar_chart(frame, app, area, &data);
}

struct ChartBucket {
    label: String,
    period: Option<PeriodDetailKey>,
    total: u64,
    models: BTreeMap<String, ModelSegment>,
}

fn daily_chart_bars(app: &App, group_by: &GroupBy) -> Vec<StackedBarData> {
    let mut days: Vec<_> = app.data.daily.iter().collect();
    days.sort_by_key(|day| day.date);
    if days.len() > 60 {
        days = days.split_off(days.len() - 60);
    }

    days.into_iter()
        .map(|day| StackedBarData {
            date: day.date.format("%m/%d").to_string(),
            period: Some(PeriodDetailKey::day(day.date)),
            models: daily_model_segments(app, group_by, day),
            total: day.tokens.total(),
        })
        .collect()
}

fn weekly_chart_bars(app: &App, group_by: &GroupBy) -> Vec<StackedBarData> {
    let mut buckets: BTreeMap<(i32, u32), ChartBucket> = BTreeMap::new();
    for day in &app.data.daily {
        let week = day.date.iso_week();
        let key = (week.year(), week.week());
        let bucket = buckets.entry(key).or_insert_with(|| ChartBucket {
            label: format!("W{:02}", week.week()),
            period: Some(PeriodDetailKey::week_containing(day.date)),
            total: 0,
            models: BTreeMap::new(),
        });
        bucket.total = bucket.total.saturating_add(day.tokens.total());
        add_day_to_chart_bucket(app, group_by, day, bucket);
    }

    chart_buckets_to_bars(buckets, 52)
}

fn monthly_chart_bars(app: &App, group_by: &GroupBy) -> Vec<StackedBarData> {
    let mut buckets: BTreeMap<(i32, u32), ChartBucket> = BTreeMap::new();
    for day in &app.data.daily {
        let key = (day.date.year(), day.date.month());
        let bucket = buckets.entry(key).or_insert_with(|| ChartBucket {
            label: day.date.format("%b '%y").to_string(),
            period: PeriodDetailKey::month(day.date.year(), day.date.month()),
            total: 0,
            models: BTreeMap::new(),
        });
        bucket.total = bucket.total.saturating_add(day.tokens.total());
        add_day_to_chart_bucket(app, group_by, day, bucket);
    }

    chart_buckets_to_bars(buckets, 24)
}

fn chart_buckets_to_bars<K: Ord>(
    buckets: BTreeMap<K, ChartBucket>,
    limit: usize,
) -> Vec<StackedBarData> {
    let mut bars: Vec<StackedBarData> = buckets
        .into_values()
        .map(|bucket| StackedBarData {
            date: bucket.label,
            period: bucket.period,
            total: bucket.total,
            models: bucket.models.into_values().collect(),
        })
        .collect();
    if bars.len() > limit {
        bars = bars.split_off(bars.len() - limit);
    }
    bars
}

fn daily_model_segments(
    app: &App,
    group_by: &GroupBy,
    day: &crate::tui::data::DailyUsage,
) -> Vec<ModelSegment> {
    let mut models = BTreeMap::new();
    let mut bucket = ChartBucket {
        label: String::new(),
        period: None,
        total: 0,
        models: BTreeMap::new(),
    };
    add_day_to_chart_bucket(app, group_by, day, &mut bucket);
    models.append(&mut bucket.models);
    models.into_values().collect()
}

fn add_day_to_chart_bucket(
    app: &App,
    group_by: &GroupBy,
    day: &crate::tui::data::DailyUsage,
    bucket: &mut ChartBucket,
) {
    for source_info in day.source_breakdown.values() {
        for info in source_info.models.values() {
            let provider = crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
            let color_key = overview_color_key(group_by, &info.color_key);
            let key = format!("{}\0{}", provider, info.display_name);
            let entry = bucket.models.entry(key).or_insert_with(|| ModelSegment {
                model_id: info.display_name.clone(),
                tokens: 0,
                color: app.model_color_for(&provider, color_key),
            });
            entry.tokens = entry.tokens.saturating_add(info.tokens.total());
        }
    }
}

fn render_overview_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let summary_height = if area.height >= 14 { 7 } else { 6 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(summary_height), Constraint::Min(0)])
        .split(area);

    render_summary_panel(frame, app, chunks[0]);
    render_provider_mix_panel(frame, app, chunks[1]);
}

fn render_summary_panel(frame: &mut Frame, app: &App, area: Rect) {
    let summary = overview_summary(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Summary ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let active = format!(
        "{} days · {} models",
        summary.active_days, summary.model_count
    );
    let lines = vec![
        metric_line(
            "Today",
            &format_cost(summary.today_cost),
            &format_tokens(summary.today_tokens),
            Color::Rgb(45, 212, 191),
            app,
        ),
        metric_line(
            "Total",
            &format_cost(summary.total_cost),
            &format_tokens(summary.total_tokens),
            Color::Rgb(96, 165, 250),
            app,
        ),
        Line::from(vec![
            Span::styled("Active ", app.theme.subtle_text_style()),
            Span::styled(active, Style::default().fg(app.theme.foreground)),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_summary_strip(frame: &mut Frame, app: &App, area: Rect) {
    let summary = overview_summary(app);
    let line = if app.overview_mode == OverviewMode::Today {
        Line::from(vec![
            Span::styled("Today ", app.theme.subtle_text_style()),
            Span::styled(
                format_cost(summary.today_cost),
                Style::default()
                    .fg(app.theme.color(Color::Rgb(45, 212, 191)))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · Tokens ", app.theme.subtle_text_style()),
            Span::styled(
                format_tokens(summary.today_tokens),
                Style::default().fg(app.theme.foreground),
            ),
            Span::styled(" · Models ", app.theme.subtle_text_style()),
            Span::styled(
                summary.model_count.to_string(),
                Style::default().fg(app.theme.foreground),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("Today ", app.theme.subtle_text_style()),
            Span::styled(
                format_cost(summary.today_cost),
                Style::default()
                    .fg(app.theme.color(Color::Rgb(45, 212, 191)))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · Total ", app.theme.subtle_text_style()),
            Span::styled(
                format_cost(summary.total_cost),
                Style::default()
                    .fg(app.theme.color(Color::Rgb(96, 165, 250)))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · Tokens ", app.theme.subtle_text_style()),
            Span::styled(
                format_tokens(summary.total_tokens),
                Style::default().fg(app.theme.foreground),
            ),
            Span::styled(" · Models ", app.theme.subtle_text_style()),
            Span::styled(
                summary.model_count.to_string(),
                Style::default().fg(app.theme.foreground),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_provider_mix_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Provider Mix ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let providers = provider_mix(app);
    if providers.is_empty() {
        let empty = Paragraph::new("No provider data")
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let rows = providers
        .into_iter()
        .map(|provider| MixRow::cost(provider.label, provider.cost, provider.color))
        .collect::<Vec<_>>();

    render_stacked_mix_summary(frame, app, inner, &rows, None);
}

fn overview_summary(app: &App) -> OverviewSummary {
    let today_usage = app.today_usage();
    let active_days = app
        .data
        .daily
        .iter()
        .filter(|day| day.tokens.total() > 0 || day.cost > 0.0)
        .count() as u32;

    if app.overview_mode == OverviewMode::Today {
        let today_cost = today_usage.map(|day| day.cost).unwrap_or(0.0);
        let today_tokens = today_usage.map(|day| day.tokens.total()).unwrap_or(0);
        return OverviewSummary {
            today_cost,
            today_tokens,
            total_cost: today_cost,
            total_tokens: today_tokens,
            active_days: u32::from(today_tokens > 0 || today_cost > 0.0),
            model_count: app.overview_model_len(),
        };
    }

    OverviewSummary {
        today_cost: today_usage.map(|day| day.cost).unwrap_or(0.0),
        today_tokens: today_usage.map(|day| day.tokens.total()).unwrap_or(0),
        total_cost: app.data.total_cost,
        total_tokens: app.data.total_tokens,
        active_days,
        model_count: app.data.models.len(),
    }
}

fn provider_mix(app: &App) -> Vec<ProviderMixRow> {
    let mut by_provider: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    for model in overview_model_rows(app) {
        let key = crate::tui::colors::provider_color_key(&model.provider, &model.color_key);
        let entry = by_provider.entry(key).or_insert((0, 0.0));
        entry.0 = entry.0.saturating_add(model.tokens_total);
        if model.cost.is_finite() {
            entry.1 += model.cost;
        }
    }

    let mut rows: Vec<ProviderMixRow> = by_provider
        .into_iter()
        .map(|(provider, (tokens, cost))| ProviderMixRow {
            label: get_provider_display_name(&provider),
            color: app.theme.color(get_provider_shade(&provider, 0)),
            provider,
            tokens,
            cost,
        })
        .collect();

    rows.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.cmp(&a.tokens))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn metric_line(
    label: &str,
    primary: &str,
    secondary: &str,
    color: Color,
    app: &App,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<6}"), app.theme.subtle_text_style()),
        Span::styled(
            primary.to_string(),
            Style::default()
                .fg(app.theme.color(color))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(secondary.to_string(), app.theme.secondary_text_style()),
    ])
}

fn render_legend(frame: &mut Frame, app: &App, area: Rect) {
    let legend_limit = if app.is_narrow() { 3 } else { 5 };
    let max_name_width = if app.is_narrow() { 12 } else { 18 };
    let muted_color = app.theme.muted;

    let top_models: Vec<(String, Color)> = overview_model_rows(app)
        .iter()
        .take(legend_limit)
        .map(|m| {
            (
                m.label.clone(),
                app.model_color_for(&m.provider, &m.color_key),
            )
        })
        .collect();

    if top_models.is_empty() {
        return;
    }

    let mut spans: Vec<Span> = Vec::new();
    for (i, (model_name, color)) in top_models.iter().enumerate() {
        let name = truncate_string(model_name, max_name_width);

        spans.push(Span::styled("●", Style::default().fg(*color)));
        spans.push(Span::raw(format!(" {}", name)));

        if i < top_models.len() - 1 {
            spans.push(Span::styled("  ·", Style::default().fg(muted_color)));
        }
    }

    let legend_line = Line::from(spans);
    let paragraph = Paragraph::new(legend_line);
    frame.render_widget(paragraph, area);
}

fn render_top_models(frame: &mut Frame, app: &mut App, area: Rect, items_per_page: usize) {
    let theme_border = app.theme.border;
    let theme_accent = app.theme.accent;
    let theme_background = app.theme.background;
    let theme_muted = app.theme.muted;
    let theme_foreground = app.theme.foreground;
    let theme_selection = app.theme.selection;
    let secondary_text_style = app.theme.secondary_text_style();
    let subtle_text_style = app.theme.subtle_text_style();
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let is_very_narrow = app.is_very_narrow();
    let sort_field = app.sort_field;
    let models_data = overview_model_rows(app);
    let total_cost = models_data
        .iter()
        .map(|m| if m.cost.is_finite() { m.cost } else { 0.0 })
        .sum::<f64>();

    let title = if is_very_narrow {
        "Top Models".to_string()
    } else {
        match sort_field {
            SortField::Tokens => match app.overview_mode {
                OverviewMode::All => "Top Models by Tokens".to_string(),
                OverviewMode::Today => "Today Models by Tokens".to_string(),
            },
            _ => match app.overview_mode {
                OverviewMode::All => "Top Models by Cost".to_string(),
                OverviewMode::Today => "Today Models by Cost".to_string(),
            },
        }
    };

    let total_label = if app.overview_mode == OverviewMode::Today {
        "Today"
    } else {
        "Total"
    };
    let title_right = if is_very_narrow {
        format_cost(total_cost)
    } else {
        format!("{total_label}: {}", format_cost(total_cost))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme_border))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(theme_accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(" {} ", title_right),
                Style::default().fg(Color::Green),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(theme_background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if models_data.is_empty() {
        let empty = Paragraph::new("No data available")
            .style(Style::default().fg(theme_muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let total_cost_for_share = total_cost.max(0.01);
    let total_tokens_for_share = models_data
        .iter()
        .map(|m| m.tokens_total)
        .sum::<u64>()
        .max(1);
    let cost_bar_scale = metric_bar_scale(models_data.iter().map(|m| m.cost));
    let models_len = models_data.len();
    let page_capacity = items_per_page
        .min(inner.height.saturating_sub(1) as usize)
        .max(1);
    let start = scroll_offset.min(models_len);
    let end = (start + page_capacity).min(models_len);

    if start >= models_len {
        return;
    }

    let rank_width = if is_very_narrow { 0 } else { 4 };
    let provider_width = if inner.width >= 82 { 14 } else { 0 };
    let cost_width = if inner.width >= 42 { 10 } else { 8 };
    let pct_width = if inner.width >= 54 { 7 } else { 0 };
    let bar_width = if inner.width >= 100 {
        22
    } else if inner.width >= 74 {
        14
    } else if inner.width >= 58 {
        9
    } else {
        0
    };
    let token_width = if inner.width >= 112 {
        10
    } else if inner.width >= 88 {
        9
    } else {
        0
    };
    let compact_token_width = if inner.width < 34 { 7 } else { 8 };
    let mut fixed_width = 0usize;
    for width in [
        rank_width,
        provider_width,
        cost_width,
        pct_width,
        bar_width,
        token_width,
        token_width,
        token_width.max(if token_width == 0 {
            compact_token_width
        } else {
            0
        }),
    ] {
        if width > 0 {
            fixed_width += width + 1;
        }
    }
    let model_width = (inner.width as usize).saturating_sub(fixed_width).max(8);

    let pad_left = |text: &str, width: usize| -> String {
        let text = truncate_string(text, width);
        format!("{text:<width$} ")
    };
    let pad_right = |text: &str, width: usize| -> String {
        let text = truncate_string(text, width);
        format!("{text:>width$} ")
    };
    let pad_right_end = |text: &str, width: usize| -> String {
        let text = truncate_string(text, width);
        format!("{text:>width$}")
    };

    let header_style = Style::default()
        .fg(theme_muted)
        .add_modifier(Modifier::BOLD);
    let header = Rect::new(inner.x, inner.y, inner.width, 1);
    let mut header_spans = Vec::new();
    if rank_width > 0 {
        header_spans.push(Span::styled(pad_right("#", rank_width), header_style));
    }
    header_spans.push(Span::styled(pad_left("Model", model_width), header_style));
    if provider_width > 0 {
        header_spans.push(Span::styled(
            pad_left("Provider", provider_width),
            header_style,
        ));
    }
    header_spans.push(Span::styled(pad_right("Cost", cost_width), header_style));
    if pct_width > 0 {
        header_spans.push(Span::styled(pad_right("%", pct_width), header_style));
    }
    if bar_width > 0 {
        header_spans.push(Span::styled(pad_left("Cost Bar", bar_width), header_style));
    }
    if token_width > 0 {
        header_spans.push(Span::styled(pad_right("Input", token_width), header_style));
        header_spans.push(Span::styled(pad_right("Output", token_width), header_style));
        header_spans.push(Span::styled(
            pad_right_end("Total", token_width),
            header_style,
        ));
    } else {
        let label = if compact_token_width < 8 {
            "Tok"
        } else {
            "Tokens"
        };
        header_spans.push(Span::styled(
            pad_right_end(label, compact_token_width),
            header_style,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header);

    let mut y = inner.y.saturating_add(1);
    for (i, model) in models_data[start..end].iter().enumerate() {
        if y >= inner.y + inner.height {
            break;
        }

        let idx = i + start;
        let is_selected = idx == selected_index;
        let row_style = if is_selected {
            Style::default().bg(theme_selection).fg(theme_foreground)
        } else {
            Style::default()
        };

        let model_color = app.model_color_for(&model.provider, &model.color_key);
        let display_name = &model.label;
        let percentage = match sort_field {
            SortField::Tokens => {
                (model.tokens_total as f64 / total_tokens_for_share as f64) * 100.0
            }
            _ if model.cost.is_finite() => (model.cost / total_cost_for_share) * 100.0,
            _ => 0.0,
        };

        let cost_text = format_cost(model.cost);
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        app.add_click_area(
            row_area,
            ClickAction::OpenModelDetail(crate::tui::app::ModelDetailKey {
                provider: model.provider.clone(),
                model: model.label.clone(),
                color_key: model.color_key.clone(),
            }),
        );
        frame.render_widget(Paragraph::new("").style(row_style), row_area);

        let row_fg = if is_selected {
            theme_foreground
        } else {
            model_color
        };
        let mut spans = Vec::new();
        if rank_width > 0 {
            let marker = if is_selected { "▶" } else { " " };
            spans.push(Span::styled(
                pad_right(&format!("{marker}{}", idx + 1), rank_width),
                Style::default().fg(if is_selected {
                    theme_foreground
                } else {
                    theme_muted
                }),
            ));
        }
        let name_width = model_width.saturating_sub(2).max(1);
        spans.push(Span::styled("● ", Style::default().fg(model_color)));
        spans.push(Span::styled(
            pad_left(&truncate_string(display_name, name_width), name_width),
            Style::default().fg(row_fg).add_modifier(Modifier::BOLD),
        ));
        if provider_width > 0 {
            let provider =
                crate::tui::colors::provider_color_key(&model.provider, &model.color_key);
            spans.push(Span::styled(
                pad_left(&get_provider_display_name(&provider), provider_width),
                secondary_text_style,
            ));
        }
        spans.push(Span::styled(
            pad_right(&cost_text, cost_width),
            Style::default()
                .fg(if is_selected {
                    theme_foreground
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        ));
        if pct_width > 0 {
            spans.push(Span::styled(
                pad_right(&format!("{percentage:.1}%"), pct_width),
                subtle_text_style,
            ));
        }
        if bar_width > 0 {
            spans.extend(light_ratio_bar_spans(
                metric_bar_ratio(model.cost, &cost_bar_scale),
                bar_width,
                Style::default().fg(model_color),
                subtle_text_style,
            ));
            spans.push(Span::raw(" "));
        }
        if token_width > 0 {
            spans.push(Span::styled(
                pad_right(&format_tokens(model.tokens_input), token_width),
                secondary_text_style,
            ));
            spans.push(Span::styled(
                pad_right(&format_tokens(model.tokens_output), token_width),
                secondary_text_style,
            ));
            spans.push(Span::styled(
                pad_right_end(&format_tokens(model.tokens_total), token_width),
                secondary_text_style,
            ));
        } else {
            spans.push(Span::styled(
                pad_right_end(&format_tokens_tiny(model.tokens_total), compact_token_width),
                secondary_text_style,
            ));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), row_area);

        y += 1;
    }

    if models_len > page_capacity {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        let mut scrollbar_state = scrollbar_state(models_len, scroll_offset, page_capacity);

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

fn format_tokens_tiny(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        let value = tokens as f64 / 1_000_000_000.0;
        if value >= 10.0 {
            format!("{value:.0}B")
        } else {
            format!("{value:.1}B")
        }
    } else if tokens >= 1_000_000 {
        let value = tokens as f64 / 1_000_000.0;
        if value >= 10.0 {
            format!("{value:.0}M")
        } else {
            format!("{value:.1}M")
        }
    } else if tokens >= 1_000 {
        let value = tokens as f64 / 1_000.0;
        if value >= 100.0 {
            format!("{value:.0}K")
        } else if value >= 10.0 {
            format!("{value:.0}K")
        } else {
            format!("{value:.1}K")
        }
    } else {
        tokens.to_string()
    }
}

fn metric_bar_scale(values: impl IntoIterator<Item = f64>) -> MetricBarScale {
    let mut finite_values: Vec<f64> = values
        .into_iter()
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect();
    finite_values.sort_by(|a, b| a.total_cmp(b));

    let actual_max = finite_values.last().copied().unwrap_or(1.0).max(1.0);
    if finite_values.len() < 8 {
        return MetricBarScale {
            display_max: actual_max,
            actual_max,
            focus_max: actual_max,
            compressed: false,
        };
    }

    let focus_max = percentile_value(&finite_values, 0.9)
        .max(1.0)
        .min(actual_max);
    let should_compress = actual_max > focus_max * 1.35;
    let display_max = if should_compress {
        focus_max + focus_max * 0.35
    } else {
        actual_max
    };

    MetricBarScale {
        display_max,
        actual_max,
        focus_max,
        compressed: should_compress,
    }
}

#[cfg(test)]
fn metric_bar_filled(value: f64, scale: &MetricBarScale, width: usize) -> usize {
    if width == 0 || !value.is_finite() || value <= 0.0 {
        return 0;
    }

    (metric_bar_ratio(value, scale) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize
}

fn metric_bar_ratio(value: f64, scale: &MetricBarScale) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }

    let display_value = metric_bar_display_value(value, scale);
    (display_value / scale.display_max.max(1.0)).clamp(0.0, 1.0)
}

fn metric_bar_display_value(value: f64, scale: &MetricBarScale) -> f64 {
    let value = value.max(0.0).min(scale.actual_max);
    if !scale.compressed || value <= scale.focus_max {
        return value.min(scale.display_max);
    }

    let actual_overflow = (scale.actual_max - scale.focus_max).max(1.0);
    let display_overflow = (scale.display_max - scale.focus_max).max(1.0);
    scale.focus_max + ((value - scale.focus_max) / actual_overflow) * display_overflow
}

fn percentile_value(sorted_values: &[f64], percentile: f64) -> f64 {
    if sorted_values.is_empty() {
        return 1.0;
    }

    let index = ((sorted_values.len() - 1) as f64 * percentile)
        .round()
        .clamp(0.0, (sorted_values.len() - 1) as f64) as usize;
    sorted_values[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{OverviewMode, Tab, TuiConfig};
    use crate::tui::data::{
        DailyModelInfo, DailySourceInfo, DailyUsage, ModelUsage, TokenBreakdown, UsageData,
    };
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};
    use std::collections::BTreeMap;

    fn make_app(width: u16) -> App {
        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: Some(Tab::Overview),
        };
        let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();
        app.terminal_width = width;
        app.current_tab = Tab::Overview;
        app.data.models = vec![
            model_usage("gpt-4.1", "openai", 120_000, 35_000, 18.50),
            model_usage("claude-sonnet-4.5", "anthropic", 95_000, 42_000, 14.25),
            model_usage("qwen3-coder", "openrouter", 66_000, 18_000, 6.40),
        ];
        app.data.total_tokens = app.data.models.iter().map(|m| m.tokens.total()).sum();
        app.data.total_cost = app.data.models.iter().map(|m| m.cost).sum();
        app.data.daily = vec![daily_usage(
            chrono::Local::now().date_naive(),
            app.data.total_cost,
            app.data.total_tokens,
        )];
        app
    }

    fn model_usage(model: &str, provider: &str, input: u64, output: u64, cost: f64) -> ModelUsage {
        ModelUsage {
            model: model.to_string(),
            provider: provider.to_string(),
            client: "opencode".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown {
                input,
                output,
                cache_read: input / 2,
                cache_write: input / 8,
                reasoning: 0,
            },
            cost,
            performance: Default::default(),
            session_count: 1,
        }
    }

    fn daily_usage(date: NaiveDate, cost: f64, total_tokens: u64) -> DailyUsage {
        DailyUsage {
            date,
            tokens: TokenBreakdown {
                input: total_tokens,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost,
            source_breakdown: BTreeMap::new(),
            message_count: 10,
            turn_count: 5,
        }
    }

    fn daily_usage_with_model(
        date: NaiveDate,
        provider: &str,
        display_name: &str,
        color_key: &str,
        input: u64,
        output: u64,
        cost: f64,
    ) -> DailyUsage {
        let tokens = TokenBreakdown {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };
        let mut models = BTreeMap::new();
        models.insert(
            display_name.to_string(),
            DailyModelInfo {
                provider: provider.to_string(),
                display_name: display_name.to_string(),
                color_key: color_key.to_string(),
                tokens: tokens.clone(),
                cost,
                messages: 3,
            },
        );
        let mut source_breakdown = BTreeMap::new();
        source_breakdown.insert(
            "opencode".to_string(),
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
            message_count: 3,
            turn_count: 2,
        }
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

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn narrow_chart_selector_uses_actual_scope() {
        let mut app = make_app(28);
        app.overview_mode = OverviewMode::All;
        let all = line_text(&chart_granularity_selector(
            &mut app,
            Rect::new(0, 0, 28, 1),
        ));
        assert_eq!(all, " All ");

        app.overview_mode = OverviewMode::Today;
        let today = line_text(&chart_granularity_selector(
            &mut app,
            Rect::new(0, 0, 28, 1),
        ));
        assert_eq!(today, " Today ");
    }

    #[test]
    fn tiny_token_formatter_preserves_units() {
        assert_eq!(format_tokens_tiny(721), "721");
        assert_eq!(format_tokens_tiny(1_800), "1.8K");
        assert_eq!(format_tokens_tiny(721_000), "721K");
        assert_eq!(format_tokens_tiny(1_800_000), "1.8M");
        assert_eq!(format_tokens_tiny(298_700_000), "299M");
        assert_eq!(format_tokens_tiny(1_800_000_000), "1.8B");
    }

    #[test]
    fn very_narrow_model_rows_keep_token_units() {
        let mut app = make_app(28);
        let body = render_body(&mut app, 28, 24);

        assert!(body.contains("230K"), "missing token unit\n{body}");
    }

    #[test]
    fn very_narrow_large_model_rows_keep_token_units() {
        let mut app = make_app(28);
        app.data.models = vec![ModelUsage {
            model: "claude-4-6-opus-high-thinking".to_string(),
            provider: "anthropic".to_string(),
            client: "cursor".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown {
                input: 298_700_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 288.20,
            performance: Default::default(),
            session_count: 1,
        }];
        app.data.total_tokens = 298_700_000;
        app.data.total_cost = 288.20;
        app.data.daily = vec![daily_usage(
            chrono::Local::now().date_naive(),
            288.20,
            298_700_000,
        )];

        let body = render_body(&mut app, 28, 24);

        assert!(body.contains("299M"), "missing large token unit\n{body}");
    }

    #[test]
    fn wide_overview_renders_dashboard_sections() {
        let mut app = make_app(120);
        let body = render_body(&mut app, 120, 30);

        assert!(body.contains("Overview"), "missing overview title\n{body}");
        assert!(
            body.contains("Usage Trend (Daily)"),
            "missing chart body\n{body}"
        );
        assert!(body.contains("Summary"), "missing summary panel\n{body}");
        assert!(
            body.contains("Provider Mix"),
            "missing provider mix\n{body}"
        );
        assert!(body.contains("Top Models"), "missing model list\n{body}");
        assert!(body.contains("OpenAI"), "missing provider row\n{body}");
        assert!(body.contains("gpt-4.1"), "missing model row\n{body}");
        assert!(app.max_visible_items >= 1);
    }

    #[test]
    fn overview_provider_mix_uses_stacked_summary() {
        let mut app = make_app(120);
        app.data.models.push(model_usage(
            "claude-sonnet",
            "anthropic",
            120_000,
            10_000,
            2.0,
        ));
        app.data.total_tokens = app.data.models.iter().map(|m| m.tokens.total()).sum();
        app.data.total_cost = app.data.models.iter().map(|m| m.cost).sum();

        let body = render_body(&mut app, 120, 30);

        assert!(body.contains("Provider Mix"), "{body}");
        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("Anthropic"), "{body}");
        assert!(body.contains("█"), "{body}");
    }

    #[test]
    fn overview_cost_bar_uses_dotted_track() {
        let mut app = make_app(120);
        let body = render_body(&mut app, 120, 30);

        assert!(body.contains("Cost Bar"), "{body}");
        assert!(body.contains("·"), "{body}");
    }

    #[test]
    fn compact_overview_keeps_summary_and_model_list() {
        let mut app = make_app(72);
        let body = render_body(&mut app, 72, 20);

        assert!(body.contains("Overview"), "missing overview title\n{body}");
        assert!(
            body.contains("Usage Trend (Daily)"),
            "missing chart body\n{body}"
        );
        assert!(body.contains("Today"), "missing compact summary\n{body}");
        assert!(body.contains("Top Models"), "missing model list\n{body}");
        assert!(body.contains("gpt-4.1"), "missing model row\n{body}");
        assert!(app.max_visible_items >= 1);
    }

    #[test]
    fn provider_mix_groups_gateway_models_by_model_owner() {
        let mut app = make_app(120);
        app.data.models = vec![
            model_usage("qwen3-coder", "openrouter", 50_000, 10_000, 7.0),
            model_usage("kimi-k2", "openrouter", 40_000, 8_000, 5.0),
        ];
        app.data.total_tokens = app.data.models.iter().map(|m| m.tokens.total()).sum();
        app.data.total_cost = app.data.models.iter().map(|m| m.cost).sum();

        let body = render_body(&mut app, 120, 30);
        assert!(
            body.contains("Qwen"),
            "missing inferred Qwen provider\n{body}"
        );
        assert!(
            body.contains("Moonshot AI"),
            "missing inferred Moonshot provider\n{body}"
        );
    }

    #[test]
    fn today_overview_uses_today_daily_breakdown() {
        let mut app = make_app(120);
        app.overview_mode = OverviewMode::Today;
        app.data.models = vec![model_usage("gpt-4.1", "openai", 120_000, 35_000, 18.50)];
        app.data.daily = vec![daily_usage_with_model(
            chrono::Local::now().date_naive(),
            "openrouter",
            "qwen3-coder",
            "qwen3-coder",
            50_000,
            10_000,
            7.0,
        )];

        let body = render_body(&mut app, 120, 30);

        assert!(body.contains("Today"), "missing Today title\n{body}");
        assert!(body.contains("Qwen"), "missing inferred provider\n{body}");
        assert!(body.contains("qwen3-coder"), "missing today model\n{body}");
        assert!(
            !body.contains("gpt-4.1"),
            "today view leaked all-time model rows\n{body}"
        );
    }

    #[test]
    fn metric_bar_scale_normalizes_to_visible_max() {
        let scale = metric_bar_scale([10.0, 20.0, 30.0]);

        assert!(!scale.compressed);
        assert_eq!(metric_bar_filled(30.0, &scale, 20), 20);
        assert_eq!(metric_bar_filled(15.0, &scale, 20), 10);
        assert_eq!(metric_bar_filled(0.0, &scale, 20), 0);
    }

    #[test]
    fn metric_bar_scale_compresses_outliers_without_equalizing_them() {
        let scale = metric_bar_scale([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 100.0]);

        assert!(scale.compressed);
        assert_eq!(metric_bar_filled(100.0, &scale, 20), 20);
        assert!(metric_bar_filled(50.0, &scale, 20) < 20);
        assert!(metric_bar_filled(50.0, &scale, 20) > metric_bar_filled(9.0, &scale, 20));
        assert!(metric_bar_filled(9.0, &scale, 20) > metric_bar_filled(5.0, &scale, 20));
    }
}
