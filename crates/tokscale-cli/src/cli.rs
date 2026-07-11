use crate::tui::ThemePreference;
use crate::{ClientFlags, DateRangeFlags};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tokscale")]
#[command(author, version, about = "Local-first AI and personal telemetry")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[arg(short, long, default_value = "0")]
    pub(crate) refresh: u64,

    #[arg(long)]
    pub(crate) debug: bool,

    #[arg(
        long,
        global = true,
        value_name = "THEME",
        value_parser = parse_theme_preference,
        help = "TUI color theme: dark, light, or auto"
    )]
    pub(crate) theme: Option<ThemePreference>,

    #[arg(long, help = "Output as JSON")]
    pub(crate) json: bool,

    #[arg(long, help = "Use plain CLI table output")]
    pub(crate) light: bool,

    #[arg(
        long = "write-cache",
        requires = "light",
        conflicts_with = "no_write_cache",
        help = "After --light renders, atomically overwrite the TUI cache with this report's data so the next `tokscale tui` starts from fresh data. Persists across invocations via settings.json `light.writeCache`."
    )]
    pub(crate) write_cache: bool,

    #[arg(
        long = "no-write-cache",
        requires = "light",
        conflicts_with = "write_cache",
        help = "Skip cache write even if settings.json `light.writeCache` is true. Only valid with --light."
    )]
    pub(crate) no_write_cache: bool,

    #[command(flatten)]
    pub(crate) clients: ClientFlags,

    #[command(flatten)]
    pub(crate) date: DateRangeFlags,

    #[arg(
        long,
        value_name = "PATH",
        global = true,
        help = "Read local session data from this home directory for local report commands; TUI and remote sync commands reject this flag"
    )]
    pub(crate) home: Option<String>,

    #[arg(long, help = "Show processing time")]
    pub(crate) benchmark: bool,

    #[arg(
        long,
        value_name = "STRATEGY",
        default_value = "client,model",
        help = "Grouping strategy for --light and --json output: model, client,model, client,provider,model, workspace,model, session,model, client,session,model"
    )]
    pub(crate) group_by: String,

    #[arg(long, help = "Disable spinner (for AI agents and scripts)")]
    pub(crate) no_spinner: bool,
}

