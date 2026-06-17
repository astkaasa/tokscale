mod claude_diagnostics;
mod cli;
mod client_filter;
mod commands;
mod date_filter;
mod integrations;
mod paths;
mod report_format;
mod report_support;
mod spinner;
mod tui;

use crate::cli::{Cli, Commands};
use crate::commands::reports::{ModelsReportArgs, PeriodReportArgs, TimeMetricsReportArgs};
use crate::date_filter::{build_date_filter, normalize_year_filter};
use crate::integrations::{antigravity, cursor, trae, warp};
use crate::report_support::auto_sync_cursor_before_tui;
use anyhow::Result;
use clap::Parser;
use client_filter::build_client_filter;
pub(crate) use client_filter::ClientFilter;
pub(crate) use client_filter::{ClientFlags, DateRangeFlags};
use std::io::IsTerminal;
use tui::Tab;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let can_use_tui = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    match cli.command {
        Some(Commands::Models {
            json,
            light,
            clients,
            date,
            benchmark,
            group_by,
            write_cache,
            no_write_cache,
            no_spinner,
        }) => {
            use tokscale_core::GroupBy;

            let group_by: GroupBy = group_by.parse().unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });
            let today = date.today;
            let week = date.week;
            let month = date.month;
            let (since, until) = build_date_filter(today, week, month, date.since, date.until);
            let year = normalize_year_filter(today, week, month, date.year);
            let clients = build_client_filter(clients, &cli.home);
            if json || light || !can_use_tui {
                commands::reports::run_models_report(ModelsReportArgs {
                    json,
                    home_dir: cli.home.clone(),
                    clients,
                    since,
                    until,
                    year,
                    benchmark,
                    no_spinner: no_spinner || !can_use_tui,
                    today,
                    week,
                    month,
                    group_by,
                    write_cache,
                    no_write_cache,
                })
            } else {
                ensure_home_supported_for_tui(&cli.home)?;
                auto_sync_cursor_before_tui(&cli.home, &clients)?;
                tui::run(
                    &cli.theme,
                    cli.refresh,
                    cli.debug,
                    clients,
                    since,
                    until,
                    year,
                    Some(Tab::Models),
                )
            }
        }
        Some(Commands::Monthly {
            json,
            light,
            clients,
            date,
            benchmark,
            no_spinner,
        }) => {
            let today = date.today;
            let week = date.week;
            let month = date.month;
            let (since, until) = build_date_filter(today, week, month, date.since, date.until);
            let year = normalize_year_filter(today, week, month, date.year);
            let clients = build_client_filter(clients, &cli.home);
            if json || light || !can_use_tui {
                commands::reports::run_monthly_report(PeriodReportArgs {
                    json,
                    home_dir: cli.home.clone(),
                    clients,
                    since,
                    until,
                    year,
                    benchmark,
                    no_spinner: no_spinner || !can_use_tui,
                    today,
                    week,
                    month,
                })
            } else {
                ensure_home_supported_for_tui(&cli.home)?;
                auto_sync_cursor_before_tui(&cli.home, &clients)?;
                tui::run(
                    &cli.theme,
                    cli.refresh,
                    cli.debug,
                    clients,
                    since,
                    until,
                    year,
                    Some(Tab::Daily),
                )
            }
        }
        Some(Commands::Hourly {
            json,
            light,
            clients,
            date,
            benchmark,
            no_spinner,
        }) => {
            let today = date.today;
            let week = date.week;
            let month = date.month;
            let (since, until) = build_date_filter(today, week, month, date.since, date.until);
            let year = normalize_year_filter(today, week, month, date.year);
            let clients = build_client_filter(clients, &cli.home);
            if json || light || !can_use_tui {
                commands::reports::run_hourly_report(PeriodReportArgs {
                    json,
                    home_dir: cli.home.clone(),
                    clients,
                    since,
                    until,
                    year,
                    benchmark,
                    no_spinner: no_spinner || !can_use_tui,
                    today,
                    week,
                    month,
                })
            } else {
                ensure_home_supported_for_tui(&cli.home)?;
                auto_sync_cursor_before_tui(&cli.home, &clients)?;
                tui::run(
                    &cli.theme,
                    cli.refresh,
                    cli.debug,
                    clients,
                    since,
                    until,
                    year,
                    Some(Tab::Hourly),
                )
            }
        }
        Some(Commands::Pricing {
            model_id,
            json,
            provider,
            no_spinner,
        }) => {
            reject_unsupported_home_override(&cli.home, "pricing")?;
            commands::pricing::run(&model_id, json, provider.as_deref(), no_spinner)
        }
        Some(Commands::Clients { json }) => commands::clients::run(json, cli.home.clone()),
        Some(Commands::Tui { clients, date }) => {
            ensure_home_supported_for_tui(&cli.home)?;
            let today = date.today;
            let week = date.week;
            let month = date.month;
            let (since, until) = build_date_filter(today, week, month, date.since, date.until);
            let year = normalize_year_filter(today, week, month, date.year);
            let clients = build_client_filter(clients, &cli.home);
            auto_sync_cursor_before_tui(&cli.home, &clients)?;
            tui::run(
                &cli.theme,
                cli.refresh,
                cli.debug,
                clients,
                since,
                until,
                year,
                None,
            )
        }
        Some(Commands::Headless {
            source,
            args,
            format,
            output,
            no_auto_flags,
        }) => {
            reject_unsupported_home_override(&cli.home, "headless")?;
            commands::headless::run(&source, args, format, output, no_auto_flags)
        }
        Some(Commands::Cursor { subcommand }) => {
            reject_unsupported_home_override(&cli.home, "cursor")?;
            cursor::run_cli_command(subcommand)
        }
        Some(Commands::Antigravity { subcommand }) => {
            reject_unsupported_home_override(&cli.home, "antigravity")?;
            antigravity::run_cli_command(subcommand)
        }
        Some(Commands::Usage { json, light }) => {
            reject_unsupported_home_override(&cli.home, "usage")?;
            commands::usage::run(json, light)
        }
        Some(Commands::Pulse { json, weekly: _ }) => {
            reject_unsupported_home_override(&cli.home, "pulse")?;
            commands::pulse::run(json)
        }
        Some(Commands::Trae { subcommand }) => {
            reject_unsupported_home_override(&cli.home, "trae")?;
            trae::run_cli_command(subcommand)
        }
        Some(Commands::Warp { subcommand }) => {
            reject_unsupported_home_override(&cli.home, "warp")?;
            warp::run_cli_command(subcommand)
        }
        Some(Commands::TimeMetrics {
            json,
            clients,
            date,
            no_spinner,
        }) => {
            let today = date.today;
            let week = date.week;
            let month = date.month;
            let (since, until) = build_date_filter(today, week, month, date.since, date.until);
            let year = normalize_year_filter(today, week, month, date.year);
            let clients = build_client_filter(clients, &cli.home);
            commands::reports::run_time_metrics_report(TimeMetricsReportArgs {
                json,
                home_dir: cli.home.clone(),
                clients,
                since,
                until,
                year,
                no_spinner,
            })
        }
        None => {
            let today = cli.date.today;
            let week = cli.date.week;
            let month = cli.date.month;
            let clients = build_client_filter(cli.clients, &cli.home);
            let (since, until) =
                build_date_filter(today, week, month, cli.date.since, cli.date.until);
            let year = normalize_year_filter(today, week, month, cli.date.year);
            let group_by: tokscale_core::GroupBy = cli.group_by.parse().unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });

            if cli.json {
                commands::reports::run_models_report(ModelsReportArgs {
                    json: cli.json,
                    home_dir: cli.home.clone(),
                    clients,
                    since,
                    until,
                    year,
                    benchmark: cli.benchmark,
                    no_spinner: cli.no_spinner || cli.json,
                    today,
                    week,
                    month,
                    group_by,
                    write_cache: cli.write_cache,
                    no_write_cache: cli.no_write_cache,
                })
            } else if cli.light || !can_use_tui {
                commands::reports::run_models_report(ModelsReportArgs {
                    json: false,
                    home_dir: cli.home.clone(),
                    clients,
                    since,
                    until,
                    year,
                    benchmark: cli.benchmark,
                    no_spinner: cli.no_spinner || !can_use_tui,
                    today,
                    week,
                    month,
                    group_by,
                    write_cache: cli.write_cache,
                    no_write_cache: cli.no_write_cache,
                })
            } else {
                ensure_home_supported_for_tui(&cli.home)?;
                auto_sync_cursor_before_tui(&cli.home, &clients)?;
                tui::run(
                    &cli.theme,
                    cli.refresh,
                    cli.debug,
                    clients,
                    since,
                    until,
                    year,
                    None,
                )
            }
        }
    }
}

fn reject_unsupported_home_override(home_dir: &Option<String>, command: &str) -> Result<()> {
    if home_dir.is_some() {
        return Err(anyhow::anyhow!(
            "--home is currently supported only for local report commands. It is not supported for `{}`.",
            command
        ));
    }

    Ok(())
}

fn ensure_home_supported_for_tui(home_dir: &Option<String>) -> Result<()> {
    if home_dir.is_some() {
        return Err(anyhow::anyhow!(
            "--home is currently supported for local report commands only. Use `--json`, `--light`, `models`, `monthly`, `hourly`, `time-metrics`, or `clients` instead of TUI mode."
        ));
    }

    Ok(())
}
