use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

use super::text_width::display_width;
use super::text_width::truncate_display;
use super::widgets::{format_cost, format_tokens, light_ratio_bar_spans};
use crate::tui::app::{App, ClickAction};
use tokscale_core::pulse::weread::{
    format_compare_ratio, format_read_duration, now_millis, WeReadBookRef, WeReadCategory,
    WeReadFocusBook, WeReadMonthly, WeReadState, WeReadStatus, WeReadWeekly,
};
use tokscale_core::pulse::{
    PulseCoverage, PulseFreshness, PulseSnapshotV1, ReadingPulse, SignalLevel, SourceHealth,
};

const DAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MIN_WEEK_TABLE_WIDTH: u16 = 27;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.background)),
        area,
    );

    if app.pulse.snapshot.is_some() {
        render_snapshot(frame, app, area);
    } else if area.width < 44 || area.height < 15 {
        render_narrow(frame, app, area);
    } else {
        render_wide(frame, app, area);
    }
}

fn render_snapshot(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let summary_height = area.height.min(5);
    let remaining_height = area.height.saturating_sub(summary_height);
    let attention_reserve = if remaining_height >= 3 { 3 } else { 0 };
    let rhythm_height = remaining_height.saturating_sub(attention_reserve).min(7);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Length(rhythm_height),
            Constraint::Min(0),
        ])
        .split(area);

    render_snapshot_summary(frame, app, sections[0]);
    render_snapshot_rhythm(frame, app, sections[1]);

    let detail = sections[2];
    if detail.width == 0 || detail.height == 0 {
        return;
    }

    if detail.width >= 120 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Min(0)])
            .split(detail);
        render_attention(frame, app, columns[0]);
        render_source_context(frame, app, columns[1]);
    } else if detail.height >= 8 {
        let attention_height = detail
            .height
            .div_ceil(2)
            .max(4)
            .min(detail.height.saturating_sub(3));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(attention_height), Constraint::Min(0)])
            .split(detail);
        render_attention(frame, app, rows[0]);
        render_source_context(frame, app, rows[1]);
    } else {
        render_compact_snapshot_detail(frame, app, detail);
    }
}

fn render_snapshot_summary(frame: &mut Frame, app: &App, area: Rect) {
    let Some(snapshot) = &app.pulse.snapshot else {
        return;
    };
    let period_end = snapshot
        .period
        .end_exclusive
        .pred_opt()
        .unwrap_or(snapshot.period.end_exclusive);
    let block = panel_block(
        app,
        format!("Weekly Pulse  {} to {period_end}", snapshot.period.start),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let metrics = snapshot_summary_metrics(snapshot);

    if area.width >= 120 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(32),
                Constraint::Length(2),
                Constraint::Percentage(32),
                Constraint::Length(2),
                Constraint::Min(0),
            ])
            .split(inner);
        for ((title, primary, secondary), column) in metrics
            .into_iter()
            .zip([columns[0], columns[2], columns[4]])
        {
            render_summary_column(frame, app, column, title, &primary, &secondary);
        }
    } else {
        let lines = metrics
            .into_iter()
            .take(inner.height as usize)
            .map(|(title, primary, secondary)| {
                compact_summary_line(app, title, &primary, &secondary, inner.width as usize)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
            inner,
        );
    }
}

