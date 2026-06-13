use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation};

use super::mix::token_profile_lines;
use super::widgets::{
    format_cache_hit_rate, format_cost, format_cost_per_million, format_ms_per_1k, format_tokens,
    get_client_display_name, get_provider_display_name, light_ratio_bar_spans, scrollbar_state,
    truncate_ellipsis as truncate,
};
use crate::tui::app::{App, ClickAction, SortDirection, SortField};
use crate::tui::data::ModelUsage;
use tokscale_core::GroupBy;

const INSPECTOR_MIN_WIDTH: u16 = 36;
const INSPECTOR_MAX_WIDTH: u16 = 52;
const WIDE_LAYOUT_MIN_WIDTH: u16 = 104;

#[derive(Clone, Copy)]
struct RankingLayout {
    rank: usize,
    model: usize,
    provider: usize,
    cost: usize,
    pct: usize,
    tokens: usize,
    input: usize,
    output: usize,
    cache: usize,
    sessions: usize,
    perf: usize,
}

impl RankingLayout {
    fn for_width(width: u16, is_very_narrow: bool) -> Self {
        let width = width as usize;
        let rank = if is_very_narrow { 0 } else { 4 };
        let provider = if width >= 72 {
            14
        } else if width >= 58 {
            10
        } else {
            0
        };
        let cost = if width >= 34 { 9 } else { 8 };
        let pct = if width >= 52 { 6 } else { 0 };
        let tokens = if width >= 44 { 9 } else { 0 };
        let input = if width >= 84 { 9 } else { 0 };
        let output = if width >= 84 { 9 } else { 0 };
        let cache = if width >= 104 { 8 } else { 0 };
        let sessions = if width >= 112 { 8 } else { 0 };
        let perf = if width >= 122 { 8 } else { 0 };

        let mut fixed = 0usize;
        for column in [
            rank, provider, cost, pct, tokens, input, output, cache, sessions, perf,
        ] {
            if column > 0 {
                fixed += column + 1;
            }
        }

        let model = width.saturating_sub(fixed).max(8);

        Self {
            rank,
            model,
            provider,
            cost,
            pct,
            tokens,
            input,
            output,
            cache,
            sessions,
            perf,
        }
    }
}

fn workspace_label(model: &ModelUsage) -> &str {
    model
        .workspace_label
        .as_deref()
        .unwrap_or("Unknown workspace")
}

fn model_display_name(model: &ModelUsage, group_by: &GroupBy) -> String {
    if *group_by == GroupBy::WorkspaceModel {
        format!("{} / {}", workspace_label(model), model.model)
    } else {
        model.model.clone()
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.background)),
        area,
    );

    if area.width >= WIDE_LAYOUT_MIN_WIDTH && area.height >= 12 {
        render_wide(frame, app, area);
    } else {
        render_ranking(frame, app, area);
    }
}

fn render_wide(frame: &mut Frame, app: &mut App, area: Rect) {
    let inspector_width = ((area.width as f64) * 0.31).round() as u16;
    let inspector_width = inspector_width.clamp(INSPECTOR_MIN_WIDTH, INSPECTOR_MAX_WIDTH);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(64), Constraint::Length(inspector_width)])
        .split(area);

    render_ranking(frame, app, chunks[0]);
    render_inspector(frame, app, chunks[1]);
}

