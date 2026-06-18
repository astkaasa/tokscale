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

const DEFAULT_WIDTH: u16 = 160;
const DEFAULT_HEIGHT: u16 = 48;
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
    <pre class="terminal-screen" role="img" aria-label="Tokscale Overview rendered from the TUI buffer" data-cols="{width}" data-rows="{height}">{surface}</pre>
  </main>
</body>
</html>
"#,
        css = terminal_css(),
        width = width,
        height = height,
        surface = render_buffer_html(buffer, width, height),
    )
}

fn render_buffer_html(buffer: &Buffer, width: u16, height: u16) -> String {
    let mut html = String::new();
    for y in 0..height {
        html.push_str(r#"<span class="terminal-row">"#);
        let mut x = 0;
        while x < width {
            let cell = &buffer[(x, y)];
            let style = HtmlCellStyle::from_cell(cell);
            let mut text = cell_text(cell);
            x += 1;

            while x < width {
                let next = &buffer[(x, y)];
                let next_style = HtmlCellStyle::from_cell(next);
                if next_style != style {
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
        if y + 1 < height {
            html.push('\n');
        }
    }
    html
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

fn cell_text(cell: &Cell) -> String {
    let symbol = cell.symbol();
    if symbol.is_empty() {
        " ".to_string()
    } else {
        symbol.to_string()
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

fn terminal_css() -> &'static str {
    r#"    :root {
      color-scheme: dark;
      --bg: #0d1117;
      --fg: #c9d1d9;
    }

    * {
      box-sizing: border-box;
    }

    html,
    body {
      width: 100%;
      height: 100%;
    }

    body {
      margin: 0;
      overflow: hidden;
      background: var(--bg);
      color: var(--fg);
      font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Monaco, Consolas, "Liberation Mono", monospace;
      font-variant-ligatures: none;
      font-variant-numeric: tabular-nums;
      letter-spacing: 0;
    }

    .viewport {
      width: 100vw;
      height: 100vh;
      margin: 0;
      overflow: auto;
      background: var(--bg);
    }

    .terminal-screen {
      display: inline-block;
      min-width: 100vw;
      min-height: 100vh;
      margin: 0;
      padding: 8px;
      background: var(--bg);
      color: var(--fg);
      font: inherit;
      font-size: clamp(10px, min(calc((100vw - 16px) / 96), calc((100vh - 16px) / 48)), 16px);
      line-height: 1;
      white-space: pre;
      text-rendering: geometricPrecision;
    }

    .terminal-row {
      display: block;
      height: 1em;
      white-space: pre;
    }

    .terminal-row > span {
      white-space: pre;
    }
"#
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
        assert!(!html.contains("<script>"));
        assert!(!html.contains("overview-grid"));
        assert!(!html.contains("<table>"));
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
}
