use anyhow::Result;
use chrono::Local;
use ratatui::{
    buffer::{Buffer, Cell},
    style::{Color, Modifier},
};
use serde::Serialize;
use tokscale_core::GroupBy;

use crate::report_format::format_currency;
use crate::tui::{surface::render_app_buffer, App, Theme, TuiConfig, UsageData};

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
    pub today_tokens: u64,
    pub today_cost: f64,
    pub today_cost_label: String,
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
    let today = Local::now().date_naive();
    let today_usage = data.daily.iter().find(|day| day.date == today);
    let today_tokens = today_usage.map(|day| day.tokens.total()).unwrap_or(0);
    let today_cost = today_usage.map(|day| day.cost).unwrap_or(0.0);

    OverviewJson {
        generated_at: generated_at(),
        range_label,
        width,
        height,
        today_tokens,
        today_cost,
        today_cost_label: format_currency(today_cost),
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
    let (surface, palette) = render_overview_surface_parts(data, options)?;
    Ok(render_buffer_page(&surface, width, height, &palette))
}

pub(crate) fn render_overview_surface(
    data: UsageData,
    options: OverviewRenderOptions,
) -> Result<String> {
    let (surface, _) = render_overview_surface_parts(data, options)?;
    Ok(surface)
}

fn render_overview_surface_parts(
    data: UsageData,
    options: OverviewRenderOptions,
) -> Result<(String, HtmlColorPalette)> {
    let width = options.width.max(40);
    let height = options.height.max(16);
    let mut app = App::new_surface_with_cached_data(
        TuiConfig {
            theme: None,
            refresh: 0,
            clients: options.clients,
            since: options.since,
            until: options.until,
            year: options.year,
            initial_tab: Some(crate::tui::Tab::Overview),
            initial_timeline_granularity: None,
        },
        Some(data),
    )?;
    app.theme = Theme::for_web_with_preference(app.settings.ui_theme);

    *app.group_by.borrow_mut() = options.group_by;
    app.status_message = None;
    app.status_message_time = None;
    app.set_background_loading(false);
    app.build_model_shade_map();

    let palette = HtmlColorPalette::from_theme(&app.theme);
    let buffer = render_app_buffer(&mut app, width, height)?;
    Ok((
        render_buffer_surface(&buffer, width, height, &palette),
        palette,
    ))
}

fn render_buffer_page(
    surface: &str,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
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
    <span id="cell-probe" aria-hidden="true">00000000000000000000</span>
    <div id="terminal-container">{surface}</div>
  </main>
  <script>
{script}
  </script>
</body>
</html>
"#,
        css = terminal_css(palette),
        surface = surface,
        script = resize_script(width, height),
    )
}

fn render_buffer_surface(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    format!(
        r#"<pre class="terminal-screen" role="img" aria-label="Tokscale Overview rendered from the TUI buffer" data-cols="{width}" data-rows="{height}" style="{terminal_style}">{surface}<span class="terminal-overlay" aria-hidden="true">{overlay}</span></pre>"#,
        width = width,
        height = height,
        terminal_style = terminal_style(width, height, palette),
        surface = render_buffer_html(buffer, width, height, palette),
        overlay = render_block_overlay(buffer, width, height, palette),
    )
}

fn render_buffer_html(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    let overlay_region = chart_overlay_region(buffer, width, height);
    let provider_mix_bar = provider_mix_bar_overlay(buffer, width, height, palette);
    render_buffer_html_with_region(
        buffer,
        width,
        height,
        overlay_region,
        provider_mix_bar.as_ref(),
        palette,
    )
}

