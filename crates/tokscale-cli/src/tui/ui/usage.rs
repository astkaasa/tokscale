use ratatui::layout::Flex;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, TableState};

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};

use crate::commands::usage::{
    helpers, UsageFetchDiagnostic, UsageFetchDiagnosticSeverity, UsageMetric, UsageOutput,
};
use crate::tui::app::{App, ClickAction};
use crate::tui::codex_login::CodexLoginOutcome;
use crate::tui::privacy::looks_like_email;
use crate::tui::ui::widgets::{
    format_tokens, get_provider_shade, light_ratio_bar_spans, table_right_cell, table_text_cell,
    truncate_ellipsis as truncate_string,
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
    Warning,
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

struct UsageRowView<'a> {
    account: String,
    account_summary: String,
    plan: String,
    limit: String,
    reset: String,
    readiness: UsageReadiness,
    metric: Option<&'a UsageMetric>,
}

const WIDE_ACCOUNT_ACTIVITY_MIN_CONTENT_WIDTH: u16 = 120;
const WIDE_ACCOUNT_ACTIVITY_MIN_HEIGHT: u16 = 28;
const ACCOUNT_ACTIVITY_HEATMAP_WEEKS: i64 = 52;
const ACCOUNT_ACTIVITY_HEATMAP_CELL_WIDTH: usize = 2;
const ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP: usize = 1;
const ACCOUNT_ACTIVITY_HEATMAP_GLYPH: char = '▆';

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
        if app.usage_fetch_diagnostics.is_empty() {
            "No data".to_string()
        } else {
            usage_issue_count_label(app.usage_fetch_diagnostics.len())
        }
    } else if inventory.providers == 0 {
        "Not loaded".to_string()
    } else if app.usage_fetch_diagnostics.is_empty() {
        format!(
            "{} providers · {}",
            inventory.providers,
            identity_count_label(inventory.saved, inventory.managed)
        )
    } else {
        format!(
            "{} providers · {} · {}",
            inventory.providers,
            identity_count_label(inventory.saved, inventory.managed),
            usage_issue_count_label(app.usage_fetch_diagnostics.len())
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

fn usage_issue_count_label(count: usize) -> String {
    match count {
        1 => "1 issue".to_string(),
        count => format!("{count} issues"),
    }
}

fn render_action_bar(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    if area.height == 0 {
        return area;
    }
    let compact = area.width < 48;
    let show_prefix = area.width >= 36;

    let refresh_label = if app.is_fetching_usage() {
        if compact { "r Sync" } else { "r Syncing" }.to_string()
    } else {
        "r Refresh".to_string()
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
    if let Some(button) = selected_reset_action_button(app) {
        buttons.push(button);
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

fn selected_reset_action_button(app: &App) -> Option<ButtonSpec> {
    let output = app.subscription_usage.get(app.selected_index)?;
    if !has_available_reset_credit(output) {
        return None;
    }

    let account_id = output.account.as_ref()?.id.clone();
    Some(ButtonSpec {
        label: "x Reset".to_string(),
        kind: ButtonKind::Warning,
        action: ClickAction::CodexResetAccount { account_id },
    })
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
        return app.theme.active_control_style();
    }

    match kind {
        ButtonKind::Primary => Style::default()
            .fg(app.theme.background)
            .bg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
        ButtonKind::Secondary => Style::default().fg(app.theme.accent).bg(app.theme.border),
        ButtonKind::Warning => Style::default()
            .fg(app.theme.background)
            .bg(app.theme.warning_color()),
        ButtonKind::Danger => app.theme.danger_style().bg(app.theme.border),
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
    if app.is_codex_login_running() || app.codex_login_outcome.is_some() {
        let action_label = if app.is_codex_login_running() {
            "[Cancel]"
        } else {
            "[Dismiss]"
        };
        let action_width = action_label.chars().count() as u16;
        let used_width = Line::from(header_spans.clone()).width();
        let padding = (area.width as usize).saturating_sub(used_width + action_width as usize);
        header_spans.push(Span::raw(" ".repeat(padding)));
        header_spans.push(Span::styled(
            action_label,
            Style::default().fg(app.theme.accent),
        ));
        let x = area
            .x
            .saturating_add(area.width.saturating_sub(action_width));
        app.add_click_area(
            Rect::new(x, area.y, action_width.min(area.width), 1),
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
            CodexLoginOutcome::Failed(error) => (format!("  {error}"), app.theme.danger_style()),
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
    let center = centered_rect(area, 4);
    let lines = if let Some(diagnostic) = app.usage_fetch_diagnostics.first() {
        if area.width < 40 {
            vec![
                Line::from(Span::styled(
                    "Usage fetch failed",
                    Style::default().fg(app.theme.muted),
                )),
                Line::from(Span::styled(
                    truncate_string(&diagnostic.display_name(), area.width as usize),
                    Style::default().fg(diagnostic_severity_color(app, diagnostic.severity)),
                )),
            ]
        } else {
            vec![
                Line::from(Span::styled(
                    "Usage fetch failed",
                    Style::default()
                        .fg(app.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    truncate_string(&diagnostic.display_name(), area.width as usize),
                    Style::default().fg(diagnostic_severity_color(app, diagnostic.severity)),
                )),
                Line::from(Span::styled(
                    truncate_string(&diagnostic.message, area.width as usize),
                    Style::default().fg(app.theme.muted),
                )),
            ]
        }
    } else if area.width < 40 {
        vec![Line::from(Span::styled(
            "No usage data",
            Style::default().fg(app.theme.muted),
        ))]
    } else {
        vec![Line::from(Span::styled(
            "No subscription data available",
            Style::default().fg(app.theme.muted),
        ))]
    };
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
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

    if area.width < 132 {
        render_medium_loaded(frame, app, area, outputs);
        return;
    }

    let top_height = if area.height >= 36 {
        19
    } else if area.height >= 31 {
        17
    } else {
        (area.height / 2).clamp(9, 13)
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

fn render_medium_loaded(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    let selected_index = app.selected_index;
    let selected = &outputs[selected_index];
    let summary_height = medium_summary_height(outputs, area.height).min(area.height);
    let accounts_height = medium_accounts_table_height(
        app,
        outputs,
        area.width,
        area.height.saturating_sub(summary_height),
    );
    let selected_available = area
        .height
        .saturating_sub(summary_height)
        .saturating_sub(accounts_height);
    let selected_height = medium_selected_account_height(selected, selected_available);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Length(selected_height),
            Constraint::Length(accounts_height),
            Constraint::Min(0),
        ])
        .split(area);

    render_usage_status(frame, app, chunks[0], outputs);
    render_selected_account(frame, app, chunks[1], &outputs[selected_index], outputs);
    render_accounts_table(frame, app, chunks[2], outputs);
}

fn medium_summary_height(outputs: &[UsageOutput], area_height: u16) -> u16 {
    if area_height >= 36 && outputs.iter().any(has_available_reset_credit) {
        11
    } else if area_height >= 32 {
        8
    } else {
        6
    }
}

fn medium_selected_account_height(selected: &UsageOutput, available: u16) -> u16 {
    let preferred = medium_selected_account_preferred_height(selected);
    preferred.min(available).max(available.min(9))
}

fn medium_selected_account_preferred_height(selected: &UsageOutput) -> u16 {
    let status_rows = 3
        + usize::from(credits_status_line(selected).is_some())
        + reset_credit_detail_rows(selected);
    let limit_rows = 2 + selected.metrics.len().min(4);
    let action_rows = 3;
    (status_rows + limit_rows + action_rows + 2).clamp(9, 22) as u16
}

fn medium_accounts_table_height(
    app: &App,
    outputs: &[UsageOutput],
    width: u16,
    available: u16,
) -> u16 {
    if available == 0 {
        return 0;
    }

    let visible_rows = outputs.len().clamp(1, 5) as u16;
    let activity_expanded = outputs
        .iter()
        .any(|output| app.is_usage_account_activity_expanded(output));
    let base_height = 3 + visible_rows.saturating_mul(2);
    let wide_table_height = available.saturating_sub(9);
    if activity_expanded
        && account_activity_content_width(width.saturating_sub(2))
            >= WIDE_ACCOUNT_ACTIVITY_MIN_CONTENT_WIDTH
        && wide_table_height >= base_height.saturating_add(WIDE_ACCOUNT_ACTIVITY_MIN_HEIGHT)
    {
        return wide_table_height;
    }

    let activity_height = if activity_expanded {
        account_activity_preferred_height(100)
    } else {
        0
    };
    let desired = base_height + activity_height;
    desired.min(available).max(available.min(5))
}

fn usage_top_column_percentages(width: u16) -> (u16, u16) {
    if width >= 128 {
        (50, 50)
    } else {
        (48, 52)
    }
}

fn render_compact_loaded(frame: &mut Frame, app: &mut App, area: Rect, outputs: &[UsageOutput]) {
    let selected_index = app.selected_index;
    if compact_should_prioritize_selected(area, outputs) {
        let summary_height = compact_summary_height(outputs, area.height);
        let accounts_height = medium_accounts_table_height(
            app,
            outputs,
            area.width,
            area.height.saturating_sub(summary_height),
        );
        let selected_available = area
            .height
            .saturating_sub(summary_height)
            .saturating_sub(accounts_height);
        let selected_height =
            medium_selected_account_height(&outputs[selected_index], selected_available);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(summary_height),
                Constraint::Length(selected_height),
                Constraint::Length(accounts_height),
                Constraint::Min(0),
            ])
            .split(area);
        render_usage_status(frame, app, chunks[0], outputs);
        render_selected_account(frame, app, chunks[1], &outputs[selected_index], outputs);
        render_accounts_table(frame, app, chunks[2], outputs);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(7.min(area.height))])
        .split(area);
    render_accounts_table(frame, app, chunks[0], outputs);
    if chunks.len() > 1 && chunks[1].height > 0 {
        render_selected_account(frame, app, chunks[1], &outputs[selected_index], outputs);
    }
}

fn compact_should_prioritize_selected(area: Rect, outputs: &[UsageOutput]) -> bool {
    area.height >= 24 && outputs.len() <= 3
}

fn compact_summary_height(outputs: &[UsageOutput], area_height: u16) -> u16 {
    if area_height >= 32 && outputs.iter().any(has_available_reset_credit) {
        9
    } else {
        7
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

    let diagnostic_reserve = diagnostic_line_reserve(app, inner.height as usize);
    let summary_height = (inner.height as usize).saturating_sub(diagnostic_reserve);
    let mut lines = usage_status_summary_lines(app, outputs, inner.width as usize, summary_height);
    append_usage_diagnostic_lines(&mut lines, app, inner.width as usize, inner.height as usize);

    push_section_spacing(&mut lines, inner.height as usize);
    append_credit_bank_summary_lines(
        &mut lines,
        app,
        outputs,
        inner.width as usize,
        inner.height as usize,
    );

    let attention_outputs = attention_outputs(outputs);
    push_section_spacing(&mut lines, inner.height as usize);
    if lines.len() + 1 < inner.height as usize {
        lines.push(section_heading("Attention", app));
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
                lines.push(attention_more_line(app, hidden_count, inner.width as usize));
            }
        }
    }

    push_section_spacing(&mut lines, inner.height as usize);
    append_provider_summary_lines(
        &mut lines,
        app,
        outputs,
        inner.width as usize,
        inner.height as usize,
    );

    frame.render_widget(Paragraph::new(lines), inner);
}

fn usage_status_summary_lines(
    app: &App,
    outputs: &[UsageOutput],
    width: usize,
    height: usize,
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
    let push_state = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "State",
            overall_state_label(outputs),
            Style::default()
                .fg(readiness_color(app, overall))
                .add_modifier(Modifier::BOLD),
            width,
        );
    };
    let push_active = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "Active",
            &active,
            app.theme.success_style().add_modifier(Modifier::BOLD),
            width,
        );
    };
    let push_capacity = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "Capacity",
            &capacity,
            app.theme.secondary_text_style(),
            width,
        );
    };
    let push_fallback = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "Fallback",
            &fallback,
            app.theme.secondary_text_style(),
            width,
        );
    };
    let push_next_reset = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "Next Reset",
            &next_reset,
            app.theme.secondary_text_style(),
            width,
        );
    };
    let push_action = |lines: &mut Vec<Line<'static>>| {
        push_kv_styled(
            lines,
            app,
            "Action",
            &action,
            Style::default()
                .fg(readiness_color(app, overall))
                .add_modifier(Modifier::BOLD),
            width,
        );
    };

    match height {
        0 => {}
        1 => push_state(&mut lines),
        2 => {
            push_state(&mut lines);
            push_action(&mut lines);
        }
        3 => {
            push_state(&mut lines);
            push_active(&mut lines);
            push_action(&mut lines);
        }
        4 => {
            push_state(&mut lines);
            push_active(&mut lines);
            push_capacity(&mut lines);
            push_action(&mut lines);
        }
        5 => {
            push_state(&mut lines);
            push_active(&mut lines);
            push_capacity(&mut lines);
            push_next_reset(&mut lines);
            push_action(&mut lines);
        }
        _ => {
            push_state(&mut lines);
            push_active(&mut lines);
            push_capacity(&mut lines);
            push_fallback(&mut lines);
            push_next_reset(&mut lines);
            push_action(&mut lines);
        }
    }
    lines
}

fn diagnostic_line_reserve(app: &App, max_lines: usize) -> usize {
    if app.usage_fetch_diagnostics.is_empty() {
        0
    } else if max_lines >= 7 {
        3
    } else {
        2.min(max_lines)
    }
}

