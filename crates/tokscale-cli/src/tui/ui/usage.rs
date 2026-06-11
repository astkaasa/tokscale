use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::commands::usage::{helpers, UsageMetric, UsageOutput};
use crate::tui::app::{App, ClickAction, CodexLoginOutcome};
use crate::tui::ui::widgets::{
    get_provider_shade, light_ratio_bar_spans, truncate_ellipsis as truncate_string,
};

struct ButtonSpec {
    label: String,
    kind: ButtonKind,
    action: ClickAction,
}

#[derive(Clone, Copy)]
enum ButtonKind {
    Primary,
    Secondary,
    Danger,
    Disabled,
}

struct UsageProviderGroup<'a> {
    provider: &'a str,
    outputs: Vec<(usize, &'a UsageOutput)>,
}

struct UsageInventory {
    providers: usize,
    saved: usize,
    managed: usize,
}

#[derive(Clone, Copy)]
struct AccountTableColumns {
    marker: usize,
    provider: usize,
    account: usize,
    plan: usize,
    auth: usize,
    health: usize,
    limit: usize,
    reset: usize,
}

#[derive(Clone, Copy)]
struct AccountTableRects {
    marker: Rect,
    provider: Rect,
    account: Rect,
    plan: Rect,
    auth: Rect,
    health: Rect,
    limit: Rect,
    reset: Rect,
}

struct UsageRowView<'a> {
    account: String,
    account_summary: String,
    plan: String,
    limit: String,
    reset: String,
    readiness: UsageReadiness,
    metric: Option<&'a UsageMetric>,
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Usage ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                status_label(app),
                app.theme.subtle_text_style(),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let content = render_action_bar(frame, app, inner);
    let content = render_codex_login_panel(frame, app, content);

    let outputs = app.subscription_usage.clone();
    if outputs.is_empty() {
        if app.is_fetching_usage() {
            render_fetching(frame, app, content);
        } else if app.usage_fetch_attempted {
            render_empty(frame, app, content);
        } else {
            render_ready(frame, app, content);
        }
    } else if outputs.iter().all(|output| output.metrics.is_empty()) {
        render_empty(frame, app, content);
    } else {
        render_loaded(frame, app, content, &outputs);
    }
}

fn status_label(app: &App) -> String {
    if app.is_fetching_usage() {
        return "Syncing usage".to_string();
    }
    if app.is_codex_login_running() {
        return "Codex login".to_string();
    }

    let inventory = usage_inventory(&app.subscription_usage);

    if inventory.providers == 0 && app.usage_fetch_attempted {
        "No data".to_string()
    } else if inventory.providers == 0 {
        "Not loaded".to_string()
    } else {
        format!(
            "{} providers · {}",
            inventory.providers,
            identity_count_label(inventory.saved, inventory.managed)
        )
    }
}

fn usage_inventory(outputs: &[UsageOutput]) -> UsageInventory {
    let providers = outputs
        .iter()
        .map(|output| output.provider.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let saved = outputs
        .iter()
        .filter(|output| output.account.is_some())
        .count();
    UsageInventory {
        providers,
        saved,
        managed: outputs.len().saturating_sub(saved),
    }
}

fn identity_count_label(saved: usize, managed: usize) -> String {
    match (saved, managed) {
        (0, 0) => "0 saved".to_string(),
        (saved, 0) => format!("{saved} saved"),
        (0, managed) => format!("{managed} managed"),
        (saved, managed) => format!("{saved} saved · {managed} managed"),
    }
}

fn render_action_bar(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    if area.height == 0 {
        return area;
    }
    let compact = area.width < 48;
    let show_prefix = area.width >= 36;

    let refresh_label = if app.is_fetching_usage() {
        if compact { "u Sync" } else { "u Syncing" }.to_string()
    } else {
        "u Refresh".to_string()
    };
    let refresh_style = if app.is_fetching_usage() {
        ButtonKind::Disabled
    } else {
        ButtonKind::Primary
    };

    let add_label = if app.is_codex_login_running() {
        if compact {
            "a Adding"
        } else {
            "a Adding Codex"
        }
        .to_string()
    } else {
        if compact { "a Add" } else { "a Add Codex" }.to_string()
    };
    let add_style = if app.is_codex_login_running() {
        ButtonKind::Disabled
    } else {
        ButtonKind::Secondary
    };

    let mut buttons = vec![
        ButtonSpec {
            label: refresh_label,
            kind: refresh_style,
            action: ClickAction::UsageRefresh,
        },
        ButtonSpec {
            label: add_label,
            kind: add_style,
            action: ClickAction::CodexStartLogin,
        },
    ];
    if !app.subscription_usage.is_empty() {
        buttons.push(ButtonSpec {
            label: if app.hide_usage_emails {
                if compact { "m Show" } else { "m Show Emails" }.to_string()
            } else {
                if compact { "m Hide" } else { "m Hide Emails" }.to_string()
            },
            kind: ButtonKind::Secondary,
            action: ClickAction::UsageToggleEmailPrivacy,
        });
    }

    let mut spans = Vec::new();
    if show_prefix {
        spans.push(Span::styled(" Actions ", app.theme.subtle_text_style()));
    }
    let start_x = area.x + Line::from(spans.clone()).width() as u16;
    push_click_buttons(&mut spans, app, buttons, start_x, area.y, area.right());

    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    if area.height > 1 {
        Rect::new(area.x, area.y + 1, area.width, area.height - 1)
    } else {
        Rect::new(area.x, area.y, area.width, 0)
    }
}

fn push_click_buttons(
    spans: &mut Vec<Span<'static>>,
    app: &mut App,
    buttons: Vec<ButtonSpec>,
    start_x: u16,
    y: u16,
    right_edge: u16,
) {
    let mut x = start_x;
    for (index, button) in buttons.into_iter().enumerate() {
        let rendered = button_label(&button.label);
        let width = Line::from(rendered.as_str()).width() as u16;
        let separator_width = u16::from(index > 0);
        if x.saturating_add(separator_width).saturating_add(width) > right_edge {
            break;
        }

        if index > 0 {
            spans.push(Span::raw(" "));
            x = x.saturating_add(1);
        }

        spans.push(Span::styled(
            rendered,
            button_style(app, button.kind, false),
        ));

        if x < right_edge {
            app.add_click_area(Rect::new(x, y, width.min(right_edge - x), 1), button.action);
        }
        x = x.saturating_add(width);
    }
}

fn button_label(label: &str) -> String {
    format!(" {label} ")
}

fn button_style(app: &App, kind: ButtonKind, selected: bool) -> Style {
    if selected {
        return Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD);
    }

    match kind {
        ButtonKind::Primary => Style::default()
            .fg(app.theme.background)
            .bg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
        ButtonKind::Secondary => Style::default().fg(app.theme.accent).bg(app.theme.border),
        ButtonKind::Danger => Style::default().fg(Color::Red).bg(app.theme.border),
        ButtonKind::Disabled => Style::default().fg(app.theme.muted).bg(app.theme.border),
    }
}

