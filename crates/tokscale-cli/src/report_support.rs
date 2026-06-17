use crate::client_filter::{
    client_filter_explicitly_requests_cursor, client_filter_explicitly_requests_warp,
    client_filter_includes_cursor,
};
use crate::{cursor, warp};
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug)]
struct CursorSetupState {
    has_credentials: bool,
    has_cache: bool,
    cache_glob: String,
    home_override: bool,
}

fn cursor_setup_state(home_dir: &Option<String>) -> Option<CursorSetupState> {
    let (home_path, home_override) = match home_dir {
        Some(home) => (PathBuf::from(home), true),
        None => (dirs::home_dir()?, false),
    };
    let has_credentials = if home_override {
        cursor::has_active_credentials_in_home(&home_path)
    } else {
        cursor::is_cursor_logged_in()
    };
    let has_cache = cursor::has_cursor_usage_cache_in_home(&home_path);
    let cache_glob = if home_override {
        home_path
            .join(".config/tokscale/cursor-cache/usage*.csv")
            .to_string_lossy()
            .to_string()
    } else {
        "~/.config/tokscale/cursor-cache/usage*.csv".to_string()
    };

    Some(CursorSetupState {
        has_credentials,
        has_cache,
        cache_glob,
        home_override,
    })
}

pub(crate) fn has_cursor_usage_cache_for_report(home_dir: &Option<String>) -> bool {
    cursor_setup_state(home_dir).is_some_and(|state| state.has_cache)
}

fn cursor_setup_warnings_for_report(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> Vec<String> {
    if !client_filter_explicitly_requests_cursor(clients) {
        return Vec::new();
    }

    let Some(state) = cursor_setup_state(home_dir) else {
        return vec![
            "Cursor usage requires Tokscale's Cursor API cache, but the home directory could not be resolved. Run `tokscale cursor login` and `tokscale cursor sync --json`. Tokscale does not parse local `~/.cursor` session data.".to_string(),
        ];
    };
    if state.has_cache {
        return Vec::new();
    }

    let action = if state.home_override {
        "run `tokscale cursor login` and `tokscale cursor sync --json`, or populate that cache before running a report with --home"
    } else if state.has_credentials {
        "run `tokscale cursor sync --json`"
    } else {
        "run `tokscale cursor login` and `tokscale cursor sync --json`"
    };

    vec![format!(
        "Cursor usage requires Tokscale's Cursor API cache at `{}`; {}. Tokscale does not parse local `~/.cursor` session data.",
        state.cache_glob, action
    )]
}

pub(crate) fn emit_cursor_setup_warnings(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }

    use colored::Colorize;
    for warning in warnings {
        eprintln!("{}", format!("  Warning: {}", warning).yellow());
    }
}

