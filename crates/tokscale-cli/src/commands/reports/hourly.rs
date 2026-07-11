use super::{build_report_options, PeriodReportArgs, TABLE_PRESET};
use crate::date_filter::get_date_range_label;
use crate::report_format::{
    capitalize_client, dim_borders, format_cost_per_million, format_currency, format_model_name,
    format_tokens_with_commas,
};
use crate::report_support::{emit_setup_warnings, setup_warnings_for_report};
use crate::spinner::LightSpinner;
use anyhow::Result;
use std::io::IsTerminal;

pub fn run_hourly_report(args: PeriodReportArgs) -> Result<()> {
    use std::time::Instant;
    use tokio::runtime::Runtime;
    use tokscale_core::{get_hourly_report_with_telemetry, GroupBy};

    let PeriodReportArgs {
        json,
        home_dir,
        clients,
        since,
        until,
        year,
        benchmark,
        no_spinner,
        today,
        week,
        month,
    } = args;

    let date_range = get_date_range_label(today, week, month, &since, &until, &year);

    let spinner = if no_spinner {
        None
    } else {
        Some(LightSpinner::start("Scanning session data..."))
    };
    let setup_warnings = setup_warnings_for_report(&home_dir, &clients);
    let start = Instant::now();
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
            get_hourly_report_with_telemetry(options, telemetry_store_path).await
        })
        .map_err(|e| anyhow::anyhow!(e))?;

    if let Some(spinner) = spinner {
        spinner.stop();
    }

    let processing_time_ms = start.elapsed().as_millis();

    if json {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct HourlyUsageJson {
            hour: String,
            clients: Vec<String>,
            models: Vec<String>,
            input: i64,
            output: i64,
            cache_read: i64,
            cache_write: i64,
            message_count: i32,
            turn_count: i32,
            cost: f64,
        }

        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct HourlyReportJson {
            entries: Vec<HourlyUsageJson>,
            total_cost: f64,
            processing_time_ms: u32,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            warnings: Vec<String>,
        }

        let output = HourlyReportJson {
            entries: report
                .entries
                .into_iter()
                .map(|e| HourlyUsageJson {
                    hour: e.hour,
                    clients: e.clients,
                    models: e.models,
                    input: e.input,
                    output: e.output,
                    cache_read: e.cache_read,
                    cache_write: e.cache_write,
                    message_count: e.message_count,
                    turn_count: e.turn_count,
                    cost: e.cost,
                })
                .collect(),
            total_cost: report.total_cost,
            processing_time_ms: report.processing_time_ms,
            warnings: setup_warnings,
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        use comfy_table::{Cell, CellAlignment, Color, ContentArrangement, Table};

        emit_setup_warnings(&setup_warnings);
        let term_width = crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(120);
        let compact = term_width < 100;

        let mut table = Table::new();
        table.load_preset(TABLE_PRESET);
        let arrangement = if std::io::stdout().is_terminal() {
            ContentArrangement::DynamicFullWidth
        } else {
            ContentArrangement::Dynamic
        };
        table.set_content_arrangement(arrangement);
        table.enforce_styling();

        if compact {
            table.set_header(vec![
                Cell::new("Hour").fg(Color::Cyan),
                Cell::new("Source").fg(Color::Cyan),
                Cell::new("Turn").fg(Color::Cyan),
                Cell::new("Msgs").fg(Color::Cyan),
                Cell::new("Input").fg(Color::Cyan),
                Cell::new("Output").fg(Color::Cyan),
                Cell::new("Cost").fg(Color::Cyan),
                Cell::new("Cost/1M").fg(Color::Cyan),
            ]);

            for entry in &report.entries {
                let clients_col = {
                    let mut c: Vec<String> =
                        entry.clients.iter().map(|s| capitalize_client(s)).collect();
                    c.sort();
                    c.join(", ")
                };
                let turn_display = if entry.turn_count > 0 {
                    entry.turn_count.to_string()
                } else {
                    "—".to_string()
                };
                let total_tokens =
                    entry.input + entry.output + entry.cache_read + entry.cache_write;
                table.add_row(vec![
                    Cell::new(&entry.hour).fg(Color::White),
                    Cell::new(&clients_col),
                    Cell::new(&turn_display).set_alignment(CellAlignment::Right),
                    Cell::new(entry.message_count).set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.input))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.output))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_currency(entry.cost))
                        .fg(Color::Green)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_cost_per_million(entry.cost, total_tokens))
                        .set_alignment(CellAlignment::Right),
                ]);
            }
        } else {
            table.set_header(vec![
                Cell::new("Hour").fg(Color::Cyan),
                Cell::new("Source").fg(Color::Cyan),
                Cell::new("Models").fg(Color::Cyan),
                Cell::new("Turn").fg(Color::Cyan),
                Cell::new("Msgs").fg(Color::Cyan),
                Cell::new("Input").fg(Color::Cyan),
                Cell::new("Output").fg(Color::Cyan),
                Cell::new("Cache R").fg(Color::Cyan),
                Cell::new("Cache W").fg(Color::Cyan),
                Cell::new("Cache Ratio").fg(Color::Cyan),
                Cell::new("Cost").fg(Color::Cyan),
                Cell::new("Cost/1M").fg(Color::Cyan),
            ]);

            for entry in &report.entries {
                let clients_col = {
                    let mut c: Vec<String> =
                        entry.clients.iter().map(|s| capitalize_client(s)).collect();
                    c.sort();
                    c.join(", ")
                };
                let models_col = if entry.models.is_empty() {
                    "-".to_string()
                } else {
                    let mut unique: Vec<String> = entry
                        .models
                        .iter()
                        .map(|m| format_model_name(m))
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    unique.sort();
                    unique.join(", ")
                };

                let cache_ratio = format_cache_ratio(
                    entry.cache_read as u64,
                    entry.input as u64,
                    entry.cache_write as u64,
                );

                let turn_display = if entry.turn_count > 0 {
                    entry.turn_count.to_string()
                } else {
                    "—".to_string()
                };

                let total_tokens =
                    entry.input + entry.output + entry.cache_read + entry.cache_write;

                table.add_row(vec![
                    Cell::new(&entry.hour).fg(Color::White),
                    Cell::new(&clients_col),
                    Cell::new(&models_col),
                    Cell::new(&turn_display).set_alignment(CellAlignment::Right),
                    Cell::new(entry.message_count).set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.input))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.output))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.cache_read))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_tokens_with_commas(entry.cache_write))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(&cache_ratio)
                        .fg(Color::Cyan)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_currency(entry.cost))
                        .fg(Color::Green)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(format_cost_per_million(entry.cost, total_tokens))
                        .set_alignment(CellAlignment::Right),
                ]);
            }
        }

        // Title
        use colored::Colorize;
        let title = if let Some(ref range) = date_range {
            format!("Hourly Usage ({})", range)
        } else {
            "Hourly Usage".to_string()
        };
        println!("\n  {}\n", title.bold());

        // Table
        let table_str = table.to_string();
        println!("{}", dim_borders(&table_str));

        // Footer with total
        println!(
            "\n  {}  {}",
            "Total:".bold(),
            format_currency(report.total_cost).green().bold()
        );

        if benchmark {
            println!(
                "{}",
                format!("  Processing time: {}ms (Rust native)", processing_time_ms).bright_black()
            );
        }
    }

    Ok(())
}

fn format_cache_ratio(cache_read: u64, input: u64, cache_write: u64) -> String {
    let input_side = input.saturating_add(cache_read).saturating_add(cache_write);
    if input_side == 0 {
        return "—".to_string();
    }
    format!("{:.1}%", cache_read as f64 / input_side as f64 * 100.0)
}
