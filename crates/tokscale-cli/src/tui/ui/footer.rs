use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tokscale_core::pulse::weread::{format_read_duration, WeReadStatus};

use super::spinner::{get_phase_message, get_scanner_spans};
use super::widgets::{format_cost, format_tokens};
use crate::tui::app::{App, ClickAction, DrilldownView, OverviewMode, SortField, Tab};

const COMPACT_HINT_WIDTH: u16 = 64;
const MIN_ACTION_HINT_WIDTH: u16 = 10;
const MIN_SUMMARY_WIDTH: u16 = 8;
const SUMMARY_SCOPE_FIRST_WIDTH: u16 = 20;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(summary_width(app, inner.width)),
        ])
        .split(Rect::new(inner.x, inner.y, inner.width, 1));

    render_action_or_status(frame, app, chunks[0]);
    render_scope_summary(frame, app, chunks[1]);
}

fn current_count_label(app: &App) -> String {
    if app.is_drilldown_active() {
        return format!(" ({} rows)", app.drilldown_list_len());
    }

    match app.current_tab {
        Tab::Overview => {
            let (_, _, model_count) = app.overview_totals();
            format!(" ({} models)", model_count)
        }
        Tab::Pulse => String::new(),
        Tab::Models => format!(" ({} models)", app.data.models.len()),
        Tab::Agents => format!(" ({} agents)", app.data.agents.len()),
        Tab::Daily if app.is_daily_detail_active() => {
            format!(" ({} models)", app.get_sorted_daily_detail_rows().len())
        }
        Tab::Daily => match app.timeline_granularity {
            crate::tui::app::TimelineGranularity::Day => {
                format!(" ({} days)", app.data.daily.len())
            }
            crate::tui::app::TimelineGranularity::Hour => {
                format!(" ({} hours)", app.data.hourly.len())
            }
        },
        Tab::Hourly => format!(" ({} hours)", app.data.hourly.len()),
        Tab::Minutely => format!(" ({} minutes)", app.data.minutely.len()),
        Tab::Stats | Tab::Usage => String::new(),
    }
}

fn render_action_or_status(frame: &mut Frame, app: &mut App, area: Rect) {
    let should_show_status = app.data.loading
        || app.status_message.is_some()
        || (app.background_loading && !app.has_visible_data());
    let spans = if should_show_status {
        status_spans(app, area.width)
    } else {
        let mut spans = action_spans(app, area.x, area.y, area.width);
        if app.background_loading {
            push_background_refresh_hint_fit(&mut spans, app, area.width);
        }
        spans
    };

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn push_background_refresh_hint_fit(
    spans: &mut Vec<Span<'static>>,
    app: &App,
    available_width: u16,
) {
    let used = Line::from(spans.clone()).width() as u16;
    let separator = "  · ";
    let candidates = if available_width < COMPACT_HINT_WIDTH {
        ["Syncing", "Refreshing"]
    } else {
        ["Refreshing", "Syncing"]
    };

    for candidate in candidates {
        let width = separator.chars().count() as u16 + candidate.chars().count() as u16;
        if used.saturating_add(width) <= available_width {
            spans.push(Span::styled(separator, app.theme.subtle_text_style()));
            spans.push(Span::styled(
                candidate.to_string(),
                app.theme.subtle_text_style(),
            ));
            return;
        }
    }
}

fn status_spans(app: &App, width: u16) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    if app.data.loading {
        return loading_status_spans(app, width);
    } else if app.background_loading {
        if app.has_visible_data() {
            return vec![Span::styled(
                fit_status_text(refreshing_status_text(width), width),
                Style::default().fg(app.theme.muted),
            )];
        } else {
            return loading_status_spans(app, width);
        }
    } else if let Some(ref msg) = app.status_message {
        return vec![Span::styled(
            fit_status_text(msg, width),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )];
    }

    Vec::new()
}

fn loading_status_spans(app: &App, width: u16) -> Vec<Span<'static>> {
    if width >= 26 {
        let mut spans = get_scanner_spans(app.spinner_frame, &app.theme);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            get_phase_message("parsing-sources"),
            Style::default().fg(app.theme.muted),
        ));
        return spans;
    }

    vec![Span::styled(
        fit_status_text("Loading", width),
        Style::default().fg(app.theme.muted),
    )]
}

