use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::widgets::{
    format_cache_hit_rate, format_cost, format_tokens, light_ratio_bar_spans,
    truncate_ascii as truncate,
};
use crate::tui::app::App;
use crate::tui::data::TokenBreakdown;

const EMPTY_MIX_LABEL: &str = "No mix data";
const RANKING_LABEL_MAX_WIDTH: usize = 24;

pub(crate) struct MixRow {
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) amount: f64,
    pub(crate) color: Color,
}

impl MixRow {
    pub(crate) fn cost(label: impl Into<String>, cost: f64, color: Color) -> Self {
        let amount = if cost.is_finite() { cost.max(0.0) } else { 0.0 };
        Self {
            label: label.into(),
            value: format_cost(amount),
            amount,
            color,
        }
    }
}

pub(crate) fn render_stacked_mix_summary(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    rows: &[MixRow],
    footer: Option<String>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let lines = stacked_mix_summary_lines(app, area.width, area.height as usize, rows, footer);
    frame.render_widget(Paragraph::new(lines), area);
}

pub(crate) fn stacked_mix_summary_lines(
    app: &App,
    width: u16,
    max_lines: usize,
    rows: &[MixRow],
    footer: Option<String>,
) -> Vec<Line<'static>> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    if !has_mix_data(rows) {
        return vec![Line::from(Span::styled(
            truncate(EMPTY_MIX_LABEL, width as usize),
            app.theme.subtle_text_style(),
        ))];
    }

    let total = rows.iter().map(|row| row.amount.max(0.0)).sum::<f64>();
    let legend_rows = rows
        .iter()
        .filter(|row| row.amount > 0.0)
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let show_bar = legend_rows.len() > 1 && width >= 12 && max_lines >= 3;
    if show_bar {
        lines.push(stacked_mix_bar_line(&legend_rows, total, width));
    }
    let reserve_footer = footer.is_some() && max_lines.saturating_sub(lines.len()) > 1;

    append_mix_rows(
        &mut lines,
        app,
        MixRowsConfig {
            width,
            max_lines,
            legend_rows: &legend_rows,
            total,
            footer,
            reserve_footer,
            legend_line: stacked_mix_legend_line,
        },
    );

    lines
}

pub(crate) fn compact_mix_summary_lines(
    app: &App,
    width: u16,
    max_lines: usize,
    rows: &[MixRow],
    footer: Option<String>,
) -> Vec<Line<'static>> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    if !has_mix_data(rows) {
        return vec![Line::from(Span::styled(
            truncate(EMPTY_MIX_LABEL, width as usize),
            app.theme.subtle_text_style(),
        ))];
    }

    let total = rows.iter().map(|row| row.amount.max(0.0)).sum::<f64>();
    let legend_rows = rows
        .iter()
        .filter(|row| row.amount > 0.0)
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let reserve_footer = footer.is_some() && max_lines > 3;
    append_mix_rows(
        &mut lines,
        app,
        MixRowsConfig {
            width,
            max_lines,
            legend_rows: &legend_rows,
            total,
            footer,
            reserve_footer,
            legend_line: compact_mix_legend_line,
        },
    );

    lines
}

pub(crate) fn token_profile_lines(
    app: &App,
    width: u16,
    max_lines: usize,
    tokens: &TokenBreakdown,
) -> Vec<Line<'static>> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    if tokens.total() == 0 {
        return vec![Line::from(Span::styled(
            truncate("No token data", width as usize),
            app.theme.subtle_text_style(),
        ))];
    }

    let mut rows = vec![
        (
            "Input",
            format_tokens(tokens.input),
            Color::Rgb(96, 165, 250),
        ),
        (
            "Output",
            format_tokens(tokens.output),
            Color::Rgb(74, 222, 128),
        ),
        (
            "Cache read",
            format_tokens(tokens.cache_read),
            Color::Rgb(167, 139, 250),
        ),
        (
            "Cache write",
            format_tokens(tokens.cache_write),
            Color::Rgb(251, 146, 60),
        ),
    ];
    if tokens.reasoning > 0 {
        rows.push((
            "Reasoning",
            format_tokens(tokens.reasoning),
            Color::Rgb(244, 114, 182),
        ));
    }
    rows.push((
        "Cache hit",
        format_cache_hit_rate(tokens.cache_read, tokens.input, tokens.cache_write),
        app.theme.accent,
    ));
    if max_lines > rows.len() {
        rows.push(("Total", format_tokens(tokens.total()), app.theme.foreground));
    }

    rows.into_iter()
        .take(max_lines)
        .map(|(label, value, color)| token_profile_line(label, &value, color, width, app))
        .collect()
}