fn render_codex_login_panel(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    if area.height == 0 || !app.should_show_codex_login_panel() {
        return area;
    }

    let max_output_lines = 4usize;
    let output_start = app.codex_login_lines.len().saturating_sub(max_output_lines);
    let output_lines: Vec<String> = app.codex_login_lines[output_start..].to_vec();
    let height = (2 + output_lines.len() as u16 + u16::from(app.codex_login_outcome.is_some()))
        .min(area.height);
    if height == 0 {
        return area;
    }

    let mut lines: Vec<Line> = Vec::new();
    let status = match &app.codex_login_outcome {
        Some(CodexLoginOutcome::Imported(_)) => "Imported",
        Some(CodexLoginOutcome::Failed(_)) => "Failed",
        None if app.is_codex_login_running() => "Running",
        None => "Idle",
    };

    let mut header_spans = vec![
        Span::styled(
            " Codex Login ",
            Style::default()
                .fg(app.theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(status.to_string(), app.theme.subtle_text_style()),
    ];
    if app.codex_login_outcome.is_some() {
        let dismiss = "[Dismiss]";
        let dismiss_width = dismiss.chars().count() as u16;
        let used_width = Line::from(header_spans.clone()).width();
        let padding = (area.width as usize).saturating_sub(used_width + dismiss_width as usize);
        header_spans.push(Span::raw(" ".repeat(padding)));
        header_spans.push(Span::styled(dismiss, Style::default().fg(app.theme.accent)));
        let x = area
            .x
            .saturating_add(area.width.saturating_sub(dismiss_width));
        app.add_click_area(
            Rect::new(x, area.y, dismiss_width.min(area.width), 1),
            ClickAction::CodexDismissLogin,
        );
    }
    lines.push(Line::from(header_spans));

    if output_lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Waiting for codex output...",
            Style::default().fg(app.theme.muted),
        )));
    } else {
        for line in output_lines {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_string(&line, area.width.saturating_sub(4) as usize)
                ),
                Style::default().fg(app.theme.muted),
            )));
        }
    }

    if let Some(outcome) = &app.codex_login_outcome {
        let (label, style) = match outcome {
            CodexLoginOutcome::Imported(info) => (
                format!(
                    "  Imported {}",
                    info.label.as_deref().unwrap_or(info.id.as_str())
                ),
                Style::default().fg(app.theme.accent),
            ),
            CodexLoginOutcome::Failed(error) => {
                (format!("  {error}"), Style::default().fg(Color::Red))
            }
        };
        lines.push(Line::from(Span::styled(
            truncate_string(&label, area.width as usize),
            style,
        )));
    }

    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y, area.width, height),
    );

    if area.height > height {
        Rect::new(area.x, area.y + height, area.width, area.height - height)
    } else {
        Rect::new(area.x, area.y, area.width, 0)
    }
}

fn render_fetching(frame: &mut Frame, app: &App, area: Rect) {
    let center = centered_rect(area, 3);
    let spin = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][app.spinner_frame % 10];
    let message = if area.width < 40 {
        format!("{spin} Fetching usage...")
    } else {
        format!("{spin} Fetching subscription data...")
    };
    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn render_ready(frame: &mut Frame, app: &App, area: Rect) {
    let center = centered_rect(area, 4);
    let lines = if area.width < 40 {
        vec![Line::from(Span::styled(
            "No usage data",
            Style::default()
                .fg(app.theme.foreground)
                .add_modifier(Modifier::BOLD),
        ))]
    } else {
        vec![
            Line::from(Span::styled(
                "No subscription data loaded",
                Style::default()
                    .fg(app.theme.foreground)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Use Refresh to sync provider usage, or Add Codex to save another account.",
                Style::default().fg(app.theme.muted),
            )),
        ]
    };
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn render_empty(frame: &mut Frame, app: &App, area: Rect) {
    let center = centered_rect(area, 3);
    let message = if area.width < 40 {
        "No usage data"
    } else {
        "No subscription data available"
    };
    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn centered_rect(area: Rect, height: u16) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(height.min(area.height)),
            Constraint::Percentage(40),
        ])
        .split(area);
    chunks[1]
}

fn render_loaded(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    app.selected_index = app.selected_index.min(outputs.len().saturating_sub(1));

    if area.width < 104 || area.height < 20 {
        render_compact_loaded(frame, app, area, outputs);
        return;
    }

    let top_height = if area.height >= 31 {
        12
    } else {
        (area.height / 2).clamp(8, 11)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_height), Constraint::Min(0)])
        .split(area);

    let (summary_width, detail_width) = usage_top_column_percentages(area.width);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(summary_width),
            Constraint::Percentage(detail_width),
        ])
        .split(chunks[0]);

    render_usage_status(frame, app, top[0], outputs);
    let selected_index = app.selected_index;
    render_selected_account(frame, app, top[1], &outputs[selected_index], outputs);
    render_accounts_table(frame, app, chunks[1], outputs);
}

fn usage_top_column_percentages(width: u16) -> (u16, u16) {
    if width >= 128 {
        (50, 50)
    } else {
        (48, 52)
    }
}

fn render_compact_loaded(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(7.min(area.height))])
        .split(area);
    render_accounts_table(frame, app, chunks[0], outputs);
    if chunks.len() > 1 && chunks[1].height > 0 {
        let selected_index = app.selected_index;
        render_selected_account(frame, app, chunks[1], &outputs[selected_index], outputs);
    }
}

fn render_usage_status(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Usage Summary ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines = usage_status_summary_lines(app, outputs, inner.width as usize);

    let attention_outputs = attention_outputs(outputs);
    if lines.len() + 1 < inner.height as usize {
        lines.push(Line::from(Span::styled(
            " Attention",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));

        if attention_outputs.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No accounts need attention",
                app.theme.subtle_text_style(),
            )));
        } else {
            let available = (inner.height as usize).saturating_sub(lines.len());
            let mut visible_count = attention_outputs.len().min(available.min(2));
            if attention_outputs.len() > visible_count && visible_count == available {
                visible_count = visible_count.saturating_sub(1);
            }

            for (index, output) in attention_outputs.iter().take(visible_count).copied() {
                let y = inner.y.saturating_add(lines.len() as u16);
                app.add_click_area(
                    Rect::new(inner.x, y, inner.width, 1),
                    ClickAction::UsageSelect { index },
                );
                lines.push(attention_line(app, output, inner.width as usize));
            }

            let hidden_count = attention_outputs.len().saturating_sub(visible_count);
            if hidden_count > 0 && lines.len() < inner.height as usize {
                lines.push(attention_more_line(hidden_count, inner.width as usize));
            }
        }
    }

    if lines.len() + 2 < inner.height as usize {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Providers",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for group in group_outputs_by_provider(outputs) {
            if lines.len() >= inner.height as usize {
                break;
            }
            lines.push(provider_summary_line(app, &group, inner.width as usize));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn usage_status_summary_lines(
    app: &App,
    outputs: &[UsageOutput],
    width: usize,
) -> Vec<Line<'static>> {
    let ready_count = outputs
        .iter()
        .filter(|output| readiness_status(output) == UsageReadiness::Ready)
        .count();
    let watch_count = outputs
        .iter()
        .filter(|output| readiness_status(output) == UsageReadiness::Watch)
        .count();
    let critical_count = outputs
        .iter()
        .filter(|output| readiness_status(output) == UsageReadiness::Critical)
        .count();
    let unknown_count = outputs
        .iter()
        .filter(|output| readiness_status(output) == UsageReadiness::Unknown)
        .count();

    let overall = overall_readiness(outputs);
    let active = active_output(outputs)
        .map(|output| account_name(app, output))
        .unwrap_or_else(|| "No active account".to_string());
    let fallback = best_fallback_output(outputs)
        .map(|output| {
            let score = output_score(output);
            if score > 0.0 {
                format!("{} · {:.0}% left", account_name(app, output), score)
            } else {
                account_name(app, output)
            }
        })
        .unwrap_or_else(|| "No ready fallback".to_string());
    let next_reset = next_reset_label(app, outputs).unwrap_or_else(|| "No reset data".to_string());
    let action = overall_action(app, outputs);
    let capacity = format!(
        "{ready_count} ready · {watch_count} watch · {critical_count} critical{}",
        if unknown_count > 0 {
            format!(" · {unknown_count} unknown")
        } else {
            String::new()
        }
    );

    let mut lines = Vec::new();
    push_kv_styled(
        &mut lines,
        app,
        "State",
        overall_state_label(outputs),
        Style::default()
            .fg(readiness_color(app, overall))
            .add_modifier(Modifier::BOLD),
        width,
    );
    push_kv_styled(
        &mut lines,
        app,
        "Active",
        &active,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        width,
    );
    push_kv_styled(
        &mut lines,
        app,
        "Capacity",
        &capacity,
        app.theme.secondary_text_style(),
        width,
    );
    push_kv_styled(
        &mut lines,
        app,
        "Fallback",
        &fallback,
        app.theme.secondary_text_style(),
        width,
    );
    push_kv_styled(
        &mut lines,
        app,
        "Next Reset",
        &next_reset,
        app.theme.secondary_text_style(),
        width,
    );
    push_kv_styled(
        &mut lines,
        app,
        "Action",
        &action,
        Style::default()
            .fg(readiness_color(app, overall))
            .add_modifier(Modifier::BOLD),
        width,
    );

    lines
}

