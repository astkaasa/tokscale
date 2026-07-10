use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tokscale_core::pulse::weread::{format_read_duration, WeReadStatus};

use super::spinner::{get_phase_message, get_scanner_spans};
use super::widgets::{format_cost, format_tokens};
use crate::tui::app::{
    App, ClickAction, DrilldownView, OverviewMode, SortField, Tab, TimelineGranularity,
};

const COMPACT_HINT_WIDTH: u16 = 64;
const SUMMARY_SCOPE_FIRST_WIDTH: u16 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HintDensity {
    Full,
    Compact,
    Tiny,
}

impl HintDensity {
    fn for_width(width: u16) -> Self {
        if width < COMPACT_HINT_WIDTH {
            Self::Compact
        } else {
            Self::Full
        }
    }
}

#[derive(Clone)]
struct ActionHint {
    key: &'static str,
    compact_key: Option<&'static str>,
    label: &'static str,
    compact_label: Option<&'static str>,
    tiny_label: Option<&'static str>,
    key_color: Color,
    text_color: Color,
    action: Option<ClickAction>,
}

impl ActionHint {
    fn plain(
        key: &'static str,
        label: &'static str,
        compact_label: Option<&'static str>,
        key_color: Color,
        text_color: Color,
    ) -> Self {
        Self {
            key,
            compact_key: None,
            label,
            compact_label,
            tiny_label: None,
            key_color,
            text_color,
            action: None,
        }
    }

    fn with_compact_key(mut self, compact_key: &'static str) -> Self {
        self.compact_key = Some(compact_key);
        self
    }

    fn with_tiny_label(mut self, tiny_label: &'static str) -> Self {
        self.tiny_label = Some(tiny_label);
        self
    }

    fn with_action(mut self, action: ClickAction) -> Self {
        self.action = Some(action);
        self
    }

    fn display(&self, density: HintDensity) -> (&'static str, &'static str) {
        match density {
            HintDensity::Full => (self.key, self.label),
            HintDensity::Compact => (
                self.compact_key.unwrap_or(self.key),
                self.compact_label.unwrap_or(self.label),
            ),
            HintDensity::Tiny => (
                self.compact_key.unwrap_or(self.key),
                self.tiny_label.or(self.compact_label).unwrap_or(self.label),
            ),
        }
    }
}

#[derive(Default)]
struct ActionHintGroups {
    primary: Vec<ActionHint>,
    common: Vec<ActionHint>,
    secondary: Vec<ActionHint>,
}

#[derive(Clone, Copy)]
struct ActionLayout {
    primary_density: HintDensity,
    common_density: HintDensity,
    priority_width: u16,
}

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

    let action_layout = action_layout(app, inner.width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(summary_width(
                app,
                inner.width,
                action_layout.priority_width,
            )),
        ])
        .split(Rect::new(inner.x, inner.y, inner.width, 1));

    render_action_or_status(frame, app, chunks[0], action_layout);
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
        Tab::Timeline if app.is_daily_detail_active() => {
            format!(" ({} models)", app.get_sorted_daily_detail_rows().len())
        }
        Tab::Timeline => match app.timeline_granularity {
            crate::tui::app::TimelineGranularity::Day => {
                format!(" ({} days)", app.data.daily.len())
            }
            crate::tui::app::TimelineGranularity::Hour => {
                format!(" ({} hours)", app.data.hourly.len())
            }
        },
        Tab::Usage => String::new(),
    }
}