fn snapshot_summary_metrics(snapshot: &PulseSnapshotV1) -> [(&'static str, String, String); 3] {
    let mut ai_primary = Vec::new();
    if let Some(tokens) = snapshot.ai.total_tokens {
        ai_primary.push(format!("{} tokens", format_tokens(tokens)));
    }
    if let Some(cost) = snapshot.ai.total_cost {
        ai_primary.push(format_cost(cost));
    }
    let ai_primary = nonempty_join(ai_primary, "Weekly usage unavailable");

    let mut ai_secondary = Vec::new();
    if let Some(days) = snapshot.ai.active_days {
        ai_secondary.push(format!("{days} active days"));
    }
    if let Some(change) = snapshot.ai.token_change_ratio {
        ai_secondary.push(format!(
            "{} vs previous",
            format_compare_ratio(Some(change))
        ));
    }
    if let Some(used) = snapshot.ai.max_used_percent {
        ai_secondary.push(format!(
            "Quota {used:.0}% {}",
            snapshot.ai.quota_risk.label()
        ));
    }
    if let Some(model) = &snapshot.ai.leading_model {
        ai_secondary.push(format!("Model {model}"));
    } else if let Some(provider) = &snapshot.ai.leading_provider {
        ai_secondary.push(format!("Provider {provider}"));
    }
    let ai_secondary = nonempty_join(ai_secondary, &snapshot.ai.status);

    let mut reading_primary = Vec::new();
    if let Some(days) = snapshot.reading.read_days {
        reading_primary.push(format!("{days}/7 days"));
    }
    if let Some(seconds) = snapshot.reading.weekly_total_seconds {
        reading_primary.push(format_read_duration(seconds));
    } else if let Some(label) = &snapshot.reading.weekly_total_label {
        reading_primary.push(label.clone());
    }
    let reading_primary = nonempty_join(reading_primary, "Weekly reading unavailable");

    let mut reading_secondary = Vec::new();
    if let Some(change) = snapshot.reading.week_over_week_ratio {
        reading_secondary.push(format_compare_ratio(Some(change)));
    } else if let Some(change) = &snapshot.reading.week_over_week {
        reading_secondary.push(change.clone());
    }
    if let Some(book) = &snapshot.reading.focus_book {
        reading_secondary.push(format!("Focus {book}"));
    }
    let reading_secondary = nonempty_join(reading_secondary, &snapshot.reading.status);

    let mut knowledge_primary = Vec::new();
    if let Some(notes) = snapshot.knowledge_flow.total_notes {
        knowledge_primary.push(format!("{notes} notes"));
    }
    if let Some(books) = snapshot.knowledge_flow.total_books {
        knowledge_primary.push(format!("{books} books"));
    }
    let knowledge_primary = nonempty_join(knowledge_primary, "Notebook totals unavailable");

    let mut knowledge_secondary = vec![format!(
        "{} coverage",
        snapshot.knowledge_flow.coverage.label()
    )];
    if let Some(items) = snapshot.reading.shelf_visible_items {
        knowledge_secondary.push(format!("{items} shelf"));
    }

    [
        ("AI WORK", ai_primary, ai_secondary),
        ("READING INPUT", reading_primary, reading_secondary),
        (
            "KNOWLEDGE FLOW",
            knowledge_primary,
            knowledge_secondary.join("  "),
        ),
    ]
}

fn nonempty_join(values: Vec<String>, fallback: &str) -> String {
    if values.is_empty() {
        fallback.to_string()
    } else {
        values.join("  ")
    }
}

fn render_summary_column(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    primary: &str,
    secondary: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let lines = vec![
        Line::from(Span::styled(
            truncate_display(title, width),
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(metric_span(app, truncate_display(primary, width))),
        Line::from(muted_span(app, truncate_display(secondary, width))),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn compact_summary_line(
    app: &App,
    title: &str,
    primary: &str,
    secondary: &str,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let title = truncate_display(title, width);
    let mut used = display_width(&title);
    let mut spans = vec![Span::styled(
        title,
        Style::default()
            .fg(app.theme.foreground)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    )];
    if used >= width {
        return Line::from(spans);
    }

    let gap = (width - used).min(2);
    spans.push(Span::raw(" ".repeat(gap)));
    used += gap;

    let remaining = width.saturating_sub(used);
    let primary_fits = display_width(primary) <= remaining;
    let primary = truncate_display(primary, remaining);
    used += display_width(&primary);
    spans.push(metric_span(app, primary));

    if primary_fits && !secondary.is_empty() && used < width {
        let gap = (width - used).min(2);
        spans.push(Span::raw(" ".repeat(gap)));
        used += gap;
        spans.push(muted_span(
            app,
            truncate_display(secondary, width.saturating_sub(used)),
        ));
    }

    Line::from(spans)
}

fn render_snapshot_rhythm(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let block = panel_block(app, "WEEKLY READING RHYTHM");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.add_click_area(area, ClickAction::WeReadRefresh);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snapshot) = &app.pulse.snapshot else {
        return;
    };
    let reading = &snapshot.reading;
    if inner.width >= MIN_WEEK_TABLE_WIDTH && inner.height >= 4 && !reading.days.is_empty() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(snapshot_reading_summary_line(
                app,
                reading,
                inner.width as usize,
            ))
            .style(Style::default().bg(app.theme.background)),
            chunks[0],
        );
        render_snapshot_week_table(frame, app, reading, chunks[1]);
        render_snapshot_reading_context(frame, app, reading, chunks[2]);
    } else {
        let mut lines = vec![snapshot_reading_summary_line(
            app,
            reading,
            inner.width as usize,
        )];
        if reading.days.is_empty() && inner.height > 1 {
            lines.push(Line::from(muted_span(
                app,
                truncate_display(
                    "No weekly reading rhythm is available yet",
                    inner.width as usize,
                ),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
            inner,
        );
    }
}

fn snapshot_reading_summary_line(app: &App, reading: &ReadingPulse, width: usize) -> Line<'static> {
    let mut fields = Vec::new();
    if let Some(days) = reading.read_days {
        fields.push(format!("{days}/7"));
    }
    if let Some(seconds) = reading.weekly_total_seconds {
        fields.push(format_read_duration(seconds));
    }
    if let Some(seconds) = reading.daily_average_seconds {
        fields.push(format!("avg {}", format_read_duration(seconds)));
    }
    if let Some(change) = reading.week_over_week_ratio {
        fields.push(format_compare_ratio(Some(change)));
    }
    let summary = nonempty_join(fields, &format!("Sync {}", reading.status));
    Line::from(metric_span(app, truncate_display(&summary, width)))
}

fn render_snapshot_week_table(frame: &mut Frame, app: &App, reading: &ReadingPulse, area: Rect) {
    if area.width < MIN_WEEK_TABLE_WIDTH || area.height < 3 {
        return;
    }

    let spacing = if area.width >= 54 { 2 } else { 1 };
    let day_width = area
        .width
        .saturating_sub(spacing * 6)
        .checked_div(7)
        .unwrap_or(3)
        .max(3);
    let columns = vec![Constraint::Length(day_width); 7];
    let label_cells = DAY_LABELS
        .iter()
        .map(|label| centered_cell(muted_span(app, (*label).to_string())));
    let marker_cells = (0..7).map(|index| {
        let checked_in = reading.days.get(index).is_some_and(|day| day.checked_in);
        centered_cell(Span::styled(
            if checked_in { "✓" } else { "·" },
            Style::default()
                .fg(if checked_in {
                    app.theme.success_color()
                } else {
                    app.theme.muted
                })
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ))
    });
    let duration_cells = (0..7).map(|index| {
        let text = reading
            .days
            .get(index)
            .filter(|day| day.read_seconds > 0)
            .map(|day| format_read_duration(day.read_seconds))
            .unwrap_or_else(|| "--".to_string());
        centered_cell(muted_span(app, truncate_display(&text, day_width as usize)))
    });
    let rows = vec![
        Row::new(label_cells),
        Row::new(marker_cells),
        Row::new(duration_cells),
    ];
    frame.render_widget(
        Table::new(rows, columns)
            .column_spacing(spacing)
            .style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_snapshot_reading_context(
    frame: &mut Frame,
    app: &App,
    reading: &ReadingPulse,
    area: Rect,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut fields = Vec::new();
    if let Some(book) = &reading.focus_book {
        let duration = reading
            .focus_read_seconds
            .map(format_read_duration)
            .map(|value| format!(" {value}"))
            .unwrap_or_default();
        fields.push(format!("Focus {book}{duration}"));
    }
    if let Some(total) = &reading.month_total_label {
        let days = reading
            .month_read_days
            .map(|days| format!(" / {days} days"))
            .unwrap_or_default();
        fields.push(format!("Month {total}{days}"));
    }
    if let Some(topic) = &reading.preferred_category {
        fields.push(format!("Topic {topic}"));
    }
    if let Some(items) = reading.shelf_visible_items {
        fields.push(format!("Shelf {items}"));
    }
    let text = if fields.is_empty() {
        format!("Sync  {}", reading.status)
    } else {
        fields.join("  |  ")
    };
    frame.render_widget(
        Paragraph::new(Line::from(muted_span(
            app,
            truncate_display(&text, area.width as usize),
        )))
        .style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_attention(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "ATTENTION / NEXT");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snapshot) = &app.pulse.snapshot else {
        return;
    };
    let mut insights = snapshot.insights.iter().collect::<Vec<_>>();
    insights.sort_by_key(|insight| signal_level_priority(insight.level));

    let recommendations = snapshot.recommendations.iter().take(2).collect::<Vec<_>>();
    let height = inner.height as usize;
    let reserve_next = usize::from(!recommendations.is_empty() && height >= 2);
    let insights = insights
        .into_iter()
        .take(height.saturating_sub(reserve_next).min(2))
        .collect::<Vec<_>>();
    let show_summary = height > insights.len() + reserve_next;

    let mut lines = Vec::new();
    for (index, insight) in insights.into_iter().enumerate() {
        lines.push(attention_line(
            app,
            signal_level_label(insight.level),
            &insight.title,
            signal_level_style(app, insight.level),
            inner.width as usize,
        ));
        if index == 0 && show_summary && !insight.summary.is_empty() {
            lines.push(prefixed_line(
                "WHY",
                app.theme.subtle_text_style().bg(app.theme.background),
                &insight.summary,
                app.theme.subtle_text_style().bg(app.theme.background),
                inner.width as usize,
            ));
        }
    }
    for recommendation in recommendations.into_iter().take(1) {
        lines.push(attention_line(
            app,
            "NEXT",
            &recommendation.title,
            app.theme.info_style().bg(app.theme.background),
            inner.width as usize,
        ));
    }

    if lines.is_empty() {
        lines.push(Line::from(muted_span(
            app,
            truncate_display(
                "No evidence-backed attention item for this week",
                inner.width as usize,
            ),
        )));
    }
    lines.truncate(inner.height as usize);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        inner,
    );
}

fn signal_level_label(level: SignalLevel) -> &'static str {
    match level {
        SignalLevel::High => "HIGH",
        SignalLevel::Medium => "MED",
        SignalLevel::Low => "LOW",
        SignalLevel::Unknown => "INFO",
    }
}

fn signal_level_priority(level: SignalLevel) -> u8 {
    match level {
        SignalLevel::High => 0,
        SignalLevel::Medium => 1,
        SignalLevel::Low => 2,
        SignalLevel::Unknown => 3,
    }
}

fn signal_level_style(app: &App, level: SignalLevel) -> Style {
    let style = match level {
        SignalLevel::High => app.theme.danger_style(),
        SignalLevel::Medium => app.theme.warning_style(),
        SignalLevel::Low => app.theme.success_style(),
        SignalLevel::Unknown => app.theme.info_style(),
    };
    style.bg(app.theme.background)
}

fn attention_line(
    app: &App,
    label: &str,
    title: &str,
    label_style: Style,
    width: usize,
) -> Line<'static> {
    prefixed_line(
        label,
        label_style.add_modifier(Modifier::BOLD),
        title,
        Style::default()
            .fg(app.theme.foreground)
            .bg(app.theme.background),
        width,
    )
}

fn render_source_context(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "SOURCE HEALTH / CONTEXT");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snapshot) = &app.pulse.snapshot else {
        return;
    };
    let lines = source_context_lines(app, snapshot, inner.width as usize);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        inner,
    );
}