fn warp_setup_warnings_for_report(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> Vec<String> {
    if !client_filter_explicitly_requests_warp(clients) {
        return Vec::new();
    }

    let (home_path, home_override) = match home_dir {
        Some(home) => (PathBuf::from(home), true),
        None => match dirs::home_dir() {
            Some(home) => (home, false),
            None => {
                return vec![
                    "Warp usage requires Tokscale's Warp aggregate cache, but the home directory could not be resolved. Tokscale does not parse local Warp transcripts.".to_string(),
                ];
            }
        },
    };
    let has_cache = if home_override {
        warp::has_usage_cache_in_home(&home_path)
    } else {
        warp::load_usage_cache().is_some()
    };
    if has_cache {
        return Vec::new();
    }

    let cache_glob = if home_override {
        home_path
            .join(".config/tokscale/warp-cache/usage*.json")
            .to_string_lossy()
            .to_string()
    } else {
        "~/.config/tokscale/warp-cache/usage*.json".to_string()
    };
    let action = if home_override {
        "run `tokscale warp sync` for the default profile or populate that cache before running a report with --home"
    } else if warp::has_credentials() {
        "run `tokscale warp sync`"
    } else {
        "run `tokscale warp login` and `tokscale warp sync`"
    };

    vec![format!(
        "Warp usage requires Tokscale's aggregate API cache at `{}`; {}. Tokscale does not parse local Warp/Oz session transcripts and does not infer tokens from request counts.",
        cache_glob, action
    )]
}

pub(crate) fn setup_warnings_for_report(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> Vec<String> {
    let mut warnings = cursor_setup_warnings_for_report(home_dir, clients);
    warnings.extend(warp_setup_warnings_for_report(home_dir, clients));
    warnings
}

fn should_auto_sync_cursor_for_local_report(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> bool {
    home_dir.is_none() && client_filter_includes_cursor(clients)
}

pub(crate) fn auto_sync_cursor_for_local_report(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> Option<cursor::SyncCursorResult> {
    if !should_auto_sync_cursor_for_local_report(home_dir, clients)
        || !cursor::is_cursor_logged_in()
    {
        return None;
    }

    // Skip the implicit refresh when each expected Cursor account cache is
    // recent enough — running `tokscale models` 30x in a script must not
    // produce 30 Cursor API calls. The manual `tokscale cursor sync` command
    // bypasses this gate.
    if cursor::cursor_usage_cache_is_fresh(cursor::CURSOR_AUTO_SYNC_FRESHNESS) {
        return None;
    }

    Some(run_best_effort_cursor_sync_with_runtime_factory(
        tokio::runtime::Runtime::new,
    ))
}

fn run_best_effort_cursor_sync_with_runtime_factory<F>(build_runtime: F) -> cursor::SyncCursorResult
where
    F: FnOnce() -> std::io::Result<tokio::runtime::Runtime>,
{
    match build_runtime() {
        Ok(rt) => rt.block_on(async { cursor::sync_cursor_cache().await }),
        Err(error) => cursor::SyncCursorResult {
            synced: false,
            rows: 0,
            error: Some(format!(
                "Failed to initialize Cursor sync runtime: {}",
                error
            )),
        },
    }
}

pub(crate) fn auto_sync_cursor_before_tui(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
) -> Result<()> {
    let had_cursor_cache = has_cursor_usage_cache_for_report(home_dir);
    let explicit_cursor_filter = client_filter_explicitly_requests_cursor(clients);
    let cursor_sync_result = auto_sync_cursor_for_local_report(home_dir, clients);
    emit_cursor_sync_warning(
        cursor_sync_result.as_ref(),
        had_cursor_cache,
        explicit_cursor_filter,
    );
    let cursor_setup_warnings = setup_warnings_for_report(home_dir, clients);
    emit_cursor_setup_warnings(&cursor_setup_warnings);
    Ok(())
}

pub(crate) fn emit_cursor_sync_warning(
    sync: Option<&cursor::SyncCursorResult>,
    had_cursor_cache: bool,
    explicit_cursor_filter: bool,
) {
    let Some(sync) = sync else {
        return;
    };
    let Some(error) = sync.error.as_ref() else {
        return;
    };
    if sync.synced || had_cursor_cache || explicit_cursor_filter {
        use colored::Colorize;
        let prefix = if sync.synced {
            "Cursor sync warning"
        } else if had_cursor_cache {
            "Cursor sync failed; using cached data"
        } else {
            "Cursor sync failed"
        };
        eprintln!("{}", format!("  {}: {}", prefix, error).yellow());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_setup_warning_explains_missing_aggregate_cache() {
        let temp = tempfile::TempDir::new().unwrap();
        let warnings = warp_setup_warnings_for_report(
            &Some(temp.path().to_string_lossy().to_string()),
            &Some(vec!["warp".to_string()]),
        );

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("tokscale warp"));
        assert!(warnings[0].contains("does not infer tokens from request counts"));
    }

    #[test]
    fn cursor_auto_sync_enabled_for_default_report() {
        assert!(should_auto_sync_cursor_for_local_report(&None, &None));
    }

    #[test]
    fn cursor_auto_sync_enabled_when_cursor_filter_is_explicit() {
        assert!(should_auto_sync_cursor_for_local_report(
            &None,
            &Some(vec!["cursor".to_string()])
        ));
    }

    #[test]
    fn cursor_auto_sync_disabled_when_filter_excludes_cursor() {
        assert!(!should_auto_sync_cursor_for_local_report(
            &None,
            &Some(vec!["codex".to_string()])
        ));
    }

    #[test]
    fn cursor_auto_sync_disabled_for_home_override() {
        assert!(!should_auto_sync_cursor_for_local_report(
            &Some("/tmp/other-home".to_string()),
            &None
        ));
        assert!(!should_auto_sync_cursor_for_local_report(
            &Some("/tmp/other-home".to_string()),
            &Some(vec!["cursor".to_string()])
        ));
    }

    #[test]
    fn cursor_auto_sync_runtime_init_failure_is_best_effort() {
        let result = run_best_effort_cursor_sync_with_runtime_factory(|| {
            Err(std::io::Error::other("runtime unavailable"))
        });

        assert!(!result.synced);
        assert_eq!(result.rows, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("runtime unavailable")));
    }
}