fn append_usage_diagnostic_lines(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    width: usize,
    max_lines: usize,
) {
    if app.usage_fetch_diagnostics.is_empty() || lines.len() >= max_lines {
        return;
    }

    let remaining = max_lines.saturating_sub(lines.len());
    if remaining < 2 {
        return;
    }
    if !lines.is_empty() && remaining >= 3 {
        lines.push(Line::from(""));
    }
    if max_lines.saturating_sub(lines.len()) < 2 {
        return;
    }

    lines.push(section_heading("Diagnostics", app));
    let available = max_lines.saturating_sub(lines.len());
    if available == 0 {
        return;
    }

    let visible_count = if app.usage_fetch_diagnostics.len() > available && available > 1 {
        available - 1
    } else {
        app.usage_fetch_diagnostics.len().min(available)
    };
    for diagnostic in app.usage_fetch_diagnostics.iter().take(visible_count) {
        lines.push(usage_diagnostic_line(app, diagnostic, width));
    }

    let hidden_count = app
        .usage_fetch_diagnostics
        .len()
        .saturating_sub(visible_count);
    if hidden_count > 0 && lines.len() < max_lines {
        lines.push(Line::from(Span::styled(
            truncate_string(
                &format!(
                    "  +{} more issue{}",
                    hidden_count,
                    if hidden_count == 1 { "" } else { "s" }
                ),
                width,
            ),
            app.theme.subtle_text_style(),
        )));
    }
}

fn usage_diagnostic_line(
    app: &App,
    diagnostic: &UsageFetchDiagnostic,
    width: usize,
) -> Line<'static> {
    let label = diagnostic.display_name();
    let text = format!("  {label}: {}", diagnostic.message);
    Line::from(Span::styled(
        truncate_string(&text, width),
        Style::default().fg(diagnostic_severity_color(app, diagnostic.severity)),
    ))
}

fn diagnostic_severity_color(app: &App, severity: UsageFetchDiagnosticSeverity) -> Color {
    match severity {
        UsageFetchDiagnosticSeverity::Info => app.theme.info_color(),
        UsageFetchDiagnosticSeverity::Warning => app.theme.warning_color(),
        UsageFetchDiagnosticSeverity::Error => app.theme.danger_color(),
    }
}

fn push_section_spacing(lines: &mut Vec<Line<'static>>, max_lines: usize) {
    if !lines.is_empty() && lines.len().saturating_add(1) < max_lines {
        lines.push(Line::from(""));
    }
}

fn append_provider_summary_lines(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    outputs: &[UsageOutput],
    width: usize,
    max_lines: usize,
) {
    if lines.len().saturating_add(1) >= max_lines {
        return;
    }

    lines.push(section_heading("Providers", app));

    for group in group_outputs_by_provider(outputs) {
        if lines.len() >= max_lines {
            break;
        }
        lines.push(provider_summary_line(app, &group, width));
    }
}

fn section_heading(label: &'static str, app: &App) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {label}"),
        section_heading_style(app),
    ))
}

fn section_heading_style(app: &App) -> Style {
    app.theme
        .secondary_text_style()
        .add_modifier(Modifier::BOLD)
}

fn push_kv_styled(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    key: &'static str,
    value: &str,
    value_style: Style,
    width: usize,
) {
    let max_value = width.saturating_sub(16);
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<12}", key), app.theme.subtle_text_style()),
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
        attention_severity_rank(left)
            .cmp(&attention_severity_rank(right))
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
            format!(
                "{} {}{}",
                compact_metric_label(&metric.label),
                remaining_label(metric),
                reset
            )
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

fn attention_more_line(app: &App, hidden_count: usize, width: usize) -> Line<'static> {
    let label = if hidden_count == 1 {
        "+1 more at risk".to_string()
    } else {
        format!("+{hidden_count} more at risk")
    };
    Line::from(Span::styled(
        format!("  {}", truncate_string(&label, width.saturating_sub(2))),
        app.theme.warning_style().add_modifier(Modifier::BOLD),
    ))
}

fn provider_summary_line(
    app: &App,
    group: &UsageProviderGroup<'_>,
    _width: usize,
) -> Line<'static> {
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
    let summary = if risk > 0 {
        format!("{count_label} · {ready} ready · {risk} at risk")
    } else {
        format!("{count_label} · {ready} ready")
    };
    Line::from(vec![
        Span::styled(
            format!("  {}", truncate_string(group.provider, 18)),
            Style::default()
                .fg(app.theme.color(get_provider_shade(group.provider, 0)))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {summary}"), app.theme.subtle_text_style()),
    ])
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
                    "saved store, current Codex login"
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
        if let Some(label) = credits_status_line(selected) {
            push_kv_styled(
                &mut lines,
                app,
                "Credits",
                &label,
                app.theme.secondary_text_style(),
                inner.width as usize,
            );
        }
    }
    append_selected_reset_credit_lines(
        &mut lines,
        app,
        selected,
        inner.width as usize,
        detail_limit,
    );
    push_section_spacing(&mut lines, detail_limit);
    if lines.len() < detail_limit {
        lines.push(section_heading("Limits", app));
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
    push_section_spacing(&mut lines, max_lines.saturating_sub(1));
    if lines.len() + 1 < max_lines {
        lines.push(section_heading("Actions", app));
    }
    if lines.len() < max_lines {
        let y = inner.y.saturating_add(lines.len() as u16);
        lines.push(selected_account_actions_line(app, selected, inner, y));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn append_selected_reset_credit_lines(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    selected: &UsageOutput,
    width: usize,
    max_lines: usize,
) {
    if lines.len() >= max_lines {
        return;
    }

    let Some(credits) = selected.reset_credits.as_ref() else {
        return;
    };

    let count_label = reset_credit_count_label(credits.available_count);
    let value_style = if has_available_reset_credit(selected) {
        app.theme.warning_style().add_modifier(Modifier::BOLD)
    } else {
        app.theme.secondary_text_style()
    };
    push_kv_styled(lines, app, "Reset Bank", &count_label, value_style, width);

    if lines.len() >= max_lines {
        return;
    }

    let buckets = reset_credit_buckets(credits);
    if buckets.is_empty() {
        if credits.available_count > 0 {
            lines.push(selected_reset_schedule_line(
                "expiry unknown",
                app.theme.subtle_text_style(),
                width,
            ));
        }
        return;
    }

    let available = max_lines.saturating_sub(lines.len());
    let visible_count = if buckets.len() > available {
        available.saturating_sub(1)
    } else {
        buckets.len()
    };

    for bucket in buckets.iter().take(visible_count) {
        lines.push(selected_reset_schedule_line(
            &format_selected_reset_schedule_entry(bucket),
            app.theme.secondary_text_style(),
            width,
        ));
    }

    let hidden = hidden_expiry_count(&buckets[visible_count..]);
    if hidden > 0 && lines.len() < max_lines {
        lines.push(selected_reset_schedule_line(
            &format!("+{hidden} more reset credits"),
            app.theme.subtle_text_style(),
            width,
        ));
    }
}

fn selected_reset_schedule_line(value: &str, value_style: Style, width: usize) -> Line<'static> {
    let prefix_width = 14usize;
    Line::from(vec![
        Span::raw(" ".repeat(prefix_width)),
        Span::styled(
            truncate_string(value, width.saturating_sub(prefix_width + 2)),
            value_style,
        ),
    ])
}

fn format_selected_reset_schedule_entry(bucket: &(String, usize)) -> String {
    if bucket.1 > 1 {
        format!("x{} expires {}", bucket.1, bucket.0)
    } else {
        format!("expires {}", bucket.0)
    }
}

fn reset_credit_detail_rows(selected: &UsageOutput) -> usize {
    let Some(credits) = selected.reset_credits.as_ref() else {
        return 0;
    };

    if credits.available_count == 0 {
        return 1;
    }

    let bucket_count = reset_credit_buckets(credits).len();

    1 + bucket_count.max(1)
}

fn append_credit_bank_summary_lines(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    outputs: &[UsageOutput],
    width: usize,
    max_lines: usize,
) {
    if !outputs.iter().any(has_available_reset_credit) || lines.len() >= max_lines {
        return;
    }

    lines.push(Line::from(vec![
        Span::styled("  Credit Bank  ", section_heading_style(app)),
        Span::styled(
            truncate_string(
                &reset_bank_summary(outputs, width.saturating_sub(15)),
                width.saturating_sub(15),
            ),
            app.theme.warning_style().add_modifier(Modifier::BOLD),
        ),
    ]));

    for output in outputs.iter().filter(|output| output.provider == "Codex") {
        if lines.len() >= max_lines {
            break;
        }
        let Some(credits) = output.reset_credits.as_ref() else {
            continue;
        };
        if credits.available_count == 0 {
            continue;
        }
        lines.push(reset_credit_account_line(
            app,
            output,
            credits,
            output
                .account
                .as_ref()
                .is_some_and(|account| account.is_active),
            width,
        ));
    }
}

fn reset_credit_account_line(
    app: &App,
    output: &UsageOutput,
    credits: &crate::commands::usage::UsageResetCredits,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let account = account_name(app, output);
    let count = if credits.available_count == 1 {
        "1 credit".to_string()
    } else {
        format!("{} credits", credits.available_count)
    };
    let label_width: usize = if width >= 72 { 28 } else { 18 };
    let count_width: usize = if width >= 72 { 12 } else { 10 };
    let used = 2 + label_width + count_width + 2;
    let expiry = credit_nearest_expiry_line(credits);
    let marker = if selected { "> " } else { "  " };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(
            format!(
                "{:<width$}",
                truncate_string(&account, label_width.saturating_sub(1)),
                width = label_width
            ),
            app.theme.warning_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<width$}", count, width = count_width),
            app.theme.secondary_text_style(),
        ),
        Span::styled(
            truncate_string(&expiry, width.saturating_sub(used)),
            app.theme.secondary_text_style(),
        ),
    ])
}

fn reset_credit_buckets(
    credits: &crate::commands::usage::UsageResetCredits,
) -> Vec<(String, usize)> {
    credit_expiry_buckets(
        credits
            .credits
            .iter()
            .filter_map(|credit| credit.expires_at.as_deref()),
    )
}

fn credit_nearest_expiry_line(credits: &crate::commands::usage::UsageResetCredits) -> String {
    nearest_credit_expiry_label(&reset_credit_buckets(credits))
        .unwrap_or_else(|| "expiry unknown".to_string())
}

fn nearest_credit_expiry_label(buckets: &[(String, usize)]) -> Option<String> {
    buckets
        .first()
        .map(|bucket| format!("nearest expires {}", bucket.0))
}

fn credit_expiry_buckets<'a>(expiries: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut values: Vec<String> = expiries.map(str::to_string).collect();
    values.sort();

    let mut buckets = Vec::new();
    let mut current: Option<String> = None;
    let mut count = 0usize;
    for value in values {
        if current.as_deref() == Some(value.as_str()) {
            count += 1;
            continue;
        }
        if let Some(previous) = current.replace(value) {
            buckets.push((format_credit_expiry_label(&previous), count));
        }
        count = 1;
    }
    if let Some(previous) = current {
        buckets.push((format_credit_expiry_label(&previous), count));
    }

    buckets
}

fn hidden_expiry_count(buckets: &[(String, usize)]) -> usize {
    buckets.iter().map(|(_, count)| *count).sum()
}

