use crate::claude_diagnostics;
use std::path::PathBuf;

mod args;
mod hourly;
mod models;
mod monthly;
mod time_metrics;

pub(crate) use args::{ModelsReportArgs, PeriodReportArgs, TimeMetricsReportArgs};
pub(crate) use hourly::run_hourly_report;
pub(crate) use models::run_models_report;
pub(crate) use monthly::run_monthly_report;
pub(crate) use time_metrics::run_time_metrics_report;

fn use_env_roots(home_dir: &Option<String>) -> bool {
    home_dir.is_none()
}

fn build_report_options(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
    since: &Option<String>,
    until: &Option<String>,
    year: &Option<String>,
    group_by: tokscale_core::GroupBy,
) -> (tokscale_core::ReportOptions, Option<PathBuf>) {
    let options = tokscale_core::ReportOptions {
        home_dir: home_dir.clone(),
        use_env_roots: use_env_roots(home_dir),
        clients: clients.clone(),
        since: since.clone(),
        until: until.clone(),
        year: year.clone(),
        group_by,
        scanner_settings: crate::tui::settings::load_scanner_settings_for_home(home_dir),
    };
    let telemetry_store_path = crate::paths::telemetry_store_path_for_home_override(home_dir);

    (options, telemetry_store_path)
}

fn resolve_effective_home_dir(home_dir: &Option<String>) -> Option<PathBuf> {
    home_dir.as_ref().map(PathBuf::from).or_else(dirs::home_dir)
}

fn model_usage_includes_client(entry: &tokscale_core::ModelUsage, client: &str) -> bool {
    if entry.client == client {
        return true;
    }

    entry
        .merged_clients
        .as_deref()
        .is_some_and(|clients| clients.split(", ").any(|id| id == client))
}

fn emit_client_diagnostics(diagnostics: &[claude_diagnostics::ClientDiagnostic]) {
    if diagnostics.is_empty() {
        return;
    }

    use colored::Colorize;
    for diagnostic in diagnostics {
        eprintln!(
            "{}",
            format!("  {}: {}", diagnostic.severity, diagnostic.message).yellow()
        );
        eprintln!("{}", format!("  {}", diagnostic.help).bright_black());
    }
}

const TABLE_PRESET: &str = "││──├─┼┤│─┼├┤┬┴┌┐└┘";

#[cfg(test)]
mod tests {
    use super::*;
    use tokscale_core::GroupBy;

    #[test]
    fn default_profile_uses_environment_roots_and_telemetry() {
        let home_dir = None;
        let clients = Some(vec!["claude".to_string(), "codex".to_string()]);
        let since = Some("2026-01-01".to_string());
        let until = Some("2026-01-31".to_string());
        let year = Some("2026".to_string());

        let (options, telemetry_store_path) = build_report_options(
            &home_dir,
            &clients,
            &since,
            &until,
            &year,
            GroupBy::WorkspaceModel,
        );

        assert_eq!(options.home_dir, home_dir);
        assert!(options.use_env_roots);
        assert_eq!(options.clients, clients);
        assert_eq!(options.since, since);
        assert_eq!(options.until, until);
        assert_eq!(options.year, year);
        assert_eq!(options.group_by, GroupBy::WorkspaceModel);
        assert_eq!(
            telemetry_store_path,
            Some(crate::paths::telemetry_store_path())
        );
    }

    #[test]
    fn explicit_home_disables_environment_roots_and_telemetry() {
        let home_dir = Some("/tmp/tokscale-report-profile".to_string());
        let clients = Some(vec!["gemini".to_string()]);
        let since = Some("2025-04-01".to_string());
        let until = Some("2025-04-30".to_string());
        let year = Some("2025".to_string());

        let (options, telemetry_store_path) = build_report_options(
            &home_dir,
            &clients,
            &since,
            &until,
            &year,
            GroupBy::ClientSession,
        );

        assert_eq!(options.home_dir, home_dir);
        assert!(!options.use_env_roots);
        assert_eq!(options.clients, clients);
        assert_eq!(options.since, since);
        assert_eq!(options.until, until);
        assert_eq!(options.year, year);
        assert_eq!(options.group_by, GroupBy::ClientSession);
        assert_eq!(telemetry_store_path, None);
    }
}
