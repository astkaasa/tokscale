use anyhow::Result;
use chrono::Local;
use ratatui::{
    buffer::{Buffer, Cell},
    style::{Color, Modifier},
};
use serde::Serialize;
use tokscale_core::GroupBy;

use crate::report_format::format_currency;
use crate::tui::{surface::render_app_buffer, App, TuiConfig, UsageData};

const DEFAULT_WIDTH: u16 = 220;
const DEFAULT_HEIGHT: u16 = 69;
const DEFAULT_FG: &str = "#c9d1d9";
const DEFAULT_BG: &str = "#0d1117";

#[derive(Clone, Debug)]
pub(crate) struct OverviewRenderOptions {
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub group_by: GroupBy,
    pub width: u16,
    pub height: u16,
}

impl OverviewRenderOptions {
    pub(crate) fn new(
        clients: Option<Vec<String>>,
        since: Option<String>,
        until: Option<String>,
        year: Option<String>,
        group_by: GroupBy,
    ) -> Self {
        Self {
            clients,
            since,
            until,
            year,
            group_by,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OverviewJson {
    pub generated_at: String,
    pub range_label: String,
    pub width: u16,
    pub height: u16,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub total_cost_label: String,
    pub active_days: usize,
    pub model_count: usize,
}

pub(crate) fn build_overview_json(
    data: &UsageData,
    range_label: String,
    width: u16,
    height: u16,
) -> OverviewJson {
    OverviewJson {
        generated_at: generated_at(),
        range_label,
        width,
        height,
        total_tokens: data.total_tokens,
        total_cost: data.total_cost,
        total_cost_label: format_currency(data.total_cost),
        active_days: data
            .daily
            .iter()
            .filter(|day| day.tokens.total() > 0 || day.cost > 0.0)
            .count(),
        model_count: data.models.len(),
    }
}

pub(crate) fn render_overview_html(
    data: UsageData,
    options: OverviewRenderOptions,
) -> Result<String> {
    let width = options.width.max(40);
    let height = options.height.max(16);
    let mut app = App::new_surface_with_cached_data(
        TuiConfig {
            theme: String::new(),
            refresh: 0,
            clients: options.clients,
            since: options.since,
            until: options.until,
            year: options.year,
            initial_tab: Some(crate::tui::Tab::Overview),
        },
        Some(data),
    )?;

    *app.group_by.borrow_mut() = options.group_by;
    app.status_message = None;
    app.status_message_time = None;
    app.set_background_loading(false);
    app.build_model_shade_map();

    let buffer = render_app_buffer(&mut app, width, height)?;
    Ok(render_buffer_page(&buffer, width, height))
}

fn render_buffer_page(buffer: &Buffer, width: u16, height: u16) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Tokscale Overview</title>
  <style>
{css}
  </style>
</head>
<body>
  <main class="viewport" aria-label="Tokscale terminal overview">
    <pre class="terminal-screen" role="img" aria-label="Tokscale Overview rendered from the TUI buffer" data-cols="{width}" data-rows="{height}" style="{terminal_style}">{surface}<span class="terminal-overlay" aria-hidden="true">{overlay}</span></pre>
  </main>
</body>
</html>
"#,
        css = terminal_css(width, height),
        width = width,
        height = height,
        terminal_style = terminal_style(width, height),
        surface = render_buffer_html(buffer, width, height),
        overlay = render_block_overlay(buffer, width, height),
    )
}

fn render_buffer_html(buffer: &Buffer, width: u16, height: u16) -> String {
    let overlay_region = chart_overlay_region(buffer, width, height);
    render_buffer_html_with_region(buffer, width, height, overlay_region)
}

fn render_buffer_html_with_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    overlay_region: Option<BlockOverlayRegion>,
) -> String {
    let mut html = String::new();
    for y in 0..height {
        html.push_str(r#"<span class="terminal-row">"#);
        let mut x = 0;
        while x < width {
            let cell = &buffer[(x, y)];
            let style = HtmlCellStyle::from_cell(cell);
            let symbol = cell_symbol(cell);
            if block_level(symbol).is_some()
                && overlay_region.is_some_and(|region| region.contains(x, y))
            {
                html.push_str(&style.open_span());
                html.push(' ');
                html.push_str("</span>");
                x += 1;
                continue;
            }

            let mut text = cell_text(cell);
            x += 1;

            while x < width {
                let next = &buffer[(x, y)];
                let next_style = HtmlCellStyle::from_cell(next);
                let next_symbol = cell_symbol(next);
                if next_style != style
                    || (block_level(next_symbol).is_some()
                        && overlay_region.is_some_and(|region| region.contains(x, y)))
                {
                    break;
                }
                text.push_str(&cell_text(next));
                x += 1;
            }

            html.push_str(&style.open_span());
            html.push_str(&escape_html(&text));
            html.push_str("</span>");
        }
        html.push_str("</span>");
    }
    html
}

fn render_block_overlay(buffer: &Buffer, width: u16, height: u16) -> String {
    let Some(region) = chart_overlay_region(buffer, width, height) else {
        return String::new();
    };
    render_block_overlay_region(buffer, width, height, region)
}

fn render_block_overlay_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    region: BlockOverlayRegion,
) -> String {
    let mut html = String::new();

    for x in region.x_start..region.x_end.min(width) {
        let mut current: Option<BlockRect> = None;
        for y in region.y_start..region.y_end.min(height) {
            let cell = &buffer[(x, y)];
            let Some(rect) = block_rect_for_cell(x, y, cell) else {
                flush_block_rect(&mut html, &mut current);
                continue;
            };

            if let Some(existing) = &mut current {
                if existing.can_merge(&rect) {
                    existing.bottom = existing.bottom.max(rect.bottom);
                    continue;
                }
            }

            flush_block_rect(&mut html, &mut current);
            current = Some(rect);
        }
        flush_block_rect(&mut html, &mut current);
    }

    html
}