fn render_ranking(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = ranking_title(app);
    let title_right = ranking_title_right(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(" {} ", title_right),
                app.theme.subtle_text_style(),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let page_capacity = inner.height.saturating_sub(1).max(1) as usize;
    app.set_max_visible_items(page_capacity);

    let group_by = app.group_by.borrow().clone();
    let models = app.get_sorted_models();
    if models.is_empty() {
        let empty_msg = Paragraph::new(
            "No usage data found. Press 'r' to refresh, 's' for sources, 'g' for grouping.",
        )
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    let layout = RankingLayout::for_width(inner.width, app.is_very_narrow());
    render_ranking_header(frame, app, inner, layout);

    let models_len = models.len();
    let start = app.scroll_offset.min(models_len.saturating_sub(1));
    let end = (start + page_capacity).min(models_len);
    let total_cost = app.data.total_cost.max(0.01);
    let row_clicks = models[start..end]
        .iter()
        .enumerate()
        .map(|(i, model)| {
            (
                Rect::new(
                    inner.x,
                    inner.y.saturating_add(1 + i as u16),
                    inner.width,
                    1,
                ),
                app.model_detail_key_for_usage(model),
            )
        })
        .collect::<Vec<_>>();

    let mut y = inner.y.saturating_add(1);
    for (i, model) in models[start..end].iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }

        let idx = i + start;
        let is_selected = idx == app.selected_index;
        let is_striped = idx % 2 == 1;
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        let row_style = if is_selected {
            Style::default()
                .bg(app.theme.selection)
                .fg(app.theme.foreground)
        } else if is_striped {
            app.theme.striped_row_style()
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new("").style(row_style), row_area);

        let color = app.model_color_for(&model.provider, &model.model);
        let display_name = model_display_name(model, &group_by);
        let cost_share = if model.cost.is_finite() {
            model.cost.max(0.0) / total_cost
        } else {
            0.0
        };
        let row_fg = if is_selected {
            app.theme.foreground
        } else {
            color
        };

        let mut spans = Vec::new();
        if layout.rank > 0 {
            let marker = if is_selected { "▶" } else { " " };
            spans.push(Span::styled(
                pad_right(&format!("{marker}{}", idx + 1), layout.rank),
                Style::default().fg(if is_selected {
                    app.theme.foreground
                } else {
                    app.theme.muted
                }),
            ));
        }

        let name_width = layout.model.saturating_sub(2).max(1);
        spans.push(Span::styled("● ", Style::default().fg(color)));
        spans.push(Span::styled(
            pad_left(&truncate(&display_name, name_width), name_width),
            Style::default().fg(row_fg).add_modifier(Modifier::BOLD),
        ));

        if layout.provider > 0 {
            spans.push(Span::styled(
                pad_left(&get_provider_display_name(&model.provider), layout.provider),
                app.theme.secondary_text_style(),
            ));
        }

        spans.push(Span::styled(
            pad_right(&format_cost(model.cost), layout.cost),
            Style::default()
                .fg(if is_selected {
                    app.theme.foreground
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        ));

        if layout.pct > 0 {
            spans.push(Span::styled(
                pad_right(&format!("{:.1}%", cost_share * 100.0), layout.pct),
                app.theme.subtle_text_style(),
            ));
        }

        if layout.tokens > 0 {
            spans.push(Span::styled(
                pad_right(&format_tokens(model.tokens.total()), layout.tokens),
                app.theme.secondary_text_style(),
            ));
        }
        if layout.input > 0 {
            spans.push(Span::styled(
                pad_right(&format_tokens(model.tokens.input), layout.input),
                app.theme.metric_input_style(),
            ));
        }
        if layout.output > 0 {
            spans.push(Span::styled(
                pad_right(&format_tokens(model.tokens.output), layout.output),
                app.theme.metric_output_style(),
            ));
        }
        if layout.cache > 0 {
            let cache_tokens = model
                .tokens
                .cache_read
                .saturating_add(model.tokens.cache_write);
            spans.push(Span::styled(
                pad_right(&format_tokens(cache_tokens), layout.cache),
                app.theme.metric_cache_read_style(),
            ));
        }
        if layout.sessions > 0 {
            spans.push(Span::styled(
                pad_right(&model.session_count.to_string(), layout.sessions),
                app.theme.secondary_text_style(),
            ));
        }
        if layout.perf > 0 {
            spans.push(Span::styled(
                pad_right(
                    &format_ms_per_1k(model.performance.ms_per_1k_tokens),
                    layout.perf,
                ),
                Style::default().fg(Color::Yellow),
            ));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), row_area);
        y = y.saturating_add(1);
    }

    for (rect, key) in row_clicks {
        app.add_click_area(rect, ClickAction::OpenModelDetail(key));
    }

    if models_len > page_capacity {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");
        let mut state = scrollbar_state(models_len, app.scroll_offset, page_capacity);
        frame.render_stateful_widget(scrollbar, inner, &mut state);
    }
}

fn render_ranking_header(frame: &mut Frame, app: &App, inner: Rect, layout: RankingLayout) {
    if inner.height == 0 {
        return;
    }

    let header_style = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::BOLD);
    let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let mut spans = Vec::new();
    if layout.rank > 0 {
        spans.push(Span::styled(pad_right("#", layout.rank), header_style));
    }
    spans.push(Span::styled(pad_left("Model", layout.model), header_style));
    if layout.provider > 0 {
        spans.push(Span::styled(
            pad_left("Provider", layout.provider),
            header_style,
        ));
    }
    spans.push(Span::styled(
        pad_right(
            &format!("Cost{}", sort_indicator(app, SortField::Cost)),
            layout.cost,
        ),
        header_style,
    ));
    if layout.pct > 0 {
        spans.push(Span::styled(pad_right("%", layout.pct), header_style));
    }
    if layout.tokens > 0 {
        spans.push(Span::styled(
            pad_right(
                &format!("Tokens{}", sort_indicator(app, SortField::Tokens)),
                layout.tokens,
            ),
            header_style,
        ));
    }
    if layout.input > 0 {
        spans.push(Span::styled(pad_right("Input", layout.input), header_style));
    }
    if layout.output > 0 {
        spans.push(Span::styled(
            pad_right("Output", layout.output),
            header_style,
        ));
    }
    if layout.cache > 0 {
        spans.push(Span::styled(pad_right("Cache", layout.cache), header_style));
    }
    if layout.sessions > 0 {
        spans.push(Span::styled(
            pad_right("Sessions", layout.sessions),
            header_style,
        ));
    }
    if layout.perf > 0 {
        spans.push(Span::styled(pad_right("ms/1K", layout.perf), header_style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), header_area);
}

fn render_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Selection ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let models = app.get_sorted_models();
    if models.is_empty() {
        let empty = Paragraph::new("No model selected")
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let selected = app.selected_index.min(models.len().saturating_sub(1));
    let model = models[selected];
    let model_color = app.model_color_for(&model.provider, &model.model);
    let total_cost = app.data.total_cost.max(0.01);
    let total_tokens = app.data.total_tokens.max(1);
    let cost_share = if model.cost.is_finite() {
        model.cost.max(0.0) / total_cost
    } else {
        0.0
    };
    let token_share = model.tokens.total() as f64 / total_tokens as f64;

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("● ", Style::default().fg(model_color)),
        Span::styled(
            truncate(&model.model, inner.width.saturating_sub(2) as usize),
            Style::default()
                .fg(model_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {}",
            get_provider_display_name(&model.provider),
            get_client_display_name(&model.client)
        ),
        app.theme.secondary_text_style(),
    )));

    if model.workspace_label.is_some() {
        lines.push(kv_line("Workspace", workspace_label(model), app));
    }

    lines.push(Line::from(""));
    lines.push(section_line("Share", app));
    lines.push(bar_line(
        "Cost",
        &format_cost(model.cost),
        cost_share,
        model_color,
        inner.width,
        app,
    ));
    lines.push(bar_line(
        "Tokens",
        &format_tokens(model.tokens.total()),
        token_share,
        Color::Cyan,
        inner.width,
        app,
    ));

    if model.tokens.total() > 0 {
        if let Some(body_slots) = section_body_slots(&lines, inner) {
            let token_limit = 7.min(body_slots);
            let body = token_profile_lines(app, inner.width, token_limit, &model.tokens);
            append_section_body(&mut lines, "Token Mix", body, inner, app);
        }
    }

    append_section_body(
        &mut lines,
        "Efficiency",
        vec![
            kv_line(
                "Cost / 1M",
                &format_cost_per_million(model.cost, model.tokens.total()),
                app,
            ),
            kv_line(
                "Cache reuse",
                &format_cache_hit_rate(
                    model.tokens.cache_read,
                    model.tokens.input,
                    model.tokens.cache_write,
                ),
                app,
            ),
            kv_line(
                "ms / 1K",
                &format_ms_per_1k(model.performance.ms_per_1k_tokens),
                app,
            ),
            kv_line("Sessions", &model.session_count.to_string(), app),
        ],
        inner,
        app,
    );

    let visible = inner.height as usize;
    if lines.len() > visible {
        lines.truncate(visible);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn append_section_body(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    body: Vec<Line<'static>>,
    area: Rect,
    app: &App,
) -> bool {
    let Some(body_slots) = section_body_slots(lines, area) else {
        return false;
    };
    if body.is_empty() {
        return false;
    }

    lines.push(Line::from(""));
    lines.push(section_line(title, app));
    lines.extend(body.into_iter().take(body_slots));
    true
}

fn section_body_slots(lines: &[Line<'static>], area: Rect) -> Option<usize> {
    let remaining = (area.height as usize).saturating_sub(lines.len());
    if remaining < 3 {
        return None;
    }
    Some(remaining - 2)
}

fn ranking_title(app: &App) -> String {
    match app.sort_field {
        SortField::Tokens => "Models by Tokens".to_string(),
        SortField::Cost => "Models by Cost".to_string(),
        SortField::Date => "Models".to_string(),
    }
}

fn ranking_title_right(app: &App) -> String {
    let sort = match app.sort_field {
        SortField::Cost => "Cost",
        SortField::Tokens => "Tokens",
        SortField::Date => "Name",
    };
    let direction = match app.sort_direction {
        SortDirection::Ascending => "asc",
        SortDirection::Descending => "desc",
    };
    format!(
        "{} models  •  {} {}",
        app.data.models.len(),
        sort,
        direction
    )
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
        Span::styled(format!("{label:<12}"), app.theme.subtle_text_style()),
        Span::styled(value.to_string(), app.theme.secondary_text_style()),
    ])
}