pub(crate) fn ranking_bar_line(
    label: &str,
    value: &str,
    ratio: f64,
    color: Color,
    width: u16,
    app: &App,
) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    let min_label_width = 6usize;
    let min_bar_width = 4usize;
    let value_width = ranking_value_width(value, width);
    let min_full_width = min_label_width + 1 + min_bar_width + 1 + value_width;
    if width < min_full_width {
        return compact_ranking_line(label, value, width, color, app);
    }

    let label_width = ranking_label_width(width, value_width, min_bar_width);
    let bar_width = width
        .saturating_sub(label_width + value_width + 2)
        .max(min_bar_width);
    let value = truncate(value, value_width);
    let mut spans = vec![
        Span::styled(pad_right(label, label_width), app.theme.subtle_text_style()),
        Span::raw(" "),
    ];
    spans.extend(light_ratio_bar_spans(
        ratio,
        bar_width,
        Style::default().fg(color),
        app.theme.subtle_text_style(),
    ));
    spans.extend([
        Span::raw(" "),
        Span::styled(
            pad_left(&value, value_width),
            Style::default().fg(app.theme.foreground),
        ),
    ]);
    Line::from(spans)
}

fn ranking_value_width(value: &str, width: usize) -> usize {
    let raw_width = value.chars().count();
    if width >= 42 {
        raw_width.clamp(7, 10)
    } else if width >= 32 {
        raw_width.clamp(6, 8)
    } else {
        raw_width.min(7)
    }
}

fn ranking_label_width(width: usize, value_width: usize, min_bar_width: usize) -> usize {
    let desired = if width >= 72 {
        RANKING_LABEL_MAX_WIDTH
    } else if width >= 56 {
        20
    } else if width >= 42 {
        18
    } else if width >= 32 {
        14
    } else {
        10
    };
    let max_label_width = width.saturating_sub(value_width + min_bar_width + 2);
    desired.clamp(6, max_label_width)
}

pub(crate) fn has_mix_data(rows: &[MixRow]) -> bool {
    rows.iter().any(|row| row.amount > 0.0)
}

pub(crate) fn embedded_mix_line_limit(container_height: u16, preferred: usize) -> usize {
    let cap = match container_height {
        0..=17 => 2,
        18..=26 => 3,
        27..=34 => 4,
        _ => preferred,
    };
    preferred.min(cap)
}

struct MixRowsConfig<'a> {
    width: u16,
    max_lines: usize,
    legend_rows: &'a [&'a MixRow],
    total: f64,
    footer: Option<String>,
    reserve_footer: bool,
    legend_line: fn(&MixRow, f64, u16, &App) -> Line<'static>,
}

fn append_mix_rows(lines: &mut Vec<Line<'static>>, app: &App, config: MixRowsConfig<'_>) {
    let MixRowsConfig {
        width,
        max_lines,
        legend_rows,
        total,
        footer,
        reserve_footer,
        legend_line,
    } = config;

    let footer_slots = usize::from(reserve_footer);
    let legend_capacity = max_lines.saturating_sub(lines.len() + footer_slots);
    let visible_legend_rows = if legend_rows.len() > legend_capacity && legend_capacity > 1 {
        legend_capacity - 1
    } else {
        legend_capacity
    };

    for row in legend_rows.iter().take(visible_legend_rows) {
        let ratio = if total > 0.0 {
            row.amount.max(0.0) / total
        } else {
            0.0
        };
        lines.push(legend_line(row, ratio, width, app));
    }

    if legend_rows.len() > visible_legend_rows && lines.len() < max_lines {
        lines.push(mix_more_line(
            &legend_rows[visible_legend_rows..],
            total,
            width,
            app,
        ));
    }

    if let Some(footer) = footer.filter(|_| lines.len() < max_lines) {
        lines.push(Line::from(Span::styled(
            truncate(&footer, width as usize),
            app.theme.subtle_text_style(),
        )));
    }
}