fn push_kv_styled(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    key: &'static str,
    value: &str,
    value_style: Style,
    width: usize,
) {
    let max_value = width.saturating_sub(15);
    lines.push(Line::from(vec![
        Span::styled(format!(" {:<12}", key), app.theme.subtle_text_style()),
        Span::styled(truncate_string(value, max_value), value_style),
    ]));
}

fn attention_outputs(outputs: &[UsageOutput]) -> Vec<(usize, &UsageOutput)> {
    let mut items: Vec<(usize, &UsageOutput)> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| readiness_status(output).is_at_risk())
        .collect();

    items.sort_by(|(left_index, left), (right_index, right)| {
        attention_severity_rank(*left)
            .cmp(&attention_severity_rank(*right))
            .then_with(|| attention_action_rank(left).cmp(&attention_action_rank(right)))
            .then_with(|| output_score(left).total_cmp(&output_score(right)))
            .then_with(|| left_index.cmp(right_index))
    });

    items
}

fn attention_severity_rank(output: &UsageOutput) -> u8 {
    match readiness_status(output) {
        UsageReadiness::Critical => 0,
        UsageReadiness::Watch => 1,
        UsageReadiness::Ready => 2,
        UsageReadiness::Unknown => 3,
    }
}

fn attention_action_rank(output: &UsageOutput) -> u8 {
    match &output.account {
        Some(account) if account.is_active => 0,
        Some(_) => 1,
        None => 2,
    }
}

fn attention_line(app: &App, output: &UsageOutput, width: usize) -> Line<'static> {
    let status = readiness_status(output);
    let metric = display_metric(output);
    let detail = metric
        .map(|metric| {
            let reset = metric
                .resets_at
                .as_ref()
                .map(|reset| format!(" · {}", helpers::format_reset_time(reset)))
                .unwrap_or_default();
            format!("{} {}{}", metric.label, remaining_label(metric), reset)
        })
        .unwrap_or_else(|| "No quota metrics".to_string());
    let account_width: usize = if width >= 52 { 24 } else { 18 };
    let used = 2 + 11 + account_width;
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<11}", readiness_label(status)),
            Style::default().fg(readiness_color(app, status)),
        ),
        Span::styled(
            format!(
                "{:<width$}",
                truncate_string(&account_name(app, output), account_width.saturating_sub(1)),
                width = account_width
            ),
            app.theme.secondary_text_style(),
        ),
        Span::styled(
            truncate_string(&detail, width.saturating_sub(used + 1)),
            metric
                .map(|metric| Style::default().fg(metric_color(app, metric)))
                .unwrap_or_else(|| app.theme.subtle_text_style()),
        ),
    ])
}

fn attention_more_line(hidden_count: usize, width: usize) -> Line<'static> {
    let label = if hidden_count == 1 {
        "+1 more at risk".to_string()
    } else {
        format!("+{hidden_count} more at risk")
    };
    Line::from(Span::styled(
        format!("  {}", truncate_string(&label, width.saturating_sub(2))),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ))
}