fn format_credit_expiry_label(value: &str) -> String {
    let label = format_expiry_time(value);
    label
        .strip_prefix("expires ")
        .or_else(|| label.strip_prefix("resets "))
        .unwrap_or(&label)
        .to_string()
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
        let mut buttons = Vec::new();
        let compact = area.width < 48;
        if has_available_reset_credit(selected) {
            buttons.push(reset_account_button(&account.id, compact));
        }
        if account.is_active {
            spans.push(Span::styled(
                if compact {
                    "  Current  "
                } else {
                    "  Current account  "
                },
                app.theme.subtle_text_style(),
            ));
            let x = area
                .x
                .saturating_add(Line::from(spans.clone()).width() as u16);
            push_click_buttons(&mut spans, app, buttons, x, y, area.right());
        } else {
            spans.push(Span::raw("  "));
            let x = area.x.saturating_add(2);
            let mut all_buttons = vec![use_account_button(&account.id, compact)];
            all_buttons.extend(buttons);
            all_buttons.push(remove_account_button(&account.id, compact));
            push_click_buttons(&mut spans, app, all_buttons, x, y, area.right());
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
        "  Snapshot  {ready} ready · {at_risk} at risk · {}{}",
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
    let label_width = metric_label_width(width);
    let target_reset_width = width.saturating_sub(label_width + 12).min(24);
    let bar_width = width
        .saturating_sub(label_width + target_reset_width + 14)
        .clamp(10, 34);
    let reset_width = width.saturating_sub(label_width + bar_width + 14);
    let reset = metric
        .resets_at
        .as_ref()
        .map(|r| helpers::format_reset_time(r))
        .unwrap_or_default();
    let color = metric_color(app, metric);
    let mut spans = vec![Span::styled(
        format!(
            "  {:<width$}",
            truncate_string(
                &compact_metric_label(&metric.label),
                label_width.saturating_sub(1),
            ),
            width = label_width
        ),
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
        Span::styled(
            truncate_string(&reset, reset_width),
            app.theme.subtle_text_style(),
        ),
    ]);
    Line::from(spans)
}

fn metric_label_width(width: usize) -> usize {
    if width >= 96 {
        18
    } else if width >= 78 {
        14
    } else {
        10
    }
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

    if inner.width < 132 {
        render_narrow_accounts_table(frame, app, inner, outputs);
        return;
    }

    let widths = account_table_widths(inner.width);
    let header = Table::new(Vec::<Row<'static>>::new(), widths)
        .header(account_table_header(app))
        .column_spacing(1)
        .flex(Flex::Start);
    frame.render_widget(header, Rect::new(inner.x, inner.y, inner.width, 1));

    let start = app.scroll_offset.min(outputs.len().saturating_sub(1));
    let mut y = inner.y.saturating_add(1);
    let mut rendered_items = 0usize;
    for (index, output) in outputs.iter().enumerate().skip(start) {
        if y >= inner.bottom() {
            break;
        }
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        render_account_row_table(frame, app, row_area, output, index, widths);
        app.add_click_area(row_area, ClickAction::UsageSelect { index });
        if usage_account_activity_expandable(output) {
            app.add_click_area(
                Rect::new(row_area.x, row_area.y, 3.min(row_area.width), 1),
                ClickAction::UsageToggleActivity { index },
            );
        }
        y = y.saturating_add(1);
        rendered_items += 1;

        if app.is_usage_account_activity_expanded(output) {
            let remaining = inner.bottom().saturating_sub(y);
            let trailing_accounts = outputs.len().saturating_sub(index + 1) as u16;
            let activity_height =
                account_activity_height(inner.width, remaining, trailing_accounts);
            if activity_height >= 3 {
                let activity_area = Rect::new(inner.x, y, inner.width, activity_height);
                render_account_activity(frame, app, activity_area, output);
                y = y.saturating_add(activity_height);
            }
        }
    }
    app.set_max_visible_items(rendered_items.max(1));
}

fn render_account_row_table(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    output: &UsageOutput,
    index: usize,
    widths: [Constraint; 8],
) {
    let mut state = TableState::default().with_selected((app.selected_index == index).then_some(0));
    let table = Table::new([account_table_row(app, output, index)], widths)
        .column_spacing(1)
        .highlight_spacing(HighlightSpacing::Never)
        .row_highlight_style(Style::default().bg(app.theme.selection))
        .flex(Flex::Start);
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_narrow_accounts_table(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    outputs: &[UsageOutput],
) {
    frame.render_widget(
        Paragraph::new(narrow_table_header(app, area.width)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let start = app.scroll_offset.min(outputs.len().saturating_sub(1));
    let mut y = area.y.saturating_add(1);
    let mut rendered_items = 0usize;
    for (index, output) in outputs.iter().enumerate().skip(start) {
        let remaining = area.bottom().saturating_sub(y);
        if remaining < 2 {
            break;
        }
        let row_area = Rect::new(area.x, y, area.width, 2);
        let mut table_state =
            TableState::default().with_selected((app.selected_index == index).then_some(0));
        let table = Table::new(
            [narrow_table_row(app, output, index, row_area)],
            [Constraint::Percentage(100)],
        )
        .highlight_spacing(HighlightSpacing::Never)
        .row_highlight_style(Style::default().bg(app.theme.selection))
        .flex(Flex::Start);
        frame.render_stateful_widget(table, row_area, &mut table_state);
        app.add_click_area(row_area, ClickAction::UsageSelect { index });
        if usage_account_activity_expandable(output) {
            app.add_click_area(
                Rect::new(row_area.x, row_area.y, 4.min(row_area.width), 1),
                ClickAction::UsageToggleActivity { index },
            );
        }
        y = y.saturating_add(2);
        rendered_items += 1;

        if app.is_usage_account_activity_expanded(output) {
            let remaining = area.bottom().saturating_sub(y);
            let trailing_height =
                (outputs.len().saturating_sub(index + 1) as u16).saturating_mul(2);
            let activity_height = account_activity_height(area.width, remaining, trailing_height);
            if activity_height >= 3 {
                let activity_area = Rect::new(area.x, y, area.width, activity_height);
                render_account_activity(frame, app, activity_area, output);
                y = y.saturating_add(activity_height);
            }
        }
    }
    app.set_max_visible_items(rendered_items.max(1));
}

fn account_table_header(app: &App) -> Row<'static> {
    let style = app.theme.subtle_text_style();
    Row::new([
        table_right_cell("", style),
        table_text_cell("Provider", style),
        table_text_cell("Account", style),
        table_text_cell("Plan", style),
        table_text_cell("Auth", style),
        table_text_cell("Health", style),
        table_text_cell("Limit", style),
        table_text_cell("Reset", style),
    ])
}

fn narrow_table_header(app: &App, width: u16) -> Line<'static> {
    Line::from(Span::styled(
        truncate_string("    Account / Status", width as usize),
        app.theme.subtle_text_style(),
    ))
}

fn account_table_widths(width: u16) -> [Constraint; 8] {
    if width >= 170 {
        [
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(30),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(30),
            Constraint::Length(24),
        ]
    } else {
        [
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(24),
            Constraint::Length(24),
        ]
    }
}

fn narrow_table_row(app: &mut App, output: &UsageOutput, index: usize, area: Rect) -> Row<'static> {
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
    let state_right_padding = usize::from(state_width > 0) * 2;
    let left_width = width.saturating_sub(4 + state_width + state_right_padding);
    let left = format!("{} {}", output.provider, row.account_summary);
    let disclosure = account_activity_disclosure(app, output);
    let mut first = vec![
        styled(
            app,
            format!(" {disclosure}  "),
            app.theme.secondary_text_style(),
            selected,
        ),
        styled(
            app,
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
            app,
            format!(
                "{:>width$}",
                truncate_string(&state, state_width),
                width = state_width
            ),
            Style::default().fg(readiness_color(app, row.readiness)),
            selected,
        ));
        first.push(styled(
            app,
            " ".repeat(state_right_padding),
            Style::default(),
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
        styled(app, "    ", Style::default(), selected),
        styled(
            app,
            truncate_string(&detail, detail_width.saturating_sub(1)),
            detail_style,
            selected,
        ),
    ];
    let used = Line::from(second.clone()).width();
    if action_width > 0 && area.width as usize > used + action_width {
        second.push(styled(app, " ", Style::default(), selected));
    }
    if let Some(label) = managed_label {
        second.push(styled(app, label, app.theme.subtle_text_style(), selected));
    }
    pad_selected_row(app, &mut second, area.width as usize, selected);

    Row::new([Cell::from(Text::from(vec![
        Line::from(first),
        Line::from(second),
    ]))])
    .style(account_table_row_style(app, index))
    .height(2)
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
    let limit = metric_summary(output);
    let reset = metric.and_then(display_metric_reset).unwrap_or_default();
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

fn metric_summary(output: &UsageOutput) -> String {
    if output.metrics.is_empty() {
        return "No limits".to_string();
    }

    let parts = output
        .metrics
        .iter()
        .map(|metric| {
            let label = compact_metric_label(&metric.label);
            let label = truncate_string(&label, 14);
            format!("{label} {:.0}%", metric.remaining_percent)
        })
        .collect::<Vec<_>>();
    parts.join(" · ")
}

fn compact_metric_label(label: &str) -> String {
    let mut value = label.trim().to_string();
    for prefix in [
        "GPT-5.3-Codex-",
        "GPT-5.3-",
        "Codex-",
        "codex-",
        "gpt-5.3-codex-",
        "gpt-5.3-",
    ] {
        if let Some(stripped) = value.strip_prefix(prefix) {
            value = stripped.to_string();
            break;
        }
    }

    value = value
        .replace("Codex Spark", "Spark")
        .replace("Codex-Spark", "Spark")
        .replace("codex-spark", "Spark");

    if value.eq_ignore_ascii_case("session") {
        return "5h".to_string();
    }
    if value.eq_ignore_ascii_case("spark") {
        return "Spark".to_string();
    }
    if value.eq_ignore_ascii_case("spark week") {
        return "Spark weekly".to_string();
    }
    value
}

fn display_metric_reset(metric: &UsageMetric) -> Option<String> {
    metric
        .resets_at
        .as_ref()
        .map(|reset| helpers::format_reset_time(reset))
}

fn account_table_row(app: &App, output: &UsageOutput, index: usize) -> Row<'static> {
    let row = usage_row_view(app, output);

    let auth = account_auth_label(output);
    let health = readiness_label(row.readiness);
    let auth_color = account_auth_color(app, output);
    let health_color = readiness_color(app, row.readiness);
    let metric_color = row.metric.map(|metric| metric_color(app, metric));

    Row::new([
        table_right_cell(
            account_activity_disclosure(app, output),
            app.theme.secondary_text_style(),
        ),
        table_text_cell(
            output.provider.clone(),
            Style::default()
                .fg(app.theme.color(get_provider_shade(&output.provider, 0)))
                .add_modifier(Modifier::BOLD),
        ),
        table_text_cell(row.account, app.theme.secondary_text_style()),
        table_text_cell(row.plan, app.theme.secondary_text_style()),
        table_text_cell(auth, Style::default().fg(auth_color)),
        table_text_cell(health, Style::default().fg(health_color)),
        table_text_cell(
            row.limit,
            metric_color
                .map(|color| Style::default().fg(color))
                .unwrap_or_else(|| app.theme.secondary_text_style()),
        ),
        table_text_cell(row.reset, app.theme.subtle_text_style()),
    ])
    .style(account_table_row_style(app, index))
    .height(1)
}

fn usage_account_activity_expandable(output: &UsageOutput) -> bool {
    output.provider.eq_ignore_ascii_case("Codex") && output.account.is_some()
}

fn account_activity_disclosure(app: &App, output: &UsageOutput) -> &'static str {
    if !usage_account_activity_expandable(output) {
        ""
    } else if app.is_usage_account_activity_expanded(output) {
        "▾"
    } else {
        "▸"
    }
}

fn account_activity_preferred_height(width: u16) -> u16 {
    if width >= 66 {
        13
    } else {
        15
    }
}

fn account_activity_card_inset(width: u16) -> u16 {
    if width >= 8 {
        2
    } else {
        0
    }
}

fn account_activity_content_width(width: u16) -> u16 {
    width
        .saturating_sub(account_activity_card_inset(width))
        .saturating_sub(2)
}

fn account_activity_height(width: u16, remaining: u16, trailing_height: u16) -> u16 {
    if account_activity_content_width(width) >= WIDE_ACCOUNT_ACTIVITY_MIN_CONTENT_WIDTH {
        let available = remaining.saturating_sub(trailing_height.min(remaining));
        if available >= WIDE_ACCOUNT_ACTIVITY_MIN_HEIGHT {
            return available;
        }
    }
    account_activity_preferred_height(width).min(remaining)
}

fn render_account_activity(frame: &mut Frame, app: &App, area: Rect, output: &UsageOutput) {
    let inset = account_activity_card_inset(area.width);
    let card = Rect::new(
        area.x.saturating_add(inset),
        area.y,
        area.width.saturating_sub(inset),
        area.height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Account Activity ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let activity = output
        .account
        .as_ref()
        .and_then(|account| app.account_activities.get(&account.id));
    let Some(activity) = activity else {
        if let Some(message) = app.account_activity_error.as_deref() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        format!("  {message}"),
                        app.theme.warning_style(),
                    )),
                    Line::from(Span::styled(
                        "  Check access to Tokscale's local data directory.",
                        app.theme.subtle_text_style(),
                    )),
                ]),
                inner,
            );
            return;
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "  No account activity samples yet",
                    app.theme.secondary_text_style(),
                )),
                Line::from(Span::styled(
                    "  Refresh Usage to start local history for this account.",
                    app.theme.subtle_text_style(),
                )),
            ]),
            inner,
        );
        return;
    };

    if inner.width >= WIDE_ACCOUNT_ACTIVITY_MIN_CONTENT_WIDTH
        && inner.height >= WIDE_ACCOUNT_ACTIVITY_MIN_HEIGHT
    {
        render_wide_account_activity(frame, app, inner, activity);
        return;
    }

    render_compact_account_activity(frame, app, inner, activity);
}

fn render_compact_account_activity(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let summary_height = activity
        .summary
        .as_ref()
        .map_or(0, |_| if area.width >= 66 { 5 } else { 7 });
    let summary_height = summary_height.min(area.height);
    if summary_height > 0 {
        render_compact_official_summary(
            frame,
            app,
            Rect::new(area.x, area.y, area.width, summary_height),
            activity.summary.as_ref().expect("summary checked above"),
        );
    }
    let details = Rect::new(
        area.x,
        area.y.saturating_add(summary_height),
        area.width,
        area.height.saturating_sub(summary_height),
    );
    if details.height == 0 {
        return;
    }
    let mut lines = account_activity_lines(app, activity, details.width as usize);
    lines.truncate(details.height as usize);
    frame.render_widget(Paragraph::new(lines), details);
}

fn render_compact_official_summary(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    summary: &tokscale_core::telemetry::AccountUsageSummary,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let metrics = official_summary_metrics(summary);
    let columns = if inner.width >= 66 { 3 } else { 2 };
    let row_count = metrics.len().div_ceil(columns);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(2); row_count])
        .split(inner);
    for (row_index, row) in rows.iter().enumerate() {
        let start = row_index * columns;
        let end = (start + columns).min(metrics.len());
        render_official_metric_row(frame, app, *row, &metrics[start..end], columns);
    }
}