#[derive(Clone, Copy, Debug)]
struct BlockOverlayRegion {
    x_start: u16,
    x_end: u16,
    y_start: u16,
    y_end: u16,
}

impl BlockOverlayRegion {
    fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x_start && x < self.x_end && y >= self.y_start && y < self.y_end
    }
}

fn chart_overlay_region(buffer: &Buffer, width: u16, height: u16) -> Option<BlockOverlayRegion> {
    let mut summary_x = width;
    for y in 0..height {
        let row = row_text(buffer, width, y);
        if let Some(x) = find_col(&row, "Summary") {
            summary_x = x;
            break;
        }
    }

    let mut title_y = None;
    for y in 0..height {
        if row_text(buffer, width, y).contains("Usage Trend") {
            title_y = Some(y);
            break;
        }
    }
    let title_y = title_y?;

    let mut axis_y = None;
    for y in title_y.saturating_add(1)..height {
        let row = row_text(buffer, width, y);
        if is_chart_axis_row(&row) {
            axis_y = Some(y);
            break;
        }
    }

    Some(BlockOverlayRegion {
        x_start: 0,
        x_end: summary_x.saturating_sub(3).max(1),
        y_start: title_y.saturating_add(1),
        y_end: axis_y.unwrap_or_else(|| title_y.saturating_add(22).min(height)),
    })
}

fn row_text(buffer: &Buffer, width: u16, y: u16) -> String {
    let mut text = String::new();
    for x in 0..width {
        text.push_str(cell_symbol(&buffer[(x, y)]));
    }
    text
}

fn find_col(row: &str, needle: &str) -> Option<u16> {
    let byte_index = row.find(needle)?;
    Some(row[..byte_index].chars().count() as u16)
}

fn is_chart_axis_row(row: &str) -> bool {
    let Some(axis_index) = row.find("0│") else {
        return false;
    };

    row[axis_index..].chars().filter(|ch| *ch == '─').count() >= 8
}

#[derive(Clone, Debug)]
struct BlockRect {
    x: u16,
    top: f64,
    bottom: f64,
    style: HtmlCellStyle,
}

impl BlockRect {
    fn can_merge(&self, next: &BlockRect) -> bool {
        self.x == next.x && self.style == next.style && next.top <= self.bottom + f64::EPSILON
    }
}

fn block_rect_for_cell(x: u16, y: u16, cell: &Cell) -> Option<BlockRect> {
    let level = block_level(cell_symbol(cell))?;
    let fill = block_fill_fraction(level);
    let bottom = f64::from(y) + 1.0;
    Some(BlockRect {
        x,
        top: bottom - fill,
        bottom,
        style: HtmlCellStyle::from_cell(cell),
    })
}

fn flush_block_rect(html: &mut String, rect: &mut Option<BlockRect>) {
    if let Some(rect) = rect.take() {
        html.push_str(&block_rect_html(&rect));
    }
}