fn render_action_or_status(frame: &mut Frame, app: &mut App, area: Rect, layout: ActionLayout) {
    let should_show_status = app.data.loading
        || app.status_message.is_some()
        || (app.background_loading && !app.has_visible_data());
    let spans = if should_show_status {
        status_spans(app, area.width)
    } else {
        let mut spans = action_spans_with_layout(app, area, layout);
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
    let candidates = if app.current_tab == Tab::Usage {
        if available_width < COMPACT_HINT_WIDTH {
            ["Scan", "Data scan"]
        } else {
            ["Data scan", "Scanning"]
        }
    } else if available_width < COMPACT_HINT_WIDTH {
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

#[cfg(test)]
fn action_spans(app: &mut App, x: u16, y: u16, width: u16) -> Vec<Span<'static>> {
    let density = HintDensity::for_width(width);
    action_spans_with_layout(
        app,
        Rect::new(x, y, width, 1),
        ActionLayout {
            primary_density: density,
            common_density: density,
            priority_width: width,
        },
    )
}

fn action_spans_with_layout(app: &mut App, area: Rect, layout: ActionLayout) -> Vec<Span<'static>> {
    let groups = action_hint_groups(app);
    let secondary_density = HintDensity::for_width(area.width);
    let mut spans = Vec::new();

    for hint in groups.primary {
        push_action_hint_fit(&mut spans, app, area, hint, layout.primary_density);
    }
    for hint in groups.common {
        push_action_hint_fit(&mut spans, app, area, hint, layout.common_density);
    }
    for hint in groups.secondary {
        push_action_hint_fit(&mut spans, app, area, hint, secondary_density);
    }

    spans
}

fn action_hint_groups(app: &App) -> ActionHintGroups {
    let mut groups = ActionHintGroups::default();

    if app.is_drilldown_active() {
        if app.drilldown_list_len() > 0 {
            groups.primary.push(details_hint(app));
        }
        groups.primary.push(ActionHint::plain(
            "Esc",
            "Back",
            Some("Back"),
            Color::Yellow,
            app.theme.muted,
        ));
        groups
            .primary
            .push(sort_hint(app, "c", "Cost", Some("Cost"), SortField::Cost));
        groups.primary.push(sort_hint(
            app,
            "t",
            "Tokens",
            Some("Tok"),
            SortField::Tokens,
        ));
        if matches!(app.drilldown_view(), Some(DrilldownView::Model(_))) {
            groups
                .primary
                .push(sort_hint(app, "d", "Date", Some("Date"), SortField::Date));
        }
        if app.drilldown_list_len() > 0 {
            groups.common.push(ActionHint::plain(
                "↑↓",
                "Rows",
                Some("Row"),
                Color::White,
                app.theme.muted,
            ));
        }
        push_common_workspace_hints(&mut groups.common, app);
        return groups;
    }

    match app.current_tab {
        Tab::Overview => {
            groups.primary.push(ActionHint::plain(
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
            ));
            if app.overview_model_len() > 0 {
                groups.primary.push(details_hint(app));
            }
            groups.primary.push(refresh_hint(app));
        }
        Tab::Models => {
            if !app.data.models.is_empty() {
                groups.primary.push(details_hint(app));
            }
            groups.primary.push(refresh_hint(app));
        }
        Tab::Timeline => {
            if app.timeline_granularity == TimelineGranularity::Day && !app.data.daily.is_empty() {
                groups.primary.push(details_hint(app));
            }
            groups.primary.push(refresh_hint(app));
        }
        Tab::Usage => push_usage_primary_hints(&mut groups.primary, app),
        Tab::Pulse => groups.primary.push(
            ActionHint::plain(
                "r",
                if app.is_fetching_weread() {
                    "Syncing"
                } else {
                    "Sync WeRead"
                },
                Some("Sync"),
                if app.is_fetching_weread() {
                    app.theme.muted
                } else {
                    Color::Yellow
                },
                app.theme.muted,
            )
            .with_action(ClickAction::WeReadRefresh),
        ),
    }

    if app.current_tab != Tab::Pulse {
        groups.common.push(ActionHint::plain(
            "↑↓",
            "Navigate",
            Some("Nav"),
            Color::White,
            app.theme.muted,
        ));
    }
    push_common_workspace_hints(&mut groups.common, app);

    match app.current_tab {
        Tab::Overview | Tab::Models | Tab::Timeline => {
            push_analysis_secondary_hints(&mut groups.secondary, app)
        }
        Tab::Usage | Tab::Pulse => {
            groups.secondary.push(auto_refresh_hint(app));
            groups.secondary.push(quit_hint(app));
        }
    }

    groups
}

fn details_hint(app: &App) -> ActionHint {
    ActionHint::plain(
        "Enter",
        "Details",
        Some("Details"),
        app.theme.accent,
        app.theme.muted,
    )
    .with_compact_key("↵")
}

fn refresh_hint(app: &App) -> ActionHint {
    ActionHint::plain(
        "r",
        "Refresh",
        Some("Refresh"),
        Color::Yellow,
        app.theme.muted,
    )
    .with_tiny_label("Ref")
}

fn push_usage_primary_hints(hints: &mut Vec<ActionHint>, app: &App) {
    hints.push(
        ActionHint::plain(
            "r",
            if app.is_fetching_usage() {
                "Syncing"
            } else {
                "Refresh"
            },
            Some(if app.is_fetching_usage() {
                "Sync"
            } else {
                "Refresh"
            }),
            if app.is_fetching_usage() {
                app.theme.muted
            } else {
                Color::Yellow
            },
            app.theme.muted,
        )
        .with_action(ClickAction::UsageRefresh),
    );
    hints.push(
        ActionHint::plain(
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
            app.theme.muted,
        )
        .with_action(ClickAction::CodexStartLogin),
    );
    hints.push(
        ActionHint::plain(
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
            app.theme.muted,
        )
        .with_action(ClickAction::UsageToggleEmailPrivacy),
    );
    if let Some(action) = selected_usage_use_action(app) {
        hints.push(
            ActionHint::plain("u", "Use", Some("Use"), app.theme.accent, app.theme.muted)
                .with_action(action),
        );
    }
    if let Some(action) = selected_usage_reset_action(app) {
        hints.push(
            ActionHint::plain("x", "Reset", Some("Reset"), Color::Yellow, app.theme.muted)
                .with_action(action),
        );
    }
    if let Some(action) = selected_usage_remove_action(app) {
        hints.push(
            ActionHint::plain("Del", "Remove", Some("Rm"), Color::Red, app.theme.muted)
                .with_action(action),
        );
    }
}

fn push_common_workspace_hints(hints: &mut Vec<ActionHint>, app: &App) {
    hints.push(ActionHint::plain(
        "←→",
        "Workspace",
        Some("Ws"),
        Color::White,
        app.theme.muted,
    ));
    hints.push(ActionHint::plain(
        "p",
        "Theme",
        Some("Theme"),
        app.theme.accent,
        app.theme.muted,
    ));
}

fn push_analysis_secondary_hints(hints: &mut Vec<ActionHint>, app: &App) {
    if app.current_tab == Tab::Overview && app.overview_mode == OverviewMode::All {
        hints.push(ActionHint::plain(
            "D/W/M",
            "Chart",
            Some("Chart"),
            app.theme.foreground,
            app.theme.muted,
        ));
        if app.chart_granularity != crate::tui::app::ChartGranularity::Daily {
            hints.push(ActionHint::plain(
                "⇧←→",
                "Scroll",
                Some("Scr"),
                Color::White,
                app.theme.muted,
            ));
        }
    }
    if app.current_tab == Tab::Timeline {
        hints.push(ActionHint::plain(
            "d",
            "Day",
            Some("Day"),
            timeline_key_color(app, TimelineGranularity::Day),
            app.theme.muted,
        ));
        hints.push(ActionHint::plain(
            "h",
            "Hour",
            Some("Hr"),
            timeline_key_color(app, TimelineGranularity::Hour),
            app.theme.muted,
        ));
    }

    let date_label = if app.current_tab == Tab::Models {
        "Name"
    } else if app.current_tab == Tab::Overview && app.overview_mode == OverviewMode::Today {
        "Last"
    } else {
        "Date"
    };
    if app.current_tab != Tab::Timeline {
        hints.push(sort_hint(
            app,
            "d",
            date_label,
            Some(date_label),
            SortField::Date,
        ));
    }
    hints.push(sort_hint(app, "c", "Cost", Some("Cost"), SortField::Cost));
    hints.push(sort_hint(
        app,
        if app.current_tab == Tab::Overview {
            "T"
        } else {
            "t"
        },
        "Tokens",
        Some("Tok"),
        SortField::Tokens,
    ));
    hints.push(ActionHint::plain(
        "s",
        "Sources",
        Some("Src"),
        Color::Cyan,
        app.theme.muted,
    ));
    hints.push(auto_refresh_hint(app));
    hints.push(quit_hint(app));
}

fn sort_hint(
    app: &App,
    key: &'static str,
    label: &'static str,
    compact_label: Option<&'static str>,
    field: SortField,
) -> ActionHint {
    ActionHint::plain(
        key,
        label,
        compact_label,
        if app.sort_field == field {
            app.theme.foreground
        } else {
            Color::Blue
        },
        app.theme.muted,
    )
    .with_action(ClickAction::Sort(field))
}

fn auto_refresh_hint(app: &App) -> ActionHint {
    ActionHint::plain(
        "R",
        "Auto",
        Some("Auto"),
        if app.auto_refresh {
            Color::Green
        } else {
            Color::Blue
        },
        app.theme.muted,
    )
}

fn quit_hint(app: &App) -> ActionHint {
    ActionHint::plain("q", "Quit", Some("Quit"), app.theme.muted, app.theme.muted)
}

fn timeline_key_color(app: &App, granularity: TimelineGranularity) -> Color {
    if app.timeline_granularity == granularity {
        app.theme.foreground
    } else {
        Color::Blue
    }
}

fn push_action_hint_fit(
    spans: &mut Vec<Span<'static>>,
    app: &mut App,
    area: Rect,
    hint: ActionHint,
    density: HintDensity,
) {
    let Some((key, label, display_width)) = fitting_action_hint(spans, &hint, density, area.width)
    else {
        return;
    };
    let start = Line::from(spans.clone()).width() as u16;
    push_key(spans, key, label, hint.key_color, hint.text_color);
    if let Some(action) = hint.action {
        app.add_click_area(
            Rect::new(area.x.saturating_add(start), area.y, display_width, 1),
            action,
        );
    }
}

fn fitting_action_hint(
    spans: &[Span<'static>],
    hint: &ActionHint,
    density: HintDensity,
    available_width: u16,
) -> Option<(&'static str, &'static str, u16)> {
    let used = Line::from(spans.to_vec()).width() as u16;
    let fallback_density = match density {
        HintDensity::Full => HintDensity::Compact,
        HintDensity::Compact => HintDensity::Full,
        HintDensity::Tiny => HintDensity::Compact,
    };
    let candidates = [hint.display(density), hint.display(fallback_density)];

    for (key, label) in candidates {
        let width = hint_width(!spans.is_empty(), key, label);
        if used.saturating_add(width) <= available_width {
            return Some((key, label, width));
        }
    }
    None
}

fn action_layout(app: &App, available_width: u16) -> ActionLayout {
    let groups = action_hint_groups(app);
    let density_pairs = [
        (HintDensity::Full, HintDensity::Full),
        (HintDensity::Full, HintDensity::Compact),
        (HintDensity::Compact, HintDensity::Compact),
    ];

    for (primary_density, common_density) in density_pairs {
        let priority_width = priority_hint_width(&groups, primary_density, common_density);
        if priority_width <= available_width {
            return ActionLayout {
                primary_density,
                common_density,
                priority_width,
            };
        }
    }

    ActionLayout {
        primary_density: if available_width < 40 {
            HintDensity::Tiny
        } else {
            HintDensity::Compact
        },
        common_density: HintDensity::Compact,
        priority_width: available_width,
    }
}

fn priority_hint_width(
    groups: &ActionHintGroups,
    primary_density: HintDensity,
    common_density: HintDensity,
) -> u16 {
    let mut width = 0u16;
    let mut has_prefix = false;

    for (hints, density) in [
        (groups.primary.as_slice(), primary_density),
        (groups.common.as_slice(), common_density),
    ] {
        for hint in hints {
            let (key, label) = hint.display(density);
            width = width.saturating_add(hint_width(has_prefix, key, label));
            has_prefix = true;
        }
    }

    width
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

fn auto_refresh_label(app: &App) -> String {
    if app.auto_refresh {
        format!("Auto {}s", app.auto_refresh_interval.as_secs())
    } else {
        "Auto off".to_string()
    }
}

fn auto_refresh_field(app: &App) -> Vec<Span<'static>> {
    vec![Span::styled(
        auto_refresh_label(app),
        app.theme.subtle_text_style(),
    )]
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
        (Tab::Overview, OverviewMode::Today) => "Today".to_string(),
        _ => app.report_scope_label(),
    };
    let scope_prefix = if width >= 46 { "Range: " } else { "" };
    let auto_field = auto_refresh_field(app);
    let scope_field = vec![
        Span::styled(scope_prefix, app.theme.subtle_text_style()),
        Span::styled(
            scope,
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

fn selected_usage_use_action(app: &App) -> Option<ClickAction> {
    let output = app.subscription_usage.get(app.selected_index)?;
    if output.provider != "Codex" {
        return None;
    }

    let account = output.account.as_ref()?;
    if account.is_active {
        return None;
    }

    Some(ClickAction::CodexUseAccount {
        account_id: account.id.clone(),
    })
}

fn selected_usage_reset_action(app: &App) -> Option<ClickAction> {
    let output = app.subscription_usage.get(app.selected_index)?;
    if output.provider != "Codex" {
        return None;
    }

    let available_count = output
        .reset_credits
        .as_ref()
        .map(|credits| credits.available_count)
        .unwrap_or(0);
    if available_count == 0 {
        return None;
    }

    let account_id = output.account.as_ref()?.id.clone();
    Some(ClickAction::CodexResetAccount { account_id })
}

fn selected_usage_remove_action(app: &App) -> Option<ClickAction> {
    let output = app.subscription_usage.get(app.selected_index)?;
    if output.provider != "Codex" {
        return None;
    }

    let account_id = output.account.as_ref()?.id.clone();
    Some(ClickAction::CodexRemoveAccount { account_id })
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
        auto_refresh_field(app),
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
            WeReadStatus::Loading | WeReadStatus::Partial | WeReadStatus::Stale => Color::Yellow,
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

    let mut fields = vec![
        vec![
            Span::styled("WeRead: ", app.theme.subtle_text_style()),
            Span::styled(
                status.to_string(),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ],
        auto_refresh_field(app),
        vec![Span::styled(week, app.theme.subtle_text_style())],
        vec![Span::styled(notes, app.theme.subtle_text_style())],
    ];
    if let Some(ai) = app.pulse.snapshot.as_ref().map(|snapshot| &snapshot.ai) {
        if let (Some(tokens), Some(cost)) = (ai.total_tokens, ai.total_cost) {
            fields.insert(
                1,
                vec![
                    Span::styled("AI: ", app.theme.subtle_text_style()),
                    Span::styled(
                        format!("{} · {}", format_tokens(tokens), format_cost(cost)),
                        app.theme.subtle_text_style(),
                    ),
                ],
            );
        }
    }

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

fn summary_width(app: &App, available_width: u16, priority_width: u16) -> u16 {
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

    preferred.min(available_width.saturating_sub(priority_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::{UsageAccount, UsageMetric, UsageOutput, UsageResetCredits};
    use crate::tui::app::{ModelDetailKey, PeriodDetailKey, TuiConfig};
    use crate::tui::data::{DailyUsage, DataLoader, ModelUsage, TokenBreakdown, UsageData};
    use chrono::{Datelike, NaiveDate};
    use ratatui::{backend::TestBackend, Terminal};
    use tokscale_core::ModelPerformance;

    fn make_app_with_scope(
        tab: Tab,
        since: Option<String>,
        until: Option<String>,
        year: Option<String>,
    ) -> App {
        let config = TuiConfig {
            theme: None,
            refresh: 0,
            clients: None,
            since,
            until,
            year,
            initial_tab: None,
            initial_timeline_granularity: None,
        };
        let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();
        app.current_tab = tab;
        app
    }

    fn make_app_on(tab: Tab) -> App {
        make_app_with_scope(tab, None, None, None)
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
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        }
    }

    fn model_usage(name: &str) -> ModelUsage {
        ModelUsage {
            model: name.to_string(),
            provider: "openai".to_string(),
            client: "codex".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: ModelPerformance::default(),
            session_count: 1,
        }
    }

    fn daily_usage(date: NaiveDate) -> DailyUsage {
        DailyUsage {
            date,
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            source_breakdown: std::collections::BTreeMap::new(),
            message_count: 0,
            turn_count: 0,
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
        app.handle_resize(width, 3);
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
            current_count_label(&make_app_on(Tab::Timeline)),
            " (0 days)"
        );
        assert_eq!(current_count_label(&make_app_on(Tab::Usage)), "");
    }

    #[test]
    fn usage_summary_drops_fields_without_orphan_separator() {
        let mut app = make_app_on(Tab::Usage);
        app.auto_refresh = false;
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
        assert!(compact.contains("Auto off"), "{compact}");
        assert!(compact.contains("2 providers"), "{compact}");
        assert!(!compact.ends_with("  |  "), "{compact}");

        let wide = line_text(&usage_summary_line(&app, 92));
        assert!(wide.contains("1 saved · 1 managed"), "{wide}");
        assert!(wide.contains("2 limits"), "{wide}");

        app.auto_refresh = true;
        app.auto_refresh_interval = std::time::Duration::from_secs(90);
        let with_auto = line_text(&usage_summary_line(&app, 92));
        assert!(with_auto.contains("Auto 90s"), "{with_auto}");
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
    fn pulse_summary_includes_auto_refresh_state() {
        let mut app = make_app_on(Tab::Pulse);
        app.auto_refresh = false;

        let off = line_text(&pulse_summary_line(&app, 70));
        assert!(off.contains("WeRead:"), "{off}");
        assert!(off.contains("Auto off"), "{off}");

        app.auto_refresh = true;
        app.auto_refresh_interval = std::time::Duration::from_secs(75);
        let on = line_text(&pulse_summary_line(&app, 70));
        assert!(on.contains("Auto 75s"), "{on}");
    }

    #[test]
    fn scope_summary_drops_fields_without_truncating_labels() {
        let mut app = make_app_on(Tab::Models);
        app.auto_refresh = false;
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
    fn scope_summary_labels_last_seven_days() {
        let today = chrono::Local::now().date_naive();
        let since = today
            .checked_sub_signed(chrono::Duration::days(6))
            .unwrap()
            .to_string();
        let app = make_app_with_scope(Tab::Models, Some(since), Some(today.to_string()), None);

        let summary = line_text(&scope_summary_line(&app, 100));
        assert!(summary.contains("Last 7 days"), "{summary}");
        assert!(!summary.contains("All Time"), "{summary}");
    }

    #[test]
    fn scope_summary_labels_current_month() {
        let today = chrono::Local::now().date_naive();
        let since = today.with_day(1).unwrap().to_string();
        let app = make_app_with_scope(Tab::Models, Some(since), Some(today.to_string()), None);

        let summary = line_text(&scope_summary_line(&app, 100));
        assert!(
            summary.contains(&today.format("%B %Y").to_string()),
            "{summary}"
        );
        assert!(!summary.contains("from "), "{summary}");
    }

    #[test]
    fn scope_summary_labels_custom_range() {
        let app = make_app_with_scope(
            Tab::Models,
            Some("2024-01-01".to_string()),
            Some("2024-01-31".to_string()),
            None,
        );

        let summary = line_text(&scope_summary_line(&app, 100));
        assert!(
            summary.contains("from 2024-01-01 to 2024-01-31"),
            "{summary}"
        );
        assert!(!summary.contains("All Time"), "{summary}");
    }

    #[test]
    fn scope_summary_labels_year() {
        let app = make_app_with_scope(Tab::Models, None, None, Some("2024".to_string()));

        let summary = line_text(&scope_summary_line(&app, 100));
        assert!(summary.contains("Range: 2024"), "{summary}");
        assert!(!summary.contains("All Time"), "{summary}");
    }

    #[test]
    fn scope_summary_uses_projected_web_loader_range() {
        let reference = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let mut app = make_app_on(Tab::Overview);
        app.data_loader = DataLoader::with_filters(
            Some("2026-06-12".to_string()),
            Some("2026-06-18".to_string()),
            None,
        );
        app.set_render_reference_now(reference.and_hms_opt(12, 0, 0).unwrap());

        let summary = line_text(&scope_summary_line(&app, 100));
        assert!(summary.contains("Last 7 days"), "{summary}");
        assert!(!summary.contains("All Time"), "{summary}");
    }

    #[test]
    fn overview_footer_prioritizes_actions_at_52_80_120_columns() {
        let today = chrono::Local::now().date_naive();
        let since = today
            .checked_sub_signed(chrono::Duration::days(6))
            .unwrap()
            .to_string();

        for width in [52, 80, 120] {
            let mut app = make_app_with_scope(
                Tab::Overview,
                Some(since.clone()),
                Some(today.to_string()),
                None,
            );
            app.data.models = vec![model_usage("gpt-5")];

            let body = render_footer_text(&mut app, width);
            let today_at = body.find("Today").unwrap_or_else(|| panic!("{body}"));
            let details_at = body.find("Details").unwrap_or_else(|| panic!("{body}"));
            let refresh_at = body.find("Refresh").unwrap_or_else(|| panic!("{body}"));
            let nav_at = body.find("Nav").unwrap_or_else(|| panic!("{body}"));

            assert!(today_at < details_at, "{body}");
            assert!(details_at < refresh_at, "{body}");
            assert!(refresh_at < nav_at, "{body}");
            if width >= 80 {
                assert!(body.contains("Enter"), "{body}");
                assert!(body.contains("Workspace") || body.contains(" Ws"), "{body}");
                assert!(body.contains("Theme"), "{body}");
            }
            if width == 120 {
                assert!(body.contains("Last 7 days"), "{body}");
                assert!(!body.contains("All Time"), "{body}");
            }
        }
    }

    #[test]
    fn models_and_timeline_footers_keep_executable_details_action() {
        let mut models = make_app_on(Tab::Models);
        models.data.models = vec![model_usage("gpt-5")];
        let models_body = render_footer_text(&mut models, 80);
        assert!(models_body.contains("Enter"), "{models_body}");
        assert!(models_body.contains("Details"), "{models_body}");
        assert!(models_body.contains("Refresh"), "{models_body}");

        let mut timeline = make_app_on(Tab::Timeline);
        timeline.data.daily = vec![daily_usage(NaiveDate::from_ymd_opt(2026, 7, 10).unwrap())];
        let timeline_body = render_footer_text(&mut timeline, 80);
        assert!(timeline_body.contains("Enter"), "{timeline_body}");
        assert!(timeline_body.contains("Details"), "{timeline_body}");
        assert!(timeline_body.contains("Refresh"), "{timeline_body}");

        timeline.timeline_granularity = TimelineGranularity::Hour;
        let hourly_body = render_footer_text(&mut timeline, 80);
        assert!(!hourly_body.contains("Details"), "{hourly_body}");
    }

    #[test]
    fn pulse_footer_keeps_sync_and_omits_invalid_nav_at_target_widths() {
        for width in [52, 80, 120] {
            let mut app = make_app_on(Tab::Pulse);
            let body = render_footer_text(&mut app, width);

            assert!(body.contains("Sync WeRead"), "{body}");
            assert!(body.contains("Workspace"), "{body}");
            assert!(body.contains("Theme"), "{body}");
            assert!(!body.contains("↑↓"), "{body}");
            assert!(!body.contains("Navigate"), "{body}");
        }
    }

    #[test]
    fn usage_footer_keeps_refresh_and_safe_reset_label_at_target_widths() {
        for width in [52, 80, 120] {
            let mut app = make_app_on(Tab::Usage);
            let mut output = usage_output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            );
            output.reset_credits = Some(UsageResetCredits {
                available_count: 1,
                credits: Vec::new(),
            });
            app.subscription_usage = vec![output];

            let body = render_footer_text(&mut app, width);
            assert!(body.contains(" r  Refresh"), "{body}");
            assert!(body.contains(" u  Use"), "{body}");
            assert!(body.contains(" x  Reset"), "{body}");
            assert!(!body.contains("Rst"), "{body}");
        }
    }

    #[test]
    fn tiny_footer_drops_summary_before_primary_actions() {
        let mut app = make_app_on(Tab::Overview);
        app.clear_status();
        let body = render_footer_text(&mut app, 28);

        assert!(body.contains("Today"), "{body}");
        assert!(body.contains("Ref"), "{body}");
        assert!(!body.contains("All Time"), "{body}");
    }

    #[test]
    fn tiny_overview_keeps_all_primary_actions_before_workspace_hints() {
        let mut app = make_app_on(Tab::Overview);
        app.clear_status();
        app.data.models = vec![model_usage("gpt-5")];

        let body = render_footer_text(&mut app, 35);

        assert!(body.contains("Today"), "{body}");
        assert!(body.contains("Details"), "{body}");
        assert!(body.contains(" r  Ref"), "{body}");
        assert!(!body.contains(" Ws"), "{body}");
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
    fn usage_footer_hints_selected_account_actions() {
        let mut app = make_app_on(Tab::Usage);
        app.subscription_usage = vec![
            usage_output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_work".to_string(),
                    label: Some("work".to_string()),
                    is_active: true,
                }),
            ),
            usage_output(
                "Codex",
                Some(UsageAccount {
                    id: "acct_personal".to_string(),
                    label: Some("personal".to_string()),
                    is_active: false,
                }),
            ),
        ];
        app.selected_index = 1;

        let hints = line_text(&Line::from(action_spans(&mut app, 0, 0, 140)));

        assert!(hints.contains(" u  Use"), "{hints}");
        assert!(hints.contains(" Del  Remove"), "{hints}");
        assert!(
            app.click_areas.iter().any(|area| matches!(
                &area.action,
                ClickAction::CodexUseAccount { account_id } if account_id == "acct_personal"
            )),
            "missing use-account footer click area"
        );
        assert!(
            app.click_areas.iter().any(|area| matches!(
                &area.action,
                ClickAction::CodexRemoveAccount { account_id } if account_id == "acct_personal"
            )),
            "missing remove-account footer click area"
        );
    }

    #[test]
    fn drilldown_hints_scope_sort_keys_to_detail_type() {
        let mut model_app = make_app_on(Tab::Timeline);
        model_app.open_model_detail(ModelDetailKey {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            color_key: "gpt-5".to_string(),
        });
        let model_hints = line_text(&Line::from(action_spans(&mut model_app, 0, 0, 96)));

        assert!(model_hints.contains(" c  Cost"), "{model_hints}");
        assert!(model_hints.contains(" t  Tok"), "{model_hints}");
        assert!(model_hints.contains(" d  Date"), "{model_hints}");

        let mut period_app = make_app_on(Tab::Timeline);
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
        let mut app = make_app_on(Tab::Timeline);
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
        let mut app = make_app_on(Tab::Timeline);
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
    fn action_hints_include_theme_toggle_on_wide_panes() {
        let mut app = make_app_on(Tab::Overview);
        let hints = line_text(&Line::from(action_spans(&mut app, 0, 0, 120)));

        assert!(hints.contains(" p  Theme"), "{hints}");
    }

    #[test]
    fn overview_daily_footer_omits_scroll_hint() {
        let mut app = make_app_on(Tab::Overview);
        app.chart_granularity = crate::tui::app::ChartGranularity::Daily;

        let hints = line_text(&Line::from(action_spans(&mut app, 0, 0, 160)));

        assert!(hints.contains("D/W/M"), "{hints}");
        assert!(!hints.contains("Scroll"), "{hints}");
    }

    #[test]
    fn overview_weekly_footer_keeps_scroll_hint() {
        let mut app = make_app_on(Tab::Overview);
        app.chart_granularity = crate::tui::app::ChartGranularity::Weekly;

        let hints = line_text(&Line::from(action_spans(&mut app, 0, 0, 160)));

        assert!(hints.contains("D/W/M"), "{hints}");
        assert!(hints.contains("Scroll"), "{hints}");
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
        app.auto_refresh = false;
        app.background_loading = true;
        app.status_message = None;
        app.status_message_time = None;
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