fn compact_mix_legend_line(row: &MixRow, ratio: f64, width: u16, app: &App) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    if width < 10 {
        return Line::from(Span::styled(
            truncate(&row.label, width),
            Style::default().fg(row.color).add_modifier(Modifier::BOLD),
        ));
    }

    let value_width = if width >= 30 { 9 } else { 7 };
    let value = truncate(&row.value, value_width);
    let value_width = value.chars().count();
    let show_percent = width >= 22;
    let percent = format_mix_percent(ratio).trim().to_string();
    let percent_width = if show_percent {
        percent.chars().count().clamp(3, 4)
    } else {
        0
    };
    let prefix_width = usize::from(width >= 18) * 2;
    let separators = usize::from(prefix_width > 0) + 1 + usize::from(show_percent);
    let reserved = prefix_width + value_width + percent_width + separators;

    if reserved >= width {
        return compact_value_line(row, &value, width, app);
    }

    let label_width = width - reserved;
    let mut spans = Vec::new();
    if prefix_width > 0 {
        spans.push(Span::styled("●", Style::default().fg(row.color)));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        pad_right(&row.label, label_width),
        Style::default().fg(row.color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        pad_left(&value, value_width),
        Style::default().fg(app.theme.foreground),
    ));
    if show_percent {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            pad_left(&percent, percent_width),
            app.theme.subtle_text_style(),
        ));
    }

    Line::from(spans)
}

fn stacked_mix_bar_line(rows: &[&MixRow], total: f64, width: u16) -> Line<'static> {
    let width = width as usize;
    if width == 0 || rows.is_empty() || total <= 0.0 {
        return Line::default();
    }

    let mut cells = allocated_mix_cells(rows, total, width);
    let mut spans = Vec::with_capacity(rows.len());
    for (row, cell_count) in rows.iter().zip(cells.drain(..)) {
        if cell_count == 0 {
            continue;
        }
        spans.push(Span::styled(
            "█".repeat(cell_count),
            Style::default().fg(row.color),
        ));
    }
    Line::from(spans)
}

fn allocated_mix_cells(rows: &[&MixRow], total: f64, width: usize) -> Vec<usize> {
    if width == 0 || rows.is_empty() || total <= 0.0 {
        return vec![0; rows.len()];
    }

    if width < rows.len() {
        let mut cells = vec![0; rows.len()];
        for cell in cells.iter_mut().take(width) {
            *cell = 1;
        }
        return cells;
    }

    let mut shares = rows
        .iter()
        .map(|row| {
            let exact = row.amount.max(0.0) / total * width as f64;
            let floor = exact.floor() as usize;
            (floor.max(1), exact - floor as f64)
        })
        .collect::<Vec<_>>();
    let mut used = shares.iter().map(|(cells, _)| *cells).sum::<usize>();

    while used < width {
        if let Some((cells, _)) = shares
            .iter_mut()
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        {
            *cells += 1;
            used += 1;
        } else {
            break;
        }
    }

    while used > width {
        if let Some((cells, _)) = shares.iter_mut().max_by_key(|(cells, _)| *cells) {
            if *cells <= 1 {
                break;
            }
            *cells -= 1;
            used -= 1;
        } else {
            break;
        }
    }

    shares.into_iter().map(|(cells, _)| cells).collect()
}