fn source_context_lines(app: &App, snapshot: &PulseSnapshotV1, width: usize) -> Vec<Line<'static>> {
    let short_id = snapshot
        .snapshot_id
        .rsplit('-')
        .next()
        .unwrap_or(&snapshot.snapshot_id)
        .chars()
        .take(8)
        .collect::<String>();
    let mut lines = vec![Line::from(muted_span(
        app,
        truncate_display(
            &format!("Local snapshot {short_id}  Week {}", snapshot.period.start),
            width,
        ),
    ))];

    let ready = snapshot
        .sources
        .iter()
        .filter(|source| source_health_kind(source) == SourceHealthKind::Ready)
        .count();
    let attention = snapshot
        .sources
        .iter()
        .filter(|source| {
            matches!(
                source_health_kind(source),
                SourceHealthKind::Blocked | SourceHealthKind::Attention
            )
        })
        .count();
    let missing = snapshot
        .sources
        .iter()
        .filter(|source| source_health_kind(source) == SourceHealthKind::Missing)
        .count();
    lines.push(Line::from(muted_span(
        app,
        truncate_display(
            &format!("Sources  {ready} ready  {attention} attention  {missing} missing"),
            width,
        ),
    )));

    let mut sources = snapshot.sources.iter().collect::<Vec<_>>();
    sources.sort_by_key(|source| source_health_priority(source));
    for source in sources {
        let mut detail = format!(
            "{}  {}  {}",
            source.status,
            source.coverage.label(),
            source.storage
        );
        if let Some(issue) = &source.issue_code {
            detail.push_str("  ");
            detail.push_str(issue);
        }
        lines.push(prefixed_line(
            source_display_name(&source.id),
            source_health_style(app, source).add_modifier(Modifier::BOLD),
            &detail,
            app.theme.subtle_text_style().bg(app.theme.background),
            width,
        ));
    }

    let mut month = Vec::new();
    if let Some(total) = &snapshot.reading.month_total_label {
        month.push(format!("Month {total}"));
    }
    if let Some(days) = snapshot.reading.month_read_days {
        month.push(format!("{days} days"));
    }
    if !month.is_empty() {
        lines.push(Line::from(muted_span(
            app,
            truncate_display(&month.join("  "), width),
        )));
    }
    if !snapshot.knowledge_flow.sampled_notebooks.is_empty() {
        lines.push(Line::from(muted_span(
            app,
            truncate_display(
                &format!(
                    "Sampled notebooks  {}  {} coverage",
                    snapshot.knowledge_flow.sampled_notebooks.len(),
                    snapshot.knowledge_flow.coverage.label()
                ),
                width,
            ),
        )));
    }

    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceHealthKind {
    Blocked,
    Missing,
    Attention,
    Ready,
}