fn block_rect_html(rect: &BlockRect) -> String {
    let height = rect.bottom - rect.top;
    format!(
        r#"<span class="terminal-block-rect" style="left:{x}ch;top:calc({top:.3} * var(--terminal-line-height));height:calc({height:.3} * var(--terminal-line-height));background-color:{fg};"></span>"#,
        x = rect.x,
        top = rect.top,
        fg = rect.style.fg,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HtmlCellStyle {
    fg: String,
    bg: String,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    crossed: bool,
    hidden: bool,
}

impl HtmlCellStyle {
    fn from_cell(cell: &Cell) -> Self {
        let mut fg = color_hex(cell.fg, DEFAULT_FG);
        let mut bg = color_hex(cell.bg, DEFAULT_BG);
        if cell.modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }

        Self {
            fg,
            bg,
            bold: cell.modifier.contains(Modifier::BOLD),
            dim: cell.modifier.contains(Modifier::DIM),
            italic: cell.modifier.contains(Modifier::ITALIC),
            underline: cell.modifier.contains(Modifier::UNDERLINED),
            crossed: cell.modifier.contains(Modifier::CROSSED_OUT),
            hidden: cell.modifier.contains(Modifier::HIDDEN),
        }
    }

    fn open_span(&self) -> String {
        let mut style = format!("color:{};background-color:{};", self.fg, self.bg);
        if self.bold {
            style.push_str("font-weight:700;");
        }
        if self.dim {
            style.push_str("opacity:.58;");
        }
        if self.italic {
            style.push_str("font-style:italic;");
        }
        if self.underline || self.crossed {
            let mut decorations = Vec::new();
            if self.underline {
                decorations.push("underline");
            }
            if self.crossed {
                decorations.push("line-through");
            }
            style.push_str("text-decoration:");
            style.push_str(&decorations.join(" "));
            style.push(';');
        }
        if self.hidden {
            style.push_str("visibility:hidden;");
        }
        format!(r#"<span style="{style}">"#)
    }
}

fn cell_symbol(cell: &Cell) -> &str {
    cell.symbol()
}

fn cell_text(cell: &Cell) -> String {
    let symbol = cell_symbol(cell);
    if symbol.is_empty() {
        " ".to_string()
    } else {
        symbol.to_string()
    }
}

fn block_level(symbol: &str) -> Option<u8> {
    match symbol {
        "▁" => Some(1),
        "▂" => Some(2),
        "▃" => Some(3),
        "▄" => Some(4),
        "▅" => Some(5),
        "▆" => Some(6),
        "▇" => Some(7),
        "█" => Some(8),
        _ => None,
    }
}

fn block_fill_fraction(level: u8) -> f64 {
    match level {
        1 => 0.125,
        2 => 0.25,
        3 => 0.375,
        4 => 0.5,
        5 => 0.625,
        6 => 0.75,
        7 => 0.875,
        _ => 1.0,
    }
}

fn color_hex(color: Color, fallback: &'static str) -> String {
    match color {
        Color::Reset => fallback.to_string(),
        Color::Black => "#0d1117".to_string(),
        Color::Red => "#f85149".to_string(),
        Color::Green => "#3fb950".to_string(),
        Color::Yellow => "#d29922".to_string(),
        Color::Blue => "#58a6ff".to_string(),
        Color::Magenta => "#bc8cff".to_string(),
        Color::Cyan => "#39c5cf".to_string(),
        Color::Gray => "#8b949e".to_string(),
        Color::DarkGray => "#6e7681".to_string(),
        Color::LightRed => "#ff7b72".to_string(),
        Color::LightGreen => "#56d364".to_string(),
        Color::LightYellow => "#e3b341".to_string(),
        Color::LightBlue => "#79c0ff".to_string(),
        Color::LightMagenta => "#d2a8ff".to_string(),
        Color::LightCyan => "#56d4dd".to_string(),
        Color::White => "#f0f6fc".to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => indexed_color_hex(index),
    }
}

fn indexed_color_hex(index: u8) -> String {
    match index {
        0 => "#0d1117",
        1 => "#f85149",
        2 => "#3fb950",
        3 => "#d29922",
        4 => "#58a6ff",
        5 => "#bc8cff",
        6 => "#39c5cf",
        7 => "#c9d1d9",
        8 => "#6e7681",
        9 => "#ff7b72",
        10 => "#56d364",
        11 => "#e3b341",
        12 => "#79c0ff",
        13 => "#d2a8ff",
        14 => "#56d4dd",
        15 => "#f0f6fc",
        _ => DEFAULT_FG,
    }
    .to_string()
}

fn terminal_style(width: u16, height: u16) -> String {
    format!("--terminal-cols:{width};--terminal-rows:{height};")
}

fn terminal_css(width: u16, height: u16) -> String {
    const TERMINAL_LINE_HEIGHT: f64 = 1.0;
    let width_denom = f64::from(width.max(1)) * 0.61;
    let height_denom = f64::from(height.max(1)) * TERMINAL_LINE_HEIGHT;
    format!(
        r#"    :root {{
      color-scheme: dark;
      --bg: #0d1117;
      --fg: #c9d1d9;
      --pad: 8px;
      --terminal-line-height: {TERMINAL_LINE_HEIGHT:.2}em;
      --terminal-width-denom: {width_denom:.2};
      --terminal-height-denom: {height_denom:.2};
    }}

    * {{
      box-sizing: border-box;
    }}

    html,
    body {{
      width: 100%;
      height: 100%;
      background: var(--bg);
    }}

    body {{
      margin: 0;
      overflow: hidden;
      background: var(--bg);
      color: var(--fg);
      font-family: Menlo, Monaco, "SF Mono", SFMono-Regular, Consolas, "Liberation Mono", monospace;
      font-variant-ligatures: none;
      font-variant-numeric: tabular-nums;
      font-feature-settings: "liga" 0, "calt" 0, "kern" 0;
      letter-spacing: 0;
      -webkit-font-smoothing: antialiased;
      text-rendering: geometricPrecision;
    }}

    .viewport {{
      position: fixed;
      inset: 0;
      width: 100vw;
      height: 100vh;
      margin: 0;
      overflow: auto;
      background: var(--bg);
    }}

    .terminal-screen {{
      display: block;
      position: relative;
      width: max-content;
      min-width: 100vw;
      min-height: 100vh;
      margin: 0;
      padding: var(--pad);
      background: var(--bg);
      color: var(--fg);
      font: inherit;
      font-size: clamp(
        9px,
        min(
          calc((100vw - (var(--pad) * 2)) / var(--terminal-width-denom)),
          calc((100vh - (var(--pad) * 2)) / var(--terminal-height-denom))
        ),
        16px
      );
      line-height: var(--terminal-line-height);
      white-space: pre;
      tab-size: 1;
    }}

    .terminal-row {{
      display: block;
      height: var(--terminal-line-height);
      white-space: pre;
    }}

    .terminal-row > span {{
      white-space: pre;
    }}

    .terminal-overlay {{
      position: absolute;
      left: var(--pad);
      top: var(--pad);
      z-index: 2;
      pointer-events: none;
    }}

    .terminal-block-rect {{
      position: absolute;
      width: 1ch;
    }}
"#
    )
}

