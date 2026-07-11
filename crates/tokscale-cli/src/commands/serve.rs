use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use tokscale_core::pulse::{store as pulse_store, PulseSnapshotV1};
use tokscale_core::{ClientId, GroupBy};

use crate::client_filter::ResolvedClientSelection;
use crate::date_filter::get_date_range_label;
use crate::report_support::{
    emit_setup_warnings, setup_warnings_for_report, PricingCacheOnlyGuard,
};
use crate::spinner::LightSpinner;
use crate::tui::{load_cache, CacheReportScope, CacheResult, DataLoader, UsageData};
use crate::web::overview::{
    build_overview_json, render_overview_html, render_overview_surface, OverviewRenderOptions,
};
use crate::web::review::render_weekly_review_at;
use crate::web::server::{serve_static_overview, ReviewRenderer, StaticSite};

pub(crate) struct ServeArgs {
    pub port: u16,
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub today: bool,
    pub week: bool,
    pub month: bool,
    pub group_by: GroupBy,
    pub no_spinner: bool,
    pub reference_now: NaiveDateTime,
}

struct PulseRouteContent {
    pulse_json: Option<String>,
    pulse_markdown: Option<String>,
}

pub(crate) fn run(args: ServeArgs) -> Result<()> {
    let ServeArgs {
        port,
        clients,
        since,
        until,
        year,
        today,
        week,
        month,
        group_by,
        no_spinner,
        reference_now,
    } = args;

    let date_range = get_date_range_label(today, week, month, &since, &until, &year);
    let date_range = date_range.unwrap_or_else(|| "All time".to_string());
    let selection = ResolvedClientSelection::from_configured(clients.as_deref());
    let setup_warnings = setup_warnings_for_report(&None, &clients);

    let report_scope = CacheReportScope::new(since.clone(), until.clone(), year.clone());
    let data = match fresh_cached_data(load_cache(&selection.filters, &group_by, &report_scope)) {
        Some(data) => data,
        None => scan_usage_data(
            since.clone(),
            until.clone(),
            year.clone(),
            &selection.scan_clients,
            &group_by,
            selection.include_synthetic,
            no_spinner,
        )?,
    };

    emit_setup_warnings(&setup_warnings);

    let render_options = OverviewRenderOptions::new(
        clients,
        since.clone(),
        until.clone(),
        year.clone(),
        group_by,
        today,
        reference_now,
    );
    let overview_json = build_overview_json(
        &data,
        date_range,
        render_options.width,
        render_options.height,
        render_options.reference_date(),
    );
    let html = render_overview_html(data.clone(), render_options.clone())?;
    let json = serde_json::to_string_pretty(&overview_json)?;
    let surface_data = data.clone();
    let surface_options = render_options.clone();
    let pulse_snapshot = pulse_store::load_latest();
    let PulseRouteContent {
        pulse_json,
        pulse_markdown,
    } = pulse_route_content(pulse_snapshot.as_ref())?;
    let review = pulse_snapshot.map(pulse_review_renderer);

    serve_static_overview(
        port,
        StaticSite {
            html,
            json,
            review,
            pulse_json,
            pulse_markdown,
            surface: Some(Box::new(move |size| {
                let mut options = surface_options.clone();
                options.width = size.cols;
                options.height = size.rows;
                render_overview_surface(surface_data.clone(), options)
            })),
        },
    )
}

fn pulse_route_content(snapshot: Option<&PulseSnapshotV1>) -> Result<PulseRouteContent> {
    let pulse_json = snapshot.map(serde_json::to_string_pretty).transpose()?;
    let pulse_markdown = snapshot.map(PulseSnapshotV1::to_markdown);

    Ok(PulseRouteContent {
        pulse_json,
        pulse_markdown,
    })
}

fn pulse_review_renderer(snapshot: PulseSnapshotV1) -> ReviewRenderer {
    Box::new(move || render_pulse_review_at(&snapshot, Utc::now()))
}

