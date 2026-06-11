use ratatui::prelude::*;
use std::collections::BTreeMap;

use super::widgets::format_tokens;
use crate::tui::app::{App, ChartGranularity, ClickAction, OverviewMode, PeriodDetailKey};

/// 8-level block characters for sub-cell precision (matching OpenTUI)
const BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

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

    let data = compressed_bars(data, plot_width as usize);
    if data.is_empty() {
        return;
    }

    let scale = chart_scale(&data);
    let display_data: Vec<StackedBarData> = data
        .iter()
        .map(|bar| scaled_bar_for_display(bar, &scale))
        .collect();

    let buf = frame.buffer_mut();
    let bar_count = data.len();

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

    let bar_width = target_bar_width(app, plot_width as usize, bar_count);
    let bar_positions = bar_positions(plot_width as usize, bar_count, bar_width);
    for (bar_index, bar_data) in data.iter().enumerate() {
        let Some(period) = bar_data.period.clone() else {
            continue;
        };
        let Some((offset, width)) = bar_positions.get(bar_index).copied() else {
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
        let Some((offset, width)) = bar_positions.get(bar_index).copied() else {
            continue;
        };
        let x_start = plot_x.saturating_add(offset as u16);
        for row_from_bottom in 0..plot_height as usize {
            let y = plot_y + plot_height - 1 - row_from_bottom as u16;
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

            if ch == ' ' {
                continue;
            }
            for dx in 0..width {
                let x = x_start.saturating_add(dx as u16);
                if x < plot_x.saturating_add(plot_width) {
                    buf[(x, y)].set_char(ch).set_fg(fg_color);
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
        for index in label_indices(bar_count, is_very_narrow, label_all_months) {
            let label = format_axis_label(&data[index].date, is_very_narrow);
            let (bar_x, width) = bar_positions.get(index).copied().unwrap_or((0, 1));
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

fn compressed_bars(data: &[StackedBarData], max_bars: usize) -> Vec<StackedBarData> {
    if max_bars == 0 {
        return Vec::new();
    }
    if data.len() <= max_bars {
        return data.to_vec();
    }

    let mut bars = Vec::with_capacity(max_bars);
    for index in 0..max_bars {
        let start = index * data.len() / max_bars;
        let end = ((index + 1) * data.len() / max_bars)
            .max(start + 1)
            .min(data.len());
        let mut models: BTreeMap<String, ModelSegment> = BTreeMap::new();
        let mut total = 0u64;
        for bar in &data[start..end] {
            total = total.saturating_add(bar.total);
            for segment in &bar.models {
                let entry =
                    models
                        .entry(segment.model_id.clone())
                        .or_insert_with(|| ModelSegment {
                            model_id: segment.model_id.clone(),
                            tokens: 0,
                            color: segment.color,
                        });
                entry.tokens = entry.tokens.saturating_add(segment.tokens);
            }
        }
        bars.push(StackedBarData {
            date: data[end - 1].date.clone(),
            period: if end == start + 1 {
                data[end - 1].period.clone()
            } else {
                None
            },
            models: models.into_values().collect(),
            total,
        });
    }
    bars
}

fn target_bar_width(app: &App, plot_width: usize, bar_count: usize) -> usize {
    if plot_width == 0 || bar_count == 0 {
        return 0;
    }

    let ideal_width = ideal_bar_width(app.overview_mode, app.chart_granularity);

    let min_slot_width = (0..bar_count)
        .map(|index| {
            let slot_start = index * plot_width / bar_count;
            let slot_end = ((index + 1) * plot_width / bar_count).max(slot_start + 1);
            slot_end.saturating_sub(slot_start).max(1)
        })
        .min()
        .unwrap_or(1);

    ideal_width.min(min_slot_width).max(1)
}

fn ideal_bar_width(_overview_mode: OverviewMode, _granularity: ChartGranularity) -> usize {
    1
}

fn bar_positions(plot_width: usize, bar_count: usize, bar_width: usize) -> Vec<(usize, usize)> {
    if plot_width == 0 || bar_count == 0 || bar_width == 0 {
        return Vec::new();
    }

    let min_slot_width = (0..bar_count)
        .map(|index| {
            let slot_start = index * plot_width / bar_count;
            let slot_end = ((index + 1) * plot_width / bar_count).max(slot_start + 1);
            slot_end.saturating_sub(slot_start).max(1)
        })
        .min()
        .unwrap_or(1);
    let bar_width = bar_width.min(min_slot_width).max(1);

    (0..bar_count)
        .map(|index| {
            let slot_start = index * plot_width / bar_count;
            let slot_end = ((index + 1) * plot_width / bar_count).max(slot_start + 1);
            let slot_width = slot_end.saturating_sub(slot_start).max(1);
            let bar_start = slot_start + slot_width.saturating_sub(bar_width) / 2;
            (bar_start, bar_width)
        })
        .collect()
}

fn label_indices(bar_count: usize, is_very_narrow: bool, label_all: bool) -> Vec<usize> {
    if bar_count == 0 {
        return Vec::new();
    }
    if bar_count == 1 {
        return vec![0];
    }
    if label_all {
        return (0..bar_count).collect();
    }
    if is_very_narrow {
        return vec![0, bar_count - 1];
    }
    let mid = bar_count / 2;
    let mut indices = vec![0, mid, bar_count - 1];
    indices.dedup();
    indices
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
    fn test_compressed_bars_sums_bucket_segments() {
        let data = vec![
            test_bar("1", "a", 10),
            test_bar("2", "a", 20),
            test_bar("3", "b", 30),
            test_bar("4", "b", 40),
        ];

        let bars = compressed_bars(&data, 2);

        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].date, "2");
        assert_eq!(bars[0].total, 30);
        assert_eq!(bars[0].models[0].tokens, 30);
        assert_eq!(bars[1].date, "4");
        assert_eq!(bars[1].total, 70);
        assert_eq!(bars[1].models[0].tokens, 70);
    }

    #[test]
    fn test_compressed_bars_drop_ambiguous_period_click_target() {
        let mut data = vec![
            test_bar("1", "a", 10),
            test_bar("2", "a", 20),
            test_bar("3", "a", 30),
            test_bar("4", "a", 40),
        ];
        data[0].period = Some(PeriodDetailKey::day(
            chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
        ));
        data[1].period = Some(PeriodDetailKey::day(
            chrono::NaiveDate::from_ymd_opt(2026, 5, 2).unwrap(),
        ));

        let bars = compressed_bars(&data, 2);

        assert_eq!(bars.len(), 2);
        assert!(bars[0].period.is_none());
    }

    #[test]
    fn test_label_indices_can_label_every_month() {
        assert_eq!(label_indices(4, false, true), vec![0, 1, 2, 3]);
        assert_eq!(label_indices(4, false, false), vec![0, 2, 3]);
    }

    #[test]
    fn test_bar_positions_keep_uniform_width_across_uneven_slots() {
        let positions = bar_positions(14, 3, 3);
        let widths = positions
            .iter()
            .map(|(_, width)| *width)
            .collect::<Vec<_>>();

        assert_eq!(widths, vec![3, 3, 3]);
    }

    #[test]
    fn test_bar_positions_shrink_uniformly_when_slots_are_too_narrow() {
        let positions = bar_positions(5, 3, 3);
        let widths = positions
            .iter()
            .map(|(_, width)| *width)
            .collect::<Vec<_>>();

        assert_eq!(widths, vec![1, 1, 1]);
    }

    #[test]
    fn test_ideal_bar_width_is_stable_across_overview_granularities() {
        let modes = [OverviewMode::All, OverviewMode::Today];
        let granularities = [
            ChartGranularity::Daily,
            ChartGranularity::Weekly,
            ChartGranularity::Monthly,
        ];

        for mode in modes {
            for granularity in granularities {
                assert_eq!(ideal_bar_width(mode, granularity), 1);
            }
        }
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
