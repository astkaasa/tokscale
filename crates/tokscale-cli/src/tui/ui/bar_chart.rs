use ratatui::prelude::*;

use super::widgets::format_tokens;
use crate::tui::app::{App, ChartGranularity, ClickAction, OverviewMode, PeriodDetailKey};

/// 8-level block characters for sub-cell precision.
const BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const BAR_WIDTH: usize = 1;
const BAR_GAP: usize = 1;

const MONTH_NAMES: &[&str] = &[
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A single model's contribution to a bar
#[derive(Debug, Clone)]
pub struct ModelSegment {
    pub model_id: String,
    pub tokens: u64,
    pub color: Color,
}

/// Data for a single bar in the stacked chart
#[derive(Debug, Clone)]
pub struct StackedBarData {
    pub date: String,
    pub period: Option<PeriodDetailKey>,
    pub models: Vec<ModelSegment>,
    pub total: u64,
}

struct ChartScale {
    display_max: f64,
    actual_max: f64,
    focus_max: f64,
    compressed: bool,
}

/// Render a stacked bar chart where each bar shows model breakdown
pub fn render_stacked_bar_chart(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    data: &[StackedBarData],
) {
    if data.is_empty() {
        return;
    }

    let is_very_narrow = app.is_very_narrow();
    let y_label_width: u16 = if is_very_narrow { 6 } else { 7 };
    let plot_x = area.x.saturating_add(y_label_width);
    let plot_y = area.y.saturating_add(1);
    let plot_width = area.width.saturating_sub(y_label_width);
    let plot_height = area.height.saturating_sub(3);

    if plot_width == 0 || plot_height == 0 {
        return;
    }

    let scale = chart_scale(data);
    let display_data: Vec<StackedBarData> = data
        .iter()
        .map(|bar| scaled_bar_for_display(bar, &scale))
        .collect();

    let buf = frame.buffer_mut();
    let bar_count = data.len();
    let chart_layout = chart_layout(
        plot_width as usize,
        bar_count,
        app.overview_chart_scroll_offset,
    );
    app.overview_chart_scroll_offset = chart_layout.scroll_offset;

    // Title
    let title = chart_title(app, is_very_narrow);
    let title_y = area.y;
    for (i, ch) in title.chars().enumerate() {
        let x = area.x + y_label_width + i as u16;
        if x < area.x + area.width {
            buf[(x, title_y)]
                .set_char(ch)
                .set_style(Style::default().add_modifier(Modifier::BOLD));
        }
    }
    if scale.compressed {
        let peak = format!("Peak {}", format_tokens(scale.actual_max.round() as u64));
        render_title_suffix(
            buf,
            area,
            y_label_width,
            title,
            &peak,
            app.theme.subtle_text_style(),
        );
    }
    render_horizontal_scroll_indicator(buf, app, plot_x, title_y, plot_width, &chart_layout);

    let mid_row_from_top = plot_height / 2;
    let focus_row_from_top = if scale.compressed {
        Some(row_for_display_value(
            scale.focus_max,
            scale.display_max,
            plot_height,
        ))
    } else {
        None
    };
    for row_from_top in 0..plot_height {
        let y = plot_y + row_from_top;
        let is_top = row_from_top == 0;
        let is_mid = row_from_top == mid_row_from_top && plot_height >= 6;
        let is_focus = focus_row_from_top == Some(row_from_top);

        let y_label = if is_top {
            format_y_axis_label(
                scale.actual_max.round() as u64,
                (y_label_width - 1) as usize,
            )
        } else if is_focus {
            format_y_axis_label(scale.focus_max.round() as u64, (y_label_width - 1) as usize)
        } else if is_mid {
            format_y_axis_label(
                (scale.actual_for_display_value(scale.display_max / 2.0)).round() as u64,
                (y_label_width - 1) as usize,
            )
        } else {
            String::new()
        };
        let padded_label = format!("{:>width$}│", y_label, width = (y_label_width - 1) as usize);
        for (i, ch) in padded_label.chars().enumerate() {
            let x = area.x + i as u16;
            if x < area.x + y_label_width {
                buf[(x, y)]
                    .set_char(ch)
                    .set_style(Style::default().fg(app.theme.muted));
            }
        }

        let grid_char = if is_focus || (!scale.compressed && is_mid) {
            '┈'
        } else {
            ' '
        };
        for x in plot_x..plot_x.saturating_add(plot_width) {
            buf[(x, y)]
                .set_char(grid_char)
                .set_style(Style::default().fg(app.theme.muted));
        }
    }

    for (bar_index, bar_data) in data.iter().enumerate() {
        let Some(period) = bar_data.period.clone() else {
            continue;
        };
        let Some((offset, width)) = chart_layout.positions.get(bar_index).copied().flatten() else {
            continue;
        };
        app.add_click_area(
            Rect::new(
                plot_x.saturating_add(offset as u16),
                plot_y,
                width.max(1) as u16,
                plot_height.saturating_add(2),
            ),
            ClickAction::OpenPeriodDetail(period),
        );
    }
    for (bar_index, bar_data) in display_data.iter().enumerate() {
        let Some((offset, width)) = chart_layout.positions.get(bar_index).copied().flatten() else {
            continue;
        };
        let x_start = plot_x.saturating_add(offset as u16);
        let mut rows = Vec::with_capacity(plot_height as usize);
        for row_from_top in 0..plot_height as usize {
            let row_from_bottom = plot_height as usize - 1 - row_from_top;
            let row_threshold =
                ((row_from_bottom + 1) as f64 / plot_height as f64) * scale.display_max;
            let prev_threshold = (row_from_bottom as f64 / plot_height as f64) * scale.display_max;
            let threshold_diff = row_threshold - prev_threshold;

            let (ch, fg_color) = get_stacked_bar_content(
                bar_data,
                bar_data.total as f64,
                row_threshold,
                prev_threshold,
                threshold_diff,
                app.theme.muted,
                app.theme.highlight,
            );
            rows.push((ch != ' ').then_some(BarCell { ch, fg_color }));
        }
        for (row_from_top, cell) in rows.iter().enumerate() {
            let Some(cell) = cell else { continue };
            let y = plot_y + row_from_top as u16;
            for dx in 0..width {
                let x = x_start.saturating_add(dx as u16);
                if x < plot_x.saturating_add(plot_width) {
                    buf[(x, y)].set_char(cell.ch).set_fg(cell.fg_color);
                }
            }
        }
    }

    let axis_y = plot_y + plot_height;
    if axis_y < area.y + area.height {
        let zero_label = format!("{:>width$}│", "0", width = (y_label_width - 1) as usize);
        for (i, ch) in zero_label.chars().enumerate() {
            let x = area.x + i as u16;
            if x < area.x + y_label_width {
                buf[(x, axis_y)]
                    .set_char(ch)
                    .set_style(Style::default().fg(app.theme.muted));
            }
        }
        for x in plot_x..plot_x.saturating_add(plot_width) {
            buf[(x, axis_y)]
                .set_char('─')
                .set_style(Style::default().fg(app.theme.muted));
        }
    }

    let label_y = axis_y + 1;
    if label_y < area.y + area.height && !data.is_empty() {
        let label_all_months = app.overview_mode == OverviewMode::All
            && app.chart_granularity == ChartGranularity::Monthly;
        for index in label_indices(
            bar_count,
            is_very_narrow,
            label_all_months,
            &chart_layout.positions,
        ) {
            let label = format_axis_label(&data[index].date, is_very_narrow);
            let Some((bar_x, width)) = chart_layout.positions.get(index).copied().flatten() else {
                continue;
            };
            let label_width = label.chars().count() as u16;
            let label_x = centered_label_x(plot_x, plot_width, bar_x, width, label_width);
            for (j, ch) in label.chars().enumerate() {
                let x = label_x + j as u16;
                if x < area.x + area.width {
                    buf[(x, label_y)]
                        .set_char(ch)
                        .set_style(Style::default().fg(app.theme.muted));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BarCell {
    ch: char,
    fg_color: Color,
}

fn chart_scale(data: &[StackedBarData]) -> ChartScale {
    let actual_max = data
        .iter()
        .map(|d| d.total as f64)
        .fold(0.0_f64, |a, b| a.max(b))
        .max(1.0);
    let mut values: Vec<f64> = data
        .iter()
        .map(|d| d.total as f64)
        .filter(|value| *value > 0.0)
        .collect();
    values.sort_by(|a, b| a.total_cmp(b));

    if values.len() < 10 {
        return ChartScale {
            display_max: actual_max,
            actual_max,
            focus_max: actual_max,
            compressed: false,
        };
    }

    let focus_max = percentile_value(&values, 0.9) * 1.5;
    let should_compress = actual_max > focus_max * 1.4 && focus_max > 0.0;
    let focus_max = focus_max.max(1.0).min(actual_max);
    let display_max = if should_compress {
        focus_max + focus_max * 0.35
    } else {
        actual_max
    };

    ChartScale {
        display_max,
        actual_max,
        focus_max,
        compressed: should_compress,
    }
}

impl ChartScale {
    fn display_value(&self, actual_value: f64) -> f64 {
        if !self.compressed || actual_value <= self.focus_max {
            return actual_value.min(self.display_max);
        }

        let actual_overflow = (self.actual_max - self.focus_max).max(1.0);
        let display_overflow = (self.display_max - self.focus_max).max(1.0);
        self.focus_max
            + ((actual_value - self.focus_max).max(0.0) / actual_overflow) * display_overflow
    }

    fn actual_for_display_value(&self, display_value: f64) -> f64 {
        if !self.compressed || display_value <= self.focus_max {
            return display_value.min(self.actual_max);
        }

        let actual_overflow = (self.actual_max - self.focus_max).max(1.0);
        let display_overflow = (self.display_max - self.focus_max).max(1.0);
        self.focus_max
            + ((display_value - self.focus_max).max(0.0) / display_overflow) * actual_overflow
    }
}

fn row_for_display_value(value: f64, display_max: f64, plot_height: u16) -> u16 {
    if plot_height <= 1 || display_max <= 0.0 {
        return 0;
    }

    let ratio = (value / display_max).clamp(0.0, 1.0);
    let row_from_bottom = (ratio * (plot_height - 1) as f64).round() as u16;
    plot_height - 1 - row_from_bottom
}

fn percentile_value(sorted_values: &[f64], percentile: f64) -> f64 {
    if sorted_values.is_empty() {
        return 1.0;
    }

    let index = ((sorted_values.len() - 1) as f64 * percentile)
        .round()
        .clamp(0.0, (sorted_values.len() - 1) as f64) as usize;
    sorted_values[index]
}

fn format_y_axis_label(tokens: u64, max_width: usize) -> String {
    let label = format_tokens(tokens);
    if label.chars().count() <= max_width {
        return label;
    }

    let whole = format_tokens_whole(tokens);
    if whole.chars().count() <= max_width {
        return whole;
    }

    format_tokens_unit(tokens)
}

fn format_tokens_whole(tokens: u64) -> String {
    if tokens >= 999_500_000 {
        format!("{}B", (tokens as f64 / 1_000_000_000.0).round() as u64)
    } else if tokens >= 1_000_000 {
        format!("{}M", (tokens as f64 / 1_000_000.0).round() as u64)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn format_tokens_unit(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        "B".to_string()
    } else if tokens >= 1_000_000 {
        "M".to_string()
    } else if tokens >= 1_000 {
        "K".to_string()
    } else {
        tokens.to_string()
    }
}

fn scaled_bar_for_display(bar: &StackedBarData, scale: &ChartScale) -> StackedBarData {
    if !scale.compressed || bar.total == 0 || (bar.total as f64) <= scale.focus_max {
        return bar.clone();
    }

    let display_total = scale.display_value(bar.total as f64).round().max(1.0) as u64;
    let mut assigned = 0u64;
    let mut models = Vec::with_capacity(bar.models.len());
    for (index, segment) in bar.models.iter().enumerate() {
        let tokens = if index == bar.models.len().saturating_sub(1) {
            display_total.saturating_sub(assigned)
        } else {
            let scaled = ((segment.tokens as f64 / bar.total as f64) * display_total as f64)
                .round()
                .max(0.0) as u64;
            scaled.min(display_total.saturating_sub(assigned))
        };
        assigned = assigned.saturating_add(tokens);
        models.push(ModelSegment {
            model_id: segment.model_id.clone(),
            tokens,
            color: segment.color,
        });
    }

    StackedBarData {
        date: bar.date.clone(),
        period: bar.period.clone(),
        models,
        total: display_total,
    }
}

fn render_title_suffix(
    buf: &mut Buffer,
    area: Rect,
    y_label_width: u16,
    title: &str,
    note: &str,
    style: Style,
) {
    let suffix = format!("  {}", note);
    let suffix_x = area
        .x
        .saturating_add(y_label_width)
        .saturating_add(title.chars().count() as u16)
        .saturating_add(1);

    for (index, ch) in suffix.chars().enumerate() {
        let x = suffix_x.saturating_add(index as u16);
        if x < area.right() {
            buf[(x, area.y)].set_char(ch).set_style(style);
        }
    }
}

fn centered_label_x(
    plot_x: u16,
    plot_width: u16,
    bar_x: usize,
    bar_width: usize,
    label_width: u16,
) -> u16 {
    if label_width >= plot_width {
        return plot_x;
    }

    let center = plot_x
        .saturating_add(bar_x as u16)
        .saturating_add((bar_width / 2) as u16);
    let min_x = plot_x;
    let max_x = plot_x.saturating_add(plot_width.saturating_sub(label_width));
    center.saturating_sub(label_width / 2).clamp(min_x, max_x)
}

#[derive(Debug, Clone)]
struct ChartLayout {
    positions: Vec<Option<(usize, usize)>>,
    scroll_offset: usize,
    max_scroll: usize,
    virtual_width: usize,
}

impl ChartLayout {
    fn is_scrollable(&self) -> bool {
        self.max_scroll > 0
    }
}

fn chart_layout(
    plot_width: usize,
    bar_count: usize,
    requested_scroll_offset: usize,
) -> ChartLayout {
    if plot_width == 0 || bar_count == 0 {
        return ChartLayout {
            positions: Vec::new(),
            scroll_offset: 0,
            max_scroll: 0,
            virtual_width: 0,
        };
    }

    let spaced_width = spaced_chart_width(bar_count);
    if spaced_width > plot_width {
        let max_scroll = spaced_width.saturating_sub(plot_width);
        let scroll_offset = requested_scroll_offset.min(max_scroll);
        let viewport_end = scroll_offset.saturating_add(plot_width);
        let positions = (0..bar_count)
            .map(|index| {
                let virtual_x = index.saturating_mul(BAR_WIDTH + BAR_GAP);
                if virtual_x >= scroll_offset && virtual_x < viewport_end {
                    Some((virtual_x - scroll_offset, BAR_WIDTH))
                } else {
                    None
                }
            })
            .collect();

        return ChartLayout {
            positions,
            scroll_offset,
            max_scroll,
            virtual_width: spaced_width,
        };
    }

    ChartLayout {
        positions: distributed_bar_positions(plot_width, bar_count),
        scroll_offset: 0,
        max_scroll: 0,
        virtual_width: plot_width,
    }
}

fn spaced_chart_width(bar_count: usize) -> usize {
    bar_count
        .saturating_mul(BAR_WIDTH)
        .saturating_add(bar_count.saturating_sub(1).saturating_mul(BAR_GAP))
}

fn distributed_bar_positions(plot_width: usize, bar_count: usize) -> Vec<Option<(usize, usize)>> {
    if plot_width == 0 || bar_count == 0 {
        return Vec::new();
    }

    (0..bar_count)
        .map(|index| {
            let slot_start = index * plot_width / bar_count;
            let slot_end = ((index + 1) * plot_width / bar_count).max(slot_start + 1);
            let slot_width = slot_end.saturating_sub(slot_start).max(1);
            let bar_start = slot_start + slot_width.saturating_sub(BAR_WIDTH) / 2;
            Some((bar_start, BAR_WIDTH))
        })
        .collect()
}

fn label_indices(
    bar_count: usize,
    is_very_narrow: bool,
    label_all: bool,
    positions: &[Option<(usize, usize)>],
) -> Vec<usize> {
    if bar_count == 0 {
        return Vec::new();
    }
    let visible_indices = positions
        .iter()
        .enumerate()
        .filter_map(|(index, position)| position.is_some().then_some(index))
        .collect::<Vec<_>>();
    if visible_indices.is_empty() {
        return Vec::new();
    }
    if bar_count == 1 {
        return vec![0];
    }
    if label_all {
        return visible_indices;
    }
    if is_very_narrow {
        return vec![
            *visible_indices.first().unwrap(),
            *visible_indices.last().unwrap(),
        ];
    }
    let mid = visible_indices[visible_indices.len() / 2];
    let mut indices = vec![
        *visible_indices.first().unwrap(),
        mid,
        *visible_indices.last().unwrap(),
    ];
    indices.dedup();
    indices
}

fn render_horizontal_scroll_indicator(
    buf: &mut Buffer,
    app: &App,
    plot_x: u16,
    y: u16,
    plot_width: u16,
    layout: &ChartLayout,
) {
    if !layout.is_scrollable() || plot_width < 14 || layout.virtual_width == 0 {
        return;
    }

    let width = plot_width.min(if plot_width >= 80 { 24 } else { 14 });
    let x_start = plot_x.saturating_add(plot_width.saturating_sub(width));
    let inner_width = usize::from(width.saturating_sub(2)).max(1);
    let thumb_width =
        ((inner_width * usize::from(plot_width)) / layout.virtual_width).clamp(1, inner_width);
    let thumb_range = inner_width.saturating_sub(thumb_width);
    let thumb_start = if layout.max_scroll == 0 {
        0
    } else {
        layout.scroll_offset.saturating_mul(thumb_range) / layout.max_scroll
    };

    let left = if layout.scroll_offset > 0 { '◀' } else { ' ' };
    let right = if layout.scroll_offset < layout.max_scroll {
        '▶'
    } else {
        ' '
    };
    buf[(x_start, y)]
        .set_char(left)
        .set_style(Style::default().fg(app.theme.muted));
    for i in 0..inner_width {
        let ch = if i >= thumb_start && i < thumb_start + thumb_width {
            '━'
        } else {
            '─'
        };
        let style = if ch == '━' {
            Style::default().fg(app.theme.foreground)
        } else {
            Style::default().fg(app.theme.muted)
        };
        let x = x_start.saturating_add(1 + i as u16);
        buf[(x, y)].set_char(ch).set_style(style);
    }
    buf[(x_start.saturating_add(width.saturating_sub(1)), y)]
        .set_char(right)
        .set_style(Style::default().fg(app.theme.muted));
}

fn format_axis_label(date_str: &str, is_very_narrow: bool) -> String {
    if let Some((month_str, day_str)) = date_str.split_once('/') {
        if let (Ok(month), Ok(day)) = (month_str.parse::<usize>(), day_str.parse::<u32>()) {
            if (1..=12).contains(&month) {
                return if is_very_narrow {
                    format!("{}/{}", month, day)
                } else {
                    format!("{} {}", MONTH_NAMES[month - 1], day)
                };
            }
        }
    }
    date_str.to_string()
}

fn chart_title(app: &App, is_very_narrow: bool) -> &'static str {
    if is_very_narrow {
        return "Tokens";
    }

    if app.overview_mode == OverviewMode::Today {
        "Usage Trend (Today)"
    } else if app.chart_granularity == ChartGranularity::Weekly {
        "Usage Trend (Weekly)"
    } else if app.chart_granularity == ChartGranularity::Monthly {
        "Usage Trend (Monthly)"
    } else {
        "Usage Trend (Daily)"
    }
}

fn get_stacked_bar_content(
    bar_data: &StackedBarData,
    total: f64,
    row_threshold: f64,
    prev_threshold: f64,
    threshold_diff: f64,
    muted_color: Color,
    fallback_color: Color,
) -> (char, Color) {
    if total <= prev_threshold {
        return (' ', muted_color);
    }

    if bar_data.models.is_empty() {
        return (' ', muted_color);
    }

    // Note: Sorting happens per cell render. If performance becomes an issue,
    // consider pre-sorting the model list before calling this function.
    let mut sorted_models: Vec<&ModelSegment> = bar_data.models.iter().collect();
    sorted_models.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    let row_start = prev_threshold;
    let row_end = row_threshold;

    let mut current_height: f64 = 0.0;
    let mut max_overlap: f64 = 0.0;
    let mut best_color = sorted_models
        .first()
        .map(|m| m.color)
        .unwrap_or(fallback_color);

    for model in &sorted_models {
        let m_start = current_height;
        let m_end = current_height + model.tokens as f64;
        current_height += model.tokens as f64;

        let overlap_start = m_start.max(row_start);
        let overlap_end = m_end.min(row_end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > max_overlap {
            max_overlap = overlap;
            best_color = model.color;
        }
    }

    if total >= row_threshold {
        return (BLOCKS[8], best_color);
    }

    let ratio = if threshold_diff > 0.0 {
        (total - prev_threshold) / threshold_diff
    } else {
        1.0
    };
    let block_index = (ratio * 8.0).floor().clamp(1.0, 8.0) as usize;
    (BLOCKS[block_index], best_color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_label_x_stays_inside_plot() {
        assert_eq!(centered_label_x(10, 20, 0, 1, 6), 10);
        assert_eq!(centered_label_x(10, 20, 10, 2, 6), 18);
        assert_eq!(centered_label_x(10, 20, 19, 1, 6), 24);
    }

    #[test]
    fn test_label_indices_can_label_every_visible_month() {
        let positions = vec![Some((0, 1)), Some((2, 1)), Some((4, 1)), Some((6, 1))];

        assert_eq!(label_indices(4, false, true, &positions), vec![0, 1, 2, 3]);
        assert_eq!(label_indices(4, false, false, &positions), vec![0, 2, 3]);
    }

    #[test]
    fn test_chart_layout_distributes_when_spacing_fits() {
        let layout = chart_layout(14, 3, 0);

        assert!(!layout.is_scrollable());
        assert_eq!(
            layout.positions,
            vec![Some((1, 1)), Some((6, 1)), Some((11, 1))]
        );
    }

    #[test]
    fn test_chart_layout_scrolls_without_compressing_bars() {
        let layout = chart_layout(5, 5, 2);

        assert!(layout.is_scrollable());
        assert_eq!(layout.virtual_width, 9);
        assert_eq!(layout.max_scroll, 4);
        assert_eq!(layout.scroll_offset, 2);
        assert_eq!(
            layout.positions,
            vec![None, Some((0, 1)), Some((2, 1)), Some((4, 1)), None]
        );
    }

    #[test]
    fn test_chart_layout_clamps_scroll_to_end() {
        let layout = chart_layout(5, 5, usize::MAX);

        assert_eq!(layout.scroll_offset, 4);
        assert_eq!(
            layout.positions,
            vec![None, None, Some((0, 1)), Some((2, 1)), Some((4, 1))]
        );
    }

    #[test]
    fn test_full_chart_cell_uses_full_block_glyph() {
        let bar = test_bar("1", "a", 100);
        let (ch, _) =
            get_stacked_bar_content(&bar, 100.0, 50.0, 0.0, 50.0, Color::Gray, Color::Cyan);

        assert_eq!(ch, '█');
    }

    #[test]
    fn test_format_y_axis_label_fits_axis_width() {
        assert_eq!(format_y_axis_label(95_700_000, 6), "95.7M");
        assert_eq!(format_y_axis_label(421_300_000, 6), "421.3M");
        assert_eq!(format_y_axis_label(421_300_000, 5), "421M");
        assert_eq!(format_y_axis_label(1_200_000_000, 4), "1.2B");
    }

    #[test]
    fn test_chart_scale_compresses_large_outlier() {
        let mut data = (0..12)
            .map(|index| test_bar(&index.to_string(), "a", 10))
            .collect::<Vec<_>>();
        data.push(test_bar("peak", "a", 500));

        let scale = chart_scale(&data);

        assert!(scale.compressed);
        assert!(scale.focus_max < scale.actual_max);
        assert!(scale.display_max < scale.actual_max);
        assert_eq!(scale.display_value(10.0), 10.0);
        assert!(scale.display_value(250.0) > scale.display_value(scale.focus_max));
        assert!(scale.display_value(500.0) > scale.display_value(250.0));
    }

    #[test]
    fn test_chart_scale_keeps_normal_distribution_linear() {
        let data = (1..=12)
            .map(|tokens| test_bar(&tokens.to_string(), "a", tokens))
            .collect::<Vec<_>>();

        let scale = chart_scale(&data);

        assert!(!scale.compressed);
        assert_eq!(scale.display_max, scale.actual_max);
    }

    #[test]
    fn test_scaled_bar_for_display_preserves_compressed_total() {
        let bar = StackedBarData {
            date: "peak".to_string(),
            period: None,
            total: 200,
            models: vec![
                ModelSegment {
                    model_id: "a".to_string(),
                    tokens: 50,
                    color: Color::Green,
                },
                ModelSegment {
                    model_id: "b".to_string(),
                    tokens: 150,
                    color: Color::Blue,
                },
            ],
        };
        let scale = ChartScale {
            focus_max: 100.0,
            display_max: 135.0,
            actual_max: 300.0,
            compressed: true,
        };

        let scaled = scaled_bar_for_display(&bar, &scale);

        assert_eq!(scaled.total, 118);
        assert_eq!(
            scaled.models.iter().map(|model| model.tokens).sum::<u64>(),
            118
        );
        assert_eq!(scaled.models[0].tokens, 30);
        assert_eq!(scaled.models[1].tokens, 88);
    }

    fn test_bar(date: &str, model_id: &str, tokens: u64) -> StackedBarData {
        StackedBarData {
            date: date.to_string(),
            period: None,
            total: tokens,
            models: vec![ModelSegment {
                model_id: model_id.to_string(),
                tokens,
                color: Color::Green,
            }],
        }
    }
}