fn official_summary_metrics(
    summary: &tokscale_core::telemetry::AccountUsageSummary,
) -> Vec<(String, &'static str)> {
    vec![
        (
            summary
                .lifetime_tokens
                .map(format_activity_tokens)
                .unwrap_or_else(|| "—".to_string()),
            "Lifetime tokens",
        ),
        (
            summary
                .peak_daily_tokens
                .map(format_activity_tokens)
                .unwrap_or_else(|| "—".to_string()),
            "Peak day",
        ),
        (
            summary
                .longest_running_turn_seconds
                .map(format_activity_seconds)
                .unwrap_or_else(|| "—".to_string()),
            "Longest turn",
        ),
        (
            summary
                .current_streak_days
                .map(format_activity_days)
                .unwrap_or_else(|| "—".to_string()),
            "Current streak",
        ),
        (
            summary
                .longest_streak_days
                .map(format_activity_days)
                .unwrap_or_else(|| "—".to_string()),
            "Longest streak",
        ),
    ]
}

fn render_official_metric_row(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    metrics: &[(String, &'static str)],
    columns: usize,
) {
    let constraints = vec![Constraint::Ratio(1, columns as u32); columns];
    let cells = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);
    for (index, (value, label)) in metrics.iter().enumerate() {
        let separator = index + 1 < metrics.len();
        let block = Block::default()
            .borders(if separator {
                Borders::RIGHT
            } else {
                Borders::NONE
            })
            .border_style(Style::default().fg(app.theme.border));
        let inner = block.inner(cells[index]);
        frame.render_widget(block, cells[index]);
        if inner.height == 0 || inner.width == 0 {
            continue;
        }
        let content = Rect::new(
            inner.x.saturating_add(1).min(inner.right()),
            inner.y,
            inner.width.saturating_sub(1),
            inner.height,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    truncate_string(value, content.width as usize),
                    app.theme
                        .secondary_text_style()
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    truncate_string(label, content.width as usize),
                    app.theme.subtle_text_style(),
                )),
            ]),
            content,
        );
    }
}

fn render_wide_account_activity(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(11),
            Constraint::Min(0),
        ])
        .split(area);

    render_wide_official_summary(frame, app, sections[0], activity);
    render_wide_activity_heatmap(frame, app, sections[1], activity);
    render_wide_activity_detail(frame, app, sections[2], activity);
}

fn render_wide_official_summary(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.border));
    let content = block.inner(area);
    frame.render_widget(block, area);
    if content.width == 0 || content.height < 3 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(content);
    let activity_range = official_activity_range(activity)
        .unwrap_or_else(|| "waiting for official daily buckets".to_string());
    frame.render_widget(
        Paragraph::new(edge_aligned_line(
            "OFFICIAL ACCOUNT SUMMARY · CODEX APP-SERVER",
            &activity_range,
            rows[0].width as usize,
            app.theme.subtle_text_style(),
            app.theme.subtle_text_style(),
        )),
        rows[0],
    );

    if let Some(summary) = activity.summary.as_ref() {
        let metrics = official_summary_metrics(summary);
        render_official_metric_row(frame, app, rows[1], &metrics, metrics.len());
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Official account summary unavailable",
                app.theme.subtle_text_style(),
            )))
            .alignment(Alignment::Center),
            rows[1],
        );
    }
}

fn render_wide_activity_heatmap(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.border));
    let content = block.inner(area);
    frame.render_widget(block, area);
    if content.width == 0 || content.height < 10 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(7),
            Constraint::Length(1),
        ])
        .split(content);
    let peak = official_peak_day(activity)
        .map(|(date, tokens)| {
            format!(
                "Peak {} · {}",
                date.format("%b %d"),
                format_activity_tokens(tokens)
            )
        })
        .unwrap_or_else(|| "official buckets".to_string());
    frame.render_widget(
        Paragraph::new(edge_aligned_line(
            "OFFICIAL TOKEN ACTIVITY · LAST 12 MONTHS",
            &peak,
            rows[0].width as usize,
            app.theme.subtle_text_style(),
            app.theme.secondary_text_style(),
        )),
        rows[0],
    );

    let today = app.overview_date();
    let start = activity_heatmap_start(today);
    let returned_range = official_bucket_date_range(activity);
    render_activity_month_labels(frame, app, rows[1], start);
    render_activity_heatmap_rows(frame, app, rows[2], activity, start, today, returned_range);
    let range = returned_range
        .map(|(first, last)| {
            format!(
                "Returned {}–{} · {} active days",
                first.format("%b %d"),
                last.format("%b %d"),
                activity
                    .daily_usage
                    .iter()
                    .filter(|bucket| bucket.tokens > 0)
                    .count()
            )
        })
        .unwrap_or_else(|| "Waiting for official daily buckets".to_string());
    render_activity_heatmap_footer(frame, app, rows[3], &range);
}

fn render_wide_activity_detail(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    render_wide_recent_window(frame, app, columns[0], activity);
    render_wide_local_evidence(frame, app, columns[1], activity);
}

fn render_wide_recent_window(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let content = activity_panel(frame, app, area, true);
    if content.width == 0 || content.height == 0 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(content);
    let today = app.overview_date();
    let total = if activity.daily_usage.is_empty() {
        "—".to_string()
    } else {
        format_activity_tokens(daily_tokens_in_window(activity, today, 7))
    };
    frame.render_widget(
        Paragraph::new(edge_aligned_line(
            &format!("CURRENT 7-DAY WINDOW · {}", seven_day_date_range(today)),
            &total,
            rows[0].width as usize,
            app.theme.subtle_text_style(),
            app.theme.secondary_text_style(),
        )),
        rows[0],
    );
    render_daily_activity_rows(
        frame,
        app,
        Rect::new(rows[1].x, rows[1].y, rows[1].width, rows[1].height.min(7)),
        activity,
        today,
    );
}

fn render_wide_local_evidence(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) {
    let content = activity_panel(frame, app, area, false);
    if content.width == 0 || content.height == 0 {
        return;
    }
    let mut lines = vec![edge_aligned_line(
        "QUOTA & LOCAL EVIDENCE",
        "this machine",
        content.width as usize,
        app.theme.subtle_text_style(),
        app.theme.subtle_text_style(),
    )];
    let Some(window) = primary_activity_window(activity) else {
        lines.push(Line::from(Span::styled(
            "Waiting for a successful quota sample",
            app.theme.subtle_text_style(),
        )));
        frame.render_widget(Paragraph::new(lines), content);
        return;
    };

    let current_used = window
        .points
        .last()
        .map(|point| point.used_percent)
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    lines.push(edge_aligned_line(
        &format!(
            "{} {} used",
            window.limit_label,
            format_activity_percent(current_used)
        ),
        &format!("{} left", format_activity_percent(100.0 - current_used)),
        content.width as usize,
        app.theme
            .secondary_text_style()
            .add_modifier(Modifier::BOLD),
        app.theme.success_style().add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(light_ratio_bar_spans(
        current_used / 100.0,
        content.width as usize,
        Style::default().fg(app.theme.accent),
        app.theme.subtle_text_style(),
    )));
    lines.push(Line::from(Span::styled(
        quota_stability_label(window),
        app.theme
            .secondary_text_style()
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "Coverage {}–{} · {}",
            activity_clock(window.first_observed_at_ms),
            activity_clock(window.last_observed_at_ms),
            activity_duration_label(
                window
                    .last_observed_at_ms
                    .saturating_sub(window.first_observed_at_ms),
                true,
            )
        ),
        app.theme.subtle_text_style(),
    )));
    lines.push(edge_aligned_line(
        &format!("{} samples", window.points.len()),
        &format!(
            "{} max gap",
            activity_duration_label(window.max_gap_ms, false)
        ),
        content.width as usize,
        app.theme.subtle_text_style(),
        app.theme.subtle_text_style(),
    ));
    let (reset_label, reset_style) = primary_reset_status(app, activity, window);
    lines.push(Line::from(Span::styled(reset_label, reset_style)));
    let next_reset = window
        .points
        .last()
        .and_then(|point| point.resets_at_ms)
        .map(activity_reset_label)
        .unwrap_or_else(|| "unknown".to_string());
    lines.push(edge_aligned_line(
        "Next scheduled",
        &next_reset,
        content.width as usize,
        app.theme.subtle_text_style(),
        app.theme.subtle_text_style(),
    ));
    let last_synced = latest_activity_timestamp_ms(activity)
        .map(activity_clock)
        .unwrap_or_else(|| "—".to_string());
    lines.push(edge_aligned_line(
        "Last synced",
        &format!("{last_synced} · local ledger"),
        content.width as usize,
        app.theme.subtle_text_style(),
        app.theme.subtle_text_style(),
    ));
    lines.truncate(content.height as usize);
    frame.render_widget(Paragraph::new(lines), content);
}

fn activity_panel(frame: &mut Frame, app: &App, area: Rect, right_separator: bool) -> Rect {
    let borders = if right_separator {
        Borders::RIGHT
    } else {
        Borders::NONE
    };
    let block = Block::default()
        .borders(borders)
        .border_style(Style::default().fg(app.theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    Rect::new(
        inner.x.saturating_add(1),
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    )
}

fn render_daily_activity_rows(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    today: NaiveDate,
) {
    let bars = daily_activity_bars(activity, today);
    if area.height < bars.len() as u16 || area.width < 32 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                daily_activity_spark(activity, today),
                Style::default().fg(app.theme.accent),
            )))
            .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let maximum = bars.iter().map(|(_, value, _)| *value).max().unwrap_or(1);
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Ratio(1, bars.len() as u32); bars.len()])
        .split(area);
    for ((label, value, text_value), row_area) in bars.into_iter().zip(row_areas.iter()) {
        let y = row_area
            .y
            .saturating_add(row_area.height.saturating_sub(1) / 2);
        let line_area = Rect::new(row_area.x, y, row_area.width, 1);
        let label_width = 11usize.min(line_area.width as usize);
        let value_width = 9usize.min(line_area.width as usize);
        let bar_width = (line_area.width as usize)
            .saturating_sub(label_width + value_width + 2)
            .max(1);
        let ratio = if maximum > 0 {
            value as f64 / maximum as f64
        } else {
            0.0
        };
        let is_peak = value > 0 && value == maximum;
        let mut spans = vec![Span::styled(
            format!("{label:<label_width$}"),
            if is_peak {
                app.theme
                    .secondary_text_style()
                    .add_modifier(Modifier::BOLD)
            } else {
                app.theme.subtle_text_style()
            },
        )];
        spans.extend(light_ratio_bar_spans(
            ratio,
            bar_width,
            Style::default().fg(app.theme.accent),
            app.theme.subtle_text_style(),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{text_value:>value_width$}"),
            if is_peak {
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                app.theme.secondary_text_style()
            },
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), line_area);
    }
}

fn daily_activity_bars(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    today: NaiveDate,
) -> Vec<(String, u64, String)> {
    (0_i64..7)
        .rev()
        .map(|offset| {
            let date = today
                .checked_sub_signed(ChronoDuration::days(offset))
                .unwrap_or(today);
            let value = daily_tokens_on(activity, date);
            (
                date.format("%a %m/%d").to_string(),
                value.unwrap_or(0),
                value
                    .map(format_activity_tokens)
                    .unwrap_or_else(|| "—".to_string()),
            )
        })
        .collect()
}

fn edge_aligned_line(
    left: &str,
    right: &str,
    width: usize,
    left_style: Style,
    right_style: Style,
) -> Line<'static> {
    let right = truncate_string(right, width);
    let right_width = Line::from(right.clone()).width();
    let left_limit = width.saturating_sub(right_width.saturating_add(1));
    let left = truncate_string(left, left_limit);
    let left_width = Line::from(left.clone()).width();
    let padding = width.saturating_sub(left_width.saturating_add(right_width));
    Line::from(vec![
        Span::styled(left, left_style),
        Span::raw(" ".repeat(padding)),
        Span::styled(right, right_style),
    ])
}

fn official_bucket_date_range(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) -> Option<(NaiveDate, NaiveDate)> {
    activity
        .daily_usage
        .iter()
        .filter_map(|bucket| NaiveDate::parse_from_str(&bucket.start_date, "%Y-%m-%d").ok())
        .fold(None, |range, date| match range {
            None => Some((date, date)),
            Some((first, last)) => Some((first.min(date), last.max(date))),
        })
}

fn official_activity_range(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) -> Option<String> {
    let (first, last) = official_bucket_date_range(activity)?;
    let active_days = activity
        .daily_usage
        .iter()
        .filter(|bucket| bucket.tokens > 0)
        .count();
    Some(format!(
        "{active_days} active days · {}–{}",
        first.format("%b %d"),
        last.format("%b %d")
    ))
}

fn official_peak_day(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) -> Option<(NaiveDate, u64)> {
    activity
        .daily_usage
        .iter()
        .filter_map(|bucket| {
            NaiveDate::parse_from_str(&bucket.start_date, "%Y-%m-%d")
                .ok()
                .map(|date| (date, bucket.tokens))
        })
        .max_by_key(|(date, tokens)| (*tokens, *date))
}

fn activity_heatmap_start(today: NaiveDate) -> NaiveDate {
    let current_week_start = today
        .checked_sub_signed(ChronoDuration::days(
            today.weekday().num_days_from_sunday() as i64
        ))
        .unwrap_or(today);
    current_week_start
        .checked_sub_signed(ChronoDuration::weeks(
            ACCOUNT_ACTIVITY_HEATMAP_WEEKS.saturating_sub(1),
        ))
        .unwrap_or(current_week_start)
}