fn stacked_mix_legend_line(row: &MixRow, ratio: f64, width: u16, app: &App) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    if width < 12 {
        return compact_value_line(row, &truncate(&row.value, width), width, app);
    }

    let value = truncate(&row.value, if width >= 34 { 9 } else { 7 });
    let show_percent = width >= 22;
    let metric = if show_percent {
        format!("{} {}", value, format_mix_percent(ratio).trim())
    } else {
        value
    };
    let metric_width = metric.chars().count().min(width.saturating_sub(3));
    let prefix_width = usize::from(width >= 16) * 2;
    let reserved = prefix_width + metric_width + 1;
    if reserved >= width {
        return compact_value_line(row, &metric, width, app);
    }

    let label_width = width - reserved;
    let mut spans = Vec::new();
    if prefix_width > 0 {
        spans.push(Span::styled("●", Style::default().fg(row.color)));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        pad_right(&row.label, label_width),
        Style::default().fg(row.color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        pad_left(&metric, metric_width),
        app.theme.secondary_text_style(),
    ));
    Line::from(spans)
}

fn compact_value_line(row: &MixRow, value: &str, width: usize, app: &App) -> Line<'static> {
    let value_width = value.chars().count();
    if width <= value_width + 1 {
        return Line::from(Span::styled(
            truncate(&row.label, width),
            Style::default().fg(row.color).add_modifier(Modifier::BOLD),
        ));
    }

    let label_width = width.saturating_sub(value_width + 1);
    Line::from(vec![
        Span::styled(
            pad_right(&row.label, label_width),
            Style::default().fg(row.color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            pad_left(value, value_width),
            Style::default().fg(app.theme.foreground),
        ),
    ])
}

fn mix_more_line(rows: &[&MixRow], total: f64, width: u16, app: &App) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    let label = format!("+{} more", rows.len());
    let hidden_amount = rows.iter().map(|row| row.amount.max(0.0)).sum::<f64>();
    let hidden_value = format_cost(hidden_amount);
    let hidden_ratio = if total > 0.0 {
        hidden_amount / total
    } else {
        0.0
    };
    let full = format!(
        "{label} {hidden_value} {}",
        format_mix_percent(hidden_ratio)
    );
    let value_only = format!("{label} {hidden_value}");
    let text = [&full, &value_only, &label]
        .into_iter()
        .find(|candidate| candidate.chars().count() <= width)
        .unwrap_or(&label);

    Line::from(Span::styled(
        truncate(text, width),
        app.theme.subtle_text_style(),
    ))
}

fn token_profile_line(
    label: &str,
    value: &str,
    color: Color,
    width: u16,
    app: &App,
) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    if width < 14 {
        return profile_value_line(label, value, color, width, app);
    }

    let label_width = if width >= 30 {
        12
    } else if width >= 22 {
        11
    } else {
        10
    }
    .min(width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width + 1);
    Line::from(vec![
        Span::styled(
            pad_right(label, label_width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            pad_right(value, value_width),
            Style::default().fg(app.theme.foreground),
        ),
    ])
}

fn profile_value_line(
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
            pad_right(label, label_width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(app.theme.foreground)),
    ])
}

fn compact_ranking_line(
    label: &str,
    value: &str,
    width: usize,
    color: Color,
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
            pad_right(label, label_width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(app.theme.foreground)),
    ])
}

fn format_mix_percent(ratio: f64) -> String {
    let percent = (ratio * 100.0).max(0.0);
    if percent > 0.0 && percent < 1.0 {
        " <1%".to_string()
    } else {
        format!("{percent:>3.0}%")
    }
}

fn pad_left(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:>width$}")
}