fn provider_summary_line(app: &App, group: &UsageProviderGroup<'_>, width: usize) -> Line<'static> {
    let saved = group
        .outputs
        .iter()
        .filter(|(_, output)| output.account.is_some())
        .count();
    let managed = group.outputs.len().saturating_sub(saved);
    let count_label = identity_count_label(saved, managed);
    let ready = group
        .outputs
        .iter()
        .filter(|(_, output)| readiness_status(output) == UsageReadiness::Ready)
        .count();
    let risk = group
        .outputs
        .iter()
        .filter(|(_, output)| readiness_status(output).is_at_risk())
        .count();
    let active = group.outputs.iter().find_map(|(_, output)| {
        output
            .account
            .as_ref()
            .filter(|a| a.is_active)
            .map(|_| account_name(app, output))
    });

    let summary = if risk > 0 {
        format!("{count_label} · {ready} ready · {risk} at risk")
    } else {
        format!("{count_label} · {ready} ready")
    };
    let mut spans = vec![
        Span::styled(
            format!(" {}", truncate_string(group.provider, 18)),
            Style::default()
                .fg(get_provider_shade(group.provider, 0))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {summary}"), app.theme.subtle_text_style()),
    ];

    if let Some(active) = active {
        let used = Line::from(spans.clone()).width();
        let suffix = format!("Active: {}", truncate_string(&active, 20));
        let padding = width.saturating_sub(used + suffix.chars().count());
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(
            suffix,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    }

    Line::from(spans)
}

fn render_selected_account(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    selected: &UsageOutput,
    outputs: &[UsageOutput],
) {
    let title = format!(" Selected Account  {} ", output_display_name(app, selected));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            truncate_string(&title, area.width.saturating_sub(4) as usize),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let readiness = readiness_status(selected);
    let max_lines = inner.height as usize;
    let action_lines = max_lines.min(2);
    let detail_limit = max_lines.saturating_sub(action_lines);
    let mut lines = Vec::new();

    if lines.len() < detail_limit {
        push_kv_styled(
            &mut lines,
            app,
            "Status",
            &selected_status_line(selected),
            Style::default()
                .fg(readiness_color(app, readiness))
                .add_modifier(Modifier::BOLD),
            inner.width as usize,
        );
    }
    if lines.len() < detail_limit {
        push_kv_styled(
            &mut lines,
            app,
            "Email",
            &email_display(app, selected.email.as_deref()),
            app.theme.secondary_text_style(),
            inner.width as usize,
        );
    }
    if lines.len() < detail_limit {
        if let Some(account) = &selected.account {
            push_kv_styled(
                &mut lines,
                app,
                "Credential",
                if account.is_active {
                    "saved store, active auth.json"
                } else {
                    "saved store"
                },
                app.theme.secondary_text_style(),
                inner.width as usize,
            );
        } else {
            push_kv_styled(
                &mut lines,
                app,
                "Credential",
                "managed externally",
                app.theme.secondary_text_style(),
                inner.width as usize,
            );
        }
    }
    if lines.len() < detail_limit {
        lines.push(Line::from(Span::styled(
            " Limits",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let mut metric_index = 0usize;
    while lines.len() < detail_limit {
        if selected.metrics.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No quota metrics returned",
                app.theme.subtle_text_style(),
            )));
            break;
        }
        if let Some(metric) = selected.metrics.get(metric_index) {
            lines.push(metric_detail_line(app, metric, inner.width as usize));
            metric_index += 1;
        } else {
            break;
        }
    }
    if lines.len() < detail_limit {
        lines.push(snapshot_line(app, outputs, inner.width as usize));
    }
    if lines.len() + 1 < max_lines {
        lines.push(Line::from(Span::styled(
            " Actions",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if lines.len() < max_lines {
        let y = inner.y.saturating_add(lines.len() as u16);
        lines.push(selected_account_actions_line(app, selected, inner, y));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn selected_status_line(output: &UsageOutput) -> String {
    let plan = output.plan.as_deref().unwrap_or("Unknown");
    format!(
        "{} · {}",
        plan,
        account_readiness_label(output, readiness_status(output))
    )
}

fn selected_account_actions_line(
    app: &mut App,
    selected: &UsageOutput,
    area: Rect,
    y: u16,
) -> Line<'static> {
    if let Some(account) = &selected.account {
        let mut spans = Vec::new();
        if account.is_active {
            spans.push(Span::styled(
                "  Current account  ",
                app.theme.subtle_text_style(),
            ));
            let x = area
                .x
                .saturating_add(Line::from(spans.clone()).width() as u16);
            push_click_buttons(
                &mut spans,
                app,
                vec![remove_account_button(&account.id)],
                x,
                y,
                area.right(),
            );
        } else {
            spans.push(Span::raw("  "));
            let x = area.x.saturating_add(2);
            push_click_buttons(
                &mut spans,
                app,
                vec![
                    use_account_button(&account.id),
                    remove_account_button(&account.id),
                ],
                x,
                y,
                area.right(),
            );
        }
        return Line::from(spans);
    }

    Line::from(Span::styled(
        "  Managed externally",
        app.theme.subtle_text_style(),
    ))
}

fn account_plan_label(account: &str, plan: Option<&str>) -> String {
    match plan.map(str::trim).filter(|plan| !plan.is_empty()) {
        Some(plan) => format!("{account}  {plan}"),
        None => account.to_string(),
    }
}

fn account_readiness_label(output: &UsageOutput, readiness: UsageReadiness) -> String {
    let account_state = account_state_label(output);
    let readiness = readiness_label(readiness);
    if account_state.eq_ignore_ascii_case(readiness) {
        readiness.to_string()
    } else {
        format!("{account_state} · {readiness}")
    }
}

fn snapshot_line(app: &App, outputs: &[UsageOutput], width: usize) -> Line<'static> {
    let ready = outputs
        .iter()
        .filter(|output| readiness_status(output) == UsageReadiness::Ready)
        .count();
    let at_risk = outputs
        .iter()
        .filter(|output| readiness_status(output).is_at_risk())
        .count();
    let inventory = usage_inventory(outputs);
    let summary = format!(
        " Snapshot  {ready} ready · {at_risk} at risk · {}{}",
        identity_count_label(inventory.saved, inventory.managed),
        if app.hide_usage_emails {
            " · emails hidden"
        } else {
            ""
        }
    );
    Line::from(Span::styled(
        truncate_string(&summary, width),
        app.theme.subtle_text_style(),
    ))
}

fn metric_detail_line(app: &App, metric: &UsageMetric, width: usize) -> Line<'static> {
    let remaining = remaining_label(metric);
    let bar_width = width.saturating_sub(34).clamp(10, 32);
    let reset = metric
        .resets_at
        .as_ref()
        .map(|r| helpers::format_reset_time(r))
        .unwrap_or_default();
    let color = metric_color(app, metric);
    let mut spans = vec![Span::styled(
        format!(" {:<10}", truncate_string(&metric.label, 10)),
        app.theme.subtle_text_style(),
    )];
    spans.extend(quota_bar_spans(
        metric.remaining_percent,
        bar_width,
        color,
        app,
    ));
    spans.extend([
        Span::raw(" "),
        Span::styled(
            format!("{:<11}", truncate_string(&remaining, 11)),
            Style::default().fg(color),
        ),
        Span::styled(reset, app.theme.subtle_text_style()),
    ]);
    Line::from(spans)
}

fn render_accounts_table(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Accounts ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if account_table_columns(inner.width).is_none() {
        render_narrow_accounts_table(frame, app, inner, outputs);
        return;
    }

    let max_rows = inner.height.saturating_sub(1) as usize;
    app.set_max_visible_items(max_rows.max(1));
    render_account_table_header(frame, app, inner);
    let start = app
        .scroll_offset
        .min(outputs.len().saturating_sub(max_rows));
    for (visible_row, (index, output)) in outputs
        .iter()
        .enumerate()
        .skip(start)
        .take(max_rows)
        .enumerate()
    {
        let y = inner.y.saturating_add(1 + visible_row as u16);
        app.add_click_area(
            Rect::new(inner.x, y, inner.width, 1),
            ClickAction::UsageSelect { index },
        );
        render_account_table_row(
            frame,
            app,
            Rect::new(inner.x, y, inner.width, 1),
            output,
            index,
        );
    }
}

fn render_narrow_accounts_table(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    outputs: &[UsageOutput],
) {
    let mut lines = vec![narrow_table_header(app, area.width)];
    let row_height = 2usize;
    let max_items = (area.height.saturating_sub(1) as usize / row_height).max(1);
    app.set_max_visible_items(max_items);
    let start = app
        .scroll_offset
        .min(outputs.len().saturating_sub(max_items));

    for (visible_row, (index, output)) in outputs
        .iter()
        .enumerate()
        .skip(start)
        .take(max_items)
        .enumerate()
    {
        let y = area
            .y
            .saturating_add(1)
            .saturating_add((visible_row * row_height) as u16);
        app.add_click_area(
            Rect::new(
                area.x,
                y,
                area.width,
                row_height.min(area.bottom().saturating_sub(y) as usize) as u16,
            ),
            ClickAction::UsageSelect { index },
        );
        lines.extend(narrow_table_row(app, output, index, area, y));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_account_table_header(frame: &mut Frame, app: &App, area: Rect) {
    let Some(rects) = account_table_rects(area) else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                " #  Provider       Account                 Auth     Health      Limits / Reset",
                app.theme.subtle_text_style(),
            )])),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    };

    let style = app.theme.subtle_text_style();
    render_table_cell(frame, "#", rects.marker, style, CellAlign::Right);
    render_table_cell(frame, "Provider", rects.provider, style, CellAlign::Left);
    render_table_cell(frame, "Account", rects.account, style, CellAlign::Left);
    render_table_cell(frame, "Plan", rects.plan, style, CellAlign::Left);
    render_table_cell(frame, "Auth", rects.auth, style, CellAlign::Left);
    render_table_cell(frame, "Health", rects.health, style, CellAlign::Left);
    render_table_cell(frame, "Limit", rects.limit, style, CellAlign::Left);
    render_table_cell(frame, "Reset", rects.reset, style, CellAlign::Left);
}

fn narrow_table_header(app: &App, width: u16) -> Line<'static> {
    Line::from(Span::styled(
        truncate_string(" #  Account / Status", width as usize),
        app.theme.subtle_text_style(),
    ))
}

fn account_table_columns(width: u16) -> Option<AccountTableColumns> {
    if width < 132 {
        return None;
    }

    let width = width as usize;
    let marker = 4;
    let provider = if width >= 170 { 14 } else { 12 };
    let plan = if width >= 170 { 12 } else { 10 };
    let auth = if width >= 170 { 9 } else { 8 };
    let health = if width >= 170 { 12 } else { 10 };
    let limit = if width >= 170 { 22 } else { 20 };
    let column_count = 8;
    let separators = column_count;
    let fixed = marker + provider + plan + auth + health + limit + separators;
    let remaining = width.saturating_sub(fixed);
    let min_account = if width >= 170 { 28 } else { 22 };
    let min_reset = 14;
    if remaining < min_account + min_reset {
        None
    } else {
        let preferred_account = if width >= 190 {
            36
        } else if width >= 170 {
            32
        } else {
            26
        };
        let account = preferred_account.min(remaining.saturating_sub(min_reset));
        let reset = remaining
            .saturating_sub(account)
            .min(if width >= 170 { 28 } else { 22 });
        Some(AccountTableColumns {
            marker,
            provider,
            account,
            plan,
            auth,
            health,
            limit,
            reset,
        })
    }
}

fn account_table_rects(area: Rect) -> Option<AccountTableRects> {
    let columns = account_table_columns(area.width)?;
    let mut x = area.x;
    let y = area.y;
    let right = area.right();

    fn take_cell(x: &mut u16, y: u16, right: u16, width: usize) -> Rect {
        let available = right.saturating_sub(*x);
        let width = (width as u16).min(available);
        let rect = Rect::new(*x, y, width, 1);
        *x = x.saturating_add(width).saturating_add(1);
        rect
    }

    Some(AccountTableRects {
        marker: take_cell(&mut x, y, right, columns.marker),
        provider: take_cell(&mut x, y, right, columns.provider),
        account: take_cell(&mut x, y, right, columns.account),
        plan: take_cell(&mut x, y, right, columns.plan),
        auth: take_cell(&mut x, y, right, columns.auth),
        health: take_cell(&mut x, y, right, columns.health),
        limit: take_cell(&mut x, y, right, columns.limit),
        reset: take_cell(&mut x, y, right, columns.reset),
    })
}

fn narrow_table_row(
    app: &mut App,
    output: &UsageOutput,
    index: usize,
    area: Rect,
    _y: u16,
) -> Vec<Line<'static>> {
    let selected = app.selected_index == index;
    let width = area.width as usize;
    let row = usage_row_view(app, output);
    let state = if width >= 70 {
        account_readiness_label(output, row.readiness)
    } else {
        readiness_label(row.readiness).to_string()
    };
    let state_width = if width >= 52 {
        14usize
    } else if width >= 40 {
        10usize
    } else {
        0usize
    };
    let left_width = width.saturating_sub(4 + state_width);
    let left = format!("{} {}", output.provider, row.account_summary);
    let mut first = vec![
        styled(
            format!(" {:<2} ", index + 1),
            app.theme.secondary_text_style(),
            selected,
        ),
        styled(
            format!(
                "{:<width$}",
                truncate_string(&left, left_width.saturating_sub(1)),
                width = left_width
            ),
            app.theme.secondary_text_style(),
            selected,
        ),
    ];
    if state_width > 0 {
        first.push(styled(
            truncate_string(&state, state_width),
            Style::default().fg(readiness_color(app, row.readiness)),
            selected,
        ));
    }

    let detail = if row.reset.is_empty() {
        row.limit.clone()
    } else {
        format!("{} · {}", row.limit, row.reset)
    };

    let managed_label = if output.account.is_none() {
        Some("Managed")
    } else {
        None
    };
    let action_width = managed_label.map(str::len).unwrap_or(0);
    let available_detail_width = width
        .saturating_sub(4 + action_width + usize::from(action_width > 0))
        .max(8);
    let detail_width = available_detail_width.min(if width >= 72 { 48 } else { 34 });
    let detail_style = row
        .metric
        .map(|metric| Style::default().fg(metric_color(app, metric)))
        .unwrap_or_else(|| app.theme.secondary_text_style());
    let mut second = vec![
        styled("    ", Style::default(), selected),
        styled(
            truncate_string(&detail, detail_width.saturating_sub(1)),
            detail_style,
            selected,
        ),
    ];
    let used = Line::from(second.clone()).width();
    if action_width > 0 && area.width as usize > used + action_width {
        second.push(styled(" ", Style::default(), selected));
    }
    if let Some(label) = managed_label {
        second.push(styled(label, app.theme.subtle_text_style(), selected));
    }
    pad_selected_row(&mut second, area.width as usize, selected);

    vec![Line::from(first), Line::from(second)]
}

fn usage_row_view<'a>(app: &App, output: &'a UsageOutput) -> UsageRowView<'a> {
    let account = account_name(app, output);
    let metric = display_metric(output);
    let plan = output
        .plan
        .as_deref()
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .unwrap_or("Unknown")
        .to_string();
    let limit = metric
        .map(|metric| format!("{} {}", metric.label, remaining_label(metric)))
        .unwrap_or_else(|| "No limits".to_string());
    let reset = metric
        .and_then(|metric| metric.resets_at.as_ref())
        .map(|reset| helpers::format_reset_time(reset))
        .unwrap_or_default();
    let account_summary = account_plan_label(&account, output.plan.as_deref());

    UsageRowView {
        account,
        account_summary,
        plan,
        limit,
        reset,
        readiness: readiness_status(output),
        metric,
    }
}