fn bar_line(
    label: &str,
    value: &str,
    pct: f64,
    color: Color,
    width: u16,
    app: &App,
) -> Line<'static> {
    let width = width as usize;
    let label_width = 12usize;
    let value_width = 9usize;
    let pct_width = 6usize;
    let fixed = label_width + value_width + pct_width + 3;
    let pct = pct.clamp(0.0, 1.0);
    if width <= fixed + 4 {
        return compact_share_line(label, value, pct, width, app);
    }

    let bar_width = width.saturating_sub(fixed).min(18);

    let mut spans = vec![Span::styled(
        format!("{label:<label_width$}"),
        app.theme.subtle_text_style(),
    )];
    spans.extend(light_ratio_bar_spans(
        pct,
        bar_width,
        Style::default().fg(color),
        app.theme.subtle_text_style(),
    ));
    spans.extend([
        Span::raw(" "),
        Span::styled(
            format!("{value:>value_width$}"),
            app.theme.secondary_text_style(),
        ),
        Span::styled(
            format!(" {:>4.0}%", pct * 100.0),
            app.theme.subtle_text_style(),
        ),
    ]);
    Line::from(spans)
}

fn compact_share_line(
    label: &str,
    value: &str,
    pct: f64,
    width: usize,
    app: &App,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let pct_text = format!(" {:>4.0}%", pct * 100.0);
    let show_pct = width >= 18;
    let pct_width = if show_pct {
        pct_text.chars().count()
    } else {
        0
    };
    let label_width = if width >= 28 {
        12
    } else if show_pct {
        6
    } else {
        width.min(10)
    };
    let value_width = width.saturating_sub(label_width + pct_width);

    if value_width == 0 {
        return Line::from(Span::styled(
            truncate(label, width),
            app.theme.subtle_text_style(),
        ));
    }

    let mut spans = vec![
        Span::styled(
            format!("{:<label_width$}", truncate(label, label_width)),
            app.theme.subtle_text_style(),
        ),
        Span::styled(
            format!("{:>value_width$}", truncate(value, value_width)),
            app.theme.secondary_text_style(),
        ),
    ];

    if show_pct {
        spans.push(Span::styled(pct_text, app.theme.subtle_text_style()));
    }

    Line::from(spans)
}