fn pad_right(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    format!("{text:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiConfig;
    use crate::tui::data::UsageData;

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

    #[test]
    fn compact_mix_legend_line_uses_metric_rows_without_bars() {
        let app = test_app();
        let row = MixRow::cost("OpenAI", 123.45, Color::Green);

        let line = compact_mix_legend_line(&row, 0.42, 32, &app);
        let text = line_text(&line);

        assert!(line.width() <= 32, "{text}");
        assert!(text.contains("OpenAI"), "{text}");
        assert!(text.contains("$123.45"), "{text}");
        assert!(text.contains("42%"), "{text}");
        assert!(!text.contains("█"), "{text}");
        assert!(!text.contains("·"), "{text}");
    }

    #[test]
    fn compact_mix_legend_line_fits_narrow_panels() {
        let app = test_app();
        let row = MixRow::cost("VeryLongProviderName", 123.45, Color::Green);

        for width in [4, 8, 14, 18, 24, 36] {
            let line = compact_mix_legend_line(&row, 0.42, width, &app);
            assert!(
                line.width() <= width as usize,
                "{} cols in {width}: {}",
                line.width(),
                line_text(&line)
            );
        }
    }

    #[test]
    fn token_profile_lines_show_cache_hit_without_ratio_bars() {
        let app = test_app();
        let tokens = TokenBreakdown {
            input: 490_000,
            output: 78_000,
            cache_read: 14_100_000,
            cache_write: 0,
            reasoning: 0,
        };

        let lines = token_profile_lines(&app, 33, 6, &tokens);
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(body.contains("Input"), "{body}");
        assert!(body.contains("Cache read"), "{body}");
        assert!(body.contains("Cache write"), "{body}");
        assert!(body.contains("Cache hit"), "{body}");
        assert!(body.contains("28.8x"), "{body}");
        assert!(!body.contains("█"), "{body}");
        assert!(!body.contains("▏"), "{body}");
        assert!(lines.iter().all(|line| line.width() <= 33), "{body}");
    }

    #[test]
    fn ranking_bar_lines_keep_fixed_bar_and_value_columns() {
        let app = test_app();
        let short = line_text(&ranking_bar_line(
            "gpt-5.5",
            "$195.98",
            0.98,
            Color::Green,
            48,
            &app,
        ));
        let long = line_text(&ranking_bar_line(
            "claude-4-6-sonnet-medium-thinking",
            "$2.47",
            0.02,
            Color::LightRed,
            48,
            &app,
        ));

        let first_bar_col = |text: &str| {
            text.chars()
                .position(|ch| matches!(ch, '█' | '▏' | '·'))
                .unwrap()
        };
        assert_eq!(short.chars().count(), 48, "{short}");
        assert_eq!(long.chars().count(), 48, "{long}");
        assert_eq!(first_bar_col(&short), first_bar_col(&long));
        assert_eq!(short.chars().last(), Some('8'));
        assert_eq!(long.chars().last(), Some('7'));
    }

    #[test]
    fn ranking_bar_line_caps_long_labels() {
        let app = test_app();
        let line = ranking_bar_line(
            "claude-4-6-opus-high-thinking-with-extra-suffix",
            "$62.43",
            0.68,
            Color::Green,
            72,
            &app,
        );
        let text = line_text(&line);

        assert_eq!(line.width(), 72, "{text}");
        assert!(text.starts_with("claude-4-6-opus-high-... "), "{text}");
        assert!(text.contains("█"), "{text}");
    }

    #[test]
    fn stacked_mix_summary_uses_single_composition_bar_then_legend_rows() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
            MixRow::cost("Cursor", 1.0, Color::Magenta),
        ];

        let lines = stacked_mix_summary_lines(&app, 36, 5, &rows, None);
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines[0].width(), 36, "{body}");
        assert!(line_text(&lines[0]).contains("█"), "{body}");
        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("$10.00"), "{body}");
        assert!(body.contains("%"), "{body}");
        assert!(!line_text(&lines[1]).contains("█"), "{body}");
        assert!(!body.contains("·"), "{body}");
    }

    #[test]
    fn stacked_mix_summary_omits_redundant_bar_for_single_item() {
        let app = test_app();
        let rows = vec![MixRow::cost("OpenAI", 10.0, Color::Green)];

        let lines = stacked_mix_summary_lines(&app, 36, 5, &rows, None);
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("100%"), "{body}");
        assert!(!body.contains("█"), "{body}");
    }

    #[test]
    fn stacked_mix_summary_keeps_footer_when_space_allows() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
        ];

        let lines = stacked_mix_summary_lines(&app, 34, 6, &rows, Some("Source mix".to_string()));
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(lines.iter().all(|line| line.width() <= 34), "{body}");
        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("$10.00"), "{body}");
        assert!(body.contains("Source mix"), "{body}");
    }

    #[test]
    fn embedded_mix_line_limit_compacts_short_sidebars() {
        assert_eq!(embedded_mix_line_limit(16, 6), 2);
        assert_eq!(embedded_mix_line_limit(24, 6), 3);
        assert_eq!(embedded_mix_line_limit(32, 6), 4);
        assert_eq!(embedded_mix_line_limit(40, 6), 6);
        assert_eq!(embedded_mix_line_limit(16, 1), 1);
    }

    #[test]
    fn compact_mix_summary_lines_start_with_labeled_rank_rows() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
        ];

        let lines = compact_mix_summary_lines(&app, 32, 3, &rows, None);
        let first = line_text(&lines[0]);

        assert!(first.contains("OpenAI"), "{first}");
        assert!(first.contains("$10.00"), "{first}");
        assert!(first.contains("67%"), "{first}");
    }

    #[test]
    fn two_line_compact_mix_keeps_dominant_row_and_aggregate_more_summary() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
            MixRow::cost("Cursor", 1.0, Color::Magenta),
        ];

        let lines = compact_mix_summary_lines(&app, 24, 2, &rows, None);
        let body = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines.len(), 2);
        assert!(body.contains("OpenAI"), "{body}");
        assert!(body.contains("+2 more"), "{body}");
        assert!(body.contains("$6.00"), "{body}");
    }

    #[test]
    fn hidden_compact_mix_summary_reports_aggregate_amount_and_share() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
            MixRow::cost("Cursor", 3.0, Color::Magenta),
            MixRow::cost("Other", 2.0, Color::Blue),
        ];

        let lines = compact_mix_summary_lines(&app, 36, 3, &rows, None);
        let more = line_text(lines.last().expect("more row"));

        assert!(more.contains("+2 more"), "{more}");
        assert!(more.contains("$5.00"), "{more}");
        assert!(more.contains("25%"), "{more}");
    }

    #[test]
    fn hidden_compact_mix_summary_fits_narrow_widths() {
        let app = test_app();
        let rows = vec![
            MixRow::cost("OpenAI", 10.0, Color::Green),
            MixRow::cost("Anthropic", 5.0, Color::LightRed),
            MixRow::cost("Cursor", 3.0, Color::Magenta),
            MixRow::cost("Other", 2.0, Color::Blue),
        ];

        for width in [4, 8, 12] {
            let lines = compact_mix_summary_lines(&app, width, 3, &rows, None);
            let line = lines.last().expect("more row");
            assert!(
                line.width() <= width as usize,
                "{} cols in {width}: {}",
                line.width(),
                line_text(line)
            );
        }
    }

    #[test]
    fn positive_sub_percent_mix_uses_less_than_one_label() {
        let app = test_app();
        let row = MixRow::cost("Tiny", 0.5, Color::Green);

        let line = compact_mix_legend_line(&row, 0.004, 30, &app);
        let text = line_text(&line);

        assert!(line.width() <= 30, "{text}");
        assert!(text.contains("<1%"), "{text}");
        assert!(!text.contains("  0%"), "{text}");
    }

    #[test]
    fn has_mix_data_ignores_zero_rows() {
        assert!(!has_mix_data(&[]));
        assert!(!has_mix_data(&[MixRow::cost("OpenAI", 0.0, Color::Green)]));
        assert!(has_mix_data(&[MixRow::cost("OpenAI", 1.0, Color::Green)]));
    }

    #[test]
    fn compact_mix_summary_lines_reports_empty_data() {
        let app = test_app();
        let lines = compact_mix_summary_lines(
            &app,
            12,
            4,
            &[MixRow::cost("OpenAI", 0.0, Color::Green)],
            None,
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "No mix data");
    }
}