fn generated_at() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;
    use ratatui::{buffer::Buffer, layout::Rect};
    use tokscale_core::ModelPerformance;

    use super::*;
    use crate::tui::data::{DailySourceInfo, DailyUsage, ModelUsage, TokenBreakdown};

    fn usage_fixture() -> UsageData {
        let model = ModelUsage {
            model: "claude-3-5-sonnet-20241022<script>".to_string(),
            provider: "anthropic".to_string(),
            client: "codex".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown {
                input: 1_000,
                output: 2_000,
                cache_read: 500,
                cache_write: 100,
                reasoning: 50,
            },
            cost: 1.25,
            performance: ModelPerformance::default(),
            session_count: 3,
        };

        let mut source_breakdown = BTreeMap::new();
        source_breakdown.insert(
            "codex".to_string(),
            DailySourceInfo {
                tokens: model.tokens.clone(),
                cost: model.cost,
                models: BTreeMap::new(),
            },
        );

        UsageData {
            total_tokens: model.tokens.total(),
            total_cost: model.cost,
            models: vec![model],
            daily: vec![DailyUsage {
                date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
                tokens: TokenBreakdown {
                    input: 1_000,
                    output: 2_000,
                    cache_read: 500,
                    cache_write: 100,
                    reasoning: 50,
                },
                cost: 1.25,
                source_breakdown,
                message_count: 4,
                turn_count: 2,
            }],
            current_streak: 1,
            longest_streak: 2,
            ..UsageData::default()
        }
    }

    #[test]
    fn overview_html_uses_tui_buffer_surface() {
        let html = render_overview_html(
            usage_fixture(),
            OverviewRenderOptions {
                clients: None,
                since: None,
                until: None,
                year: None,
                group_by: GroupBy::Model,
                width: 100,
                height: 28,
            },
        )
        .unwrap();

        assert!(html.contains("terminal-screen"));
        assert!(html.contains("Tokscale"));
        assert!(html.contains("Overview"));
        assert!(html.contains("Top Models"));
        assert!(html.contains(r#"data-cols="100""#));
        assert!(html.contains(r#"style="--terminal-cols:100;--terminal-rows:28;""#));
        assert!(html.contains("--terminal-width-denom: 61.00;"));
        assert!(html.contains("terminal-overlay"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("overview-grid"));
        assert!(!html.contains("<table>"));
        assert!(!html.contains("/ 96"));
    }

    #[test]
    fn buffer_html_escapes_cell_text() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer[(0, 0)].set_symbol("<");
        buffer[(1, 0)].set_symbol("&");
        buffer[(2, 0)].set_symbol(">");
        buffer[(3, 0)].set_symbol("\"");

        let html = render_buffer_html(&buffer, 4, 1);

        assert!(html.contains("&lt;&amp;&gt;&quot;"));
        assert!(!html.contains("<&>\""));
    }

    #[test]
    fn buffer_html_uses_block_rows_without_literal_line_breaks() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 2));
        write_buffer_row(&mut buffer, 0, "ab");
        write_buffer_row(&mut buffer, 1, "cd");

        let html = render_buffer_html(&buffer, 2, 2);

        assert!(!html.contains('\n'));
        assert_eq!(html.matches(r#"class="terminal-row""#).count(), 2);
    }

    #[test]
    fn buffer_html_replaces_block_glyphs_with_spaces() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("▆");

        let html = render_buffer_html_with_region(
            &buffer,
            1,
            1,
            Some(BlockOverlayRegion {
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 1,
            }),
        );

        assert!(html.contains("> </span>"));
        assert!(!html.contains("▆"));
    }

    #[test]
    fn chart_overlay_region_uses_left_chart_zero_axis() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 7));
        write_buffer_row(&mut buffer, 0, " Usage Trend (Daily)");
        write_buffer_row(&mut buffer, 1, "398.5M│     █");
        write_buffer_row(
            &mut buffer,
            2,
            "112.0M│     █        │┌ Provider Mix ─────────────────────┐",
        );
        write_buffer_row(&mut buffer, 3, "      │     █");
        write_buffer_row(
            &mut buffer,
            4,
            "     0│────────────────────────────────────────────",
        );
        write_buffer_row(&mut buffer, 5, "       Mar 22");

        let region = chart_overlay_region(&buffer, 80, 7).expect("chart region");

        assert_eq!(region.y_start, 1);
        assert_eq!(region.y_end, 4);
    }

    #[test]
    fn block_overlay_renders_partial_block_cell_height() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("▆");

        let html = render_block_overlay_region(
            &buffer,
            1,
            1,
            BlockOverlayRegion {
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 1,
            },
        );

        assert_eq!(html.matches("terminal-block-rect").count(), 1);
        assert!(html.contains("top:calc(0.250 * var(--terminal-line-height));"));
        assert!(html.contains("height:calc(0.750 * var(--terminal-line-height));"));
    }

    #[test]
    fn block_overlay_merges_contiguous_full_blocks() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 9));
        for y in 0..9 {
            buffer[(0, y)].set_symbol("█");
        }

        let html = render_block_overlay_region(
            &buffer,
            1,
            9,
            BlockOverlayRegion {
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 9,
            },
        );

        assert_eq!(html.matches("terminal-block-rect").count(), 1);
        assert!(html.contains("top:calc(0.000 * var(--terminal-line-height));"));
        assert!(html.contains("height:calc(9.000 * var(--terminal-line-height));"));
    }

    #[test]
    fn overview_json_summarizes_core_data() {
        let json = build_overview_json(&usage_fixture(), "All time".to_string(), 160, 48);

        assert_eq!(json.range_label, "All time");
        assert_eq!(json.width, 160);
        assert_eq!(json.height, 48);
        assert_eq!(json.total_tokens, 3_650);
        assert_eq!(json.total_cost_label, "$1.25");
        assert_eq!(json.active_days, 1);
        assert_eq!(json.model_count, 1);
    }

    fn write_buffer_row(buffer: &mut Buffer, y: u16, text: &str) {
        for (x, ch) in text.chars().enumerate() {
            buffer[(x as u16, y)].set_char(ch);
        }
    }
}
