use crate::client_filter::{resolve_default_tui_filter_set, ResolvedClientSelection};
use crate::tui;

pub(crate) fn resolve_should_write_cache(
    cli_write: bool,
    cli_no_write: bool,
    settings: &tui::settings::Settings,
) -> bool {
    if cli_no_write {
        return false;
    }
    if cli_write {
        return true;
    }
    settings.light.write_cache
}

fn resolve_light_cache_selection(clients: &Option<Vec<String>>) -> ResolvedClientSelection {
    if clients.is_some() {
        ResolvedClientSelection::from_configured(clients.as_deref())
    } else {
        ResolvedClientSelection::from_filters(resolve_default_tui_filter_set())
    }
}

pub(crate) fn write(
    home_dir: &Option<String>,
    clients: &Option<Vec<String>>,
    since: &Option<String>,
    until: &Option<String>,
    year: &Option<String>,
    group_by: &tokscale_core::GroupBy,
) {
    use crate::tui::{save_cached_data, CacheReportScope, DataLoader};

    // The TUI cache key includes date filters, but not `--home`. Writing
    // home-scoped data would still poison the default cache, so keep that
    // guard until home is part of the cache key.
    if home_dir.is_some() {
        eprintln!(
            "tokscale: --write-cache skipped because --home is set; \
             the TUI cache key does not include that filter and writing would poison future TUI launches."
        );
        return;
    }

    let selection = resolve_light_cache_selection(clients);

    // Cache writes are best-effort: the report has already been flushed
    // to stdout by the time we reach here, so a scan failure from the
    // background loader must NOT propagate up and turn a successful
    // user-visible report into a non-zero exit code.
    let loader = DataLoader::with_filters(since.clone(), until.clone(), year.clone());
    let report_scope = CacheReportScope::new(since.clone(), until.clone(), year.clone());
    if let Ok(observation) = loader.load(
        &selection.scan_clients,
        group_by,
        selection.include_synthetic,
    ) {
        save_cached_data(&observation, &selection.filters, group_by, &report_scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cli_write_overrides_settings_false() {
        let settings = tui::settings::Settings {
            light: tui::settings::LightSettings { write_cache: false },
            ..tui::settings::Settings::default()
        };
        assert!(resolve_should_write_cache(true, false, &settings));
    }

    #[test]
    fn resolve_cli_no_write_overrides_settings_true() {
        let settings = tui::settings::Settings {
            light: tui::settings::LightSettings { write_cache: true },
            ..tui::settings::Settings::default()
        };
        assert!(!resolve_should_write_cache(false, true, &settings));
    }

    #[test]
    fn resolve_settings_true_with_no_cli_flag() {
        let settings = tui::settings::Settings {
            light: tui::settings::LightSettings { write_cache: true },
            ..tui::settings::Settings::default()
        };
        assert!(resolve_should_write_cache(false, false, &settings));
    }

    #[test]
    fn resolve_settings_false_with_no_cli_flag() {
        let settings = tui::settings::Settings {
            light: tui::settings::LightSettings { write_cache: false },
            ..tui::settings::Settings::default()
        };
        assert!(!resolve_should_write_cache(false, false, &settings));
    }

    #[test]
    fn resolve_settings_default_returns_false() {
        let settings = tui::settings::Settings::default();
        assert!(!resolve_should_write_cache(false, false, &settings));
    }

    #[test]
    fn write_refuses_when_home_dir_set() {
        let group_by = tokscale_core::GroupBy::default();
        write(
            &Some("/tmp/fake-home".to_string()),
            &None,
            &None,
            &None,
            &None,
            &group_by,
        );
    }
}