fn source_health_kind(source: &SourceHealth) -> SourceHealthKind {
    let blocking_issue = matches!(
        source.issue_code.as_deref(),
        Some("auth_missing" | "upgrade_required")
    );
    let blocking_status = matches!(
        source.status.as_str(),
        "auth missing" | "auth_missing" | "upgrade required" | "upgrade_required" | "error"
    );
    if blocking_issue || blocking_status {
        SourceHealthKind::Blocked
    } else if source.freshness == PulseFreshness::Missing {
        SourceHealthKind::Missing
    } else if source.issue_code.is_some()
        || source.freshness == PulseFreshness::Stale
        || source.coverage != PulseCoverage::Complete
        || source.status != "fresh"
    {
        SourceHealthKind::Attention
    } else {
        SourceHealthKind::Ready
    }
}

fn source_health_priority(source: &SourceHealth) -> u8 {
    match source_health_kind(source) {
        SourceHealthKind::Blocked => 0,
        SourceHealthKind::Missing => 1,
        SourceHealthKind::Attention => 2,
        SourceHealthKind::Ready => 3,
    }
}

fn source_display_name(id: &str) -> &str {
    match id {
        "local-ai-usage" => "AI usage",
        "subscription-usage-cache" => "Quota cache",
        "weread" => "WeRead",
        _ => id,
    }
}

fn source_health_style(app: &App, source: &SourceHealth) -> Style {
    let style = match source_health_kind(source) {
        SourceHealthKind::Blocked | SourceHealthKind::Missing => app.theme.danger_style(),
        SourceHealthKind::Attention => app.theme.warning_style(),
        SourceHealthKind::Ready => app.theme.success_style(),
    };
    style.bg(app.theme.background)
}

fn prefixed_line(
    prefix: &str,
    prefix_style: Style,
    detail: &str,
    detail_style: Style,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let prefix = truncate_display(prefix, width);
    let mut used = display_width(&prefix);
    let mut spans = vec![Span::styled(prefix, prefix_style)];
    if used >= width {
        return Line::from(spans);
    }

    let gap = (width - used).min(2);
    spans.push(Span::raw(" ".repeat(gap)));
    used += gap;
    spans.push(Span::styled(
        truncate_display(detail, width.saturating_sub(used)),
        detail_style,
    ));
    Line::from(spans)
}

fn render_compact_snapshot_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "ATTENTION / NEXT");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snapshot) = &app.pulse.snapshot else {
        return;
    };
    let mut lines = Vec::new();
    if let Some(insight) = snapshot
        .insights
        .iter()
        .min_by_key(|insight| signal_level_priority(insight.level))
    {
        lines.push(attention_line(
            app,
            signal_level_label(insight.level),
            &insight.title,
            signal_level_style(app, insight.level),
            inner.width as usize,
        ));
    }
    if inner.height > 1 || lines.is_empty() {
        if let Some(recommendation) = snapshot.recommendations.first() {
            lines.push(attention_line(
                app,
                "NEXT",
                &recommendation.title,
                app.theme.info_style().bg(app.theme.background),
                inner.width as usize,
            ));
        }
    }
    lines.extend(
        source_context_lines(app, snapshot, inner.width as usize)
            .into_iter()
            .take(inner.height.saturating_sub(lines.len() as u16) as usize),
    );
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        inner,
    );
}

fn render_wide(frame: &mut Frame, app: &mut App, area: Rect) {
    let bottom_height = area.height.saturating_sub(8).clamp(7, 15);
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(bottom_height),
            Constraint::Min(0),
        ])
        .split(area);

    render_weread_pulse(frame, app, outer[0]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(outer[1]);

    render_month_rhythm(frame, app, bottom[0]);
    render_library_signals(frame, app, bottom[1]);
}

fn render_narrow(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(area);

    render_weread_pulse(frame, app, chunks[0]);
    render_month_rhythm(frame, app, chunks[1]);
    render_library_signals(frame, app, chunks[2]);
}

fn render_weread_pulse(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = panel_block(app, "WeRead Pulse");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.add_click_area(area, ClickAction::WeReadRefresh);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match &app.pulse.weread.weekly {
        Some(weekly) if inner.width >= MIN_WEEK_TABLE_WIDTH && inner.height >= 5 => {
            let focus_height = u16::from(weekly.focus.is_some() && inner.height >= 6);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(focus_height),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(inner);

            frame.render_widget(
                Paragraph::new(weekly_summary_line(app, weekly))
                    .style(Style::default().bg(app.theme.background)),
                chunks[0],
            );
            render_week_table(frame, app, weekly, chunks[1]);
            if let Some(focus) = &weekly.focus {
                render_focus_line(frame, app, focus, chunks[2]);
            }
            frame.render_widget(
                Paragraph::new(status_line(app, &app.pulse.weread))
                    .style(Style::default().bg(app.theme.background)),
                chunks[3],
            );
        }
        _ => {
            let lines = weread_pulse_lines(app, &app.pulse.weread, inner.width);
            frame.render_widget(
                Paragraph::new(lines)
                    .style(
                        Style::default()
                            .fg(app.theme.foreground)
                            .bg(app.theme.background),
                    )
                    .wrap(Wrap { trim: true }),
                inner,
            );
        }
    }
}