fn render_pulse_review_at(snapshot: &PulseSnapshotV1, now: DateTime<Utc>) -> String {
    let presentation = snapshot.for_presentation_at(now);
    render_weekly_review_at(&presentation, now)
}

fn fresh_cached_data(result: CacheResult) -> Option<UsageData> {
    match result {
        CacheResult::Fresh(data) => Some(data),
        CacheResult::Stale(_) | CacheResult::StaleSubset(_) | CacheResult::Miss => None,
    }
}

fn scan_usage_data(
    since: Option<String>,
    until: Option<String>,
    year: Option<String>,
    enabled_clients: &[ClientId],
    group_by: &GroupBy,
    include_synthetic: bool,
    no_spinner: bool,
) -> Result<UsageData> {
    let spinner = if no_spinner {
        None
    } else {
        Some(LightSpinner::start(
            "Scanning local telemetry for web overview...",
        ))
    };

    let _pricing_cache_only = PricingCacheOnlyGuard::enable();
    let loader = DataLoader::with_filters(since, until, year);
    let data = loader.load(enabled_clients, group_by, include_synthetic)?;

    if let Some(spinner) = spinner {
        spinner.stop();
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokscale_core::pulse::weread::WeReadSyncState;
    use tokscale_core::pulse::{AiQuotaMetric, AiQuotaSource, AiWorkInput, PulseFreshness};

    #[test]
    fn pulse_http_exports_are_durable_and_markdown_is_byte_identical() {
        let observed_at = Utc::now();
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            AiWorkInput {
                quota_sources: vec![AiQuotaSource {
                    provider: "Codex".to_string(),
                    metrics: vec![AiQuotaMetric {
                        label: "weekly".to_string(),
                        used_percent: 75.0,
                    }],
                }],
                ..AiWorkInput::default()
            },
            None,
            Some(observed_at),
            WeReadSyncState::default(),
        );
        let stale_at = observed_at + chrono::Duration::seconds(301);
        let presentation = snapshot.for_presentation_at(stale_at);
        let first = pulse_route_content(Some(&snapshot)).unwrap();
        let second = pulse_route_content(Some(&snapshot)).unwrap();
        let expected_json = serde_json::to_string_pretty(&snapshot).unwrap();
        let expected_markdown = snapshot.to_markdown();
        let fresh_review =
            render_pulse_review_at(&snapshot, observed_at + chrono::Duration::seconds(300));
        let stale_review = render_pulse_review_at(&snapshot, stale_at);

        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|source| source.id == "subscription-usage-cache")
                .unwrap()
                .freshness,
            PulseFreshness::Fresh
        );
        assert_eq!(
            presentation
                .sources
                .iter()
                .find(|source| source.id == "subscription-usage-cache")
                .unwrap()
                .freshness,
            PulseFreshness::Stale
        );
        assert_eq!(first.pulse_json.as_deref(), Some(expected_json.as_str()));
        assert_eq!(first.pulse_json, second.pulse_json);
        assert!(expected_markdown.ends_with('\n'));
        assert_eq!(
            first.pulse_markdown.as_deref().unwrap().as_bytes(),
            expected_markdown.as_bytes()
        );
        assert_eq!(first.pulse_markdown, second.pulse_markdown);
        assert_ne!(fresh_review, stale_review);
        assert!(stale_review.contains("Snapshot generated"));
        assert!(stale_review.contains(&format!("health evaluated {}", stale_at.to_rfc3339())));
    }

    #[test]
    fn serve_cache_accepts_only_fresh_data() {
        let fresh = UsageData {
            total_tokens: 42,
            ..UsageData::default()
        };

        assert_eq!(
            fresh_cached_data(CacheResult::Fresh(fresh)).map(|data| data.total_tokens),
            Some(42)
        );
        assert!(fresh_cached_data(CacheResult::Stale(UsageData::default())).is_none());
        assert!(fresh_cached_data(CacheResult::StaleSubset(UsageData::default())).is_none());
        assert!(fresh_cached_data(CacheResult::Miss).is_none());
    }
}