fn render_account_table_row(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    output: &UsageOutput,
    index: usize,
) {
    let selected = app.selected_index == index;
    let row = usage_row_view(app, output);
    let Some(rects) = account_table_rects(area) else {
        return;
    };

    let auth = account_auth_label(output);
    let health = readiness_label(row.readiness);
    let row_style = usage_table_row_style(app, index, selected);
    frame.render_widget(Paragraph::new("").style(row_style), area);

    let provider_style = if selected {
        usage_table_text_style(app, selected).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(get_provider_shade(&output.provider, 0))
            .add_modifier(Modifier::BOLD)
    };
    let auth_color = account_auth_color(output);
    let health_color = readiness_color(app, row.readiness);
    let metric_color = row.metric.map(|metric| metric_color(app, metric));
    render_table_cell(
        frame,
        &usage_row_marker(index),
        rects.marker,
        usage_table_text_style(app, selected),
        CellAlign::Right,
    );
    render_table_cell(
        frame,
        &output.provider,
        rects.provider,
        provider_style,
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        &row.account,
        rects.account,
        usage_table_text_style(app, selected),
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        &row.plan,
        rects.plan,
        usage_table_text_style(app, selected),
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        auth,
        rects.auth,
        usage_table_color_style(app, selected, auth_color),
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        health,
        rects.health,
        usage_table_color_style(app, selected, health_color),
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        &row.limit,
        rects.limit,
        metric_color
            .map(|color| usage_table_color_style(app, selected, color))
            .unwrap_or_else(|| usage_table_text_style(app, selected)),
        CellAlign::Left,
    );
    render_table_cell(
        frame,
        &row.reset,
        rects.reset,
        usage_table_subtle_style(app, selected),
        CellAlign::Left,
    );
}

fn pad_selected_row(spans: &mut Vec<Span<'static>>, width: usize, selected: bool) {
    let used = Line::from(spans.clone()).width();
    if used < width {
        spans.push(styled(" ".repeat(width - used), Style::default(), selected));
    }
}

fn fit_cell(text: &str, width: usize) -> String {
    let mut value = truncate_string(text, width);
    while Line::from(value.as_str()).width() > width {
        value.pop();
    }
    value
}

#[derive(Clone, Copy)]
enum CellAlign {
    Left,
    Right,
}

