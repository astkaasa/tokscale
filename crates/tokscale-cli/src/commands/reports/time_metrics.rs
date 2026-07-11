use super::{build_report_options, TimeMetricsReportArgs};
use crate::report_format::format_duration_ms;
use crate::report_support::{emit_setup_warnings, setup_warnings_for_report};
use crate::spinner::LightSpinner;
use anyhow::Result;

pub fn run_time_metrics_report(args: TimeMetricsReportArgs) -> Result<()> {
    use tokio::runtime::Runtime;
    use tokscale_core::{get_time_metrics_report_with_telemetry, GroupBy};

    let TimeMetricsReportArgs {
        json,
        home_dir,
        clients,
        since,
        until,
        year,
        no_spinner,
    } = args;

    let spinner = if no_spinner {
        None
    } else {
        Some(LightSpinner::start("Computing time metrics..."))
    };
    let setup_warnings = setup_warnings_for_report(&home_dir, &clients);
    let rt = Runtime::new()?;
    let report = rt
        .block_on(async {
            let (options, telemetry_store_path) = build_report_options(
                &home_dir,
                &clients,
                &since,
                &until,
                &year,
                GroupBy::default(),
            );
            get_time_metrics_report_with_telemetry(options, telemetry_store_path).await
        })
        .map_err(|e| anyhow::anyhow!(e))?;

    if let Some(spinner) = spinner {
        spinner.stop();
    }

    let m = &report.metrics;

    if json {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct TimeMetricsReportJson<'a> {
            metrics: &'a tokscale_core::TimeMetrics,
            processing_time_ms: u32,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            warnings: Vec<String>,
        }

        let output = TimeMetricsReportJson {
            metrics: &report.metrics,
            processing_time_ms: report.processing_time_ms,
            warnings: setup_warnings,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        emit_setup_warnings(&setup_warnings);
        println!("Session Time Metrics");
        println!("====================");
        println!(
            "Total active time:       {}",
            format_duration_ms(m.total_active_time_ms)
        );
        println!(
            "Total wall-clock time:   {}",
            format_duration_ms(m.total_wall_time_ms)
        );
        println!(
            "Longest continuous use:  {}",
            format_duration_ms(m.longest_continuous_ms)
        );
        println!("Max concurrent sessions: {}", m.max_concurrent_sessions);
        println!("Total sessions:          {}", m.session_count);
        println!("Processing time:         {}ms", report.processing_time_ms);
    }

    Ok(())
}