fn activity_heatmap_geometry(area: Rect) -> (u16, usize) {
    let label_width = 4_u16.min(area.width);
    let available = area.width.saturating_sub(label_width);
    let weeks = ACCOUNT_ACTIVITY_HEATMAP_WEEKS as usize;
    let preferred_width = weeks
        .saturating_mul(ACCOUNT_ACTIVITY_HEATMAP_CELL_WIDTH + ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP)
        .saturating_sub(ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP);
    let cell_width = if available as usize >= preferred_width {
        ACCOUNT_ACTIVITY_HEATMAP_CELL_WIDTH
    } else {
        1
    };
    let stride = cell_width + ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP;
    let grid_width = weeks
        .saturating_mul(stride)
        .saturating_sub(ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP) as u16;
    let leading = available.saturating_sub(grid_width) / 2;
    (
        area.x.saturating_add(label_width).saturating_add(leading),
        stride,
    )
}

fn render_activity_month_labels(frame: &mut Frame, app: &App, area: Rect, start: NaiveDate) {
    let (grid_x, stride) = activity_heatmap_geometry(area);
    let mut labels = vec![' '; stride.saturating_mul(ACCOUNT_ACTIVITY_HEATMAP_WEEKS as usize)];
    for week in 0..ACCOUNT_ACTIVITY_HEATMAP_WEEKS {
        for day in 0..7_i64 {
            let date = start
                .checked_add_signed(ChronoDuration::days(week * 7 + day))
                .unwrap_or(start);
            if date.day() != 1 {
                continue;
            }
            let position = week as usize * stride;
            for (offset, character) in date.format("%b").to_string().chars().enumerate() {
                if let Some(slot) = labels.get_mut(position + offset) {
                    *slot = character;
                }
            }
            break;
        }
    }
    let prefix = " ".repeat(grid_x.saturating_sub(area.x) as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{prefix}{}", labels.into_iter().collect::<String>()),
            app.theme.subtle_text_style(),
        ))),
        area,
    );
}

fn render_activity_heatmap_rows(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    start: NaiveDate,
    today: NaiveDate,
    returned_range: Option<(NaiveDate, NaiveDate)>,
) {
    let (grid_x, stride) = activity_heatmap_geometry(area);
    let cell_width = stride.saturating_sub(ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP);
    let labels = ["", "Mon", "", "Wed", "", "Fri", ""];
    for day in 0..7_i64 {
        let mut spans = vec![Span::styled(
            format!("{:<4}", labels[day as usize]),
            app.theme.subtle_text_style(),
        )];
        spans.push(Span::raw(
            " ".repeat(grid_x.saturating_sub(area.x.saturating_add(4)) as usize),
        ));
        for week in 0..ACCOUNT_ACTIVITY_HEATMAP_WEEKS {
            let date = start
                .checked_add_signed(ChronoDuration::days(week * 7 + day))
                .unwrap_or(start);
            let returned = date <= today
                && returned_range.is_some_and(|(first, last)| date >= first && date <= last);
            spans.push(Span::styled(
                ACCOUNT_ACTIVITY_HEATMAP_GLYPH
                    .to_string()
                    .repeat(cell_width),
                activity_heat_cell_style(app, daily_tokens_on(activity, date), returned),
            ));
            if week + 1 < ACCOUNT_ACTIVITY_HEATMAP_WEEKS {
                spans.push(Span::raw(" ".repeat(ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP)));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y.saturating_add(day as u16), area.width, 1),
        );
    }
}

fn render_activity_heatmap_footer(frame: &mut Frame, app: &App, area: Rect, range: &str) {
    let colors = activity_heat_colors(app);
    let subtle = app.theme.subtle_text_style();
    let mut legend = vec![
        Span::styled("No data ", subtle),
        activity_heat_legend_swatch(colors[0]),
        Span::styled("   Less ", subtle),
    ];
    for (index, color) in colors[1..].iter().copied().enumerate() {
        legend.push(activity_heat_legend_swatch(color));
        if index + 1 < colors.len() - 1 {
            legend.push(Span::raw(" "));
        }
    }
    legend.push(Span::styled(" More", subtle));
    let legend_width = Line::from(legend.clone()).width();
    let range_width = Line::from(range).width();
    let spacer = (area.width as usize).saturating_sub(range_width + legend_width);
    let mut spans = vec![Span::styled(range.to_string(), subtle)];
    spans.push(Span::raw(" ".repeat(spacer)));
    spans.extend(legend);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn activity_heat_legend_swatch(color: Color) -> Span<'static> {
    Span::styled(
        ACCOUNT_ACTIVITY_HEATMAP_GLYPH.to_string().repeat(2),
        Style::default().fg(color),
    )
}

fn activity_heat_cell_style(app: &App, tokens: Option<u64>, returned: bool) -> Style {
    let colors = activity_heat_colors(app);
    let index = if !returned {
        0
    } else {
        match tokens.unwrap_or(0) {
            0 => 1,
            tokens if tokens < 100_000_000 => 2,
            tokens if tokens < 500_000_000 => 3,
            tokens if tokens < 1_000_000_000 => 4,
            _ => 5,
        }
    };
    Style::default().fg(colors[index])
}

fn activity_heat_colors(app: &App) -> [Color; 6] {
    if app.theme.background == Color::Black {
        return [
            Color::DarkGray,
            Color::Gray,
            Color::Blue,
            Color::Cyan,
            Color::LightCyan,
            Color::White,
        ];
    }
    if app.theme.background == Color::White {
        return [
            Color::Gray,
            Color::DarkGray,
            Color::LightBlue,
            Color::Blue,
            Color::Cyan,
            Color::Black,
        ];
    }
    let light = matches!(
        app.theme.background,
        Color::Gray | Color::Rgb(192..=255, 192..=255, 192..=255)
    );
    let colors = if light {
        [
            Color::Rgb(242, 244, 248),
            Color::Rgb(224, 229, 238),
            Color::Rgb(198, 211, 250),
            Color::Rgb(143, 166, 245),
            Color::Rgb(91, 121, 239),
            Color::Rgb(59, 92, 230),
        ]
    } else {
        [
            Color::Rgb(22, 27, 34),
            Color::Rgb(31, 40, 45),
            Color::Rgb(28, 67, 68),
            Color::Rgb(40, 98, 95),
            Color::Rgb(70, 143, 135),
            Color::Rgb(118, 198, 183),
        ]
    };
    colors.map(|color| app.theme.color(color))
}

fn quota_stability_label(window: &tokscale_core::telemetry::QuotaWindowActivity) -> String {
    let Some(first) = window.points.first() else {
        return "No quota samples yet".to_string();
    };
    let last = window.points.last().unwrap_or(first);
    let duration = activity_duration_label(
        window
            .last_observed_at_ms
            .saturating_sub(window.first_observed_at_ms),
        true,
    );
    let (minimum, maximum) = window.points.iter().fold(
        (first.used_percent, first.used_percent),
        |(minimum, maximum), point| {
            (
                minimum.min(point.used_percent),
                maximum.max(point.used_percent),
            )
        },
    );
    if (maximum - minimum).abs() < 0.05 {
        format!(
            "Stable at {} for {duration}",
            format_activity_percent(last.used_percent)
        )
    } else {
        format!(
            "Observed {} → {} over {duration}",
            format_activity_percent(first.used_percent),
            format_activity_percent(last.used_percent)
        )
    }
}

fn primary_reset_status(
    app: &App,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    window: &tokscale_core::telemetry::QuotaWindowActivity,
) -> (String, Style) {
    let events = activity.reset_events.iter().filter(|event| {
        event
            .limit_id
            .as_deref()
            .is_none_or(|limit_id| limit_id == window.limit_id)
    });
    let Some(event) = events.max_by_key(|event| event.observed_at_ms) else {
        return (
            "No reset observed today".to_string(),
            app.theme.success_style(),
        );
    };
    use tokscale_core::telemetry::QuotaResetEventType;
    let label = match event.event_type {
        QuotaResetEventType::ConfirmedManual => format!(
            "Reset here {} · confirmed",
            activity_clock(event.observed_at_ms)
        ),
        QuotaResetEventType::ObservedScheduledRollover => format!(
            "Scheduled rollover {} · inferred",
            activity_clock(event.occurred_at_ms)
        ),
        QuotaResetEventType::ObservedEarlyRollover => format!(
            "Early rollover {} · source unknown",
            activity_clock(event.observed_at_ms)
        ),
        QuotaResetEventType::ObservedRollover => format!(
            "Rollover observed {} · inferred",
            activity_clock(event.observed_at_ms)
        ),
    };
    (label, app.theme.warning_style())
}

fn format_activity_seconds(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn format_activity_days(days: u64) -> String {
    if days == 1 {
        "1 day".to_string()
    } else {
        format!("{days} days")
    }
}

fn format_activity_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.2}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        format_tokens(tokens)
    }
}

