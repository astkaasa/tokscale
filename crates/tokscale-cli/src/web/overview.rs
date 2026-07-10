use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::Serialize;
use tokscale_core::GroupBy;

use crate::report_format::format_currency;
use crate::tui::settings::Settings;
use crate::tui::{surface::render_app_buffer, App, DataLoader, Theme, TuiConfig, UsageData};

mod html;
mod overlay;
mod page;
mod surface;

use html::HtmlColorPalette;
use page::render_buffer_page;
use surface::render_buffer_surface;

const DEFAULT_WIDTH: u16 = 220;
const DEFAULT_HEIGHT: u16 = 69;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverviewReference {
    date: NaiveDate,
    is_today: bool,
}

impl OverviewReference {
    fn from_filters(
        since: &Option<String>,
        until: &Option<String>,
        year: &Option<String>,
        date: NaiveDate,
    ) -> Self {
        let date_filter = date.format("%Y-%m-%d").to_string();
        let is_today = year.is_none()
            && since.as_deref() == Some(date_filter.as_str())
            && until.as_deref() == Some(date_filter.as_str());

        Self { date, is_today }
    }

    fn for_serve(
        since: &Option<String>,
        until: &Option<String>,
        year: &Option<String>,
        today_requested: bool,
        fallback_date: NaiveDate,
    ) -> Self {
        if today_requested {
            let date = since
                .as_deref()
                .filter(|since| until.as_deref() == Some(*since) && year.is_none())
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                .unwrap_or(fallback_date);
            return Self {
                date,
                is_today: true,
            };
        }

        Self::from_filters(since, until, year, fallback_date)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OverviewRenderOptions {
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub group_by: GroupBy,
    pub width: u16,
    pub height: u16,
    settings: Settings,
    reference: OverviewReference,
    reference_now: NaiveDateTime,
}

impl OverviewRenderOptions {
    pub(crate) fn new(
        clients: Option<Vec<String>>,
        since: Option<String>,
        until: Option<String>,
        year: Option<String>,
        group_by: GroupBy,
        today_requested: bool,
        reference_now: NaiveDateTime,
    ) -> Self {
        let reference = OverviewReference::for_serve(
            &since,
            &until,
            &year,
            today_requested,
            reference_now.date(),
        );
        Self {
            clients,
            since,
            until,
            year,
            group_by,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            settings: Settings::load(),
            reference,
            reference_now,
        }
    }

    pub(crate) fn reference_date(&self) -> NaiveDate {
        self.reference.date
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
    reference_date: NaiveDate,
) -> OverviewJson {
    let (today_tokens, today_cost) = today_totals(data, reference_date);

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

fn today_totals(data: &UsageData, today: chrono::NaiveDate) -> (u64, f64) {
    if let Some(day) = data
        .daily
        .iter()
        .find(|day| day.date == today && (day.tokens.total() > 0 || day.cost > 0.0))
    {
        return (day.tokens.total(), day.cost);
    }

    let mut tokens = 0u64;
    let mut cost = 0.0;
    for hour in data
        .hourly
        .iter()
        .filter(|hour| hour.datetime.date() == today)
    {
        tokens = tokens.saturating_add(hour.tokens.total());
        if hour.cost.is_finite() {
            cost += hour.cost;
        }
    }

    (tokens, cost)
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
    let settings = options.settings.clone();
    let data_loader = DataLoader::with_filters(
        options.since.clone(),
        options.until.clone(),
        options.year.clone(),
    );
    let mut app = App::new_surface_with_cached_data(
        TuiConfig {
            theme: None,
            refresh: 0,
            clients: options.clients,
            since: None,
            until: None,
            year: None,
            initial_tab: Some(crate::tui::Tab::Overview),
            initial_timeline_granularity: None,
        },
        Some(data),
        settings,
    )?;
    app.theme = Theme::for_web_with_preference(app.settings.ui_theme);
    app.set_render_reference_now(options.reference_now);
    app.data_loader = data_loader;
    if options.reference.is_today {
        app.toggle_overview_mode();
    }

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

fn generated_at() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use chrono::NaiveDate;
    use ratatui::{
        buffer::{Buffer, Cell},
        layout::Rect,
        style::Color,
    };
    use tokscale_core::ModelPerformance;

    use super::*;
    use super::{
        html::{HtmlColorPalette, DEFAULT_BG},
        overlay::{
            chart_overlay_region, daily_heatmap_legend_fills, daily_heatmap_overlay_cell,
            render_block_overlay, render_block_overlay_region, web_heatmap_metrics,
            BlockOverlayKind, BlockOverlayRegion, DailyHeatmapGeometry,
        },
        surface::{render_buffer_html, render_buffer_html_with_region},
    };
    use crate::tui::data::{DailySourceInfo, DailyUsage, HourlyUsage, ModelUsage, TokenBreakdown};

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
                settings: Settings::default(),
                reference: OverviewReference {
                    date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
                    is_today: false,
                },
                reference_now: NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            },
        )
        .unwrap();

        assert!(html.contains("terminal-screen"));
        assert!(html.contains("Tokscale"));
        assert!(html.contains("Overview"));
        assert!(html.contains("Today"), "{html}");
        assert!(html.contains("3K"), "{html}");
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
                settings: Settings::default(),
                reference: OverviewReference {
                    date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
                    is_today: false,
                },
                reference_now: NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
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
    fn overview_surface_preserves_web_sizing_contract() {
        let html = render_overview_surface(
            usage_fixture(),
            OverviewRenderOptions {
                clients: None,
                since: None,
                until: None,
                year: None,
                group_by: GroupBy::Model,
                width: 132,
                height: 37,
                settings: Settings::default(),
                reference: OverviewReference {
                    date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
                    is_today: false,
                },
                reference_now: NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            },
        )
        .unwrap();

        assert!(html.contains(r#"data-cols="132""#));
        assert!(html.contains(r#"data-rows="37""#));
        assert!(html.contains("--terminal-cols:132;"));
        assert!(html.contains("--terminal-rows:37;"));
        assert!(html.contains(r#"<span class="terminal-overlay" aria-hidden="true">"#));
    }

    #[test]
    fn overview_reference_keeps_startup_today_scope_after_rollover() {
        let startup_date = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let next_date = startup_date.succ_opt().unwrap();
        let date_filter = startup_date.format("%Y-%m-%d").to_string();
        let since = Some(date_filter.clone());
        let until = Some(date_filter);

        let frozen = OverviewReference::from_filters(&since, &until, &None, startup_date);
        let recomputed_after_midnight =
            OverviewReference::from_filters(&since, &until, &None, next_date);
        let explicit_today_after_midnight =
            OverviewReference::for_serve(&since, &until, &None, true, next_date);

        assert_eq!(frozen.date, startup_date);
        assert!(frozen.is_today);
        assert!(!recomputed_after_midnight.is_today);
        assert_eq!(explicit_today_after_midnight.date, startup_date);
        assert!(explicit_today_after_midnight.is_today);
    }

    #[test]
    fn surface_keeps_frozen_today_date_and_time() {
        let startup_date = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let mut usage = usage_fixture();
        usage.daily[0].date = startup_date;
        usage.hourly = vec![HourlyUsage {
            datetime: startup_date.and_hms_opt(9, 30, 0).unwrap(),
            tokens: TokenBreakdown::default(),
            cost: 1.25,
            clients: BTreeSet::new(),
            models: BTreeMap::new(),
            message_count: 1,
            turn_count: 1,
        }];
        let date_filter = startup_date.format("%Y-%m-%d").to_string();
        let options = OverviewRenderOptions {
            clients: None,
            since: Some(date_filter.clone()),
            until: Some(date_filter),
            year: None,
            group_by: GroupBy::Model,
            width: 120,
            height: 30,
            settings: Settings::default(),
            reference: OverviewReference {
                date: startup_date,
                is_today: true,
            },
            reference_now: startup_date.and_hms_opt(16, 30, 0).unwrap(),
        };

        let html = render_overview_surface(usage, options).unwrap();

        assert!(html.contains("Today"), "{html}");
        assert!(html.contains("Now 16:30"), "{html}");
    }

    #[test]
    fn surface_applies_frozen_today_scope() {
        let today = Local::now().date_naive();
        let date_filter = today.format("%Y-%m-%d").to_string();
        let mut usage = usage_fixture();
        usage.daily[0].date = today;
        let options = OverviewRenderOptions {
            clients: None,
            since: Some(date_filter.clone()),
            until: Some(date_filter),
            year: None,
            group_by: GroupBy::Model,
            width: 88,
            height: 24,
            settings: Settings::default(),
            reference: OverviewReference {
                date: today,
                is_today: true,
            },
            reference_now: today.and_hms_opt(12, 0, 0).unwrap(),
        };

        let html = render_overview_surface(usage, options).unwrap();

        assert!(html.contains("Today"));
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
    fn daily_heatmap_overlay_preserves_grid_and_legend_contract() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 72, 8));
        write_buffer_row(&mut buffer, 0, " Daily Activity (52w)");
        write_buffer_row(&mut buffer, 1, "     Jun");
        buffer[(5, 2)].set_symbol("█").set_fg(Color::Green);
        buffer[(7, 2)].set_symbol("·").set_fg(Color::DarkGray);
        write_buffer_row(&mut buffer, 5, "     Less █ █ █ █ More");
        write_buffer_row(&mut buffer, 6, "Models  ● gpt-5.5");

        let overlay = render_block_overlay(&buffer, 72, 8, &palette);

        assert!(overlay.contains("--heatmap-col-pitch:"), "{overlay}");
        assert!(overlay.contains("--heatmap-row-pitch:"), "{overlay}");
        assert!(overlay.contains("width:var(--heatmap-width);"), "{overlay}");
        assert!(
            overlay.contains("height:var(--heatmap-height);"),
            "{overlay}"
        );
        assert_eq!(overlay.matches("terminal-heatmap-axis-label").count(), 3);
        assert_eq!(overlay.matches("terminal-heatmap-legend-cell").count(), 5);
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
    fn provider_mix_bar_overlay_preserves_single_row_geometry() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 48, 3));
        write_buffer_row(&mut buffer, 0, "│┌ Provider Mix ─────────────────────┐");
        write_buffer_row(&mut buffer, 1, "││███████████████████████████████████│");

        for x in 2..14 {
            buffer[(x, 1)].set_symbol("█").set_fg(Color::Green);
        }
        for x in 14..20 {
            buffer[(x, 1)].set_symbol("█").set_fg(Color::Red);
        }
        for x in 20..37 {
            buffer[(x, 1)].set_symbol(" ");
        }

        let overlay = render_block_overlay(&buffer, 48, 3, &palette);

        assert_eq!(overlay.matches("terminal-mix-bar-segment").count(), 2);
        assert!(overlay.contains("left:2ch;top:calc(1 * var(--terminal-line-height) + (var(--terminal-line-height) * .14));width:calc(12ch + .5px);"));
        assert!(overlay.contains("left:14ch;top:calc(1 * var(--terminal-line-height) + (var(--terminal-line-height) * .14));width:calc(6ch + .5px);"));
    }

    #[test]
    fn today_cost_by_hour_chart_uses_block_overlay() {
        let palette = HtmlColorPalette::dark();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 64, 8));
        write_buffer_row(&mut buffer, 0, " Cost by hour                     Summary");
        buffer[(8, 2)].set_symbol("█").set_fg(Color::Green);
        buffer[(8, 3)].set_symbol("█").set_fg(Color::Green);
        write_buffer_row(&mut buffer, 5, "      └────────");

        let html = render_block_overlay(&buffer, 64, 8, &palette);

        assert!(html.contains("terminal-block-rect"), "{html}");
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

        let today = Local::now().date_naive();
        let json = build_overview_json(&usage, "All time".to_string(), 160, 48, today);

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

    #[test]
    fn overview_json_falls_back_to_hourly_today_totals() {
        let today = Local::now().date_naive();
        let mut usage = usage_fixture();
        usage.daily.clear();
        usage.hourly = vec![HourlyUsage {
            datetime: today.and_hms_opt(9, 0, 0).unwrap(),
            tokens: TokenBreakdown {
                input: 4_000,
                output: 1_500,
                cache_read: 500,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 2.75,
            clients: BTreeSet::new(),
            models: BTreeMap::new(),
            message_count: 1,
            turn_count: 1,
        }];

        let json = build_overview_json(&usage, "Today".to_string(), 160, 48, today);

        assert_eq!(json.today_tokens, 6_000);
        assert_eq!(json.today_cost_label, "$2.75");
    }

    fn write_buffer_row(buffer: &mut Buffer, y: u16, text: &str) {
        for (x, ch) in text.chars().enumerate() {
            buffer[(x as u16, y)].set_char(ch);
        }
    }
}