fn render_table_cell(frame: &mut Frame, text: &str, area: Rect, style: Style, align: CellAlign) {
    if area.width == 0 {
        return;
    }
    let text = fit_cell(text, area.width as usize);
    let content = match align {
        CellAlign::Left => text,
        CellAlign::Right => {
            let used = Line::from(text.as_str()).width();
            let width = area.width as usize;
            format!("{}{}", " ".repeat(width.saturating_sub(used)), text)
        }
    };
    frame.render_widget(Paragraph::new(Span::styled(content, style)), area);
}

fn usage_row_marker(index: usize) -> String {
    (index + 1).to_string()
}

fn usage_table_row_style(app: &App, index: usize, selected: bool) -> Style {
    if selected {
        Style::default().bg(app.theme.selection)
    } else if index % 2 == 1 {
        app.theme.striped_row_style()
    } else {
        Style::default()
    }
}

fn usage_table_text_style(app: &App, selected: bool) -> Style {
    if selected {
        Style::default().fg(app.theme.foreground)
    } else {
        app.theme.secondary_text_style()
    }
}

fn usage_table_subtle_style(app: &App, selected: bool) -> Style {
    if selected {
        Style::default().fg(app.theme.foreground)
    } else {
        app.theme.subtle_text_style()
    }
}

fn usage_table_color_style(app: &App, selected: bool, color: Color) -> Style {
    if selected {
        Style::default().fg(app.theme.foreground)
    } else {
        Style::default().fg(color)
    }
}

fn use_account_button(account_id: &str) -> ButtonSpec {
    ButtonSpec {
        label: "Use Account".to_string(),
        kind: ButtonKind::Primary,
        action: ClickAction::CodexUseAccount {
            account_id: account_id.to_string(),
        },
    }
}

fn remove_account_button(account_id: &str) -> ButtonSpec {
    ButtonSpec {
        label: "Remove".to_string(),
        kind: ButtonKind::Danger,
        action: ClickAction::CodexRemoveAccount {
            account_id: account_id.to_string(),
        },
    }
}

fn group_outputs_by_provider(outputs: &[UsageOutput]) -> Vec<UsageProviderGroup<'_>> {
    let mut groups: Vec<UsageProviderGroup<'_>> = Vec::new();

    for (index, output) in outputs.iter().enumerate() {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.provider == output.provider)
        {
            group.outputs.push((index, output));
        } else {
            groups.push(UsageProviderGroup {
                provider: &output.provider,
                outputs: vec![(index, output)],
            });
        }
    }

    groups
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UsageReadiness {
    Ready,
    Watch,
    Critical,
    Unknown,
}

impl UsageReadiness {
    fn is_at_risk(self) -> bool {
        matches!(self, UsageReadiness::Watch | UsageReadiness::Critical)
    }
}

fn readiness_status(output: &UsageOutput) -> UsageReadiness {
    if output.metrics.is_empty() {
        return UsageReadiness::Unknown;
    }

    let lowest = output_score(output);
    if lowest < 10.0 {
        UsageReadiness::Critical
    } else if lowest < 25.0 {
        UsageReadiness::Watch
    } else {
        UsageReadiness::Ready
    }
}

fn overall_readiness(outputs: &[UsageOutput]) -> UsageReadiness {
    if outputs.is_empty() {
        return UsageReadiness::Unknown;
    }

    if let Some(active) = active_output(outputs) {
        let active_status = readiness_status(active);
        if active_status.is_at_risk() {
            return active_status;
        }
    }

    if outputs
        .iter()
        .any(|output| readiness_status(output) == UsageReadiness::Critical)
    {
        UsageReadiness::Watch
    } else if outputs
        .iter()
        .any(|output| readiness_status(output) == UsageReadiness::Watch)
    {
        UsageReadiness::Watch
    } else if outputs
        .iter()
        .any(|output| readiness_status(output) == UsageReadiness::Ready)
    {
        UsageReadiness::Ready
    } else {
        UsageReadiness::Unknown
    }
}

fn overall_state_label(outputs: &[UsageOutput]) -> &'static str {
    if let Some(active) = active_output(outputs) {
        if readiness_status(active) == UsageReadiness::Critical
            && best_fallback_output(outputs).is_some()
        {
            return "Switch recommended";
        }
    }

    match overall_readiness(outputs) {
        UsageReadiness::Ready => "Ready",
        UsageReadiness::Watch => "Ready with warnings",
        UsageReadiness::Critical => "Quota low",
        UsageReadiness::Unknown => "Unknown",
    }
}

fn readiness_label(status: UsageReadiness) -> &'static str {
    match status {
        UsageReadiness::Ready => "Ready",
        UsageReadiness::Watch => "Watch",
        UsageReadiness::Critical => "Quota Low",
        UsageReadiness::Unknown => "Unknown",
    }
}

fn readiness_color(app: &App, status: UsageReadiness) -> Color {
    match status {
        UsageReadiness::Ready => app.theme.accent,
        UsageReadiness::Watch => Color::Yellow,
        UsageReadiness::Critical => Color::Red,
        UsageReadiness::Unknown => app.theme.muted,
    }
}

fn active_output(outputs: &[UsageOutput]) -> Option<&UsageOutput> {
    outputs.iter().find(|output| {
        output
            .account
            .as_ref()
            .is_some_and(|account| account.is_active)
    })
}

fn best_fallback_output(outputs: &[UsageOutput]) -> Option<&UsageOutput> {
    outputs
        .iter()
        .filter(|output| {
            !output
                .account
                .as_ref()
                .is_some_and(|account| account.is_active)
                && readiness_status(output) == UsageReadiness::Ready
        })
        .max_by(|a, b| output_score(a).total_cmp(&output_score(b)))
}

fn output_score(output: &UsageOutput) -> f64 {
    output
        .metrics
        .iter()
        .map(|metric| metric.remaining_percent)
        .min_by(|a, b| a.total_cmp(b))
        .unwrap_or(0.0)
}

fn display_metric(output: &UsageOutput) -> Option<&UsageMetric> {
    if output
        .metrics
        .iter()
        .any(|metric| metric.remaining_percent < 25.0)
    {
        output
            .metrics
            .iter()
            .min_by(|a, b| a.remaining_percent.total_cmp(&b.remaining_percent))
    } else {
        output.metrics.first()
    }
}

fn next_reset_label(app: &App, outputs: &[UsageOutput]) -> Option<String> {
    if let Some(active) = active_output(outputs) {
        if let Some(label) = output_reset_label(app, active) {
            return Some(label);
        }
    }

    outputs
        .iter()
        .find_map(|output| output_reset_label(app, output))
}

fn output_reset_label(app: &App, output: &UsageOutput) -> Option<String> {
    let metric = display_metric(output)?;
    let reset = metric.resets_at.as_ref()?;
    Some(format!(
        "{} · {}",
        truncate_string(&account_name(app, output), 22),
        helpers::format_reset_time(reset)
    ))
}

fn overall_action(app: &App, outputs: &[UsageOutput]) -> String {
    let Some(active) = active_output(outputs) else {
        return "Choose an active account".to_string();
    };

    match readiness_status(active) {
        UsageReadiness::Ready => {
            if outputs
                .iter()
                .any(|output| readiness_status(output) == UsageReadiness::Unknown)
            {
                "Refresh accounts with unknown limits".to_string()
            } else {
                "Keep current account".to_string()
            }
        }
        UsageReadiness::Watch => "Monitor active quota".to_string(),
        UsageReadiness::Critical => best_fallback_output(outputs)
            .map(|fallback| format!("Use {}", account_name(app, fallback)))
            .unwrap_or_else(|| "Wait for reset or refresh".to_string()),
        UsageReadiness::Unknown => "Refresh active account".to_string(),
    }
}

