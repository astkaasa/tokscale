use std::collections::HashSet;

use anyhow::Result;
use tokscale_core::{ClientId, GroupBy};

use crate::client_filter::ClientFilter;
use crate::date_filter::get_date_range_label;
use crate::report_support::{emit_cursor_setup_warnings, setup_warnings_for_report};
use crate::spinner::LightSpinner;
use crate::tui::{load_cache, CacheReportScope, CacheResult, DataLoader, UsageData};
use crate::web::overview::{
    build_overview_json, render_overview_html, render_overview_surface, OverviewRenderOptions,
};
use crate::web::server::{serve_static_overview, StaticSite};

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
    } = args;

    let date_range = get_date_range_label(today, week, month, &since, &until, &year);
    let date_range = date_range.unwrap_or_else(|| "All time".to_string());
    let (enabled_filters, enabled_clients, include_synthetic) = resolve_loader_clients(&clients);
    let cursor_setup_warnings = setup_warnings_for_report(&None, &clients);

    let report_scope = CacheReportScope::new(since.clone(), until.clone(), year.clone());
    let data = match load_cache(&enabled_filters, &group_by, &report_scope) {
        CacheResult::Fresh(data) | CacheResult::Stale(data) => data,
        CacheResult::Miss => scan_usage_data(
            since.clone(),
            until.clone(),
            year.clone(),
            &enabled_clients,
            &group_by,
            include_synthetic,
            no_spinner,
        )?,
    };

    emit_cursor_setup_warnings(&cursor_setup_warnings);

    let render_options = OverviewRenderOptions::new(
        clients,
        since.clone(),
        until.clone(),
        year.clone(),
        group_by,
    );
    let overview_json = build_overview_json(
        &data,
        date_range,
        render_options.width,
        render_options.height,
    );
    let html = render_overview_html(data.clone(), render_options.clone())?;
    let json = serde_json::to_string_pretty(&overview_json)?;
    let surface_data = data.clone();
    let surface_options = render_options.clone();

    serve_static_overview(
        port,
        StaticSite {
            html,
            json,
            surface: Some(Box::new(move |size| {
                let mut options = surface_options.clone();
                options.width = size.cols;
                options.height = size.rows;
                render_overview_surface(surface_data.clone(), options)
            })),
        },
    )
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

    let loader = DataLoader::with_filters(since, until, year);
    let data = loader.load(enabled_clients, group_by, include_synthetic)?;

    if let Some(spinner) = spinner {
        spinner.stop();
    }

    Ok(data)
}

fn resolve_loader_clients(
    clients: &Option<Vec<String>>,
) -> (HashSet<ClientFilter>, Vec<ClientId>, bool) {
    let filters: HashSet<ClientFilter> = clients
        .as_ref()
        .map(|configured| {
            configured
                .iter()
                .filter_map(|client| ClientFilter::from_filter_str(client))
                .collect()
        })
        .unwrap_or_else(ClientFilter::default_set);

    let include_synthetic = filters
        .iter()
        .any(|filter| matches!(filter, ClientFilter::Synthetic));
    let enabled_clients = filters
        .iter()
        .copied()
        .filter_map(ClientFilter::to_client_id)
        .collect();

    (filters, enabled_clients, include_synthetic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_clients_default_to_real_clients() {
        let (filters, clients, include_synthetic) = resolve_loader_clients(&None);

        assert!(!filters.is_empty());
        assert!(!clients.is_empty());
        assert!(!include_synthetic);
    }

    #[test]
    fn loader_clients_preserve_synthetic_flag() {
        let (filters, clients, include_synthetic) =
            resolve_loader_clients(&Some(vec!["codex".to_string(), "synthetic".to_string()]));

        assert!(filters.contains(&ClientFilter::Codex));
        assert!(filters.contains(&ClientFilter::Synthetic));
        assert_eq!(clients, vec![ClientId::Codex]);
        assert!(include_synthetic);
    }
}