fn refreshing_status_text(width: u16) -> &'static str {
    match width {
        40.. => "Refreshing cached data in background...",
        18.. => "Refreshing data...",
        10.. => "Refreshing",
        7.. => "Refresh",
        _ => "Sync",
    }
}

fn fit_status_text(text: &str, width: u16) -> String {
    let max_chars = width as usize;
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }

    let head = text.chars().take(max_chars - 3).collect::<String>();
    format!("{head}...")
}

fn action_spans(app: &mut App, x: u16, y: u16, width: u16) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    if app.is_drilldown_active() {
        push_key_fit(
            &mut spans,
            "↑↓",
            "Rows",
            Some("Row"),
            Color::White,
            app.theme.muted,
            width,
        );
        let open_key = if width < COMPACT_HINT_WIDTH {
            "↵"
        } else {
            "Enter"
        };
        push_key_fit(
            &mut spans,
            open_key,
            "Details",
            Some("Open"),
            app.theme.accent,
            app.theme.muted,
            width,
        );
        push_key_fit(
            &mut spans,
            "Esc",
            "Back",
            Some("Back"),
            Color::Yellow,
            app.theme.muted,
            width,
        );
        push_sort_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "c",
            "Cost",
            Some("Cost"),
            SortField::Cost,
        );
        push_sort_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "t",
            "Tokens",
            Some("Tok"),
            SortField::Tokens,
        );
        if matches!(app.drilldown_view(), Some(DrilldownView::Model(_))) {
            push_sort_key_fit(
                &mut spans,
                app,
                x,
                y,
                width,
                "d",
                "Date",
                Some("Date"),
                SortField::Date,
            );
        }
        return spans;
    }

    push_key_fit(
        &mut spans,
        "↑↓",
        "Navigate",
        Some("Nav"),
        Color::White,
        app.theme.muted,
        width,
    );
    push_key_fit(
        &mut spans,
        "←→",
        "Workspace",
        Some("Ws"),
        Color::White,
        app.theme.muted,
        width,
    );

    if app.current_tab == Tab::Usage {
        push_action_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "u",
            if app.is_fetching_usage() {
                "Syncing"
            } else {
                "Refresh"
            },
            Some(if app.is_fetching_usage() {
                "Sync"
            } else {
                "Reload"
            }),
            if app.is_fetching_usage() {
                app.theme.muted
            } else {
                Color::Yellow
            },
            ClickAction::UsageRefresh,
        );
        push_action_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "a",
            if app.is_codex_login_running() {
                "Adding"
            } else {
                "Add Codex"
            },
            Some(if app.is_codex_login_running() {
                "Adding"
            } else {
                "Add"
            }),
            if app.is_codex_login_running() {
                app.theme.muted
            } else {
                app.theme.accent
            },
            ClickAction::CodexStartLogin,
        );
        push_action_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "m",
            if app.hide_usage_emails {
                "Show Emails"
            } else {
                "Hide Emails"
            },
            Some(if app.hide_usage_emails {
                "Show"
            } else {
                "Hide"
            }),
            if app.hide_usage_emails {
                Color::Green
            } else {
                Color::Blue
            },
            ClickAction::UsageToggleEmailPrivacy,
        );
        push_key_fit(
            &mut spans,
            "R",
            "Auto",
            Some("Auto"),
            if app.auto_refresh {
                Color::Green
            } else {
                Color::Blue
            },
            app.theme.muted,
            width,
        );
        push_key_fit(
            &mut spans,
            "q",
            "Quit",
            Some("Quit"),
            app.theme.muted,
            app.theme.muted,
            width,
        );
        return spans;
    }

    if app.current_tab == Tab::Pulse {
        push_action_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "r",
            if app.is_fetching_weread() {
                "Syncing"
            } else {
                "Sync WeRead"
            },
            Some(if app.is_fetching_weread() {
                "Sync"
            } else {
                "WeRead"
            }),
            if app.is_fetching_weread() {
                app.theme.muted
            } else {
                Color::Yellow
            },
            ClickAction::WeReadRefresh,
        );
        push_key_fit(
            &mut spans,
            "q",
            "Quit",
            Some("Quit"),
            app.theme.muted,
            app.theme.muted,
            width,
        );
        return spans;
    }

    if app.current_tab == Tab::Overview {
        push_key_fit(
            &mut spans,
            "t",
            if app.overview_mode == OverviewMode::Today {
                "All"
            } else {
                "Today"
            },
            Some(if app.overview_mode == OverviewMode::Today {
                "All"
            } else {
                "Today"
            }),
            Color::Yellow,
            app.theme.muted,
            width,
        );
        if app.overview_mode == OverviewMode::All {
            push_key_fit(
                &mut spans,
                "D/W/M",
                "Chart",
                Some("Chart"),
                app.theme.foreground,
                app.theme.muted,
                width,
            );
        }
    }
    if app.current_tab == Tab::Daily && !app.is_daily_detail_active() {
        push_key_fit(
            &mut spans,
            "d",
            "Day",
            Some("Day"),
            timeline_key_color(app, crate::tui::app::TimelineGranularity::Day),
            app.theme.muted,
            width,
        );
        push_key_fit(
            &mut spans,
            "h",
            "Hour",
            Some("Hr"),
            timeline_key_color(app, crate::tui::app::TimelineGranularity::Hour),
            app.theme.muted,
            width,
        );
    }
    let date_label = if app.current_tab == Tab::Models {
        "Name"
    } else if app.current_tab == Tab::Overview && app.overview_mode == OverviewMode::Today {
        "Last"
    } else {
        "Date"
    };
    if app.current_tab != Tab::Daily {
        push_sort_key_fit(
            &mut spans,
            app,
            x,
            y,
            width,
            "d",
            date_label,
            Some(date_label),
            SortField::Date,
        );
    }
    push_sort_key_fit(
        &mut spans,
        app,
        x,
        y,
        width,
        "c",
        "Cost",
        Some("Cost"),
        SortField::Cost,
    );
    push_sort_key_fit(
        &mut spans,
        app,
        x,
        y,
        width,
        if app.current_tab == Tab::Overview {
            "T"
        } else {
            "t"
        },
        "Tokens",
        Some("Tok"),
        SortField::Tokens,
    );
    push_key_fit(
        &mut spans,
        "s",
        "Sources",
        Some("Src"),
        Color::Cyan,
        app.theme.muted,
        width,
    );
    push_key_fit(
        &mut spans,
        "r",
        "Refresh",
        Some("Reload"),
        Color::Yellow,
        app.theme.muted,
        width,
    );
    push_key_fit(
        &mut spans,
        "R",
        "Auto",
        Some("Auto"),
        if app.auto_refresh {
            Color::Green
        } else {
            Color::Blue
        },
        app.theme.muted,
        width,
    );
    push_key_fit(
        &mut spans,
        "q",
        "Quit",
        Some("Quit"),
        app.theme.muted,
        app.theme.muted,
        width,
    );
    spans
}