fn output_display_name(app: &App, output: &UsageOutput) -> String {
    match &output.account {
        Some(_) => format!("{} ({})", output.provider, account_name(app, output)),
        None => {
            if output.email.is_some() {
                format!("{} ({})", output.provider, account_name(app, output))
            } else {
                output.provider.clone()
            }
        }
    }
}

fn account_name(app: &App, output: &UsageOutput) -> String {
    if app.hide_usage_emails {
        if let Some(account) = &output.account {
            if let Some(label) = account
                .label_name()
                .filter(|label| !looks_like_email(label))
            {
                return label.to_string();
            }
            return format!("Account {}", account.short_id());
        }

        if output.email.as_deref().is_some_and(looks_like_email) {
            return "[hidden email]".to_string();
        }
    }

    output
        .account_display_name()
        .or_else(|| output.email.clone())
        .map(|value| privacy_text(app, &value))
        .unwrap_or_else(|| output.provider.clone())
}

fn email_display(app: &App, email: Option<&str>) -> String {
    match email {
        Some(email) => privacy_text(app, email),
        None => "Unknown".to_string(),
    }
}

fn privacy_text(app: &App, value: &str) -> String {
    if app.hide_usage_emails && looks_like_email(value) {
        "[hidden email]".to_string()
    } else {
        value.to_string()
    }
}

fn looks_like_email(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.contains('@') && trimmed.split('@').count() == 2
}

fn account_state_label(output: &UsageOutput) -> String {
    match &output.account {
        Some(account) if account.is_active => "Active".to_string(),
        Some(_) => "Saved".to_string(),
        None if output.metrics.iter().any(|m| m.remaining_percent < 25.0) => {
            "Quota low".to_string()
        }
        None => "Authenticated".to_string(),
    }
}

fn account_auth_label(output: &UsageOutput) -> &'static str {
    match &output.account {
        Some(account) if account.is_active => "Active",
        Some(_) => "Saved",
        None => "Managed",
    }
}

fn account_auth_color(output: &UsageOutput) -> Color {
    match &output.account {
        Some(account) if account.is_active => Color::Green,
        Some(_) => Color::Blue,
        None => Color::Yellow,
    }
}

fn remaining_label(metric: &UsageMetric) -> String {
    metric
        .remaining_label
        .clone()
        .unwrap_or_else(|| format!("{:.0}% left", metric.remaining_percent))
}

fn metric_color(app: &App, metric: &UsageMetric) -> Color {
    if metric.remaining_percent < 10.0 {
        Color::Red
    } else if metric.remaining_percent < 25.0 {
        Color::Yellow
    } else {
        app.theme.accent
    }
}

fn quota_bar_spans(
    remaining_percent: f64,
    width: usize,
    color: Color,
    app: &App,
) -> Vec<Span<'static>> {
    light_ratio_bar_spans(
        remaining_percent / 100.0,
        width,
        Style::default().fg(color),
        app.theme.subtle_text_style(),
    )
}