fn parse_theme_preference(value: &str) -> Result<ThemePreference, String> {
    value.parse()
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    #[command(about = "Show model usage report")]
    Models {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        light: bool,
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
        #[arg(long, help = "Show processing time")]
        benchmark: bool,
        #[arg(
            long,
            value_name = "STRATEGY",
            default_value = "client,model",
            help = "Grouping strategy for --light and --json output: model, client,model, client,provider,model, workspace,model, session,model, client,session,model"
        )]
        group_by: String,
        #[arg(
            long = "write-cache",
            requires = "light",
            conflicts_with = "no_write_cache",
            help = "After --light renders, atomically overwrite the TUI cache with this report's data so the next `tokscale tui` starts from fresh data. Persists across invocations via settings.json `light.writeCache`."
        )]
        write_cache: bool,
        #[arg(
            long = "no-write-cache",
            requires = "light",
            conflicts_with = "write_cache",
            help = "Skip cache write even if settings.json `light.writeCache` is true. Only valid with --light."
        )]
        no_write_cache: bool,
        #[arg(long, help = "Disable spinner")]
        no_spinner: bool,
    },
    #[command(about = "Show monthly usage report")]
    Monthly {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        light: bool,
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
        #[arg(long, help = "Show processing time")]
        benchmark: bool,
        #[arg(long, help = "Disable spinner")]
        no_spinner: bool,
    },
    #[command(about = "Show hourly usage report")]
    Hourly {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        light: bool,
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
        #[arg(long, help = "Show processing time")]
        benchmark: bool,
        #[arg(long, help = "Disable spinner")]
        no_spinner: bool,
    },
    #[command(about = "Show pricing for a model")]
    Pricing {
        #[arg(help = "Model ID to look up, or `list-overrides`")]
        model_id: String,
        #[arg(long, help = "Output as JSON")]
        json: bool,
        #[arg(
            long,
            help = "Force specific pricing source (custom, litellm, openrouter, or models.dev)"
        )]
        provider: Option<String>,
        #[arg(long, help = "Disable spinner")]
        no_spinner: bool,
    },
    #[command(about = "Show local scan locations and session counts")]
    Clients {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Launch interactive TUI with optional filters")]
    Tui {
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
    },
    #[command(about = "Serve local read-only telemetry surfaces")]
    Serve {
        #[arg(long, default_value_t = 8765, help = "Localhost port to bind")]
        port: u16,
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
        #[arg(
            long,
            value_name = "STRATEGY",
            default_value = "model",
            help = "Grouping strategy for the overview: model, client,model, client,provider,model, workspace,model, session,model, client,session,model"
        )]
        group_by: String,
        #[arg(long, help = "Disable startup spinner")]
        no_spinner: bool,
    },
    #[command(about = "Capture subprocess output for token usage tracking")]
    Headless {
        #[arg(help = "Source CLI (currently only 'codex' supported)")]
        source: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long, help = "Override output format (json or jsonl)")]
        format: Option<String>,
        #[arg(long, help = "Write captured output to file")]
        output: Option<String>,
        #[arg(long, help = "Do not auto-add JSON output flags")]
        no_auto_flags: bool,
    },
    #[command(about = "Show subscription usage and quota for AI providers")]
    Usage {
        #[arg(long, help = "Output as JSON")]
        json: bool,
        #[arg(long, help = "Light terminal output (no TUI)")]
        light: bool,
    },
    #[command(about = "Generate a local Personal Pulse digest")]
    Pulse {
        #[command(subcommand)]
        subcommand: Option<PulseSubcommand>,
        #[arg(long, global = true, help = "Output agent-readable JSON")]
        json: bool,
        #[arg(
            long,
            global = true,
            conflicts_with = "json",
            help = "Output Markdown weekly digest"
        )]
        weekly: bool,
        #[arg(long, help = "Refresh local and connector inputs before rendering")]
        refresh: bool,
        #[arg(long, global = true, help = "Disable Pulse spinner")]
        no_spinner: bool,
    },
    #[command(about = "Archive and import local Cursor history")]
    Cursor {
        #[command(subcommand)]
        subcommand: CursorSubcommand,
    },
    #[command(about = "Antigravity integration commands")]
    Antigravity {
        #[command(subcommand)]
        subcommand: AntigravitySubcommand,
    },
    #[command(about = "Trae IDE integration commands")]
    Trae {
        #[command(subcommand)]
        subcommand: TraeSubcommand,
    },
    #[command(about = "Warp/Oz aggregate usage integration commands")]
    Warp {
        #[command(subcommand)]
        subcommand: WarpSubcommand,
    },
    #[command(
        about = "Show session time metrics (usage time, longest continuous, max concurrent)"
    )]
    TimeMetrics {
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        clients: ClientFlags,
        #[command(flatten)]
        date: DateRangeFlags,
        #[arg(long, help = "Disable spinner")]
        no_spinner: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum PulseSubcommand {
    #[command(about = "Refresh Pulse inputs and write the local snapshot")]
    Sync,
}

#[derive(Subcommand)]
pub(crate) enum CursorSubcommand {
    #[command(about = "Archive and import existing local Cursor usage CSV files")]
    Import {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AntigravitySubcommand {
    #[command(about = "Sync usage from running Antigravity language servers")]
    Sync,
    #[command(about = "Show Antigravity sync status")]
    Status {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Delete cached Antigravity usage artifacts")]
    PurgeCache,
}

#[derive(Subcommand)]
pub(crate) enum TraeSubcommand {
    #[command(about = "Authenticate Trae — auto-detect from desktop client or paste JWT")]
    Login {
        #[arg(long, help = "Paste access token directly (for manual fallback)")]
        manual: bool,
        #[arg(long, help = "Target Trae variant (solo, ide)")]
        variant: Option<String>,
    },
    #[command(about = "Remove cached Trae credentials")]
    Logout {
        #[arg(long, help = "Target Trae variant (solo, ide)")]
        variant: Option<String>,
    },
    #[command(about = "Show Trae authentication status")]
    Status {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Sync Trae usage data into local cache")]
    Sync {
        #[arg(long, help = "Number of days to sync (default: 30)")]
        since: Option<i64>,
        #[arg(long, help = "Include auxiliary usage types (not just main chat)")]
        include_aux: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WarpSubcommand {
    #[command(about = "Save Warp GraphQL authentication for aggregate usage sync")]
    Login {
        #[arg(long, help = "Warp bearer token or cookie header value")]
        token: Option<String>,
        #[arg(
            long,
            help = "Treat token as a Cookie header instead of a bearer token"
        )]
        cookie: bool,
    },
    #[command(about = "Remove cached Warp credentials")]
    Logout {
        #[arg(long, help = "Also delete cached Warp aggregate usage")]
        purge_cache: bool,
    },
    #[command(about = "Show Warp aggregate sync status")]
    Status {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Sync Warp aggregate usage into local cache")]
    Sync {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::pulse::PulseRunArgs;
    use clap::Parser;

    fn assert_pulse_sync_export_rejected(args: &[&str]) {
        let cli = Cli::try_parse_from(args).expect("clap should accept either flag position");
        let Some(Commands::Pulse {
            subcommand,
            json,
            weekly,
            refresh,
            no_spinner,
        }) = cli.command
        else {
            panic!("expected pulse command");
        };
        let error = PulseRunArgs {
            json,
            weekly,
            refresh,
            sync_only: matches!(subcommand, Some(PulseSubcommand::Sync)),
            no_spinner,
        }
        .validate()
        .unwrap_err();

        assert!(error.to_string().contains("cannot be combined"));
    }

    #[test]
    fn clap_rejects_write_cache_without_light() {
        assert!(Cli::try_parse_from(["tokscale", "--write-cache"]).is_err());
    }

    #[test]
    fn clap_rejects_no_write_cache_without_light() {
        assert!(Cli::try_parse_from(["tokscale", "--no-write-cache"]).is_err());
    }

    #[test]
    fn clap_rejects_both_write_flags_together() {
        assert!(
            Cli::try_parse_from(["tokscale", "--light", "--write-cache", "--no-write-cache",])
                .is_err()
        );
    }

    #[test]
    fn clap_accepts_models_light_write_cache_after_subcommand() {
        assert!(Cli::try_parse_from(["tokscale", "models", "--light", "--write-cache"]).is_ok());
    }

    #[test]
    fn clap_accepts_supported_theme_values() {
        assert_eq!(
            Cli::try_parse_from(["tokscale", "--theme", "light"])
                .unwrap()
                .theme,
            Some(ThemePreference::Light)
        );
        assert!(Cli::try_parse_from(["tokscale", "tui", "--theme", "auto"]).is_ok());
    }

    #[test]
    fn clap_rejects_unknown_theme_values() {
        assert!(Cli::try_parse_from(["tokscale", "--theme", "blue"]).is_err());
    }

    #[test]
    fn clap_accepts_serve_command() {
        assert!(Cli::try_parse_from(["tokscale", "serve"]).is_ok());
        assert!(Cli::try_parse_from([
            "tokscale",
            "serve",
            "--port",
            "0",
            "--client",
            "codex,claude",
            "--week",
            "--no-spinner",
        ])
        .is_ok());
    }

    #[test]
    fn clap_accepts_pulse_exports_and_sync() {
        assert!(Cli::try_parse_from(["tokscale", "pulse", "--weekly"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "pulse", "--json", "--refresh"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "pulse", "sync", "--no-spinner"]).is_ok());
    }

    #[test]
    fn pulse_sync_rejects_export_flags_before_subcommand() {
        assert_pulse_sync_export_rejected(&["tokscale", "pulse", "--json", "sync"]);
        assert_pulse_sync_export_rejected(&["tokscale", "pulse", "--weekly", "sync"]);
    }

    #[test]
    fn pulse_sync_rejects_export_flags_after_subcommand() {
        assert_pulse_sync_export_rejected(&["tokscale", "pulse", "sync", "--json"]);
        assert_pulse_sync_export_rejected(&["tokscale", "pulse", "sync", "--weekly"]);
    }

    #[test]
    fn clap_accepts_cursor_import_command() {
        assert!(Cli::try_parse_from(["tokscale", "cursor", "import"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "cursor", "import", "--json"]).is_ok());
    }

    #[test]
    fn clap_rejects_removed_cursor_control_commands() {
        for command in ["login", "logout", "status", "accounts", "sync", "switch"] {
            assert!(Cli::try_parse_from(["tokscale", "cursor", command]).is_err());
        }
    }

    #[test]
    fn clap_accepts_warp_status_and_sync_commands() {
        assert!(Cli::try_parse_from(["tokscale", "warp", "status"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "warp", "status", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "warp", "sync"]).is_ok());
        assert!(Cli::try_parse_from(["tokscale", "warp", "sync", "--json"]).is_ok());
    }
}