fn timeline_key_color(app: &App, granularity: crate::tui::app::TimelineGranularity) -> Color {
    if app.timeline_granularity == granularity {
        app.theme.foreground
    } else {
        Color::Blue
    }
}

fn push_sort_key_fit(
    spans: &mut Vec<Span<'static>>,
    app: &mut App,
    x: u16,
    y: u16,
    available_width: u16,
    key: &'static str,
    label: &'static str,
    compact_label: Option<&'static str>,
    field: SortField,
) {
    let color = if app.sort_field == field {
        app.theme.foreground
    } else {
        Color::Blue
    };
    let Some((display_label, display_width)) =
        fitting_hint(spans, key, label, compact_label, available_width)
    else {
        return;
    };
    let start = Line::from(spans.clone()).width() as u16;
    push_key(spans, key, display_label, color, app.theme.muted);
    app.add_click_area(
        Rect::new(x.saturating_add(start), y, display_width, 1),
        ClickAction::Sort(field),
    );
}

fn push_action_key_fit(
    spans: &mut Vec<Span<'static>>,
    app: &mut App,
    x: u16,
    y: u16,
    available_width: u16,
    key: &'static str,
    label: &'static str,
    compact_label: Option<&'static str>,
    key_color: Color,
    action: ClickAction,
) {
    let Some((display_label, display_width)) =
        fitting_hint(spans, key, label, compact_label, available_width)
    else {
        return;
    };
    let start = Line::from(spans.clone()).width() as u16;
    push_key(spans, key, display_label, key_color, app.theme.muted);
    app.add_click_area(
        Rect::new(x.saturating_add(start), y, display_width, 1),
        action,
    );
}

