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