fn pad_left(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:<width$} ")
}

fn pad_right(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:>width$} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Tab, TuiConfig};
    use crate::tui::data::{ModelUsage, TokenBreakdown, UsageData};
    use ratatui::{backend::TestBackend, Terminal};
    use tokscale_core::ModelPerformance;

    fn model(name: &str, provider: &str, cost: f64) -> ModelUsage {
        ModelUsage {
            model: name.to_string(),
            provider: provider.to_string(),
            client: "claude".to_string(),
            workspace_key: Some("tokscale".to_string()),
            workspace_label: Some("tokscale".to_string()),
            tokens: TokenBreakdown {
                input: 1_000_000,
                output: 800_000,
                cache_read: 200_000,
                cache_write: 50_000,
                reasoning: 0,
            },
            cost,
            performance: ModelPerformance {
                ms_per_1k_tokens: Some(1463.0),
                total_duration_ms: 3_000_000,
                timed_tokens: 2_050_000,
                sample_count: 12,
                token_coverage: 1.0,
            },
            session_count: 7,
        }
    }

    fn render_models(width: u16) -> String {
        let data = UsageData {
            total_cost: 42.0,
            total_tokens: 2_050_000,
            models: vec![model("claude-4-sonnet", "anthropic", 32.0)],
            ..UsageData::default()
        };

        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: Some(Tab::Models),
        };
        let mut app = App::new_with_cached_data(config, Some(data)).unwrap();
        app.terminal_width = width;

        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, Rect::new(0, 0, width, 24)))
            .unwrap();

        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn test_app() -> App {
        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: Some(Tab::Models),
        };
        App::new_with_cached_data(config, Some(UsageData::default())).unwrap()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn wide_models_renders_selected_model_inspector() {
        let rendered = render_models(140);

        assert!(rendered.contains("Selection"));
        assert!(rendered.contains("Token Mix"));
        assert!(rendered.contains("Cache hit"));
        assert!(rendered.contains("Cost / 1M"));
    }

    #[test]
    fn medium_models_renders_selected_model_inspector() {
        let rendered = render_models(110);

        assert!(rendered.contains("Selection"));
        assert!(rendered.contains("Token Mix"));
    }

    #[test]
    fn compact_models_keeps_table_without_inspector() {
        let rendered = render_models(80);

        assert!(rendered.contains("Models by Cost"));
        assert!(!rendered.contains("Selection"));
    }

    #[test]
    fn compact_share_line_keeps_percent_when_bar_is_hidden() {
        let app = test_app();
        let line = bar_line("Cost", "$288.20", 0.272, Color::Green, 34, &app);
        let text = line_text(&line);

        assert!(line.width() <= 34, "{text}");
        assert!(text.contains("$288.20"), "{text}");
        assert!(text.contains("27%"), "{text}");

        for width in [1, 4, 10, 18] {
            let line = bar_line("Tokens", "298.7M", 0.13, Color::Cyan, width, &app);
            assert!(
                line.width() <= width as usize,
                "{} cols in {width}: {}",
                line.width(),
                line_text(&line)
            );
        }
    }

    #[test]
    fn inspector_section_body_skips_when_only_title_would_fit() {
        let app = test_app();
        let mut lines = vec![Line::from("filled"); 8];

        let appended = append_section_body(
            &mut lines,
            "Efficiency",
            vec![kv_line("Sessions", "7", &app)],
            Rect::new(0, 0, 34, 10),
            &app,
        );

        assert!(!appended);
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn inspector_section_body_truncates_to_remaining_space() {
        let app = test_app();
        let mut lines = vec![Line::from("filled"); 6];

        let appended = append_section_body(
            &mut lines,
            "Efficiency",
            vec![
                kv_line("Cost / 1M", "$0.96", &app),
                kv_line("Cache reuse", "17.2x", &app),
                kv_line("Sessions", "109", &app),
            ],
            Rect::new(0, 0, 34, 10),
            &app,
        );
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(appended);
        assert_eq!(lines.len(), 10);
        assert!(body.contains("Efficiency"), "{body}");
        assert!(body.contains("Cost / 1M"), "{body}");
        assert!(body.contains("Cache reuse"), "{body}");
        assert!(!body.contains("Sessions"), "{body}");
    }
}
