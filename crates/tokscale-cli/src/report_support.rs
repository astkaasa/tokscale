use crate::client_filter::client_filter_explicitly_requests_warp;
use crate::warp;
use std::ffi::OsString;
use std::path::PathBuf;

pub(crate) struct PricingCacheOnlyGuard(Option<OsString>);

impl PricingCacheOnlyGuard {
    pub(crate) fn enable() -> Self {
        let previous = std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY");
        unsafe { std::env::set_var("TOKSCALE_PRICING_CACHE_ONLY", "1") };
        Self(previous)
    }
}

impl Drop for PricingCacheOnlyGuard {
    fn drop(&mut self) {
        unsafe {
            match self.0.take() {
                Some(value) => std::env::set_var("TOKSCALE_PRICING_CACHE_ONLY", value),
                None => std::env::remove_var("TOKSCALE_PRICING_CACHE_ONLY"),
            }
        }
    }
}

pub(crate) fn emit_setup_warnings(warnings: &[String]) {
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
    warp_setup_warnings_for_report(home_dir, clients)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn pricing_cache_only_guard_restores_environment() {
        let original = std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY");

        unsafe { std::env::set_var("TOKSCALE_PRICING_CACHE_ONLY", "previous") };
        let existing_enabled = {
            let _guard = PricingCacheOnlyGuard::enable();
            std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY")
        };
        let existing_restored = std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY");

        unsafe { std::env::remove_var("TOKSCALE_PRICING_CACHE_ONLY") };
        let missing_enabled = {
            let _guard = PricingCacheOnlyGuard::enable();
            std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY")
        };
        let missing_restored = std::env::var_os("TOKSCALE_PRICING_CACHE_ONLY");

        unsafe {
            match original {
                Some(value) => std::env::set_var("TOKSCALE_PRICING_CACHE_ONLY", value),
                None => std::env::remove_var("TOKSCALE_PRICING_CACHE_ONLY"),
            }
        }

        assert_eq!(existing_enabled.as_deref(), Some(std::ffi::OsStr::new("1")));
        assert_eq!(
            existing_restored.as_deref(),
            Some(std::ffi::OsStr::new("previous"))
        );
        assert_eq!(missing_enabled.as_deref(), Some(std::ffi::OsStr::new("1")));
        assert_eq!(missing_restored, None);
    }

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
}