fn render_week_table(frame: &mut Frame, app: &App, weekly: &WeReadWeekly, area: Rect) {
    if area.width < MIN_WEEK_TABLE_WIDTH || area.height < 3 {
        return;
    }

    let spacing = if area.width >= 54 { 2 } else { 1 };
    let day_width = area
        .width
        .saturating_sub(spacing * 6)
        .checked_div(7)
        .unwrap_or(3)
        .clamp(3, 7);
    let columns = vec![Constraint::Length(day_width); 7];

    let label_cells = DAY_LABELS
        .iter()
        .map(|label| centered_cell(muted_span(app, (*label).to_string())));
    let marker_cells = weekly.days.iter().map(|day| {
        let symbol = if day.checked_in { "✓" } else { "·" };
        let color = if day.checked_in {
            Color::Green
        } else {
            app.theme.muted
        };
        centered_cell(Span::styled(
            symbol,
            Style::default()
                .fg(color)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ))
    });
    let duration_cells = weekly.days.iter().map(|day| {
        let text = if day.read_seconds == 0 {
            "--".to_string()
        } else {
            format_read_duration(day.read_seconds)
        };
        centered_cell(muted_span(app, truncate_display(&text, day_width as usize)))
    });

    let rows = vec![
        Row::new(label_cells),
        Row::new(marker_cells),
        Row::new(duration_cells),
    ];
    let table = Table::new(rows, columns)
        .column_spacing(spacing)
        .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn centered_cell(span: Span<'static>) -> Cell<'static> {
    Cell::from(Line::from(span).centered())
}

fn render_focus_line(frame: &mut Frame, app: &App, focus: &WeReadFocusBook, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label = format!(
        "Focus  {}  {} this week",
        focus.title,
        format_read_duration(focus.read_seconds)
    );
    frame.render_widget(
        Paragraph::new(Line::from(muted_span(
            app,
            truncate_display(&label, area.width as usize),
        )))
        .style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_month_rhythm(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "Month Rhythm");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if let Some(monthly) = &app.pulse.weread.monthly {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(inner);

        render_month_summary(frame, app, monthly, chunks[0]);
        render_category_table(frame, app, monthly.categories.as_slice(), chunks[1]);
    } else {
        frame.render_widget(
            Paragraph::new(empty_state_line(app)).style(Style::default().bg(app.theme.background)),
            inner,
        );
    }
}

fn render_month_summary(frame: &mut Frame, app: &App, monthly: &WeReadMonthly, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut lines = vec![Line::from(vec![
        metric_span(app, format_read_duration(monthly.total_seconds)),
        muted_span(app, format!("  {} days", monthly.read_days)),
        muted_span(
            app,
            format!(
                "  avg {}",
                format_read_duration(monthly.day_average_seconds)
            ),
        ),
    ])];

    if area.height > 1 {
        let preference = monthly.prefer_category_word.as_deref().unwrap_or("");
        lines.push(Line::from(muted_span(
            app,
            truncate_display(preference, area.width as usize),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_category_table(frame: &mut Frame, app: &App, categories: &[WeReadCategory], area: Rect) {
    if area.width < 14 || area.height == 0 {
        return;
    }

    let rank_width = 2u16;
    let time_width = 6u16;
    let spacing = 1u16;
    let min_label_width = 4u16;
    let fixed_width = rank_width + time_width + min_label_width + spacing * 3;
    let bar_width = area.width.saturating_sub(fixed_width).clamp(6, 22);
    let label_width = area
        .width
        .saturating_sub(rank_width + bar_width + time_width + spacing * 3)
        .max(min_label_width);

    let rows = categories
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(index, category)| category_row(app, category, index + 1, bar_width as usize));

    let table = Table::new(
        rows,
        [
            Constraint::Length(rank_width),
            Constraint::Length(bar_width),
            Constraint::Length(time_width),
            Constraint::Length(label_width),
        ],
    )
    .column_spacing(spacing)
    .style(Style::default().bg(app.theme.background));

    frame.render_widget(table, area);
}

fn category_row<'a>(
    app: &App,
    category: &WeReadCategory,
    rank: usize,
    bar_width: usize,
) -> Row<'a> {
    let rank = Line::from(Span::styled(
        rank.to_string(),
        Style::default()
            .fg(app.theme.accent)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    ))
    .right_aligned();
    let bar = Line::from(light_ratio_bar_spans(
        category.weight,
        bar_width,
        Style::default().fg(Color::Green).bg(app.theme.background),
        app.theme.subtle_text_style().bg(app.theme.background),
    ));
    let time = Line::from(muted_span(
        app,
        format_read_duration(category.reading_seconds),
    ))
    .right_aligned();

    Row::new([
        Cell::from(rank),
        Cell::from(bar),
        Cell::from(time),
        Cell::from(Line::from(muted_span(app, category.title.clone()))),
    ])
    .height(1)
    .style(Style::default().bg(app.theme.background))
}

fn render_library_signals(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "Library Signals");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let has_stats = app.pulse.weread.shelf.is_some() || app.pulse.weread.notes.is_some();
    let recent = app
        .pulse
        .weread
        .shelf
        .as_ref()
        .map(|shelf| shelf.recent.as_slice())
        .unwrap_or(&[]);

    if !has_stats && recent.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_state_line(app)).style(Style::default().bg(app.theme.background)),
            inner,
        );
        return;
    }

    let stats_height = if has_stats {
        u16::from(app.pulse.weread.shelf.is_some()) + u16::from(app.pulse.weread.notes.is_some())
    } else {
        0
    };
    let heading_height = if recent.is_empty() { 0 } else { 2 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(stats_height),
            Constraint::Length(heading_height),
            Constraint::Min(0),
        ])
        .split(inner);

    render_library_stats(frame, app, chunks[0]);
    render_recent_heading(frame, app, chunks[1]);
    render_recent_books(frame, app, recent, chunks[2]);
}