fn format_activity_percent(percent: f64) -> String {
    if (percent - percent.round()).abs() < 0.05 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

fn activity_duration_label(duration_ms: i64, round_minutes: bool) -> String {
    if duration_ms <= 0 {
        return "0m".to_string();
    }
    let minutes = if round_minutes {
        duration_ms.saturating_add(30_000) / 60_000
    } else {
        duration_ms / 60_000
    };
    if minutes >= 60 {
        let hours = minutes / 60;
        let remainder = minutes % 60;
        if remainder > 0 {
            format!("{hours}h{remainder}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".to_string()
    }
}

fn activity_reset_label(timestamp_ms: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|value| value.format("%a %b %-d %H:%M").to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn latest_activity_timestamp_ms(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) -> Option<i64> {
    activity
        .summary
        .iter()
        .map(|summary| summary.fetched_at_ms)
        .chain(
            activity
                .daily_usage
                .iter()
                .map(|bucket| bucket.fetched_at_ms),
        )
        .chain(
            activity
                .quota_windows
                .iter()
                .map(|window| window.last_observed_at_ms),
        )
        .chain(
            activity
                .reset_events
                .iter()
                .map(|event| event.observed_at_ms),
        )
        .max()
}

fn account_activity_lines(
    app: &App,
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    width: usize,
) -> Vec<Line<'static>> {
    if activity.summary.is_none()
        && activity.daily_usage.is_empty()
        && activity.quota_windows.is_empty()
        && activity.reset_events.is_empty()
    {
        return vec![
            Line::from(Span::styled(
                "  Waiting for the first successful activity sample",
                app.theme.secondary_text_style(),
            )),
            Line::from(Span::styled(
                "  Official tokens and locally observed quota will appear here.",
                app.theme.subtle_text_style(),
            )),
        ];
    }

    let today = app.overview_date();
    let mut lines = Vec::new();
    if !activity.daily_usage.is_empty() {
        let today_label = daily_tokens_on(activity, today)
            .map(format_activity_tokens)
            .unwrap_or_else(|| "—".to_string());
        let seven_day_label = format_activity_tokens(daily_tokens_in_window(activity, today, 7));
        lines.push(compact_activity_line(
            app,
            "Official",
            &format!("7d {seven_day_label} · Today {today_label}"),
            width,
            app.theme.secondary_text_style(),
        ));
        lines.push(compact_activity_line(
            app,
            "Daily rhythm",
            &format!(
                "{} · peak {}",
                daily_activity_spark(activity, today),
                daily_peak_label(activity, today)
            ),
            width,
            Style::default().fg(app.theme.accent),
        ));
    } else {
        lines.push(compact_activity_line(
            app,
            "Official",
            "waiting for daily buckets",
            width,
            app.theme.subtle_text_style(),
        ));
    }

    if let Some(window) = primary_activity_window(activity) {
        let current_used = window
            .points
            .last()
            .map(|point| point.used_percent)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        lines.push(compact_activity_line(
            app,
            &format!("Quota {}", window.limit_label),
            &format!(
                "{} used · {} left",
                format_activity_percent(current_used),
                format_activity_percent(100.0 - current_used)
            ),
            width,
            app.theme.secondary_text_style(),
        ));
        lines.push(compact_activity_line(
            app,
            "Observed",
            &format!(
                "{} · {} samples",
                quota_stability_label(window),
                window.points.len()
            ),
            width,
            app.theme.secondary_text_style(),
        ));
        let (reset_label, _) = primary_reset_status(app, activity, window);
        lines.push(compact_activity_line(
            app,
            "Quality",
            &format!(
                "max gap {} · {}",
                activity_duration_label(window.max_gap_ms, false),
                reset_label
            ),
            width,
            app.theme.subtle_text_style(),
        ));
        let next_reset = window
            .points
            .last()
            .and_then(|point| point.resets_at_ms)
            .map(activity_reset_label)
            .unwrap_or_else(|| "unknown".to_string());
        let synced = latest_activity_timestamp_ms(activity)
            .map(activity_clock)
            .unwrap_or_else(|| "—".to_string());
        lines.push(compact_activity_line(
            app,
            "Reset / sync",
            &format!("{next_reset} · synced {synced}"),
            width,
            app.theme.subtle_text_style(),
        ));
    } else {
        lines.push(compact_activity_line(
            app,
            "Quota",
            "waiting for a successful local sample",
            width,
            app.theme.subtle_text_style(),
        ));
        lines.push(compact_activity_line(
            app,
            "Reset",
            "no local reset evidence yet",
            width,
            app.theme.subtle_text_style(),
        ));
    }

    lines
}

fn compact_activity_line(
    app: &App,
    label: &str,
    value: &str,
    width: usize,
    value_style: Style,
) -> Line<'static> {
    let label_width: usize = if width >= 78 { 16 } else { 13 };
    let label = truncate_string(label, label_width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width);
    Line::from(vec![
        Span::styled(
            format!(
                "  {label:<padding$}",
                padding = label_width.saturating_sub(2)
            ),
            app.theme.subtle_text_style(),
        ),
        Span::styled(truncate_string(value, value_width), value_style),
    ])
}

fn daily_peak_label(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    today: NaiveDate,
) -> String {
    (0_i64..7)
        .filter_map(|offset| {
            let date = today.checked_sub_signed(ChronoDuration::days(offset))?;
            daily_tokens_on(activity, date)
        })
        .max()
        .map(format_activity_tokens)
        .unwrap_or_else(|| "—".to_string())
}

fn daily_tokens_on(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    date: NaiveDate,
) -> Option<u64> {
    let date = date.format("%Y-%m-%d").to_string();
    activity
        .daily_usage
        .iter()
        .find(|bucket| bucket.start_date == date)
        .map(|bucket| bucket.tokens)
}

fn daily_tokens_in_window(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    end: NaiveDate,
    days: i64,
) -> u64 {
    let start = end
        .checked_sub_signed(ChronoDuration::days(days.saturating_sub(1)))
        .unwrap_or(end);
    activity
        .daily_usage
        .iter()
        .filter_map(|bucket| {
            let date = NaiveDate::parse_from_str(&bucket.start_date, "%Y-%m-%d").ok()?;
            (date >= start && date <= end).then_some(bucket.tokens)
        })
        .fold(0_u64, u64::saturating_add)
}

fn seven_day_date_range(today: NaiveDate) -> String {
    let start = today
        .checked_sub_signed(ChronoDuration::days(6))
        .unwrap_or(today);
    format!("{}–{}", start.format("%m/%d"), today.format("%m/%d"))
}

fn daily_activity_spark(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
    today: NaiveDate,
) -> String {
    let mut values = Vec::with_capacity(7);
    for offset in (0_i64..7).rev() {
        let date = today
            .checked_sub_signed(ChronoDuration::days(offset))
            .unwrap_or(today);
        values.push(daily_tokens_on(activity, date));
    }
    let maximum = values.iter().flatten().copied().max().unwrap_or(0);
    values
        .into_iter()
        .map(|value| match value {
            None => '·',
            Some(0) => '▁',
            Some(value) if maximum > 0 => {
                const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
                let index = ((value as f64 / maximum as f64) * 7.0).round() as usize;
                LEVELS[index.min(LEVELS.len() - 1)]
            }
            Some(_) => '▁',
        })
        .collect()
}

fn primary_activity_window(
    activity: &tokscale_core::telemetry::AccountActivitySnapshot,
) -> Option<&tokscale_core::telemetry::QuotaWindowActivity> {
    activity.quota_windows.iter().min_by_key(|window| {
        (
            window.limit_id != "codex:primary",
            window.window_seconds.unwrap_or(i64::MAX),
            window.limit_id.as_str(),
        )
    })
}

fn activity_clock(timestamp_ms: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|value| value.format("%H:%M").to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn pad_selected_row(app: &App, spans: &mut Vec<Span<'static>>, width: usize, selected: bool) {
    let used = Line::from(spans.clone()).width();
    if used < width {
        spans.push(styled(
            app,
            " ".repeat(width - used),
            Style::default(),
            selected,
        ));
    }
}

fn account_table_row_style(app: &App, index: usize) -> Style {
    if index % 2 == 1 {
        app.theme.striped_row_style()
    } else {
        Style::default()
    }
}

fn use_account_button(account_id: &str, compact: bool) -> ButtonSpec {
    ButtonSpec {
        label: if compact { "u Use" } else { "u Use Account" }.to_string(),
        kind: ButtonKind::Primary,
        action: ClickAction::CodexUseAccount {
            account_id: account_id.to_string(),
        },
    }
}

fn remove_account_button(account_id: &str, compact: bool) -> ButtonSpec {
    ButtonSpec {
        label: if compact { "Del Rm" } else { "Del Remove" }.to_string(),
        kind: ButtonKind::Danger,
        action: ClickAction::CodexRemoveAccount {
            account_id: account_id.to_string(),
        },
    }
}

fn reset_account_button(account_id: &str, compact: bool) -> ButtonSpec {
    ButtonSpec {
        label: if compact { "x Rst" } else { "x Reset" }.to_string(),
        kind: ButtonKind::Warning,
        action: ClickAction::CodexResetAccount {
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

    if outputs.iter().any(|output| {
        matches!(
            readiness_status(output),
            UsageReadiness::Critical | UsageReadiness::Watch
        )
    }) {
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
        UsageReadiness::Ready => app.theme.success_color(),
        UsageReadiness::Watch => app.theme.warning_color(),
        UsageReadiness::Critical => app.theme.danger_color(),
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

fn account_auth_color(app: &App, output: &UsageOutput) -> Color {
    match &output.account {
        Some(account) if account.is_active => app.theme.success_color(),
        Some(_) => app.theme.info_color(),
        None => app.theme.warning_color(),
    }
}

fn remaining_label(metric: &UsageMetric) -> String {
    metric
        .remaining_label
        .clone()
        .unwrap_or_else(|| format!("{:.0}% left", metric.remaining_percent))
}

fn has_available_reset_credit(output: &UsageOutput) -> bool {
    output.provider == "Codex"
        && output
            .reset_credits
            .as_ref()
            .is_some_and(|credits| credits.available_count > 0)
}

fn reset_credit_count_label(count: u32) -> String {
    if count == 1 {
        "1 available".to_string()
    } else {
        format!("{count} available")
    }
}

fn reset_bank_summary(outputs: &[UsageOutput], width: usize) -> String {
    let available: u32 = outputs
        .iter()
        .filter(|output| output.provider == "Codex")
        .filter_map(|output| output.reset_credits.as_ref())
        .map(|credits| credits.available_count)
        .sum();
    if available == 0 {
        return "No reset credits".to_string();
    }

    let expiries = credit_expiry_buckets(
        outputs
            .iter()
            .filter_map(|output| output.reset_credits.as_ref())
            .flat_map(|credits| credits.credits.iter())
            .filter_map(|credit| credit.expires_at.as_deref()),
    );

    let count = if available == 1 {
        reset_credit_count_label(available)
    } else {
        format!("{available} available across accounts")
    };
    match nearest_credit_expiry_label(&expiries) {
        Some(nearest) => truncate_string(&format!("{count} · {nearest}"), width),
        None => count,
    }
}

fn credits_status_line(output: &UsageOutput) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(credits) = &output.credit_status {
        if let Some(balance) = credits.balance.as_deref() {
            parts.push(format!("API credits {balance}"));
        }
        if credits.unlimited == Some(true) {
            parts.push("unlimited".to_string());
        }
        if credits.has_credits == Some(false) {
            parts.push("no API credits".to_string());
        }
        if credits.overage_limit_reached == Some(true) {
            parts.push("API overage reached".to_string());
        }
    }
    if let Some(control) = &output.spend_control {
        if control.reached == Some(true) {
            parts.push("spend limit reached".to_string());
        } else if control.reached == Some(false) {
            parts.push("spend OK".to_string());
        }
        if let Some(limit) = control.individual_limit.as_deref() {
            parts.push(format!("spend limit {limit}"));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn format_expiry_time(value: &str) -> String {
    helpers::format_reset_time(value).replace("resets", "expires")
}

fn metric_color(app: &App, metric: &UsageMetric) -> Color {
    if metric.remaining_percent < 10.0 {
        app.theme.danger_color()
    } else if metric.remaining_percent < 25.0 {
        app.theme.warning_color()
    } else {
        app.theme.success_color()
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

fn styled<T: Into<String>>(app: &App, text: T, style: Style, selected: bool) -> Span<'static> {
    let style = if selected {
        style.bg(app.theme.selection).fg(app.theme.foreground)
    } else {
        style
    };
    Span::styled(text.into(), style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::{
        UsageAccount, UsageCreditStatus, UsageFetchDiagnostic, UsageResetCredit, UsageResetCredits,
        UsageSpendControl,
    };
    use crate::tui::app::{Tab, TuiConfig};
    use crate::tui::data::UsageData;
    use crate::tui::themes::{TerminalBackground, TerminalColorMode, Theme, ThemePreference};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
    use tokscale_core::telemetry::{
        AccountActivitySnapshot, AccountDailyUsage, AccountUsageSummary, QuotaActivityPoint,
        QuotaResetConfidence, QuotaResetEvent, QuotaResetEventType, QuotaWindowActivity,
    };

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
            reset_credits: None,
            credit_status: None,
            spend_control: None,
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

    fn output_with_reset_credits(
        provider: &str,
        account: Option<UsageAccount>,
        available_count: u32,
    ) -> UsageOutput {
        let mut output = output(provider, account);
        output.reset_credits = Some(UsageResetCredits {
            available_count,
            credits: vec![UsageResetCredit {
                id: Some("credit_1".to_string()),
                status: Some("available".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                expires_at: Some("2026-07-12T01:31:33Z".to_string()),
                title: Some("One free rate limit reset".to_string()),
                description: None,
            }],
        });
        output
    }

    fn output_with_reset_credit_expiries(
        provider: &str,
        account: Option<UsageAccount>,
        expiries: &[&str],
    ) -> UsageOutput {
        let mut output = output(provider, account);
        output.reset_credits = Some(UsageResetCredits {
            available_count: expiries.len() as u32,
            credits: expiries
                .iter()
                .enumerate()
                .map(|(index, expiry)| UsageResetCredit {
                    id: Some(format!("credit_{index}")),
                    status: Some("available".to_string()),
                    reset_type: Some("codex_rate_limits".to_string()),
                    expires_at: Some((*expiry).to_string()),
                    title: Some("One free rate limit reset".to_string()),
                    description: None,
                })
                .collect(),
        });
        output
    }

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
        let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();
        app.current_tab = Tab::Usage;
        app
    }

    fn account_activity() -> AccountActivitySnapshot {
        AccountActivitySnapshot {
            provider: "Codex".to_string(),
            account_id: "acct_work".to_string(),
            summary: Some(AccountUsageSummary {
                lifetime_tokens: Some(18_440_303_510),
                peak_daily_tokens: Some(1_862_755_230),
                longest_running_turn_seconds: Some(219_563),
                current_streak_days: Some(27),
                longest_streak_days: Some(27),
                fetched_at_ms: 30,
            }),
            daily_usage: vec![
                AccountDailyUsage {
                    start_date: "2026-07-22".to_string(),
                    tokens: 20,
                    fetched_at_ms: 10,
                },
                AccountDailyUsage {
                    start_date: "2026-07-23".to_string(),
                    tokens: 130,
                    fetched_at_ms: 20,
                },
            ],
            quota_windows: vec![QuotaWindowActivity {
                limit_id: "codex:primary".to_string(),
                limit_label: "5h".to_string(),
                window_seconds: Some(18_000),
                observed_consumption_percent: 110.0,
                reset_count: 1,
                first_observed_at_ms: 1_774_747_880_000,
                last_observed_at_ms: 1_774_751_480_000,
                max_gap_ms: 1_800_000,
                points: vec![
                    QuotaActivityPoint {
                        used_percent: 20.0,
                        resets_at_ms: Some(1_774_760_000_000),
                        observed_at_ms: 1_774_747_880_000,
                    },
                    QuotaActivityPoint {
                        used_percent: 100.0,
                        resets_at_ms: Some(1_774_760_000_000),
                        observed_at_ms: 1_774_749_680_000,
                    },
                    QuotaActivityPoint {
                        used_percent: 30.0,
                        resets_at_ms: Some(1_774_780_000_000),
                        observed_at_ms: 1_774_751_480_000,
                    },
                ],
            }],
            reset_events: vec![QuotaResetEvent {
                event_id: "observed-reset".to_string(),
                limit_id: Some("codex:primary".to_string()),
                event_type: QuotaResetEventType::ObservedEarlyRollover,
                occurred_at_ms: 1_774_751_480_000,
                observed_at_ms: 1_774_751_480_000,
                confidence: QuotaResetConfidence::Inferred,
            }],
        }
    }

    fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, app, Rect::new(0, 0, width, height)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_body(app: &mut App, width: u16, height: u16) -> String {
        buffer_text(&render_buffer(app, width, height))
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
    fn light_theme_maps_usage_status_colors_without_changing_labels() {
        let mut app = make_app();
        app.theme = Theme::with_terminal(
            ThemePreference::Light,
            TerminalColorMode::FullColor,
            TerminalBackground::Light,
        );

        assert_eq!(
            readiness_color(&app, UsageReadiness::Ready),
            app.theme.success_color()
        );
        assert_eq!(
            readiness_color(&app, UsageReadiness::Watch),
            app.theme.warning_color()
        );
        assert_eq!(
            readiness_color(&app, UsageReadiness::Critical),
            app.theme.danger_color()
        );
        assert_eq!(
            diagnostic_severity_color(&app, UsageFetchDiagnosticSeverity::Info),
            app.theme.info_color()
        );
        assert_eq!(
            diagnostic_severity_color(&app, UsageFetchDiagnosticSeverity::Warning),
            app.theme.warning_color()
        );
        assert_eq!(
            diagnostic_severity_color(&app, UsageFetchDiagnosticSeverity::Error),
            app.theme.danger_color()
        );

        let line = attention_more_line(&app, 2, 24);
        assert_eq!(line_text(&line), "  +2 more at risk");
        assert!(line.width() <= 24);
        assert_eq!(line.spans[0].style.fg, Some(app.theme.warning_color()));

        let selected = styled(&app, "Ready", Style::default(), true);
        assert_eq!(selected.content, "Ready");
        assert_eq!(selected.style.fg, Some(app.theme.foreground));
        assert_eq!(selected.style.bg, Some(app.theme.selection));
    }

    #[test]
    fn compatible_usage_render_downgrades_provider_rgb_colors() {
        let mut app = make_app();
        app.theme = Theme::with_terminal(
            ThemePreference::Light,
            TerminalColorMode::Compatible,
            TerminalBackground::Light,
        );
        app.subscription_usage = vec![
            output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            output_with_remaining("Claude", None, 20.0),
        ];

        assert!(matches!(get_provider_shade("Codex", 0), Color::Rgb(..)));
        let buffer = render_buffer(&mut app, 180, 24);
        let body = buffer_text(&buffer);

        assert!(body.contains("Usage Summary"), "{body}");
        assert!(body.contains("Selected Account"), "{body}");
        assert!(body.contains("Accounts"), "{body}");
        for cell in &buffer.content {
            assert!(
                !matches!(cell.fg, Color::Rgb(..)),
                "compatible usage foreground leaked RGB at cell {cell:?}"
            );
            assert!(
                !matches!(cell.bg, Color::Rgb(..)),
                "compatible usage background leaked RGB at cell {cell:?}"
            );
        }
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
    fn usage_summary_lines_fit_available_height() {
        let app = make_app();
        let outputs = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];

        let lines = usage_status_summary_lines(&app, &outputs, 80, 4);
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines.len(), 4);
        assert!(body.contains("State"), "{body}");
        assert!(body.contains("Active"), "{body}");
        assert!(body.contains("Capacity"), "{body}");
        assert!(body.contains("Action"), "{body}");
        assert!(!body.contains("Fallback"), "{body}");
        assert!(!body.contains("Next Reset"), "{body}");
    }

    #[test]
    fn usage_credit_status_uses_user_facing_copy() {
        let mut output = output("Codex", None);
        output.credit_status = Some(UsageCreditStatus {
            balance: Some("0".to_string()),
            has_credits: Some(false),
            unlimited: Some(false),
            overage_limit_reached: Some(true),
        });
        output.spend_control = Some(UsageSpendControl {
            individual_limit: Some("$10".to_string()),
            reached: Some(false),
        });

        let label = credits_status_line(&output).expect("missing credit status");

        assert!(label.contains("API credits 0"), "{label}");
        assert!(label.contains("no API credits"), "{label}");
        assert!(label.contains("API overage reached"), "{label}");
        assert!(label.contains("spend OK"), "{label}");
        assert!(label.contains("spend limit $10"), "{label}");
        assert!(!label.contains("billing"), "{label}");
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

        let body = render_body(&mut app, 150, 32);

        assert!(body.contains("Usage Summary"), "{body}");
        assert!(body.contains("Selected Account"), "{body}");
        assert!(body.contains("Accounts"), "{body}");
        assert!(body.contains("Attention"), "{body}");
        assert!(body.contains("Account") && body.contains("Plan"), "{body}");
        assert!(body.contains("Keep current account"), "{body}");
        assert!(body.contains("work"), "{body}");
        assert!(body.contains("personal"), "{body}");
        assert!(body.contains(" Del Remove "), "{body}");
        assert!(body.contains("Show Emails"), "{body}");
    }

    #[test]
    fn usage_summary_reserves_space_for_diagnostics() {
        let mut app = make_app();
        app.usage_fetch_diagnostics = vec![UsageFetchDiagnostic::new(
            "Codex",
            Some(UsageAccount {
                id: "acct_personal".to_string(),
                label: Some("personal".to_string()),
                is_active: false,
            }),
            "usage endpoint rejected credentials",
        )];
        let outputs = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        let max_lines = 6;
        let reserve = diagnostic_line_reserve(&app, max_lines);
        let mut lines = usage_status_summary_lines(&app, &outputs, 80, max_lines - reserve);
        append_usage_diagnostic_lines(&mut lines, &app, 80, max_lines);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Diagnostics"), "{rendered}");
        assert!(rendered.contains("usage endpoint rejected"), "{rendered}");
    }

    #[test]
    fn usage_summary_compacts_hidden_diagnostics() {
        let mut app = make_app();
        app.usage_fetch_diagnostics = vec![
            UsageFetchDiagnostic::new("Codex", None, "first issue"),
            UsageFetchDiagnostic::new("Claude", None, "second issue"),
            UsageFetchDiagnostic::new("Amp", None, "third issue"),
        ];
        let mut lines = Vec::new();

        append_usage_diagnostic_lines(&mut lines, &app, 80, 3);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Diagnostics"), "{rendered}");
        assert!(rendered.contains("first issue"), "{rendered}");
        assert!(rendered.contains("+2 more issues"), "{rendered}");
        assert!(!rendered.contains("second issue"), "{rendered}");
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
        assert!(body.contains("u Use Account"), "{body}");
        assert!(body.contains("saved store"), "{body}");
    }

    #[test]
    fn usage_renders_saved_account_when_metrics_are_empty() {
        let mut app = make_app();
        let mut saved = output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        );
        saved.metrics.clear();
        app.subscription_usage = vec![saved];

        let body = render_body(&mut app, 150, 28);

        assert!(body.contains("Selected Account"), "{body}");
        assert!(body.contains("Codex (work)"), "{body}");
        assert!(body.contains("No quota metrics returned"), "{body}");
        assert!(!body.contains("No usage data loaded"), "{body}");
    }

    #[test]
    fn usage_account_row_expands_inline_account_activity() {
        let mut app = make_app();
        app.set_render_reference_now(
            NaiveDate::from_ymd_opt(2026, 7, 23)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        app.account_activities
            .insert("acct_work".to_string(), account_activity());

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let body = render_body(&mut app, 150, 36);

        assert_eq!(app.expanded_usage_account_id.as_deref(), Some("acct_work"));
        assert!(body.contains("▾"), "{body}");
        assert!(body.contains("Account Activity"), "{body}");
        assert!(body.contains("18.44B"), "{body}");
        assert!(body.contains("Lifetime tokens"), "{body}");
        assert!(body.contains("7d 150 · Today 130"), "{body}");
        assert!(body.contains("30% used · 70% left"), "{body}");
        assert!(body.contains("Early rollover"), "{body}");
    }

    #[test]
    fn wide_usage_account_activity_renders_visual_dashboard() {
        let mut app = make_app();
        app.set_render_reference_now(
            NaiveDate::from_ymd_opt(2026, 7, 23)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        app.account_activities
            .insert("acct_work".to_string(), account_activity());
        app.toggle_usage_account_activity(0);

        let body = render_body(&mut app, 230, 69);

        assert!(body.contains("OFFICIAL ACCOUNT SUMMARY"), "{body}");
        assert!(body.contains("18.44B"), "{body}");
        assert!(body.contains("Lifetime tokens"), "{body}");
        assert!(body.contains("OFFICIAL TOKEN ACTIVITY"), "{body}");
        assert!(body.contains("CURRENT 7-DAY WINDOW"), "{body}");
        assert!(body.contains("Fri 07/17"), "{body}");
        assert!(body.contains("QUOTA & LOCAL EVIDENCE"), "{body}");
        assert!(body.contains("30% used"), "{body}");
        assert!(body.contains("3 samples"), "{body}");
        assert!(body.contains("Early rollover"), "{body}");
        assert!(body.contains("Last synced"), "{body}");
    }

    #[test]
    fn account_activity_wide_layout_uses_content_width_breakpoint() {
        assert_eq!(account_activity_content_width(123), 119);
        assert_eq!(account_activity_content_width(124), 120);
        assert_eq!(account_activity_height(123, 40, 0), 13);
        assert_eq!(account_activity_height(124, 40, 0), 40);
        assert_eq!(account_activity_height(124, 27, 0), 13);
    }

    #[test]
    fn account_activity_wide_layout_renders_at_common_terminal_width() {
        let mut app = make_app();
        app.set_render_reference_now(
            NaiveDate::from_ymd_opt(2026, 7, 23)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        app.account_activities
            .insert("acct_work".to_string(), account_activity());
        app.toggle_usage_account_activity(0);

        let body = render_body(&mut app, 132, 69);

        assert!(body.contains("OFFICIAL ACCOUNT SUMMARY"), "{body}");
        assert!(body.contains("OFFICIAL TOKEN ACTIVITY"), "{body}");
        assert!(body.contains("CURRENT 7-DAY WINDOW"), "{body}");
        assert!(body.contains("QUOTA & LOCAL EVIDENCE"), "{body}");
    }

    #[test]
    fn wide_activity_heatmap_uses_dense_two_column_cells() {
        let area = Rect::new(0, 0, 230, 7);
        let (grid_x, stride) = activity_heatmap_geometry(area);

        assert_eq!(stride, 3);
        assert_eq!(
            grid_x,
            4 + (230 - 4 - (52 * stride - ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP) as u16) / 2
        );

        let app = make_app();
        let colors = activity_heat_colors(&app);
        assert_ne!(colors[0], app.theme.background);
        assert!(colors.windows(2).all(|pair| pair[0] != pair[1]));

        let cell = activity_heat_cell_style(&app, Some(500_000_000), true);
        assert_eq!(cell.fg, Some(colors[4]));
        assert_eq!(cell.bg, None);
        assert_eq!(ACCOUNT_ACTIVITY_HEATMAP_GLYPH, '▆');
    }

    #[test]
    fn wide_activity_heatmap_compacts_cells_below_two_column_width() {
        let area = Rect::new(0, 0, 120, 7);
        let (grid_x, stride) = activity_heatmap_geometry(area);

        assert_eq!(stride, 2);
        assert_eq!(
            grid_x,
            4 + (120 - 4 - (52 * stride - ACCOUNT_ACTIVITY_HEATMAP_CELL_GAP) as u16) / 2
        );
    }

    #[test]
    fn quota_only_activity_prioritizes_the_available_sample() {
        let app = make_app();
        let mut activity = account_activity();
        activity.daily_usage.clear();
        let mut additional = activity.quota_windows[0].clone();
        additional.limit_id = "codex:additional".to_string();
        additional.limit_label = "1h".to_string();
        additional.window_seconds = Some(60 * 60);
        activity.quota_windows.push(additional);

        let lines = account_activity_lines(&app, &activity, 100);

        assert!(line_text(&lines[0]).contains("waiting for daily buckets"));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("Quota 5h")));
    }

    #[test]
    fn usage_account_activity_reports_local_store_errors() {
        let mut app = make_app();
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        app.account_activity_error = Some("Local activity history could not be opened".to_string());

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let body = render_body(&mut app, 150, 36);

        assert!(
            body.contains("Local activity history could not be opened"),
            "{body}"
        );
        assert!(!body.contains("No account activity samples yet"), "{body}");
    }

    #[test]
    fn usage_account_activity_accordion_keeps_only_one_account_open() {
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

        app.toggle_usage_account_activity(0);
        assert_eq!(app.expanded_usage_account_id.as_deref(), Some("acct_work"));
        app.toggle_usage_account_activity(1);
        assert_eq!(
            app.expanded_usage_account_id.as_deref(),
            Some("acct_personal")
        );
        app.toggle_usage_account_activity(1);
        assert!(app.expanded_usage_account_id.is_none());
    }

    #[test]
    fn usage_account_disclosure_click_toggles_inline_activity() {
        let mut app = make_app();
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        render_buffer(&mut app, 150, 30);
        let disclosure = app
            .click_areas
            .iter()
            .find(|area| matches!(&area.action, ClickAction::UsageToggleActivity { index: 0 }))
            .map(|area| area.rect)
            .expect("Codex account disclosure click area");

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: disclosure.x,
            row: disclosure.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.expanded_usage_account_id.as_deref(), Some("acct_work"));
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
    fn medium_usage_layout_compacts_single_account_table() {
        let app = make_app();
        let outputs = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];

        assert_eq!(medium_accounts_table_height(&app, &outputs, 100, 40), 5);
        assert_eq!(medium_accounts_table_height(&app, &outputs, 100, 4), 4);
    }

    #[test]
    fn medium_usage_layout_grows_expanded_activity_at_wide_content_width() {
        let mut app = make_app();
        let outputs = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];
        app.subscription_usage = outputs.clone();
        app.toggle_usage_account_activity(0);

        assert_eq!(medium_accounts_table_height(&app, &outputs, 126, 42), 33);
        assert_eq!(medium_accounts_table_height(&app, &outputs, 125, 42), 18);
        assert_eq!(medium_accounts_table_height(&app, &outputs, 126, 41), 18);
    }

    #[test]
    fn medium_usage_layout_grows_selected_account_for_limits() {
        let mut output = output_with_reset_credits(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            3,
        );
        output.metrics = vec![
            UsageMetric {
                label: "5h".to_string(),
                used_percent: 2.0,
                remaining_percent: 98.0,
                remaining_label: Some("98% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 15.0,
                remaining_percent: 85.0,
                remaining_label: Some("85% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark 5h".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark week".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
        ];

        assert_eq!(medium_summary_height(&[output.clone()], 48), 11);
        assert!(medium_selected_account_preferred_height(&output) >= 15);
    }

    #[test]
    fn medium_usage_renders_single_account_limits_before_accounts() {
        let mut app = make_app();
        let mut output = output_with_reset_credits(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            3,
        );
        output.metrics = vec![
            UsageMetric {
                label: "5h".to_string(),
                used_percent: 2.0,
                remaining_percent: 98.0,
                remaining_label: Some("98% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 15.0,
                remaining_percent: 85.0,
                remaining_label: Some("85% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark 5h".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark week".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
        ];
        app.subscription_usage = vec![output];

        let body = render_body(&mut app, 120, 42);
        let selected_metric = body
            .lines()
            .find(|line| line.contains("Spark 5h") && line.contains("100% left"))
            .expect("missing selected account Spark 5h metric");
        let accounts_title = body
            .lines()
            .position(|line| line.contains("Accounts"))
            .expect("missing accounts panel");
        let selected_metric_line = body
            .lines()
            .position(|line| line == selected_metric)
            .expect("missing selected metric line");

        assert!(selected_metric_line < accounts_title, "{body}");
    }

    #[test]
    fn compact_usage_renders_single_account_limits_before_accounts() {
        let mut app = make_app();
        let mut output = output_with_reset_credits(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            3,
        );
        output.metrics = vec![
            UsageMetric {
                label: "5h".to_string(),
                used_percent: 2.0,
                remaining_percent: 98.0,
                remaining_label: Some("98% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 15.0,
                remaining_percent: 85.0,
                remaining_label: Some("85% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark 5h".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark week".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
        ];
        app.subscription_usage = vec![output];

        let body = render_body(&mut app, 90, 42);
        let summary_title = body
            .lines()
            .position(|line| line.contains("Usage Summary"))
            .expect("missing compact usage summary");
        let selected_title = body
            .lines()
            .position(|line| line.contains("Selected Account"))
            .expect("missing compact selected account panel");
        let selected_metric = body
            .lines()
            .find(|line| line.contains("Spark 5h") && line.contains("100% left"))
            .expect("missing compact selected account Spark 5h metric");
        let accounts_title = body
            .lines()
            .position(|line| line.contains("Accounts"))
            .expect("missing accounts panel");
        let selected_metric_line = body
            .lines()
            .position(|line| line == selected_metric)
            .expect("missing selected metric line");

        assert!(summary_title < selected_title, "{body}");
        assert!(selected_title < selected_metric_line, "{body}");
        assert!(selected_metric_line < accounts_title, "{body}");
        assert!(compact_should_prioritize_selected(
            Rect::new(0, 0, 90, 42),
            &app.subscription_usage
        ));
    }

    #[test]
    fn compact_usage_keeps_list_first_for_many_accounts() {
        let outputs = (0..4)
            .map(|index| {
                output(
                    "Codex",
                    Some(UsageAccount {
                        id: format!("acct_{index}"),
                        label: Some(format!("acct-{index}")),
                        is_active: index == 0,
                    }),
                )
            })
            .collect::<Vec<_>>();

        assert!(!compact_should_prioritize_selected(
            Rect::new(0, 0, 90, 42),
            &outputs
        ));
        assert!(!compact_should_prioritize_selected(
            Rect::new(0, 0, 90, 19),
            &outputs[..1]
        ));
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
            .find(|line| line.contains("personal") && line.contains("5h"))
            .expect("missing saved account table row");
        let actions_line = body
            .lines()
            .find(|line| line.contains("u Use Account") && line.contains("Del Remove"))
            .expect("missing selected account action buttons");

        assert!(!saved_row.contains("Use Account"), "{saved_row}");
        assert!(!saved_row.contains("Remove"), "{saved_row}");
        assert!(
            visual_col(actions_line, "Use Account").is_some(),
            "{actions_line}"
        );
    }

    #[test]
    fn usage_reset_button_renders_when_credit_available() {
        let mut app = make_app();
        app.subscription_usage = vec![output_with_reset_credits(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            2,
        )];

        let body = render_body(&mut app, 180, 32);

        assert!(body.contains("Reset Bank"), "{body}");
        assert!(body.contains("2 available"), "{body}");
        assert!(body.contains("x Reset"), "{body}");
        assert!(body.contains(" Reset "), "{body}");
        assert!(body.contains("Credit Bank"), "{body}");
        assert!(body.contains("2 credits"), "{body}");
        assert!(body.contains("nearest expires"), "{body}");
        let state_line = body
            .lines()
            .find(|line| line.contains("State"))
            .expect("missing state line");
        let credit_heading = body
            .lines()
            .find(|line| line.contains("Credit Bank"))
            .expect("missing credit bank heading");
        let credit_account = body
            .lines()
            .find(|line| line.contains("2 credits"))
            .expect("missing selected credit account");
        assert_eq!(
            visual_col(state_line, "State"),
            visual_col(credit_heading, "Credit Bank"),
            "{state_line}\n{credit_heading}"
        );
        assert_eq!(
            visual_col(credit_heading, "Credit Bank"),
            visual_col(credit_account, "work"),
            "{credit_heading}\n{credit_account}"
        );
        assert!(!body.contains("Active:"), "{body}");
        assert!(!body.contains("Quota Recovery"), "{body}");
        assert!(app.click_areas.iter().any(|area| matches!(
            &area.action,
            ClickAction::CodexResetAccount { account_id } if account_id == "acct_work"
        )));
    }

    #[test]
    fn usage_reset_bank_summarizes_left_and_expands_selected_expiries() {
        let mut app = make_app();
        app.subscription_usage = vec![output_with_reset_credit_expiries(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            &["reset-a", "reset-b", "reset-c", "reset-d"],
        )];

        let body = render_body(&mut app, 180, 32);
        let credit_account = body
            .lines()
            .find(|line| line.contains("4 credits"))
            .expect("missing credit account row");

        assert!(body.contains("4 available"), "{body}");
        assert!(
            credit_account.contains("nearest expires reset-a"),
            "{credit_account}"
        );
        assert!(!credit_account.contains("reset-b"), "{credit_account}");
        assert!(body.contains("expires reset-a"), "{body}");
        assert!(body.contains("expires reset-d"), "{body}");
        assert!(!body.contains("scheduled resets"), "{body}");
        assert!(!body.contains("Schedule"), "{body}");
        assert!(!body.contains("+2"), "{body}");
    }

    #[test]
    fn medium_usage_reset_bank_keeps_selected_expiries_visible() {
        let mut app = make_app();
        app.subscription_usage = vec![output_with_reset_credit_expiries(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            &["reset-a", "reset-b", "reset-c", "reset-d"],
        )];

        let body = render_body(&mut app, 120, 36);

        assert!(body.contains("Reset Bank"), "{body}");
        assert!(body.contains("4 available"), "{body}");
        assert!(body.contains("expires reset-a"), "{body}");
        assert!(body.contains("expires reset-d"), "{body}");
        assert!(!body.contains("scheduled resets"), "{body}");
        assert!(!body.contains("Schedule"), "{body}");
        assert!(!body.contains("+2"), "{body}");
    }

    #[test]
    fn reset_credit_account_line_uses_nearest_expiry_only() {
        let app = make_app();
        let output = output_with_reset_credit_expiries(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            &["reset-a", "reset-b", "reset-c", "reset-d"],
        );
        let credits = output.reset_credits.as_ref().unwrap();

        let line = line_text(&reset_credit_account_line(&app, &output, credits, true, 80));

        assert!(line.contains("4 credits"), "{line}");
        assert!(line.contains("nearest expires reset-a"), "{line}");
        assert!(!line.contains("reset-b"), "{line}");
    }

    #[test]
    fn selected_reset_schedule_groups_duplicate_expiries() {
        let output =
            output_with_reset_credit_expiries("Codex", None, &["reset-a", "reset-a", "reset-b"]);
        let credits = output.reset_credits.as_ref().unwrap();

        let buckets = reset_credit_buckets(credits);

        assert_eq!(
            format_selected_reset_schedule_entry(&buckets[0]),
            "x2 expires reset-a"
        );
        assert_eq!(
            format_selected_reset_schedule_entry(&buckets[1]),
            "expires reset-b"
        );
    }

    #[test]
    fn selected_account_panel_aligns_detail_sections() {
        let mut app = make_app();
        app.subscription_usage = vec![output_with_reset_credits(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            2,
        )];

        let body = render_body(&mut app, 180, 32);
        let status_line = body
            .lines()
            .find(|line| line.contains("Status") && line.contains("Ready"))
            .expect("missing selected account status line");
        let limits_line = body
            .lines()
            .find(|line| line.contains("Limits") && !line.contains("Limits / Reset"))
            .expect("missing selected account limits heading");
        let metric_line = body
            .lines()
            .find(|line| {
                line.contains("5h") && line.contains("90% left") && !line.contains("Codex")
            })
            .expect("missing selected account metric row");
        let actions_line = body
            .lines()
            .find(|line| line.contains("Actions") && !line.contains("Add Codex"))
            .expect("missing selected account actions heading");
        let current_line = body
            .lines()
            .find(|line| line.contains("Current account"))
            .expect("missing selected account action row");
        let selected_col = visual_col(status_line, "Status");
        assert_eq!(selected_col, visual_col(limits_line, "Limits"));
        assert_eq!(selected_col, visual_col(metric_line, "5h"));
        assert_eq!(selected_col, visual_col(actions_line, "Actions"));
        assert_eq!(selected_col, visual_col(current_line, "Current account"));
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
            .find(|line| line.contains("Codex") && line.contains("work") && line.contains("5h"))
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
            visual_col(active_row, "5h"),
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
    fn usage_account_table_allocates_space_to_long_limit_summaries() {
        let mut app = make_app();
        let mut output = output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        );
        output.metrics = vec![
            UsageMetric {
                label: "5h".to_string(),
                used_percent: 8.0,
                remaining_percent: 92.0,
                remaining_label: Some("92% left".to_string()),
                resets_at: Some("resets in 4h 24m".to_string()),
            },
            UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 49.0,
                remaining_percent: 51.0,
                remaining_label: Some("51% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark 5h".to_string(),
                used_percent: 0.0,
                remaining_percent: 100.0,
                remaining_label: Some("100% left".to_string()),
                resets_at: None,
            },
            UsageMetric {
                label: "Spark weekly".to_string(),
                used_percent: 12.0,
                remaining_percent: 88.0,
                remaining_label: Some("88% left".to_string()),
                resets_at: None,
            },
        ];
        app.subscription_usage = vec![output];

        let body = render_body(&mut app, 180, 24);
        let row = body
            .lines()
            .find(|line| line.contains("Codex") && line.contains("5h 92%"))
            .expect("missing account table row");

        assert!(row.contains("Weekly 51%"), "{row}");
        assert!(row.contains("Spark 5h 100%"), "{row}");
        assert!(row.contains("Spark weekly 88%"), "{row}");
        assert!(row.contains("resets in 4h 24m"), "{row}");
    }

    #[test]
    fn usage_account_table_reset_follows_display_metric() {
        let mut app = make_app();
        let mut output = output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        );
        output.metrics = vec![
            UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 8.0,
                remaining_percent: 92.0,
                remaining_label: Some("92% left".to_string()),
                resets_at: Some("resets next week".to_string()),
            },
            UsageMetric {
                label: "Session".to_string(),
                used_percent: 96.0,
                remaining_percent: 4.0,
                remaining_label: Some("4% left".to_string()),
                resets_at: Some("resets soon".to_string()),
            },
        ];
        app.subscription_usage = vec![output];

        let body = render_body(&mut app, 180, 24);
        let row = body
            .lines()
            .find(|line| line.contains("Codex") && line.contains("Weekly 92%"))
            .expect("missing account table row");

        assert!(row.contains("resets soon"), "{row}");
        assert!(!row.contains("resets next week"), "{row}");
    }

    #[test]
    fn narrow_usage_keeps_account_actions_visible() {
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

        let body = render_body(&mut app, 90, 28);

        assert!(body.contains("work"), "{body}");
        assert!(body.contains("personal"), "{body}");
        assert!(body.contains(" Del Remove "), "{body}");
    }

    #[test]
    fn narrow_usage_account_status_keeps_right_padding() {
        let mut app = make_app();
        app.subscription_usage = vec![output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        )];

        let body = render_body(&mut app, 90, 24);
        let row = body
            .lines()
            .find(|line| {
                line.contains("Codex") && line.contains("Active") && line.contains("Ready")
            })
            .expect("missing active narrow account row");
        let ready_end = row.rfind("Ready").expect(row) + "Ready".len();

        assert!(
            row[ready_end..].starts_with("  "),
            "expected right padding after Ready: {row:?}"
        );
    }

    #[test]
    fn very_narrow_ready_usage_uses_compact_actions_and_message() {
        let mut app = make_app();
        let body = render_body(&mut app, 28, 20);

        assert!(body.contains("r Refresh"), "{body}");
        assert!(body.contains("a Add"), "{body}");
        assert!(body.contains("No usage data"), "{body}");
        assert!(!body.contains("Add Codex"), "{body}");
        assert!(!body.contains("No subscription data load"), "{body}");
    }

    #[test]
    fn empty_usage_hides_email_privacy_action() {
        let mut app = make_app();
        let body = render_body(&mut app, 150, 20);

        assert!(body.contains("r Refresh"), "{body}");
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
            reset_credits: None,
            credit_status: None,
            spend_control: None,
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