fn styled<T: Into<String>>(text: T, style: Style, selected: bool) -> Span<'static> {
    let style = if selected {
        style.bg(Color::Blue).fg(Color::White)
    } else {
        style
    };
    Span::styled(text.into(), style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::UsageAccount;
    use crate::tui::app::{Tab, TuiConfig};
    use crate::tui::data::UsageData;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};

    fn output(provider: &str, account: Option<UsageAccount>) -> UsageOutput {
        UsageOutput {
            provider: provider.to_string(),
            account,
            plan: Some("Pro".to_string()),
            email: Some("user@example.com".to_string()),
            metrics: vec![UsageMetric {
                label: "Session".to_string(),
                used_percent: 10.0,
                remaining_percent: 90.0,
                remaining_label: Some("90% left".to_string()),
                resets_at: None,
            }],
        }
    }

    fn output_with_remaining(
        provider: &str,
        account: Option<UsageAccount>,
        remaining_percent: f64,
    ) -> UsageOutput {
        let mut output = output(provider, account);
        output.metrics[0].remaining_percent = remaining_percent;
        output.metrics[0].remaining_label = Some(format!("{remaining_percent:.0}% left"));
        output
    }

    fn make_app() -> App {
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
        let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();
        app.current_tab = Tab::Usage;
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
                    .map(|cell| cell.symbol().to_string())
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

    fn visual_col(line: &str, needle: &str) -> Option<usize> {
        let byte = line.find(needle)?;
        Some(Line::from(&line[..byte]).width())
    }

    #[test]
    fn usage_quota_bar_uses_light_empty_track() {
        let app = make_app();
        let metric = UsageMetric {
            label: "Session".to_string(),
            used_percent: 50.0,
            remaining_percent: 50.0,
            remaining_label: Some("50% left".to_string()),
            resets_at: None,
        };

        let line = metric_detail_line(&app, &metric, 72);
        let text = line_text(&line);

        assert!(text.contains("█"), "{text}");
        assert!(text.contains("·"), "{text}");
        assert!(!text.contains("░"), "{text}");
    }

    #[test]
    fn usage_quota_bar_uses_trace_mark_for_sub_cell_remaining() {
        let app = make_app();
        let spans = quota_bar_spans(0.1, 20, Color::Red, &app);
        let text = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("▏"), "{text}");
        assert!(!text.contains("█"), "{text}");
        assert_eq!(text.chars().count(), 20);
    }

    #[test]
    fn usage_status_distinguishes_not_loaded_from_empty_results() {
        let mut app = make_app();
        assert_eq!(status_label(&app), "Not loaded");

        app.usage_fetch_attempted = true;
        assert_eq!(status_label(&app), "No data");
    }

    #[test]
    fn groups_usage_outputs_by_provider_preserving_first_seen_order() {
        let outputs = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output("Claude", None),
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let groups = group_outputs_by_provider(&outputs);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].provider, "Codex");
        assert_eq!(groups[0].outputs.len(), 2);
        assert_eq!(groups[0].outputs[0].0, 0);
        assert_eq!(groups[0].outputs[1].0, 2);
        assert_eq!(groups[1].provider, "Claude");
        assert_eq!(groups[1].outputs.len(), 1);
    }

    #[test]
    fn renders_usage_workspace_sections_and_codex_actions() {
        let mut app = make_app();
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let body = render_body(&mut app, 150, 32);

        assert!(body.contains("Usage Summary"), "{body}");
        assert!(body.contains("Selected Account"), "{body}");
        assert!(body.contains("Accounts"), "{body}");
        assert!(body.contains("Attention"), "{body}");
        assert!(body.contains("Account") && body.contains("Plan"), "{body}");
        assert!(body.contains("Keep current account"), "{body}");
        assert!(body.contains("work"), "{body}");
        assert!(body.contains("personal"), "{body}");
        assert!(body.contains(" Remove "), "{body}");
        assert!(body.contains("Show Emails"), "{body}");
    }

    #[test]
    fn selected_account_panel_follows_selected_index() {
        let mut app = make_app();
        app.selected_index = 1;
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let body = render_body(&mut app, 150, 28);

        assert!(body.contains("Codex (personal)"), "{body}");
        assert!(body.contains("Use Account"), "{body}");
        assert!(body.contains("saved store"), "{body}");
    }

    #[test]
    fn usage_header_counts_saved_and_managed_identities() {
        let mut app = make_app();
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output("Copilot", None),
        ];

        let body = render_body(&mut app, 160, 28);

        assert!(body.contains("2 providers · 1 saved · 1 managed"), "{body}");
        assert!(
            body.contains("Snapshot  2 ready · 0 at risk · 1 saved · 1 managed"),
            "{body}"
        );
    }

    #[test]
    fn usage_top_columns_balance_wide_and_medium_screens() {
        assert_eq!(usage_top_column_percentages(160), (50, 50));
        assert_eq!(usage_top_column_percentages(128), (50, 50));
        assert_eq!(usage_top_column_percentages(127), (48, 52));
        assert_eq!(usage_top_column_percentages(104), (48, 52));
    }

    #[test]
    fn usage_account_actions_render_in_selected_account_panel() {
        let mut app = make_app();
        app.selected_index = 1;
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let body = render_body(&mut app, 210, 30);
        let saved_row = body
            .lines()
            .find(|line| line.contains("personal") && line.contains("Session"))
            .expect("missing saved account table row");
        let actions_line = body
            .lines()
            .find(|line| line.contains("Use Account") && line.contains("Remove"))
            .expect("missing selected account action buttons");

        assert!(!saved_row.contains("Use Account"), "{saved_row}");
        assert!(!saved_row.contains("Remove"), "{saved_row}");
        assert!(
            visual_col(actions_line, "Use Account").is_some(),
            "{actions_line}"
        );
    }

    #[test]
    fn usage_account_table_header_aligns_with_wide_columns() {
        let mut app = make_app();
        app.subscription_usage = vec![
            {
                let mut output = output(
                    "Codex",
                    Some(UsageAccount {
                        id: "acct_work".to_string(),
                        label: Some("work".to_string()),
                        is_active: true,
                    }),
                );
                output.metrics[0].resets_at = Some("resets soon".to_string());
                output
            },
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let body = render_body(&mut app, 180, 24);
        let header = body
            .lines()
            .find(|line| line.contains("Provider") && line.contains("Reset"))
            .expect("missing account table header");
        let active_row = body
            .lines()
            .find(|line| {
                line.contains("Codex") && line.contains("work") && line.contains("Session")
            })
            .expect("missing active account table row");

        assert_eq!(
            visual_col(header, "Provider"),
            visual_col(active_row, "Codex"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Account"),
            visual_col(active_row, "work"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Plan"),
            visual_col(active_row, "Pro"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Auth"),
            visual_col(active_row, "Active"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Health"),
            visual_col(active_row, "Ready"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Limit"),
            visual_col(active_row, "Session"),
            "{header}\n{active_row}"
        );
        assert_eq!(
            visual_col(header, "Reset"),
            visual_col(active_row, "resets soon"),
            "{header}\n{active_row}"
        );
        assert!(!header.contains("Use"), "{header}");
        assert!(!header.contains("Remove"), "{header}");
        assert!(!active_row.contains("Remove"), "{active_row}");
    }

    #[test]
    fn narrow_usage_keeps_account_actions_visible() {
        let mut app = make_app();
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];

        let body = render_body(&mut app, 90, 28);

        assert!(body.contains("work"), "{body}");
        assert!(body.contains("personal"), "{body}");
        assert!(body.contains(" Remove "), "{body}");
    }

    #[test]
    fn very_narrow_ready_usage_uses_compact_actions_and_message() {
        let mut app = make_app();
        let body = render_body(&mut app, 28, 20);

        assert!(body.contains("u Refresh"), "{body}");
        assert!(body.contains("a Add"), "{body}");
        assert!(body.contains("No usage data"), "{body}");
        assert!(!body.contains("Add Codex"), "{body}");
        assert!(!body.contains("No subscription data load"), "{body}");
    }

    #[test]
    fn empty_usage_hides_email_privacy_action() {
        let mut app = make_app();
        let body = render_body(&mut app, 150, 20);

        assert!(body.contains("u Refresh"), "{body}");
        assert!(body.contains("a Add Codex"), "{body}");
        assert!(!body.contains("Show Emails"), "{body}");
        assert!(!body.contains("Hide Emails"), "{body}");
    }

    #[test]
    fn usage_navigation_scrolls_by_rendered_capacity() {
        let mut app = make_app();
        app.subscription_usage = (0..30)
            .map(|index| {
                output(
                    "Codex",
                    Some(UsageAccount {
                        id: format!("acct_{index:02}"),
                        label: Some(format!("acct-{index:02}")),
                        is_active: index == 0,
                    }),
                )
            })
            .collect();

        let _ = render_body(&mut app, 170, 32);
        let visible = app.max_visible_items;
        for _ in 0..visible {
            app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }

        assert!(app.scroll_offset > 0);
        assert!(app.selected_index >= visible);
    }

    #[test]
    fn attention_outputs_prioritize_actionable_accounts() {
        let outputs = vec![
            output_with_remaining("Copilot", None, 0.0),
            output_with_remaining(
                "Codex",
                Some(UsageAccount {
                    id: "acct_saved".to_string(),
                    label: Some("saved".to_string()),
                    is_active: false,
                }),
                0.0,
            ),
            output_with_remaining(
                "Codex",
                Some(UsageAccount {
                    id: "acct_active".to_string(),
                    label: Some("active".to_string()),
                    is_active: true,
                }),
                0.0,
            ),
        ];

        let ordered_indices = attention_outputs(&outputs)
            .into_iter()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        assert_eq!(ordered_indices, vec![2, 1, 0]);
    }

    #[test]
    fn usage_attention_shows_more_when_clipped() {
        let mut app = make_app();
        app.subscription_usage = vec![
            output_with_remaining(
                "Codex",
                Some(UsageAccount {
                    id: "acct_active".to_string(),
                    label: Some("active".to_string()),
                    is_active: true,
                }),
                0.0,
            ),
            output_with_remaining(
                "Codex",
                Some(UsageAccount {
                    id: "acct_saved".to_string(),
                    label: Some("saved".to_string()),
                    is_active: false,
                }),
                0.0,
            ),
            output_with_remaining("Copilot", None, 0.0),
        ];

        let body = render_body(&mut app, 210, 34);

        assert!(body.contains("3 critical"), "{body}");
        assert!(body.contains("Attention"), "{body}");
        assert!(body.contains("active"), "{body}");
        assert!(body.contains("saved"), "{body}");
        assert!(body.contains("+1 more at risk"), "{body}");
    }

    #[test]
    fn usage_email_privacy_hides_email_addresses() {
        let mut app = make_app();
        app.hide_usage_emails = true;
        app.subscription_usage = vec![UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: "acct_secret_123456789".to_string(),
                label: None,
                is_active: true,
            }),
            plan: Some("Pro".to_string()),
            email: Some("secret@example.com".to_string()),
            metrics: vec![UsageMetric {
                label: "Session".to_string(),
                used_percent: 5.0,
                remaining_percent: 95.0,
                remaining_label: Some("95% left".to_string()),
                resets_at: None,
            }],
        }];

        let body = render_body(&mut app, 170, 28);

        assert!(!body.contains("secret@example.com"), "{body}");
        assert!(body.contains("[hidden email]"), "{body}");
        assert!(body.contains("Account acct_s...6789"), "{body}");
    }

    #[test]
    fn login_panel_renders_recent_output_and_dismiss() {
        let mut app = make_app();
        app.codex_login_lines = vec!["open browser".to_string()];
        app.codex_login_outcome = Some(CodexLoginOutcome::Failed("expired".to_string()));

        let body = render_body(&mut app, 90, 14);

        assert!(body.contains("Codex Login"), "{body}");
        assert!(body.contains("open browser"), "{body}");
        assert!(body.contains("[Dismiss]"), "{body}");
    }
}
