use ratatui::buffer::{Buffer, Cell};

use super::html::{
    blend_hex_color, block_fill_fraction, block_level, cell_symbol, is_light_hex_color,
    parse_hex_color, HtmlCellStyle, HtmlColorPalette, DEFAULT_BG,
};

pub(super) fn render_block_overlay(
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

pub(super) fn render_block_overlay_region(
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
pub(super) struct DailyHeatmapOverlayCell {
    pub(super) fill: String,
    pub(super) active: bool,
}

#[derive(Clone, Debug)]
struct DailyHeatmapRenderedCell {
    column: u16,
    row: u16,
    fill: String,
    active: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DailyHeatmapMetrics {
    pub(super) col_pitch_ch: f64,
    pub(super) row_pitch_ch: f64,
    pub(super) top_offset_ch: f64,
}

pub(super) fn daily_heatmap_overlay_cell(
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

pub(super) fn web_heatmap_metrics(
    heatmap: DailyHeatmapGeometry,
    column_count: u16,
) -> DailyHeatmapMetrics {
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

pub(super) fn daily_heatmap_legend_fills(
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

#[derive(Clone, Debug)]
pub(super) struct ProviderMixBarOverlay {
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

pub(super) fn provider_mix_bar_overlay(
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

pub(super) fn overlay_cell_replacement(
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
pub(super) struct BlockOverlayRegion {
    pub(super) kind: BlockOverlayKind,
    pub(super) x_start: u16,
    pub(super) x_end: u16,
    pub(super) y_start: u16,
    pub(super) y_end: u16,
}

impl BlockOverlayRegion {
    fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x_start && x < self.x_end && y >= self.y_start && y < self.y_end
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum BlockOverlayKind {
    BlockChart,
    DailyHeatmap(DailyHeatmapGeometry),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DailyHeatmapGeometry {
    pub(super) grid_x: u16,
    pub(super) grid_y: u16,
    pub(super) grid_width: u16,
    pub(super) cell_width: u16,
    pub(super) cell_height: u16,
    pub(super) panel_bottom: u16,
    pub(super) legend_y: Option<u16>,
    pub(super) clear_x: u16,
    pub(super) clear_y: u16,
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

pub(super) fn chart_overlay_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
) -> Option<BlockOverlayRegion> {
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