fn render_library_stats(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut rows = Vec::new();
    if let Some(shelf) = &app.pulse.weread.shelf {
        let public_items = shelf.visible_items.saturating_sub(shelf.private_items);
        let mut pairs = vec![
            ("Items", shelf.visible_items.to_string()),
            ("Private", shelf.private_items.to_string()),
        ];
        if area.width >= 50 {
            pairs.push(("Public", public_items.to_string()));
        }
        rows.push(stat_row(app, &pairs));
    }
    if let Some(notes) = &app.pulse.weread.notes {
        rows.push(stat_row(
            app,
            &[
                ("Notes", notes.total_notes.to_string()),
                ("Books", notes.total_books.to_string()),
            ],
        ));
    }

    if rows.is_empty() {
        return;
    }

    let widths = if area.width >= 50 {
        vec![
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
        ]
    } else {
        vec![
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
        ]
    };

    let table = Table::new(rows, widths)
        .column_spacing(2)
        .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn stat_row<'a>(app: &App, pairs: &[(&str, String)]) -> Row<'a> {
    let mut cells = Vec::with_capacity(pairs.len() * 2);
    for (label, value) in pairs {
        cells.push(Cell::from(Line::from(muted_span(
            app,
            (*label).to_string(),
        ))));
        cells.push(Cell::from(
            Line::from(metric_span(app, value.clone())).right_aligned(),
        ));
    }

    Row::new(cells)
        .height(1)
        .style(Style::default().bg(app.theme.background))
}

fn render_recent_heading(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }

    let lines = if area.height > 1 {
        vec![
            Line::from(""),
            Line::from(muted_span(app, "Recent focus".to_string())),
        ]
    } else {
        vec![Line::from(muted_span(app, "Recent focus".to_string()))]
    };

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_recent_books(frame: &mut Frame, app: &App, books: &[WeReadBookRef], area: Rect) {
    if area.width < 8 || area.height == 0 || books.is_empty() {
        return;
    }

    let rank_width = 2u16;
    let spacing = 1u16;
    let title_width = area.width.saturating_sub(rank_width + spacing);
    let rows = books
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(index, book)| recent_book_row(app, book, index + 1, title_width as usize));

    let table = Table::new(
        rows,
        [
            Constraint::Length(rank_width),
            Constraint::Length(title_width),
        ],
    )
    .column_spacing(spacing)
    .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn recent_book_row<'a>(
    app: &App,
    book: &WeReadBookRef,
    rank: usize,
    title_width: usize,
) -> Row<'a> {
    Row::new([
        Cell::from(
            Line::from(Span::styled(
                rank.to_string(),
                Style::default()
                    .fg(app.theme.accent)
                    .bg(app.theme.background)
                    .add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        ),
        Cell::from(Line::from(muted_span(
            app,
            truncate_display(&book.title, title_width),
        ))),
    ])
    .height(1)
    .style(Style::default().bg(app.theme.background))
}

fn weread_pulse_lines(app: &App, state: &WeReadState, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match &state.weekly {
        Some(weekly) => {
            lines.push(weekly_summary_line(app, weekly));
            if width >= MIN_WEEK_TABLE_WIDTH {
                lines.push(day_labels_line(app, width));
                lines.push(day_markers_line(app, weekly, width));
                lines.push(day_duration_line(app, weekly, width));
            }
            if let Some(focus) = &weekly.focus {
                let label = format!(
                    "Focus  {}  {} this week",
                    focus.title,
                    format_read_duration(focus.read_seconds)
                );
                lines.push(Line::from(vec![Span::styled(
                    truncate_display(&label, width as usize),
                    app.theme.subtle_text_style().bg(app.theme.background),
                )]));
            }
        }
        None => {
            lines.push(empty_state_line(app));
        }
    }

    lines.push(status_line(app, state));
    lines
}

fn weekly_summary_line(app: &App, weekly: &WeReadWeekly) -> Line<'static> {
    Line::from(vec![
        metric_span(app, format!("{}/7", weekly.read_days)),
        Span::raw("  "),
        metric_span(app, format_read_duration(weekly.total_seconds)),
        muted_span(
            app,
            format!(
                "  avg {}  {}",
                format_read_duration(weekly.day_average_seconds),
                format_compare_ratio(weekly.compare_ratio)
            ),
        ),
    ])
}

fn day_labels_line(app: &App, width: u16) -> Line<'static> {
    if width < 54 {
        return Line::from(muted_span(app, DAY_LABELS.join(" ")));
    }
    Line::from(muted_span(app, DAY_LABELS.join("   ")))
}

fn day_markers_line(app: &App, weekly: &WeReadWeekly, width: u16) -> Line<'static> {
    let gap = if width < 54 { "   " } else { "     " };
    let mut spans = Vec::new();
    for (index, day) in weekly.days.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(gap));
        }
        let symbol = if day.checked_in { "✓" } else { "·" };
        let color = if day.checked_in {
            Color::Green
        } else {
            app.theme.muted
        };
        spans.push(Span::styled(
            symbol,
            Style::default()
                .fg(color)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn day_duration_line(app: &App, weekly: &WeReadWeekly, width: u16) -> Line<'static> {
    let gap = if width < 54 { " " } else { "   " };
    let mut spans = Vec::new();
    for (index, day) in weekly.days.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(gap));
        }
        let text = if day.read_seconds == 0 {
            "--".to_string()
        } else {
            format_read_duration(day.read_seconds)
        };
        spans.push(muted_span(app, format!("{text:>3}")));
    }
    Line::from(spans)
}

fn status_line(app: &App, state: &WeReadState) -> Line<'static> {
    let mut spans = vec![muted_span(app, format!("Sync  {}", state.status.label()))];

    if let Some(last) = state.last_refresh_ms {
        let age_minutes = now_millis().saturating_sub(last) / 60_000;
        spans.push(muted_span(app, format!("  {age_minutes}m ago")));
    }

    if matches!(
        state.status,
        WeReadStatus::AuthMissing | WeReadStatus::Error | WeReadStatus::UpgradeRequired
    ) {
        if let Some(error) = &state.error {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate_display(error, 42),
                Style::default().fg(Color::Yellow).bg(app.theme.background),
            ));
        }
    }

    Line::from(spans)
}

fn panel_block(app: &App, title: impl Into<String>) -> Block<'static> {
    let title = title.into();
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(app.theme.border)
                .bg(app.theme.background),
        )
        .style(Style::default().bg(app.theme.background))
}

fn metric_span(app: &App, text: String) -> Span<'static> {
    Span::styled(
        text,
        Style::default()
            .fg(app.theme.accent)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    )
}

fn muted_span(app: &App, text: String) -> Span<'static> {
    Span::styled(text, app.theme.subtle_text_style().bg(app.theme.background))
}