fn push_key_fit(
    spans: &mut Vec<Span<'static>>,
    key: &'static str,
    label: &'static str,
    compact_label: Option<&'static str>,
    key_color: Color,
    text_color: Color,
    available_width: u16,
) -> bool {
    let Some((display_label, _)) = fitting_hint(spans, key, label, compact_label, available_width)
    else {
        return false;
    };
    push_key(spans, key, display_label, key_color, text_color);
    true
}

fn fitting_hint(
    spans: &[Span<'static>],
    key: &'static str,
    label: &'static str,
    compact_label: Option<&'static str>,
    available_width: u16,
) -> Option<(&'static str, u16)> {
    let used = Line::from(spans.to_vec()).width() as u16;
    let candidates = if available_width < COMPACT_HINT_WIDTH {
        [compact_label, Some(label)]
    } else {
        [Some(label), compact_label]
    };
    for candidate in candidates.into_iter().flatten() {
        let width = hint_width(!spans.is_empty(), key, candidate);
        if used.saturating_add(width) <= available_width {
            return Some((candidate, width));
        }
    }
    None
}

fn hint_width(has_prefix: bool, key: &'static str, label: &'static str) -> u16 {
    let mut spans = Vec::new();
    if has_prefix {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::raw(format!(" {key} ")));
    spans.push(Span::raw(format!(" {label}")));
    Line::from(spans).width() as u16
}

fn push_key(
    spans: &mut Vec<Span<'static>>,
    key: &'static str,
    label: &'static str,
    key_color: Color,
    text_color: Color,
) {
    if !spans.is_empty() {
        spans.push(Span::styled("  ", Style::default().fg(text_color)));
    }
    spans.push(Span::styled(
        format!(" {key} "),
        Style::default().fg(key_color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!(" {label}"),
        Style::default().fg(text_color),
    ));
}

fn render_scope_summary(frame: &mut Frame, app: &App, area: Rect) {
    let line = if app.current_tab == Tab::Usage {
        usage_summary_line(app, area.width)
    } else if app.current_tab == Tab::Pulse {
        pulse_summary_line(app, area.width)
    } else {
        scope_summary_line(app, area.width)
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
}

fn scope_summary_line(app: &App, width: u16) -> Line<'static> {
    let (total_tokens, total_cost, _) = if app.current_tab == Tab::Overview {
        app.overview_totals()
    } else {
        (
            app.data.total_tokens,
            app.data.total_cost,
            app.data.models.len(),
        )
    };
    let scope = match (app.current_tab, app.overview_mode) {
        (Tab::Overview, OverviewMode::Today) => "Today",
        _ => "All Time",
    };
    let auto_label = if app.auto_refresh {
        format!("Auto {}s", app.auto_refresh_interval.as_secs())
    } else {
        "Auto off".to_string()
    };
    let scope_prefix = if width >= 46 { "Range: " } else { "" };
    let auto_field = vec![Span::styled(auto_label, app.theme.subtle_text_style())];
    let scope_field = vec![
        Span::styled(scope_prefix, app.theme.subtle_text_style()),
        Span::styled(
            scope.to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let totals_field = vec![
        Span::styled(
            format_tokens(total_tokens),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" ", app.theme.subtle_text_style()),
        Span::styled(
            format_cost(total_cost),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let count_field = vec![Span::styled(
        current_count_label(app).trim_start().to_string(),
        app.theme.subtle_text_style(),
    )];
    let fields = if width < SUMMARY_SCOPE_FIRST_WIDTH {
        vec![scope_field, auto_field, totals_field, count_field]
    } else {
        vec![auto_field, scope_field, totals_field, count_field]
    };

    fit_summary_fields(
        fields,
        Span::styled(" | ", app.theme.subtle_text_style()),
        width as usize,
    )
}

fn usage_summary_line(app: &App, width: u16) -> Line<'static> {
    let provider_count = app
        .subscription_usage
        .iter()
        .map(|output| output.provider.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let account_count = app
        .subscription_usage
        .iter()
        .filter(|output| output.account.is_some())
        .count();
    let managed_count = app.subscription_usage.len().saturating_sub(account_count);
    let metric_count: usize = app
        .subscription_usage
        .iter()
        .map(|output| output.metrics.len())
        .sum();

    let (status, color) = if app.is_fetching_usage() {
        ("Syncing", Color::Yellow)
    } else if app.is_codex_login_running() {
        ("Codex login", Color::Yellow)
    } else if provider_count > 0 {
        ("Loaded", Color::Green)
    } else if app.usage_fetch_attempted {
        ("No data", app.theme.muted)
    } else {
        ("Not loaded", app.theme.muted)
    };

    let fields = vec![
        vec![
            Span::styled("Usage: ", app.theme.subtle_text_style()),
            Span::styled(status.to_string(), Style::default().fg(color)),
        ],
        vec![Span::styled(
            format!("{provider_count} providers"),
            app.theme.subtle_text_style(),
        )],
        vec![Span::styled(
            identity_count_label(account_count, managed_count),
            app.theme.subtle_text_style(),
        )],
        vec![Span::styled(
            format!("{metric_count} limits"),
            app.theme.subtle_text_style(),
        )],
    ];

    fit_summary_fields(
        fields,
        Span::styled("  |  ", app.theme.subtle_text_style()),
        width as usize,
    )
}

fn pulse_summary_line(app: &App, width: u16) -> Line<'static> {
    let status = if app.is_fetching_weread() {
        "syncing"
    } else {
        app.pulse.weread.status.label()
    };
    let status_color = if app.is_fetching_weread() {
        Color::Yellow
    } else {
        match app.pulse.weread.status {
            WeReadStatus::Fresh => Color::Green,
            WeReadStatus::Loading => Color::Yellow,
            WeReadStatus::Stale => Color::Yellow,
            WeReadStatus::AuthMissing | WeReadStatus::Error | WeReadStatus::UpgradeRequired => {
                app.theme.muted
            }
        }
    };
    let week = app
        .pulse
        .weread
        .weekly
        .as_ref()
        .map(|weekly| {
            format!(
                "{}/7 · {}",
                weekly.read_days,
                format_read_duration(weekly.total_seconds)
            )
        })
        .unwrap_or_else(|| "no reading data".to_string());
    let notes = app
        .pulse
        .weread
        .notes
        .as_ref()
        .map(|notes| format!("{} notes", notes.total_notes))
        .unwrap_or_else(|| "notes n/a".to_string());

    let fields = vec![
        vec![
            Span::styled("WeRead: ", app.theme.subtle_text_style()),
            Span::styled(
                status.to_string(),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ],
        vec![Span::styled(week, app.theme.subtle_text_style())],
        vec![Span::styled(notes, app.theme.subtle_text_style())],
    ];

    fit_summary_fields(
        fields,
        Span::styled("  |  ", app.theme.subtle_text_style()),
        width as usize,
    )
}

fn fit_summary_fields(
    fields: Vec<Vec<Span<'static>>>,
    separator: Span<'static>,
    max_width: usize,
) -> Line<'static> {
    let separator_width = Line::from(vec![separator.clone()]).width();
    let mut spans = Vec::new();
    let mut used_width = 0usize;

    for field in fields {
        let field_width = Line::from(field.clone()).width();
        let needed_width = field_width + if spans.is_empty() { 0 } else { separator_width };

        if used_width + needed_width <= max_width {
            if !spans.is_empty() {
                spans.push(separator.clone());
                used_width += separator_width;
            }
            spans.extend(field);
            used_width += field_width;
        }
    }

    Line::from(spans)
}

fn identity_count_label(saved: usize, managed: usize) -> String {
    match (saved, managed) {
        (0, 0) => "0 saved".to_string(),
        (saved, 0) => format!("{saved} saved"),
        (0, managed) => format!("{managed} managed"),
        (saved, managed) => format!("{saved} saved · {managed} managed"),
    }
}

fn summary_width(app: &App, available_width: u16) -> u16 {
    let preferred = if app.is_drilldown_active() {
        if app.is_narrow() {
            34
        } else {
            50
        }
    } else if app.current_tab == Tab::Usage {
        if app.is_narrow() {
            42
        } else {
            70
        }
    } else if app.current_tab == Tab::Pulse {
        if app.is_narrow() {
            42
        } else {
            64
        }
    } else if app.is_narrow() {
        38
    } else {
        60
    };

    if available_width <= MIN_ACTION_HINT_WIDTH.saturating_add(MIN_SUMMARY_WIDTH) {
        return available_width;
    }

    preferred.min(available_width.saturating_sub(MIN_ACTION_HINT_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::{UsageAccount, UsageMetric, UsageOutput};
    use crate::tui::app::{ModelDetailKey, PeriodDetailKey, TuiConfig};
    use crate::tui::data::UsageData;
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};

    fn make_app_on(tab: Tab) -> App {
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
        app.current_tab = tab;
        app
    }

    fn usage_output(provider: &str, account: Option<UsageAccount>) -> UsageOutput {
        UsageOutput {
            provider: provider.to_string(),
            account,
            plan: Some("Pro".to_string()),
            email: None,
            metrics: vec![UsageMetric {
                label: "Session".to_string(),
                used_percent: 10.0,
                remaining_percent: 90.0,
                remaining_label: Some("90% left".to_string()),
                resets_at: None,
            }],
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn line_width(line: Line<'_>) -> usize {
        line.width()
    }

    fn render_footer_text(app: &mut App, width: u16) -> String {
        let backend = TestBackend::new(width, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, app, Rect::new(0, 0, width, 3)))
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

    #[test]
    fn test_current_count_label_matches_active_tab() {
        assert_eq!(
            current_count_label(&make_app_on(Tab::Models)),
            " (0 models)"
        );
        assert_eq!(
            current_count_label(&make_app_on(Tab::Agents)),
            " (0 agents)"
        );
        assert_eq!(current_count_label(&make_app_on(Tab::Daily)), " (0 days)");
        assert_eq!(current_count_label(&make_app_on(Tab::Hourly)), " (0 hours)");
        assert_eq!(current_count_label(&make_app_on(Tab::Stats)), "");
    }

    #[test]
    fn test_current_count_label_minutely_when_flag_enabled() {
        let mut app = make_app_on(Tab::Models);
        app.settings.minutely_tab_enabled = true;
        app.current_tab = Tab::Minutely;
        assert_eq!(current_count_label(&app), " (0 minutes)");
    }

    #[test]
    fn usage_summary_drops_fields_without_orphan_separator() {
        let mut app = make_app_on(Tab::Usage);
        app.subscription_usage = vec![
            usage_output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_a".to_string(),
                    label: Some("a".to_string()),
                    is_active: true,
                }),
            ),
            usage_output("Copilot", None),
        ];

        let compact = line_text(&usage_summary_line(&app, 44));
        assert!(compact.contains("Usage: Loaded"), "{compact}");
        assert!(compact.contains("2 providers"), "{compact}");
        assert!(!compact.ends_with("  |  "), "{compact}");

        let wide = line_text(&usage_summary_line(&app, 70));
        assert!(wide.contains("1 saved · 1 managed"), "{wide}");
        assert!(wide.contains("2 limits"), "{wide}");
    }

    #[test]
    fn usage_summary_distinguishes_not_loaded_from_empty_results() {
        let mut app = make_app_on(Tab::Usage);

        let not_loaded = line_text(&usage_summary_line(&app, 70));
        assert!(not_loaded.contains("Usage: Not loaded"), "{not_loaded}");

        app.usage_fetch_attempted = true;
        let no_data = line_text(&usage_summary_line(&app, 70));
        assert!(no_data.contains("Usage: No data"), "{no_data}");
    }

    #[test]
    fn scope_summary_drops_fields_without_truncating_labels() {
        let mut app = make_app_on(Tab::Models);
        app.data.total_tokens = 2_200_000_000;
        app.data.total_cost = 1034.56;

        let compact = scope_summary_line(&app, 20);
        let compact_text = line_text(&compact);
        assert!(compact.width() <= 20, "{compact_text}");
        assert!(
            !compact_text.contains("Range: All T"),
            "summary should drop whole fields instead of clipping labels: {compact_text}"
        );

        let wide = line_text(&scope_summary_line(&app, 60));
        assert!(wide.contains("Auto off"), "{wide}");
        assert!(wide.contains("All Time"), "{wide}");
        assert!(wide.contains("$"), "{wide}");
    }

    #[test]
    fn narrow_footer_keeps_action_hint_and_scope_summary() {
        let mut app = make_app_on(Tab::Overview);
        let body = render_footer_text(&mut app, 28);

        assert!(body.contains("Nav"), "{body}");
        assert!(body.contains("All Time"), "{body}");
    }

    #[test]
    fn action_hints_fit_available_width() {
        let cases = [
            (Tab::Overview, 32),
            (Tab::Overview, 48),
            (Tab::Usage, 32),
            (Tab::Usage, 56),
            (Tab::Models, 40),
        ];

        for (tab, width) in cases {
            let mut app = make_app_on(tab);
            let spans = action_spans(&mut app, 0, 0, width);
            let rendered_width = line_width(Line::from(spans));
            assert!(
                rendered_width <= width as usize,
                "{tab:?} hints used {rendered_width} cols in {width} cols"
            );
        }

        let mut app = make_app_on(Tab::Models);
        app.open_model_detail(ModelDetailKey {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            color_key: "gpt-5".to_string(),
        });
        for width in [28, 40, 52] {
            let spans = action_spans(&mut app, 0, 0, width);
            let rendered_width = line_width(Line::from(spans));
            assert!(
                rendered_width <= width as usize,
                "drilldown hints used {rendered_width} cols in {width} cols"
            );
        }
    }

    #[test]
    fn drilldown_hints_scope_sort_keys_to_detail_type() {
        let mut model_app = make_app_on(Tab::Daily);
        model_app.open_model_detail(ModelDetailKey {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            color_key: "gpt-5".to_string(),
        });
        let model_hints = line_text(&Line::from(action_spans(&mut model_app, 0, 0, 96)));

        assert!(model_hints.contains(" c  Cost"), "{model_hints}");
        assert!(model_hints.contains(" t  Tok"), "{model_hints}");
        assert!(model_hints.contains(" d  Date"), "{model_hints}");

        let mut period_app = make_app_on(Tab::Daily);
        period_app.open_period_detail(PeriodDetailKey::day(
            NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
        ));
        let period_hints = line_text(&Line::from(action_spans(&mut period_app, 0, 0, 96)));

        assert!(period_hints.contains(" c  Cost"), "{period_hints}");
        assert!(period_hints.contains(" t  Tok"), "{period_hints}");
        assert!(!period_hints.contains(" d  Date"), "{period_hints}");
    }

    #[test]
    fn drilldown_sort_hints_register_click_areas() {
        let mut app = make_app_on(Tab::Daily);
        app.open_model_detail(ModelDetailKey {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            color_key: "gpt-5".to_string(),
        });

        let _ = action_spans(&mut app, 5, 1, 96);

        assert!(
            app.click_areas
                .iter()
                .any(|area| matches!(area.action, ClickAction::Sort(SortField::Cost))),
            "missing cost sort click area"
        );
        assert!(
            app.click_areas
                .iter()
                .any(|area| matches!(area.action, ClickAction::Sort(SortField::Tokens))),
            "missing tokens sort click area"
        );
        assert!(
            app.click_areas
                .iter()
                .any(|area| matches!(area.action, ClickAction::Sort(SortField::Date))),
            "missing date sort click area"
        );
    }

    #[test]
    fn drilldown_footer_keeps_cost_and_token_sort_visible_on_wide_panes() {
        let mut app = make_app_on(Tab::Daily);
        app.open_period_detail(PeriodDetailKey::day(
            NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
        ));

        let body = render_footer_text(&mut app, 109);

        assert!(app.status_message.is_none());
        assert!(body.contains(" c  Cost"), "{body}");
        assert!(body.contains(" t  Tok"), "{body}");
    }

    #[test]
    fn action_hints_prefer_compact_labels_on_narrow_widths() {
        let mut app = make_app_on(Tab::Overview);
        let compact = line_text(&Line::from(action_spans(&mut app, 0, 0, 48)));

        assert!(compact.contains("Nav"), "{compact}");
        assert!(compact.contains("Ws"), "{compact}");
        assert!(!compact.contains("Navigate"), "{compact}");
        assert!(!compact.contains("Workspace"), "{compact}");

        let mut app = make_app_on(Tab::Overview);
        let wide = line_text(&Line::from(action_spans(&mut app, 0, 0, 80)));

        assert!(wide.contains("Navigate"), "{wide}");
        assert!(wide.contains("Workspace"), "{wide}");
    }

    #[test]
    fn today_overview_footer_uses_today_specific_hints() {
        let mut app = make_app_on(Tab::Overview);
        app.overview_mode = OverviewMode::Today;

        let hints = line_text(&Line::from(action_spans(&mut app, 0, 0, 120)));

        assert!(hints.contains(" t  All"), "{hints}");
        assert!(hints.contains(" d  Last"), "{hints}");
        assert!(!hints.contains("D/W/M"), "{hints}");
        assert!(!hints.contains("Chart"), "{hints}");
    }

    #[test]
    fn status_message_fits_available_width() {
        let mut app = make_app_on(Tab::Overview);
        app.status_message = Some("Usage emails visible".to_string());

        for width in [0, 1, 4, 8, 16] {
            let line = Line::from(status_spans(&app, width));
            assert!(
                line.width() <= width as usize,
                "status used {} cols in {width}: {}",
                line.width(),
                line_text(&line)
            );
        }
    }

    #[test]
    fn background_refresh_uses_compact_status_on_narrow_width() {
        assert_eq!(refreshing_status_text(8), "Refresh");
        assert_eq!(refreshing_status_text(10), "Refreshing");
        assert_eq!(refreshing_status_text(24), "Refreshing data...");
        assert_eq!(
            refreshing_status_text(40),
            "Refreshing cached data in background..."
        );
    }

    #[test]
    fn background_refresh_keeps_action_hints_when_data_is_visible() {
        let mut app = make_app_on(Tab::Overview);
        app.background_loading = true;
        app.data.total_tokens = 42;

        let body = render_footer_text(&mut app, 120);

        assert!(body.contains("Nav") || body.contains("Navigate"), "{body}");
        assert!(body.contains("Ws") || body.contains("Workspace"), "{body}");
        assert!(
            !body.contains("Refreshing cached data in background"),
            "background refresh should not replace action hints when data is visible\n{body}"
        );
    }

    #[test]
    fn background_refresh_uses_status_when_no_data_is_visible() {
        let mut app = make_app_on(Tab::Overview);
        app.background_loading = true;

        let body = render_footer_text(&mut app, 120);

        assert!(
            body.contains("Scanning") || body.contains("Loading"),
            "{body}"
        );
        assert!(!body.contains("Navigate"), "{body}");
    }

    #[test]
    fn loading_status_drops_scanner_on_tiny_width() {
        let mut app = make_app_on(Tab::Overview);
        app.data.loading = true;

        for width in [1, 4, 8] {
            let line = Line::from(status_spans(&app, width));
            assert!(
                line.width() <= width as usize,
                "loading status used {} cols in {width}: {}",
                line.width(),
                line_text(&line)
            );
            assert!(
                !line_text(&line).contains("parsing"),
                "{}",
                line_text(&line)
            );
        }
    }
}