fn render_buffer_html_with_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    overlay_region: Option<BlockOverlayRegion>,
    provider_mix_bar: Option<&ProviderMixBarOverlay>,
    palette: &HtmlColorPalette,
) -> String {
    let mut html = String::new();
    for y in 0..height {
        html.push_str(r#"<span class="terminal-row">"#);
        let mut x = 0;
        while x < width {
            let cell = &buffer[(x, y)];
            let style = HtmlCellStyle::from_cell(cell, palette);
            if let Some(replacement_style) =
                overlay_cell_replacement(buffer, overlay_region, provider_mix_bar, x, y, palette)
            {
                html.push_str(&replacement_style.open_span());
                html.push(' ');
                html.push_str("</span>");
                x += 1;
                continue;
            }

            let mut text = cell_text(cell);
            x += 1;

            while x < width {
                let next = &buffer[(x, y)];
                let next_style = HtmlCellStyle::from_cell(next, palette);
                if next_style != style
                    || overlay_cell_replacement(
                        buffer,
                        overlay_region,
                        provider_mix_bar,
                        x,
                        y,
                        palette,
                    )
                    .is_some()
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

fn render_block_overlay(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    let mut html = String::new();
    if let Some(region) = chart_overlay_region(buffer, width, height) {
        html.push_str(&render_block_overlay_region(
            buffer, width, height, region, palette,
        ));
    }
    if let Some(bar) = provider_mix_bar_overlay(buffer, width, height, palette) {
        html.push_str(&render_provider_mix_bar_overlay(&bar));
    }
    html
}

fn render_block_overlay_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    region: BlockOverlayRegion,
    palette: &HtmlColorPalette,
) -> String {
    if let BlockOverlayKind::DailyHeatmap(heatmap) = region.kind {
        return render_daily_heatmap_overlay(buffer, width, height, heatmap, palette);
    }

    let mut html = String::new();

    for x in region.x_start..region.x_end.min(width) {
        let mut current: Option<BlockRect> = None;
        for y in region.y_start..region.y_end.min(height) {
            let cell = &buffer[(x, y)];
            let Some(rect) = block_rect_for_cell(x, y, cell, palette) else {
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

fn render_daily_heatmap_overlay(
    buffer: &Buffer,
    width: u16,
    height: u16,
    heatmap: DailyHeatmapGeometry,
    palette: &HtmlColorPalette,
) -> String {
    let grid_right = heatmap.grid_x.saturating_add(heatmap.grid_width).min(width);
    let grid_bottom = heatmap.grid_bottom().min(height);
    let clear_bg = heatmap_clear_style(buffer, heatmap, palette).bg;
    let mut cells = Vec::new();

    let mut y = heatmap.grid_y;
    while y < grid_bottom {
        let mut x = heatmap.grid_x;
        while x < grid_right {
            let cell = &buffer[(x, y)];
            if let Some(cell) = daily_heatmap_overlay_cell(cell, &clear_bg, palette) {
                cells.push(DailyHeatmapRenderedCell {
                    column: x.saturating_sub(heatmap.grid_x) / heatmap.cell_width.max(1),
                    row: y.saturating_sub(heatmap.grid_y) / heatmap.cell_height.max(1),
                    fill: cell.fill,
                    active: cell.active,
                });
            }
            x = x.saturating_add(heatmap.cell_width.max(1));
        }
        y = y.saturating_add(heatmap.cell_height.max(1));
    }

    let column_count = cells
        .iter()
        .map(|cell| cell.column)
        .max()
        .map(|column| column.saturating_add(1))
        .unwrap_or(1);
    let metrics = web_heatmap_metrics(heatmap, column_count);

    let mut html = String::new();
    for cell in &cells {
        html.push_str(&daily_heatmap_cell_html(heatmap, metrics, cell));
    }
    html.push_str(&daily_heatmap_weekday_labels_html(heatmap, metrics));
    html.push_str(&daily_heatmap_legend_html(
        buffer, heatmap, metrics, &clear_bg, palette,
    ));

    html
}

#[derive(Clone, Debug)]
struct DailyHeatmapOverlayCell {
    fill: String,
    active: bool,
}

#[derive(Clone, Debug)]
struct DailyHeatmapRenderedCell {
    column: u16,
    row: u16,
    fill: String,
    active: bool,
}

#[derive(Clone, Copy, Debug)]
struct DailyHeatmapMetrics {
    col_pitch_ch: f64,
    row_pitch_ch: f64,
    top_offset_ch: f64,
}

fn daily_heatmap_overlay_cell(
    cell: &Cell,
    clear_bg: &str,
    palette: &HtmlColorPalette,
) -> Option<DailyHeatmapOverlayCell> {
    let style = HtmlCellStyle::from_cell(cell, palette);
    let symbol = cell_symbol(cell);
    if let Some(factor) = heatmap_active_intensity(symbol) {
        return Some(DailyHeatmapOverlayCell {
            fill: heatmap_active_fill(&style.fg, clear_bg, factor),
            active: true,
        });
    }

    if matches!(symbol, "·" | ".") {
        return Some(DailyHeatmapOverlayCell {
            fill: heatmap_empty_fill(clear_bg),
            active: false,
        });
    }

    if symbol.trim().is_empty() && style.bg != clear_bg {
        return Some(DailyHeatmapOverlayCell {
            fill: style.bg,
            active: false,
        });
    }

    None
}

fn web_heatmap_metrics(heatmap: DailyHeatmapGeometry, column_count: u16) -> DailyHeatmapMetrics {
    const TERMINAL_LINE_TO_CH: f64 = 1.64;
    const LEGEND_RESERVE_CH: f64 = 2.4;
    const ROW_LEGEND_RESERVE_CH: f64 = 1.0;
    const SOFT_BOTTOM_ALLOWANCE_CH: f64 = 1.6;
    let column_count = f64::from(column_count.max(1));
    let horizontal = f64::from(heatmap.grid_width.max(1)) / column_count;
    let vertical_rows = heatmap
        .panel_bottom
        .saturating_sub(heatmap.grid_y)
        .saturating_sub(1)
        .max(7);
    let available_ch = f64::from(vertical_rows) * TERMINAL_LINE_TO_CH;
    let vertical = ((available_ch - LEGEND_RESERVE_CH).max(0.0) / 7.0).max(1.8);
    let col_pitch_ch = horizontal.min(vertical).clamp(1.8, 5.0);
    let row_vertical = ((available_ch - ROW_LEGEND_RESERVE_CH).max(0.0) / 7.0).max(1.8);
    let row_pitch_ch = row_vertical
        .min(col_pitch_ch * 1.10)
        .max(col_pitch_ch)
        .clamp(1.8, 5.9);
    let content_ch = row_pitch_ch * 7.0 + LEGEND_RESERVE_CH;
    let centered_offset_ch = ((available_ch - content_ch).max(0.0) / 2.0).min(row_pitch_ch * 0.55);
    let visual_nudge_ch = (row_pitch_ch * 0.36).clamp(0.8, 1.6);
    let max_nudge_ch = (available_ch - row_pitch_ch * 7.0 - 0.8 + SOFT_BOTTOM_ALLOWANCE_CH)
        .max(0.0)
        .min(row_pitch_ch * 0.55);
    let top_offset_ch = centered_offset_ch.max(visual_nudge_ch.min(max_nudge_ch));

    DailyHeatmapMetrics {
        col_pitch_ch,
        row_pitch_ch,
        top_offset_ch,
    }
}

fn daily_heatmap_cell_html(
    heatmap: DailyHeatmapGeometry,
    metrics: DailyHeatmapMetrics,
    cell: &DailyHeatmapRenderedCell,
) -> String {
    let class = if cell.active {
        "terminal-heatmap-cell active"
    } else {
        "terminal-heatmap-cell empty"
    };
    format!(
        r#"<span class="{class}" style="--heatmap-col-pitch:{col_pitch:.3}ch;--heatmap-row-pitch:{row_pitch:.3}ch;--heatmap-gap:min(3px, calc(var(--heatmap-col-pitch) * 0.24));--heatmap-width:calc(var(--heatmap-col-pitch) - var(--heatmap-gap));--heatmap-height:calc(var(--heatmap-row-pitch) - var(--heatmap-gap));left:calc({left:.3}ch + (var(--heatmap-gap) / 2));top:calc({top_lines} * var(--terminal-line-height) + {top_ch:.3}ch + (var(--heatmap-gap) / 2));width:var(--heatmap-width);height:var(--heatmap-height);background-color:{fill};"></span>"#,
        col_pitch = metrics.col_pitch_ch,
        row_pitch = metrics.row_pitch_ch,
        top_lines = heatmap.grid_y,
        left = f64::from(heatmap.grid_x) + f64::from(cell.column) * metrics.col_pitch_ch,
        top_ch = metrics.top_offset_ch + f64::from(cell.row) * metrics.row_pitch_ch,
        fill = cell.fill,
    )
}

fn daily_heatmap_weekday_labels_html(
    heatmap: DailyHeatmapGeometry,
    metrics: DailyHeatmapMetrics,
) -> String {
    let mut html = String::new();
    for (row, label) in [(1u16, "Mon"), (3, "Wed"), (5, "Fri")] {
        html.push_str(&format!(
            r#"<span class="terminal-heatmap-axis-label" style="left:{left}ch;top:calc({top_lines} * var(--terminal-line-height) + {top_ch:.3}ch);">{label}</span>"#,
            left = heatmap.grid_x.saturating_sub(4),
            top_lines = heatmap.grid_y,
            top_ch = metrics.top_offset_ch + f64::from(row) * metrics.row_pitch_ch,
            label = label,
        ));
    }
    html
}

fn daily_heatmap_legend_html(
    buffer: &Buffer,
    heatmap: DailyHeatmapGeometry,
    metrics: DailyHeatmapMetrics,
    clear_bg: &str,
    palette: &HtmlColorPalette,
) -> String {
    let fills = daily_heatmap_legend_fills(buffer, heatmap, clear_bg, palette);
    let top_ch = metrics.top_offset_ch + 7.0 * metrics.row_pitch_ch + 0.8;
    let mut html = format!(
        r#"<span class="terminal-heatmap-legend" style="left:{left}ch;top:calc({top_lines} * var(--terminal-line-height) + {top_ch:.3}ch);"><span>Less</span>"#,
        left = heatmap.grid_x,
        top_lines = heatmap.grid_y,
        top_ch = top_ch,
    );

    for fill in fills {
        html.push_str(&format!(
            r#"<span class="terminal-heatmap-legend-cell" style="background-color:{fill};"></span>"#
        ));
    }
    html.push_str("<span>More</span></span>");
    html
}

fn daily_heatmap_legend_fills(
    buffer: &Buffer,
    heatmap: DailyHeatmapGeometry,
    clear_bg: &str,
    palette: &HtmlColorPalette,
) -> Vec<String> {
    if let Some(fills) = daily_heatmap_legend_fills_from_buffer(buffer, heatmap, clear_bg, palette)
    {
        return fills;
    }

    fallback_heatmap_legend_fills(clear_bg)
}

fn daily_heatmap_legend_fills_from_buffer(
    buffer: &Buffer,
    heatmap: DailyHeatmapGeometry,
    clear_bg: &str,
    palette: &HtmlColorPalette,
) -> Option<Vec<String>> {
    let legend_y = heatmap.legend_y?;
    let row = row_text(buffer, buffer.area.width, legend_y);
    let less_x = find_col(&row, "Less")?;
    let more_x = find_col(&row, "More")?;
    let mut fills = Vec::with_capacity(5);
    let mut current_fill = None::<String>;
    let start_x = less_x.saturating_add("Less ".len() as u16);

    for x in start_x..more_x.min(buffer.area.width) {
        let fill = daily_heatmap_overlay_cell(&buffer[(x, legend_y)], clear_bg, palette)
            .map(|cell| cell.fill);
        match (current_fill.take(), fill) {
            (Some(fill), Some(next)) if fill == next => {
                current_fill = Some(fill);
            }
            (Some(fill), Some(next)) => {
                fills.push(fill);
                current_fill = Some(next);
            }
            (Some(fill), None) => {
                fills.push(fill);
            }
            (None, Some(fill)) => {
                current_fill = Some(fill);
            }
            (None, None) => {}
        }
    }

    if let Some(fill) = current_fill {
        fills.push(fill);
    }

    (fills.len() == 5).then_some(fills)
}

fn fallback_heatmap_legend_fills(clear_bg: &str) -> Vec<String> {
    let levels = if is_light_hex_color(clear_bg) {
        ["#bdf3db", "#58d8a8", "#1ec287", "#129b67"]
    } else {
        ["#0e4429", "#006d32", "#26a641", "#39d353"]
    };
    let mut fills = Vec::with_capacity(5);
    fills.push(heatmap_empty_fill(clear_bg));
    fills.extend(levels.into_iter().map(str::to_string));
    fills
}

fn is_light_hex_color(value: &str) -> bool {
    let Some((r, g, b)) = parse_hex_color(value) else {
        return false;
    };
    u16::from(r) + u16::from(g) + u16::from(b) > 540
}

fn heatmap_empty_fill(clear_bg: &str) -> String {
    if clear_bg.eq_ignore_ascii_case(DEFAULT_BG) {
        return "#161b22".to_string();
    }

    let Some((r, g, b)) = parse_hex_color(clear_bg) else {
        return "#161b22".to_string();
    };
    let brightness = u16::from(r) + u16::from(g) + u16::from(b);
    let (target, factor) = if brightness > 540 {
        ("#000000", 0.045)
    } else {
        ("#ffffff", 0.055)
    };

    blend_hex_color(clear_bg, target, factor).unwrap_or_else(|| "#161b22".to_string())
}

fn heatmap_active_intensity(symbol: &str) -> Option<f64> {
    match symbol {
        "░" => Some(0.35),
        "▒" => Some(0.55),
        "▓" => Some(0.78),
        "█" => Some(1.0),
        _ => block_level(symbol).map(|level| match level {
            1 | 2 => 0.35,
            3 | 4 => 0.55,
            5 | 6 => 0.78,
            _ => 1.0,
        }),
    }
}

fn heatmap_active_fill(fg: &str, bg: &str, factor: f64) -> String {
    blend_hex_color(bg, fg, factor).unwrap_or_else(|| fg.to_string())
}

fn blend_hex_color(from: &str, to: &str, factor: f64) -> Option<String> {
    let (from_r, from_g, from_b) = parse_hex_color(from)?;
    let (to_r, to_g, to_b) = parse_hex_color(to)?;
    let blend = |from: u8, to: u8| -> u8 {
        let from = from as f64;
        let to = to as f64;
        (from + (to - from) * factor.clamp(0.0, 1.0))
            .round()
            .clamp(0.0, 255.0) as u8
    };

    Some(format!(
        "#{:02x}{:02x}{:02x}",
        blend(from_r, to_r),
        blend(from_g, to_g),
        blend(from_b, to_b)
    ))
}

#[derive(Clone, Debug)]
struct ProviderMixBarOverlay {
    y: u16,
    segments: Vec<ProviderMixBarSegment>,
}

impl ProviderMixBarOverlay {
    fn contains(&self, x: u16, y: u16) -> bool {
        y == self.y
            && self
                .segments
                .iter()
                .any(|segment| x >= segment.x_start && x < segment.x_end)
    }
}

#[derive(Clone, Debug)]
struct ProviderMixBarSegment {
    x_start: u16,
    x_end: u16,
    color: String,
}

fn provider_mix_bar_overlay(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> Option<ProviderMixBarOverlay> {
    for title_y in 0..height.saturating_sub(1) {
        let title_row = row_text(buffer, width, title_y);
        let Some(title_x) = find_col(&title_row, "Provider Mix") else {
            continue;
        };
        let (scan_start, scan_end) = provider_mix_bar_bounds(&title_row, title_x, width)?;
        let y = title_y.saturating_add(1);
        let segments = provider_mix_bar_segments(buffer, y, scan_start, scan_end, palette);
        if !segments.is_empty() {
            return Some(ProviderMixBarOverlay { y, segments });
        }
    }
    None
}

fn provider_mix_bar_bounds(row: &str, title_x: u16, width: u16) -> Option<(u16, u16)> {
    let chars: Vec<char> = row.chars().collect();
    let title_index = title_x as usize;
    let panel_left = chars
        .iter()
        .take(title_index)
        .rposition(|ch| matches!(ch, '┌' | '│'))? as u16;
    let panel_right = chars
        .iter()
        .enumerate()
        .skip(title_index)
        .find_map(|(index, ch)| (*ch == '┐').then_some(index as u16))
        .unwrap_or(width);

    let scan_start = panel_left.saturating_add(1).min(width);
    let scan_end = panel_right.min(width);
    (scan_start < scan_end).then_some((scan_start, scan_end))
}

fn provider_mix_bar_segments(
    buffer: &Buffer,
    y: u16,
    x_start: u16,
    x_end: u16,
    palette: &HtmlColorPalette,
) -> Vec<ProviderMixBarSegment> {
    let mut segments = Vec::new();
    let mut current: Option<ProviderMixBarSegment> = None;

    for x in x_start..x_end.min(buffer.area.width) {
        let cell = &buffer[(x, y)];
        if block_level(cell_symbol(cell)).is_none() {
            flush_provider_mix_bar_segment(&mut segments, &mut current);
            continue;
        }

        let color = HtmlCellStyle::from_cell(cell, palette).fg;
        if let Some(segment) = &mut current {
            if segment.color == color && segment.x_end == x {
                segment.x_end = x.saturating_add(1);
                continue;
            }
        }

        flush_provider_mix_bar_segment(&mut segments, &mut current);
        current = Some(ProviderMixBarSegment {
            x_start: x,
            x_end: x.saturating_add(1),
            color,
        });
    }

    flush_provider_mix_bar_segment(&mut segments, &mut current);
    segments
}

fn flush_provider_mix_bar_segment(
    segments: &mut Vec<ProviderMixBarSegment>,
    current: &mut Option<ProviderMixBarSegment>,
) {
    if let Some(segment) = current.take() {
        segments.push(segment);
    }
}

fn render_provider_mix_bar_overlay(bar: &ProviderMixBarOverlay) -> String {
    let mut html = String::new();
    for segment in &bar.segments {
        html.push_str(&format!(
            r#"<span class="terminal-mix-bar-segment" style="left:{left}ch;top:calc({top} * var(--terminal-line-height) + (var(--terminal-line-height) * .14));width:calc({width}ch + .5px);background-color:{color};"></span>"#,
            left = segment.x_start,
            top = bar.y,
            width = segment.x_end.saturating_sub(segment.x_start),
            color = segment.color,
        ));
    }
    html
}

fn overlay_cell_replacement(
    buffer: &Buffer,
    overlay_region: Option<BlockOverlayRegion>,
    provider_mix_bar: Option<&ProviderMixBarOverlay>,
    x: u16,
    y: u16,
    palette: &HtmlColorPalette,
) -> Option<HtmlCellStyle> {
    if provider_mix_bar.is_some_and(|bar| bar.contains(x, y)) {
        let mut style = HtmlCellStyle::from_cell(&buffer[(x, y)], palette);
        style.fg = style.bg.clone();
        return Some(style);
    }

    let region = overlay_region?;
    match region.kind {
        BlockOverlayKind::BlockChart => (block_level(cell_symbol(&buffer[(x, y)])).is_some()
            && region.contains(x, y))
        .then(|| HtmlCellStyle::from_cell(&buffer[(x, y)], palette)),
        BlockOverlayKind::DailyHeatmap(heatmap) => {
            if heatmap.contains_old_weekday_label(x, y) || heatmap.contains_old_legend(x, y) {
                return Some(heatmap_clear_style(buffer, heatmap, palette));
            }
            if !heatmap.contains_grid(x, y) {
                return None;
            }
            let clear_style = heatmap_clear_style(buffer, heatmap, palette);
            daily_heatmap_overlay_cell(&buffer[(x, y)], &clear_style.bg, palette)
                .map(|_| clear_style)
        }
    }
}

fn heatmap_clear_style(
    buffer: &Buffer,
    heatmap: DailyHeatmapGeometry,
    palette: &HtmlColorPalette,
) -> HtmlCellStyle {
    let area = buffer.area;
    let x = heatmap.clear_x.min(area.width.saturating_sub(1));
    let y = heatmap.clear_y.min(area.height.saturating_sub(1));
    let mut style = HtmlCellStyle::from_cell(&buffer[(x, y)], palette);
    style.fg = style.bg.clone();
    style
}

#[derive(Clone, Copy, Debug)]
struct BlockOverlayRegion {
    kind: BlockOverlayKind,
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

#[derive(Clone, Copy, Debug)]
enum BlockOverlayKind {
    BlockChart,
    DailyHeatmap(DailyHeatmapGeometry),
}

#[derive(Clone, Copy, Debug)]
struct DailyHeatmapGeometry {
    grid_x: u16,
    grid_y: u16,
    grid_width: u16,
    cell_width: u16,
    cell_height: u16,
    panel_bottom: u16,
    legend_y: Option<u16>,
    clear_x: u16,
    clear_y: u16,
}

impl DailyHeatmapGeometry {
    fn grid_bottom(self) -> u16 {
        self.grid_y
            .saturating_add(self.cell_height.saturating_mul(7))
    }

    fn contains_grid(self, x: u16, y: u16) -> bool {
        x >= self.grid_x
            && x < self.grid_x.saturating_add(self.grid_width)
            && y >= self.grid_y
            && y < self.grid_bottom()
    }

    fn contains_old_weekday_label(self, x: u16, y: u16) -> bool {
        x < self.grid_x && y >= self.grid_y && y < self.grid_bottom()
    }

    fn contains_old_legend(self, x: u16, y: u16) -> bool {
        self.legend_y == Some(y)
            && x >= self.grid_x
            && x < self.grid_x.saturating_add(self.grid_width)
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

    let mut title = None;
    for y in 0..height {
        let row = row_text(buffer, width, y);
        if let Some(kind) = chart_overlay_kind(&row) {
            title = Some((y, kind));
            break;
        }
    }
    let (title_y, kind) = title?;

    match kind {
        ChartOverlayKind::StackedBar => {
            stacked_chart_overlay_region(buffer, width, height, title_y, summary_x)
        }
        ChartOverlayKind::DailyHeatmap => {
            daily_heatmap_overlay_region(width, height, title_y, summary_x, buffer)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChartOverlayKind {
    StackedBar,
    DailyHeatmap,
}

fn chart_overlay_kind(row: &str) -> Option<ChartOverlayKind> {
    if row.contains("Daily Activity") {
        return Some(ChartOverlayKind::DailyHeatmap);
    }
    if row.contains("Usage Trend") {
        return Some(ChartOverlayKind::StackedBar);
    }
    None
}

fn stacked_chart_overlay_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    title_y: u16,
    summary_x: u16,
) -> Option<BlockOverlayRegion> {
    let mut axis_y = None;
    for y in title_y.saturating_add(1)..height {
        let row = row_text(buffer, width, y);
        if is_chart_axis_row(&row) {
            axis_y = Some(y);
            break;
        }
    }

    Some(BlockOverlayRegion {
        kind: BlockOverlayKind::BlockChart,
        x_start: 0,
        x_end: summary_x.saturating_sub(3).max(1),
        y_start: title_y.saturating_add(1),
        y_end: axis_y.unwrap_or_else(|| title_y.saturating_add(22).min(height)),
    })
}

fn daily_heatmap_overlay_region(
    width: u16,
    height: u16,
    title_y: u16,
    summary_x: u16,
    buffer: &Buffer,
) -> Option<BlockOverlayRegion> {
    if height <= title_y.saturating_add(1) {
        return None;
    }

    let title_row = row_text(buffer, width, title_y);
    let grid_x =
        find_col(&title_row, "Daily Activity").or_else(|| find_col(&title_row, "Activity"))?;
    let x_end = summary_x.saturating_sub(3).max(1);
    if grid_x >= x_end {
        return None;
    }

    let mut end_y = None;
    let mut legend_y = None;
    for y in title_y.saturating_add(1)..height {
        let row = row_text(buffer, width, y);
        if row.contains("Less") && row.contains("More") {
            legend_y = Some(y);
        }
        if is_overview_models_section_row(&row) {
            end_y = Some(y);
            break;
        }
    }

    let grid_y = title_y.saturating_add(2);
    let available_grid_height = legend_y
        .and_then(|y| y.checked_sub(grid_y).and_then(|delta| delta.checked_sub(1)))
        .unwrap_or(14);
    let cell_height = (available_grid_height / 7).clamp(1, 2);
    let grid_width = x_end.saturating_sub(grid_x);
    let cell_width = preferred_web_heatmap_cell_width(grid_width);
    let panel_bottom = end_y.unwrap_or_else(|| title_y.saturating_add(22).min(height));

    Some(BlockOverlayRegion {
        kind: BlockOverlayKind::DailyHeatmap(DailyHeatmapGeometry {
            grid_x,
            grid_y,
            grid_width,
            cell_width,
            cell_height,
            panel_bottom,
            legend_y,
            clear_x: grid_x,
            clear_y: title_y,
        }),
        x_start: 0,
        x_end,
        y_start: title_y.saturating_add(1),
        y_end: panel_bottom,
    })
}

fn preferred_web_heatmap_cell_width(grid_width: u16) -> u16 {
    const WEEK_WINDOW: u16 = 53;
    (grid_width / WEEK_WINDOW).clamp(2, 4)
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

fn is_overview_models_section_row(row: &str) -> bool {
    let trimmed = row.trim_start();
    trimmed.starts_with("Models ") || trimmed.starts_with("Top Models ")
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

fn block_rect_for_cell(
    x: u16,
    y: u16,
    cell: &Cell,
    palette: &HtmlColorPalette,
) -> Option<BlockRect> {
    let level = block_level(cell_symbol(cell))?;
    let fill = block_fill_fraction(level);
    let bottom = f64::from(y) + 1.0;
    Some(BlockRect {
        x,
        top: bottom - fill,
        bottom,
        style: HtmlCellStyle::from_cell(cell, palette),
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
struct HtmlColorPalette {
    fg: String,
    bg: String,
    muted: String,
    color_scheme: &'static str,
    ansi: Vec<String>,
}

impl HtmlColorPalette {
    fn from_theme(theme: &Theme) -> Self {
        let fg = raw_color_hex(theme.foreground, DEFAULT_FG);
        let bg = raw_color_hex(theme.background, DEFAULT_BG);
        let muted = raw_color_hex(theme.muted, "#8b949e");
        let color_scheme = if is_light_hex_color(&bg) {
            "light"
        } else {
            "dark"
        };
        let ansi = terminal_ansi_palette();

        Self {
            fg,
            bg,
            muted,
            color_scheme,
            ansi,
        }
    }

    #[cfg(test)]
    fn dark() -> Self {
        Self::from_theme(&Theme::for_web_with_preference(
            crate::tui::ThemePreference::Dark,
        ))
    }

    fn color_hex(&self, color: Color, fallback: &str) -> String {
        match color {
            Color::Reset => fallback.to_string(),
            Color::Black => self.ansi[0].clone(),
            Color::Red => self.ansi[1].clone(),
            Color::Green => self.ansi[2].clone(),
            Color::Yellow => self.ansi[3].clone(),
            Color::Blue => self.ansi[4].clone(),
            Color::Magenta => self.ansi[5].clone(),
            Color::Cyan => self.ansi[6].clone(),
            Color::Gray => self.ansi[7].clone(),
            Color::DarkGray => self.ansi[8].clone(),
            Color::LightRed => self.ansi[9].clone(),
            Color::LightGreen => self.ansi[10].clone(),
            Color::LightYellow => self.ansi[11].clone(),
            Color::LightBlue => self.ansi[12].clone(),
            Color::LightMagenta => self.ansi[13].clone(),
            Color::LightCyan => self.ansi[14].clone(),
            Color::White => self.ansi[15].clone(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Indexed(index) => self.indexed_color_hex(index),
        }
    }

    fn indexed_color_hex(&self, index: u8) -> String {
        self.ansi
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| self.fg.clone())
    }
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
    fn from_cell(cell: &Cell, palette: &HtmlColorPalette) -> Self {
        let mut fg = palette.color_hex(cell.fg, &palette.fg);
        let mut bg = palette.color_hex(cell.bg, &palette.bg);
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

fn raw_color_hex(color: Color, fallback: &str) -> String {
    match color {
        Color::Reset => fallback.to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#181818".to_string(),
        Color::Red => "#ac4242".to_string(),
        Color::Green => "#90a959".to_string(),
        Color::Yellow => "#f4bf75".to_string(),
        Color::Blue => "#6a9fb5".to_string(),
        Color::Magenta => "#aa759f".to_string(),
        Color::Cyan => "#75b5aa".to_string(),
        Color::Gray => "#d8d8d8".to_string(),
        Color::DarkGray => "#6b6b6b".to_string(),
        Color::LightRed => "#c55555".to_string(),
        Color::LightGreen => "#aac474".to_string(),
        Color::LightYellow => "#feca88".to_string(),
        Color::LightBlue => "#82b8c8".to_string(),
        Color::LightMagenta => "#c28cb8".to_string(),
        Color::LightCyan => "#93d3c3".to_string(),
        Color::White => "#f8f8f8".to_string(),
        Color::Indexed(index) => terminal_ansi_palette()
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| fallback.to_string()),
    }
}

fn terminal_ansi_palette() -> Vec<String> {
    [
        "#181818", "#ac4242", "#90a959", "#f4bf75", "#6a9fb5", "#aa759f", "#75b5aa", "#d8d8d8",
        "#6b6b6b", "#c55555", "#aac474", "#feca88", "#82b8c8", "#c28cb8", "#93d3c3", "#f8f8f8",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn terminal_style(width: u16, height: u16, palette: &HtmlColorPalette) -> String {
    format!(
        "--terminal-cols:{width};--terminal-rows:{height};--bg:{bg};--fg:{fg};--muted:{muted};",
        bg = palette.bg,
        fg = palette.fg,
        muted = palette.muted,
    )
}

fn resize_script(initial_width: u16, initial_height: u16) -> String {
    format!(
        r#"    (() => {{
      const minCols = 40;
      const maxCols = 320;
      const minRows = 16;
      const maxRows = 120;
      const pad = 8;
      const probe = document.getElementById('cell-probe');
      const container = document.getElementById('terminal-container');
      let currentKey = '{initial_width}x{initial_height}';
      let resizeTimer = 0;
      let controller = null;

      const clamp = (value, min, max) => Math.max(min, Math.min(max, value));

      const measuredCell = () => {{
        const rect = probe.getBoundingClientRect();
        const width = Math.max(1, rect.width / Math.max(1, probe.textContent.length));
        const height = Math.max(1, rect.height);
        return {{ width, height }};
      }};

      const terminalSize = () => {{
        const cell = measuredCell();
        const cols = clamp(Math.floor((window.innerWidth - pad * 2) / cell.width), minCols, maxCols);
        const rows = clamp(Math.floor((window.innerHeight - pad * 2) / cell.height), minRows, maxRows);
        return {{ cols, rows, key: `${{cols}}x${{rows}}` }};
      }};

      const refresh = async () => {{
        const size = terminalSize();
        if (size.key === currentKey) {{
          return;
        }}
        if (controller) {{
          controller.abort();
        }}
        controller = new AbortController();
        try {{
          const response = await fetch(`/surface?cols=${{size.cols}}&rows=${{size.rows}}`, {{
            signal: controller.signal,
            cache: 'no-store',
          }});
          if (!response.ok) {{
            return;
          }}
          container.innerHTML = await response.text();
          currentKey = size.key;
        }} catch (error) {{
          if (error.name !== 'AbortError') {{
            console.warn('Tokscale surface refresh failed', error);
          }}
        }}
      }};

      const schedule = () => {{
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(refresh, 80);
      }};

      window.addEventListener('resize', schedule, {{ passive: true }});
      refresh();
    }})();"#,
        initial_width = initial_width,
        initial_height = initial_height,
    )
}

fn terminal_css(palette: &HtmlColorPalette) -> String {
    const TERMINAL_LINE_HEIGHT: f64 = 1.0;
    format!(
        r#"    :root {{
      color-scheme: {color_scheme};
      --bg: {bg};
      --fg: {fg};
      --muted: {muted};
      --pad: 8px;
      --terminal-font-size: 12px;
      --terminal-line-height: {TERMINAL_LINE_HEIGHT:.2}em;
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
      font-size: var(--terminal-font-size);
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

    .terminal-mix-bar-segment {{
      position: absolute;
      z-index: 3;
      height: calc(var(--terminal-line-height) * .72);
      pointer-events: none;
    }}

    .terminal-heatmap-cell {{
      position: absolute;
      z-index: 3;
      pointer-events: none;
      border-radius: 2px;
    }}

    .terminal-heatmap-cell.empty {{
      box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.055);
    }}

    .terminal-heatmap-cell.active {{
      box-shadow:
        inset 0 0 0 1px rgba(255, 255, 255, 0.10),
        0 0 0 1px rgba(0, 0, 0, 0.10);
    }}

    .terminal-heatmap-axis-label {{
      position: absolute;
      z-index: 4;
      color: var(--muted);
      line-height: var(--terminal-line-height);
      pointer-events: none;
    }}

    .terminal-heatmap-legend {{
      position: absolute;
      z-index: 4;
      display: inline-flex;
      align-items: center;
      gap: 4px;
      color: var(--muted);
      line-height: var(--terminal-line-height);
      pointer-events: none;
      white-space: nowrap;
    }}

    .terminal-heatmap-legend-cell {{
      display: inline-block;
      width: 10px;
      height: 10px;
      border-radius: 2px;
      box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    }}

    #cell-probe {{
      position: fixed;
      left: -9999px;
      top: -9999px;
      visibility: hidden;
      white-space: pre;
      font: inherit;
      font-size: var(--terminal-font-size);
      line-height: var(--terminal-line-height);
    }}
"#,
        bg = palette.bg,
        fg = palette.fg,
        muted = palette.muted,
        color_scheme = palette.color_scheme,
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
        assert!(html.contains("--terminal-cols:100;"));
        assert!(html.contains("--terminal-rows:28;"));
        assert!(html.contains("--bg:#0d1117;"));
        assert!(html.contains("--fg:#c9d1d9;"));
        assert!(html.contains("--terminal-font-size: 12px;"));
        assert!(html.contains("/surface?cols="));
        assert!(html.contains("terminal-overlay"));
        assert!(!html.contains("20241022<script>"));
        assert!(!html.contains("overview-grid"));
        assert!(!html.contains("<table>"));
        assert!(!html.contains("/ 96"));
    }

    #[test]
    fn overview_surface_renders_only_terminal_buffer() {
        let html = render_overview_surface(
            usage_fixture(),
            OverviewRenderOptions {
                clients: None,
                since: None,
                until: None,
                year: None,
                group_by: GroupBy::Model,
                width: 88,
                height: 24,
            },
        )
        .unwrap();

        assert!(html.starts_with(r#"<pre class="terminal-screen""#));
        assert!(html.contains(r#"data-cols="88""#));
        assert!(html.contains(r#"data-rows="24""#));
        assert!(!html.contains("<!doctype html>"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn buffer_html_escapes_cell_text() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer[(0, 0)].set_symbol("<");
        buffer[(1, 0)].set_symbol("&");
        buffer[(2, 0)].set_symbol(">");
        buffer[(3, 0)].set_symbol("\"");

        let html = render_buffer_html(&buffer, 4, 1, &palette);

        assert!(html.contains("&lt;&amp;&gt;&quot;"));
        assert!(!html.contains("<&>\""));
    }

    #[test]
    fn buffer_html_uses_block_rows_without_literal_line_breaks() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 2));
        write_buffer_row(&mut buffer, 0, "ab");
        write_buffer_row(&mut buffer, 1, "cd");

        let html = render_buffer_html(&buffer, 2, 2, &palette);

        assert!(!html.contains('\n'));
        assert_eq!(html.matches(r#"class="terminal-row""#).count(), 2);
    }

    #[test]
    fn buffer_html_replaces_block_glyphs_with_spaces() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("▆");

        let html = render_buffer_html_with_region(
            &buffer,
            1,
            1,
            Some(BlockOverlayRegion {
                kind: BlockOverlayKind::BlockChart,
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 1,
            }),
            None,
            &palette,
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
    fn chart_overlay_region_tracks_daily_activity_heatmap() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 8));
        write_buffer_row(&mut buffer, 0, " Daily Activity (52w)");
        write_buffer_row(&mut buffer, 1, "     Jun");
        write_buffer_row(&mut buffer, 2, "Mon  █");
        write_buffer_row(
            &mut buffer,
            3,
            "Wed  █       │┌ Summary ─────────────────────┐",
        );
        write_buffer_row(&mut buffer, 4, "Fri");
        write_buffer_row(&mut buffer, 5, "Less █ █ █ More");
        write_buffer_row(&mut buffer, 6, "Models  ● gpt-5.5");

        let region = chart_overlay_region(&buffer, 80, 8).expect("heatmap region");

        assert_eq!(region.y_start, 1);
        assert_eq!(region.y_end, 6);
        assert!(region.x_end < 80);
    }

    #[test]
    fn buffer_html_projects_daily_heatmap_blocks_into_overlay() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 6));
        write_buffer_row(&mut buffer, 0, " Daily Activity (52w)");
        write_buffer_row(&mut buffer, 1, "     Jun");
        buffer[(1, 2)].set_symbol("·").set_fg(Color::DarkGray);
        buffer[(5, 2)].set_symbol("█").set_fg(Color::Green);
        buffer[(5, 3)].set_symbol("█").set_fg(Color::Green);
        write_buffer_row(&mut buffer, 4, "Models  ● gpt-5.5");

        let surface = render_buffer_html(&buffer, 40, 6, &palette);
        let overlay = render_block_overlay(&buffer, 40, 6, &palette);

        assert!(!surface.contains("█"), "{surface}");
        assert!(!surface.contains("·"), "{surface}");
        assert!(overlay.contains("terminal-heatmap-cell empty"), "{overlay}");
        assert!(
            overlay.contains("terminal-heatmap-cell active"),
            "{overlay}"
        );
        assert!(overlay.contains("terminal-heatmap-axis-label"), "{overlay}");
        assert!(overlay.contains("terminal-heatmap-legend"), "{overlay}");
        assert!(
            overlay.contains("--heatmap-gap:min(3px, calc(var(--heatmap-col-pitch) * 0.24))"),
            "{overlay}"
        );
        assert!(overlay.contains("--heatmap-row-pitch:"), "{overlay}");
        assert!(
            overlay.contains("left:calc(4.600ch + (var(--heatmap-gap) / 2))"),
            "{overlay}"
        );
    }

    #[test]
    fn web_heatmap_metrics_expand_and_center_tall_overview_panel() {
        let compact = DailyHeatmapGeometry {
            grid_x: 4,
            grid_y: 2,
            grid_width: 220,
            cell_width: 2,
            cell_height: 1,
            panel_bottom: 10,
            legend_y: None,
            clear_x: 4,
            clear_y: 0,
        };
        let expanded = DailyHeatmapGeometry {
            panel_bottom: 24,
            ..compact
        };
        let very_tall = DailyHeatmapGeometry {
            panel_bottom: 36,
            ..compact
        };

        let compact_metrics = web_heatmap_metrics(compact, 53);
        let expanded_metrics = web_heatmap_metrics(expanded, 53);
        let very_tall_metrics = web_heatmap_metrics(very_tall, 53);

        assert!(
            expanded_metrics.row_pitch_ch > compact_metrics.row_pitch_ch,
            "{} <= {}",
            expanded_metrics.row_pitch_ch,
            compact_metrics.row_pitch_ch
        );
        assert!(expanded_metrics.row_pitch_ch > expanded_metrics.col_pitch_ch);
        assert!(very_tall_metrics.top_offset_ch > expanded_metrics.top_offset_ch);
    }

    #[test]
    fn heatmap_overlay_preserves_glyph_intensity_as_color_tint() {
        let palette = HtmlColorPalette::dark();
        let clear_bg = DEFAULT_BG;
        let mut low = Cell::default();
        low.set_symbol("░").set_fg(Color::Red);
        let mut mid = Cell::default();
        mid.set_symbol("▒").set_fg(Color::Red);
        let mut high = Cell::default();
        high.set_symbol("▓").set_fg(Color::Red);
        let mut full = Cell::default();
        full.set_symbol("█").set_fg(Color::Red);

        let low = daily_heatmap_overlay_cell(&low, clear_bg, &palette)
            .unwrap()
            .fill;
        let mid = daily_heatmap_overlay_cell(&mid, clear_bg, &palette)
            .unwrap()
            .fill;
        let high = daily_heatmap_overlay_cell(&high, clear_bg, &palette)
            .unwrap()
            .fill;
        let full = daily_heatmap_overlay_cell(&full, clear_bg, &palette)
            .unwrap()
            .fill;

        assert_ne!(low, mid);
        assert_ne!(mid, high);
        assert_ne!(high, full);
        assert_eq!(full, "#ac4242");
    }

    #[test]
    fn heatmap_legend_falls_back_to_fixed_intensity_fills() {
        let palette = HtmlColorPalette::dark();
        let buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        let heatmap = DailyHeatmapGeometry {
            grid_x: 0,
            grid_y: 0,
            grid_width: 1,
            cell_width: 1,
            cell_height: 1,
            panel_bottom: 1,
            legend_y: None,
            clear_x: 0,
            clear_y: 0,
        };

        let dark = daily_heatmap_legend_fills(&buffer, heatmap, DEFAULT_BG, &palette);
        assert_eq!(
            dark,
            vec!["#161b22", "#0e4429", "#006d32", "#26a641", "#39d353"]
        );

        let light = daily_heatmap_legend_fills(&buffer, heatmap, "#ffffff", &palette);
        assert_eq!(&light[1..], ["#bdf3db", "#58d8a8", "#1ec287", "#129b67"]);
    }

    #[test]
    fn heatmap_legend_reads_exact_tui_swatch_colors() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 1));
        write_buffer_row(&mut buffer, 0, "Less                 More");
        let heatmap = DailyHeatmapGeometry {
            grid_x: 0,
            grid_y: 0,
            grid_width: 1,
            cell_width: 1,
            cell_height: 1,
            panel_bottom: 1,
            legend_y: Some(0),
            clear_x: 0,
            clear_y: 0,
        };

        for x in 5..7 {
            buffer[(x, 0)]
                .set_symbol(" ")
                .set_bg(Color::Rgb(22, 27, 34));
        }
        for x in 8..10 {
            buffer[(x, 0)].set_symbol("█").set_fg(Color::DarkGray);
        }
        for x in 11..13 {
            buffer[(x, 0)].set_symbol("█").set_fg(Color::Green);
        }
        for x in 14..16 {
            buffer[(x, 0)].set_symbol("█").set_fg(Color::Green);
        }
        for x in 17..19 {
            buffer[(x, 0)].set_symbol("█").set_fg(Color::LightGreen);
        }

        let fills = daily_heatmap_legend_fills(&buffer, heatmap, DEFAULT_BG, &palette);

        assert_eq!(
            fills,
            vec!["#161b22", "#6b6b6b", "#90a959", "#90a959", "#aac474"]
        );
    }

    #[test]
    fn heatmap_legend_clear_preserves_sidebar_text() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 88, 5));
        write_buffer_row(
            &mut buffer,
            0,
            " Daily Activity (52w)                  Summary",
        );
        write_buffer_row(&mut buffer, 1, "     Jun");
        write_buffer_row(&mut buffer, 2, "Mon  █");
        write_buffer_row(
            &mut buffer,
            3,
            "     Less █ █ █ More                   ● Moonshot AI",
        );
        write_buffer_row(&mut buffer, 4, "Models  ● gpt-5.5");

        let surface = render_buffer_html(&buffer, 88, 5, &palette);

        assert!(!surface.contains("Less"), "{surface}");
        assert!(surface.contains("Moonshot AI"), "{surface}");
    }

    #[test]
    fn provider_mix_bar_uses_web_rectangles_instead_of_block_glyphs() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 48, 3));
        write_buffer_row(&mut buffer, 0, "│┌ Provider Mix ─────────────────────┐");
        write_buffer_row(&mut buffer, 1, "││███████████████████████████████████│");
        write_buffer_row(&mut buffer, 2, "││● OpenAI                     $1.5K│");

        for x in 2..14 {
            buffer[(x, 1)].set_symbol("█").set_fg(Color::Green);
        }
        for x in 14..20 {
            buffer[(x, 1)].set_symbol("█").set_fg(Color::Red);
        }

        let surface = render_buffer_html(&buffer, 48, 3, &palette);
        let overlay = render_block_overlay(&buffer, 48, 3, &palette);

        assert!(!surface.contains("█"), "{surface}");
        assert!(overlay.contains("terminal-mix-bar-segment"), "{overlay}");
        assert!(overlay.contains("background-color:#90a959;"), "{overlay}");
        assert!(overlay.contains("background-color:#ac4242;"), "{overlay}");
    }

    #[test]
    fn block_overlay_renders_partial_block_cell_height() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("▆");

        let html = render_block_overlay_region(
            &buffer,
            1,
            1,
            BlockOverlayRegion {
                kind: BlockOverlayKind::BlockChart,
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 1,
            },
            &palette,
        );

        assert_eq!(html.matches("terminal-block-rect").count(), 1);
        assert!(html.contains("top:calc(0.250 * var(--terminal-line-height));"));
        assert!(html.contains("height:calc(0.750 * var(--terminal-line-height));"));
    }

    #[test]
    fn block_overlay_merges_contiguous_full_blocks() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 9));
        for y in 0..9 {
            buffer[(0, y)].set_symbol("█");
        }

        let html = render_block_overlay_region(
            &buffer,
            1,
            9,
            BlockOverlayRegion {
                kind: BlockOverlayKind::BlockChart,
                x_start: 0,
                x_end: 1,
                y_start: 0,
                y_end: 9,
            },
            &palette,
        );

        assert_eq!(html.matches("terminal-block-rect").count(), 1);
        assert!(html.contains("top:calc(0.000 * var(--terminal-line-height));"));
        assert!(html.contains("height:calc(9.000 * var(--terminal-line-height));"));
    }

    #[test]
    fn overview_json_summarizes_core_data() {
        let mut usage = usage_fixture();
        usage.daily[0].date = Local::now().date_naive();

        let json = build_overview_json(&usage, "All time".to_string(), 160, 48);

        assert_eq!(json.range_label, "All time");
        assert_eq!(json.width, 160);
        assert_eq!(json.height, 48);
        assert_eq!(json.today_tokens, 3_650);
        assert_eq!(json.today_cost_label, "$1.25");
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