fn empty_state_line(app: &App) -> Line<'static> {
    Line::from(Span::styled(
        "Set env.WEREAD_API_KEY in settings.json to enable reading pulse",
        app.theme.subtle_text_style().bg(app.theme.background),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiConfig;
    use crate::tui::data::UsageData;
    use chrono::{Duration, NaiveDate};
    use ratatui::{backend::TestBackend, Terminal};
    use tokscale_core::pulse::summary::ReadingDaySignal;
    use tokscale_core::pulse::weread::{WeReadDay, WeReadFocusBook, WeReadSyncState, WeReadWeekly};
    use tokscale_core::pulse::{
        AiQuotaMetric, AiQuotaSource, AiWorkInput, AiWorkPeriodInput, PulseInsight,
        PulseRecommendation,
    };

    fn make_app() -> App {
        let config = TuiConfig {
            theme: None,
            refresh: 0,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: None,
            initial_timeline_granularity: None,
        };
        App::new_with_cached_data(config, Some(UsageData::default())).unwrap()
    }

    fn snapshot_fixture() -> PulseSnapshotV1 {
        let mut snapshot = PulseSnapshotV1::from_inputs(
            AiWorkInput {
                current: Some(AiWorkPeriodInput {
                    total_tokens: 12_500_000,
                    total_cost: 42.50,
                    active_days: 5,
                    peak_day: None,
                    peak_day_tokens: 4_000_000,
                    leading_model: Some("claude-sonnet-4".to_string()),
                    leading_provider: Some("Anthropic".to_string()),
                }),
                previous: Some(AiWorkPeriodInput {
                    total_tokens: 10_000_000,
                    total_cost: 30.00,
                    active_days: 4,
                    peak_day: None,
                    peak_day_tokens: 3_000_000,
                    leading_model: None,
                    leading_provider: None,
                }),
                quota_sources: vec![AiQuotaSource {
                    provider: "Codex".to_string(),
                    metrics: vec![AiQuotaMetric {
                        label: "weekly".to_string(),
                        used_percent: 82.0,
                    }],
                }],
                observed_at: None,
            },
            WeReadSyncState::default(),
        );
        snapshot.snapshot_id = "pulse-test-snapshot".to_string();

        snapshot.reading.status = "partial".to_string();
        snapshot.reading.read_days = Some(4);
        snapshot.reading.weekly_total_seconds = Some(17_814);
        snapshot.reading.weekly_total_label = Some("4h56m".to_string());
        snapshot.reading.daily_average_seconds = Some(2_545);
        snapshot.reading.daily_average_label = Some("42m".to_string());
        snapshot.reading.week_over_week_ratio = Some(-0.18);
        snapshot.reading.week_over_week = Some("-18%".to_string());
        snapshot.reading.previous_read_days = Some(6);
        snapshot.reading.previous_weekly_total_seconds = Some(21_724);
        snapshot.reading.focus_book = Some("Systems Thinking".to_string());
        snapshot.reading.focus_book_id = Some("focus-1".to_string());
        snapshot.reading.focus_read_seconds = Some(3_600);
        snapshot.reading.month_total_label = Some("12h20m".to_string());
        snapshot.reading.month_read_days = Some(15);
        snapshot.reading.preferred_category = Some("science".to_string());
        snapshot.reading.shelf_visible_items = Some(128);
        snapshot.reading.days = [1_800, 0, 3_600, 4_814, 7_600, 0, 0]
            .into_iter()
            .enumerate()
            .map(|(offset, read_seconds)| ReadingDaySignal {
                date: snapshot.period.start + Duration::days(offset as i64),
                read_seconds,
                checked_in: read_seconds >= 60,
            })
            .collect();

        snapshot.knowledge_flow.total_notes = Some(77);
        snapshot.knowledge_flow.total_books = Some(34);
        snapshot.knowledge_flow.coverage = PulseCoverage::Partial;
        snapshot.insights = vec![
            PulseInsight {
                id: "reading-change".to_string(),
                level: SignalLevel::Medium,
                title: "Reading rhythm changed".to_string(),
                summary: "Long detail that belongs in the weekly review.".to_string(),
                evidence_refs: Vec::new(),
            },
            PulseInsight {
                id: "quota-pressure".to_string(),
                level: SignalLevel::High,
                title: "AI quota pressure".to_string(),
                summary: "Another long detail that the cockpit should not render.".to_string(),
                evidence_refs: Vec::new(),
            },
        ];
        snapshot.recommendations = vec![
            PulseRecommendation {
                id: "protect-reading".to_string(),
                title: "Protect one reading block".to_string(),
                rationale: "Long rationale that belongs outside the TUI.".to_string(),
                evidence_refs: Vec::new(),
            },
            PulseRecommendation {
                id: "reserve-runs".to_string(),
                title: "Reserve expensive runs for priorities".to_string(),
                rationale: "Another rationale that should stay out of the cockpit.".to_string(),
                evidence_refs: Vec::new(),
            },
        ];
        if let Some(source) = snapshot
            .sources
            .iter_mut()
            .find(|source| source.id == "weread")
        {
            source.status = "partial".to_string();
            source.freshness = PulseFreshness::Fresh;
            source.coverage = PulseCoverage::Partial;
            source.issue_code = Some("gateway_error".to_string());
        }

        snapshot
    }

    fn rendered_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
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
                let mut text = String::new();
                let mut continuation_cells = 0usize;
                for cell in row {
                    if continuation_cells > 0 {
                        continuation_cells -= 1;
                        continue;
                    }
                    let symbol = cell.symbol();
                    text.push_str(symbol);
                    continuation_cells = display_width(symbol).saturating_sub(1);
                }
                text
            })
            .collect()
    }

    #[test]
    fn renders_weekly_check_marks_without_panic() {
        let mut app = make_app();
        app.pulse.weread.weekly = Some(WeReadWeekly {
            period_start: NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 6, 14).unwrap(),
            read_days: 4,
            total_seconds: 17_814,
            day_average_seconds: 3_562,
            compare_ratio: Some(0.35),
            days: [
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(), 6089),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(), 8813),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(), 1281),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 11).unwrap(), 1631),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap(), 0),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 13).unwrap(), 0),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 14).unwrap(), 0),
            ],
            focus: Some(WeReadFocusBook {
                id: "1".to_string(),
                title: "Focus".to_string(),
                author: None,
                read_seconds: 3600,
            }),
        });

        let backend = TestBackend::new(92, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, Rect::new(0, 0, 92, 24)))
            .unwrap();

        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(92)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("WeRead Pulse"));
        assert!(output.contains("4/7"));
    }

    #[test]
    fn weekly_table_requires_room_for_all_seven_columns() {
        let app = make_app();
        let reading = snapshot_fixture().reading;

        for width in 24..=28 {
            let backend = TestBackend::new(width, 3);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_snapshot_week_table(frame, &app, &reading, Rect::new(0, 0, width, 3))
                })
                .unwrap();
            let output = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            let has_all_labels = DAY_LABELS.iter().all(|label| output.contains(label));

            assert_eq!(
                has_all_labels,
                width >= MIN_WEEK_TABLE_WIDTH,
                "unexpected weekday columns at width {width}: {output:?}"
            );
        }
    }

    #[test]
    fn renders_snapshot_weekly_cockpit_from_one_generation() {
        let mut app = make_app();
        app.pulse.weread.weekly = Some(WeReadWeekly {
            period_start: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2025, 1, 12).unwrap(),
            read_days: 7,
            total_seconds: 86_400,
            day_average_seconds: 12_342,
            compare_ratio: Some(4.0),
            days: std::array::from_fn(|offset| {
                WeReadDay::new(
                    NaiveDate::from_ymd_opt(2025, 1, 6 + offset as u32).unwrap(),
                    12_342,
                )
            }),
            focus: Some(WeReadFocusBook {
                id: "legacy".to_string(),
                title: "LEGACY OLD WEEK".to_string(),
                author: None,
                read_seconds: 86_400,
            }),
        });
        app.pulse.snapshot = Some(snapshot_fixture());

        let output = rendered_rows(&mut app, 92, 24).join("\n");

        for expected in [
            "Weekly Pulse",
            "AI WORK",
            "12.5M tokens",
            "$42.50",
            "5 active days",
            "READING INPUT",
            "4/7 days",
            "4h56m",
            "-18%",
            "KNOWLEDGE FLOW",
            "77 notes",
            "34 books",
            "partial coverage",
            "WEEKLY READING RHYTHM",
            "Focus Systems Thinking",
            "Topic science",
            "ATTENTION / NEXT",
            "AI quota pressure",
            "Protect one reading block",
            "SOURCE HEALTH / CONTEXT",
            "Local snapshot",
            "WeRead",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        assert!(!output.contains("LEGACY OLD WEEK"), "{output}");
        assert!(!output.contains("Long detail"), "{output}");
        assert!(!output.contains("Long rationale"), "{output}");

        let refresh_areas = app
            .click_areas
            .iter()
            .filter(|area| matches!(&area.action, ClickAction::WeReadRefresh))
            .collect::<Vec<_>>();
        assert_eq!(refresh_areas.len(), 1);
        assert_eq!(refresh_areas[0].rect, Rect::new(0, 5, 92, 7));
    }

    #[test]
    fn narrow_snapshot_cjk_content_stays_within_bounds() {
        let mut app = make_app();
        let mut snapshot = snapshot_fixture();
        snapshot.ai.leading_model = Some("超长中文模型名称用于验证终端宽度".repeat(2));
        snapshot.reading.focus_book = Some("深入理解计算机系统与人工智能工程实践".repeat(2));
        snapshot.reading.preferred_category = Some("计算机科学与技术".repeat(2));
        snapshot.insights[1].title = "高优先级配额压力需要立即检查".repeat(3);
        snapshot.recommendations[0].title = "保护本周阅读输入并整理长期笔记".repeat(3);
        app.pulse.snapshot = Some(snapshot);

        let rows = rendered_rows(&mut app, 44, 24);
        let output = rows.join("\n");

        for expected in ["AI WORK", "READING INPUT", "KNOWLEDGE FLOW", "ATTENTION"] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        assert!(
            output.contains('…'),
            "expected display-width truncation\n{output}"
        );
        for (row_index, row) in rows.iter().enumerate() {
            assert!(
                display_width(row) <= 44,
                "row {row_index} is {} cells wide: {row:?}",
                display_width(row)
            );
        }
        assert!(app
            .click_areas
            .iter()
            .all(|area| { area.rect.right() <= 44 && area.rect.bottom() <= 24 }));
    }

    #[test]
    fn compact_height_keeps_domains_and_highest_attention() {
        let mut app = make_app();
        app.pulse.snapshot = Some(snapshot_fixture());

        let output = rendered_rows(&mut app, 92, 12).join("\n");

        assert!(output.contains("AI WORK"), "{output}");
        assert!(output.contains("READING INPUT"), "{output}");
        assert!(output.contains("KNOWLEDGE FLOW"), "{output}");
        assert!(output.contains("AI quota pressure"), "{output}");
        assert!(!output.contains("Reading rhythm changed"), "{output}");
    }

    #[test]
    fn compact_detail_reserves_next_action_when_two_rows_fit() {
        let mut app = make_app();
        app.pulse.snapshot = Some(snapshot_fixture());

        let output = rendered_rows(&mut app, 80, 16).join("\n");

        assert!(output.contains("AI quota pressure"), "{output}");
        assert!(output.contains("Protect one reading block"), "{output}");
    }

    #[test]
    fn blocking_source_issue_overrides_fresh_complete_cache() {
        let mut snapshot = snapshot_fixture();
        let source = snapshot
            .sources
            .iter_mut()
            .find(|source| source.id == "weread")
            .unwrap();
        source.status = "upgrade required".to_string();
        source.freshness = PulseFreshness::Fresh;
        source.coverage = PulseCoverage::Complete;
        source.issue_code = Some("upgrade_required".to_string());

        assert_eq!(source_health_kind(source), SourceHealthKind::Blocked);
        assert_eq!(source_health_priority(source), 0);
    }

    #[test]
    fn truncate_display_respects_cjk_cell_width() {
        let text = "Focus  万物发明指南  5h9m this week";
        let truncated = truncate_display(text, 18);

        assert!(display_width(&truncated) <= 18);
        assert!(truncated.ends_with('…'));
    }
}
