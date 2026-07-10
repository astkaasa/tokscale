mod app;
mod background_job;
mod cache;
pub(crate) mod client_ui;
pub(crate) mod codex_login;
mod colors;
pub(crate) mod data;
mod drilldown_state;
mod event;
mod export;
mod interaction;
mod navigation;
pub(crate) mod privacy;
mod pulse_state;
pub(crate) mod settings;
pub(crate) mod surface;
mod themes;
mod ui;

pub(crate) use app::{App, PulseDataProvenance, Tab, TimelineGranularity, TuiConfig};
pub(crate) use cache::{
    load_cache, load_cache_with_observed_at, save_cached_data, CacheReportScope, CacheResult,
    TUI_DEFAULT_GROUP_BY,
};
pub(crate) use data::{DataLoader, UsageData};
pub(crate) use event::{Event, EventHandler};
pub(crate) use themes::{Theme, ThemePreference};

use std::collections::HashSet;
use std::io;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::Arc;

use std::panic;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use ratatui::prelude::*;
use tokscale_core::ClientId;

use crate::ClientFilter;

fn decide_initial_data(
    load_result: CacheResult,
    cache_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    requested_provenance: PulseDataProvenance,
) -> (
    Option<UsageData>,
    bool,
    PulseDataProvenance,
    Option<chrono::DateTime<chrono::Utc>>,
) {
    let (cached_data, provenance, observed_at) = match load_result {
        CacheResult::Fresh(data) => (Some(data), requested_provenance, cache_observed_at),
        CacheResult::Stale(data) => (Some(data), requested_provenance.as_stale(), None),
        CacheResult::StaleSubset(data) => (Some(data), PulseDataProvenance::Unverified, None),
        CacheResult::Miss => (None, PulseDataProvenance::Unverified, None),
    };

    (cached_data, true, provenance, observed_at)
}

struct BackgroundLoadResult {
    result: Result<UsageData>,
    provenance: PulseDataProvenance,
}

fn apply_background_load_result(app: &mut App, message: BackgroundLoadResult) {
    app.set_background_loading(false);
    match message.result {
        Ok(data) => match app.update_data(data, message.provenance) {
            Ok(()) => app.set_status("Data loaded"),
            Err(error) => {
                app.set_status(&format!("Pulse snapshot save failed: {error}"));
            }
        },
        Err(error) => {
            app.set_error(Some(error.to_string()));
            app.set_status(&format!("Error: {error}"));
        }
    }
}

fn background_data_loader(
    since: Option<String>,
    until: Option<String>,
    year: Option<String>,
) -> DataLoader {
    DataLoader::with_filters(since, until, year)
}

fn background_cache_scope(
    since: &Option<String>,
    until: &Option<String>,
    year: &Option<String>,
) -> CacheReportScope {
    CacheReportScope::new(since.clone(), until.clone(), year.clone())
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    refresh: u64,
    debug: bool,
    theme: Option<ThemePreference>,
    clients: Option<Vec<String>>,
    since: Option<String>,
    until: Option<String>,
    year: Option<String>,
    initial_tab: Option<Tab>,
    initial_timeline_granularity: Option<TimelineGranularity>,
) -> Result<()> {
    if debug {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();
    }

    let config = TuiConfig {
        theme,
        refresh,
        clients: clients.clone(),
        since: since.clone(),
        until: until.clone(),
        year: year.clone(),
        initial_tab,
        initial_timeline_granularity,
    };

    // Build the unified filter set used by the cache key, the App
    // constructor, and the background loader. We mirror the same
    // resolution rules App::new_with_cached_data uses so the cache
    // lookup and the in-app state always agree. Drift between them
    // makes every launch a stale-cache hit instead of a fresh one.
    let enabled_clients: HashSet<ClientFilter> = if let Some(ref cli_clients) = clients {
        cli_clients
            .iter()
            .filter_map(|s| ClientFilter::from_filter_str(&s.to_lowercase()))
            .collect()
    } else {
        ClientFilter::default_set()
    };

    // Single file read: load cache and check freshness in one pass.
    // The key MUST be `cache::TUI_DEFAULT_GROUP_BY` so TUI cache readers
    // and writers stay on the same grouping contract. Hard-coding a
    // different value here would silently invalidate otherwise fresh
    // cache entries.
    let initial_group_by = TUI_DEFAULT_GROUP_BY;
    let initial_report_scope = background_cache_scope(&since, &until, &year);
    let requested_provenance = PulseDataProvenance::from_scan_scope(
        &enabled_clients,
        &initial_group_by,
        &initial_report_scope,
    );
    let (cache_result, cache_observed_at) =
        load_cache_with_observed_at(&enabled_clients, &initial_group_by, &initial_report_scope);
    let (cached_data, needs_background_load, cached_data_provenance, cache_observed_at) =
        decide_initial_data(cache_result, cache_observed_at, requested_provenance);

    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal_best_effort();
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();

    let _ = execute!(stdout, SetTitle("Tokscale"));

    if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout, SetTitle(""));
        return Err(e.into());
    }

    let backend = CrosstermBackend::new(stdout);
    let terminal_result = Terminal::new(backend);
    let mut terminal = match terminal_result {
        Ok(t) => t,
        Err(e) => {
            restore_terminal_best_effort();
            return Err(e.into());
        }
    };

    let mut app = match App::new_with_cached_data_and_provenance(
        config,
        cached_data,
        cached_data_provenance,
        cache_observed_at,
    ) {
        Ok(a) => a,
        Err(e) => {
            restore_terminal(&mut terminal);
            return Err(e);
        }
    };

    let (bg_tx, bg_rx) = mpsc::channel::<BackgroundLoadResult>();

    if needs_background_load {
        app.set_background_loading(true);

        let tx = bg_tx.clone();
        // Project the filter set into the (clients, include_synthetic)
        // pair the loader still consumes. Keeping the projection here
        // (instead of inside DataLoader) avoids touching tokscale-core's
        // public API in this PR.
        let bg_clients: Vec<ClientId> = enabled_clients
            .iter()
            .filter_map(|f| f.to_client_id())
            .collect();
        let bg_include_synthetic = enabled_clients.contains(&ClientFilter::Synthetic);
        let bg_since = since.clone();
        let bg_until = until.clone();
        let bg_year = year.clone();
        let bg_enabled_clients = enabled_clients.clone();
        let bg_group_by = app.group_by.borrow().clone();
        let bg_report_scope = background_cache_scope(&since, &until, &year);

        thread::spawn(move || {
            let loader = background_data_loader(bg_since, bg_until, bg_year);
            let result = loader.load(&bg_clients, &bg_group_by, bg_include_synthetic);

            if let Ok(ref data) = result {
                save_cached_data(data, &bg_enabled_clients, &bg_group_by, &bg_report_scope);
            }

            let _ = tx.send(BackgroundLoadResult {
                result,
                provenance: requested_provenance,
            });
        });
    }

    #[cfg(unix)]
    let sigcont_flag = {
        let flag = Arc::new(AtomicBool::new(false));
        let _ = signal_hook::flag::register(signal_hook::consts::SIGCONT, Arc::clone(&flag));
        flag
    };

    let mut events = EventHandler::new(Duration::from_millis(100));

    let result = run_loop_with_background(
        &mut terminal,
        &mut app,
        &mut events,
        bg_tx,
        bg_rx,
        #[cfg(unix)]
        &sigcont_flag,
    );

    // Don't orphan a `codex login` child (it would keep holding the OAuth
    // port after the TUI exits).
    app.kill_codex_login_child();

    restore_terminal(&mut terminal);

    result
}

fn restore_terminal_best_effort() {
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetTitle("")
    );
    let _ = disable_raw_mode();
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetTitle("")
    );
    let _ = terminal.show_cursor();
}

fn run_loop_with_background(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    events: &mut EventHandler,
    bg_tx: mpsc::Sender<BackgroundLoadResult>,
    bg_rx: mpsc::Receiver<BackgroundLoadResult>,
    #[cfg(unix)] sigcont_flag: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        #[cfg(unix)]
        if sigcont_flag.swap(false, Ordering::Relaxed) {
            let _ = enable_raw_mode();
            let _ = execute!(
                terminal.backend_mut(),
                EnterAlternateScreen,
                EnableMouseCapture
            );
            let _ = terminal.clear();
        }

        terminal.draw(|f| ui::render(f, app))?;

        match bg_rx.try_recv() {
            Ok(message) => {
                apply_background_load_result(app, message);
            }
            Err(TryRecvError::Disconnected) => {
                if app.background_loading {
                    app.set_background_loading(false);
                    app.set_error(Some("Background thread disconnected".to_string()));
                    app.set_status("Error: Background thread disconnected");
                }
            }
            Err(TryRecvError::Empty) => {}
        }

        if app.needs_reload && !app.background_loading {
            app.needs_reload = false;
            app.set_background_loading(true);

            let tx = bg_tx.clone();
            // Boundary projection: see [`run`] above for the shape rationale.
            let clients = app.scan_clients();
            let include_synthetic = app.include_synthetic();
            let since = app.data_loader.since.clone();
            let until = app.data_loader.until.clone();
            let year = app.data_loader.year.clone();
            let enabled_clients = app.enabled_clients.borrow().clone();
            let group_by = app.group_by.borrow().clone();
            let report_scope = background_cache_scope(&since, &until, &year);
            let provenance =
                PulseDataProvenance::from_scan_scope(&enabled_clients, &group_by, &report_scope);

            thread::spawn(move || {
                let loader = background_data_loader(since, until, year);
                let result = loader.load(&clients, &group_by, include_synthetic);
                if let Ok(ref data) = result {
                    save_cached_data(data, &enabled_clients, &group_by, &report_scope);
                }
                let _ = tx.send(BackgroundLoadResult { result, provenance });
            });
        }

        match events.next()? {
            Event::Tick => {
                app.on_tick();
            }
            Event::Key(key) => {
                if app.handle_key_event(key) {
                    break;
                }
            }
            Event::Mouse(mouse) => {
                app.handle_mouse_event(mouse);
            }
            Event::Resize(w, h) => {
                app.handle_resize(w, h);
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_with_stale_cache_renders_immediately() {
        let (cached_data, needs_background_load, provenance, observed_at) = decide_initial_data(
            CacheResult::Stale(UsageData::default()),
            Some(chrono::Utc::now()),
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert!(cached_data.is_some());
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::VerifiedDefaultScopeStale);
        assert_eq!(observed_at, None);
        assert!(!provenance.can_seed_global_snapshot());
    }

    #[test]
    fn strict_subset_cache_cannot_claim_global_pulse_provenance() {
        let (cached_data, needs_background_load, provenance, observed_at) = decide_initial_data(
            CacheResult::StaleSubset(UsageData::default()),
            Some(chrono::Utc::now()),
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert!(cached_data.is_some());
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::Unverified);
        assert_eq!(observed_at, None);
    }

    #[test]
    fn background_load_does_not_mask_snapshot_persistence_failure() {
        let config = TuiConfig {
            theme: None,
            refresh: 0,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: None,
            initial_timeline_granularity: None,
        };
        let mut app = App::new_with_cached_data(config, None).unwrap();
        app.pulse.fail_snapshot_saves_for_test("disk full");

        apply_background_load_result(
            &mut app,
            BackgroundLoadResult {
                result: Ok(UsageData::default()),
                provenance: PulseDataProvenance::VerifiedDefaultScopeFresh,
            },
        );

        assert_eq!(
            app.status_message.as_deref(),
            Some("Pulse snapshot save failed: disk full")
        );
        assert!(app.pulse.snapshot.is_some());
    }

    #[test]
    fn miss_renders_empty_until_background_completes() {
        let (cached_data, needs_background_load, provenance, observed_at) = decide_initial_data(
            CacheResult::Miss,
            None,
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert!(cached_data.is_none());
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::Unverified);
        assert_eq!(observed_at, None);
    }

    #[test]
    fn background_cache_scope_uses_date_filters() {
        let scope = background_cache_scope(
            &Some("2026-05-01".to_string()),
            &Some("2026-05-07".to_string()),
            &Some("2026".to_string()),
        );

        assert_eq!(
            scope,
            crate::tui::cache::CacheReportScope::new(
                Some("2026-05-01".to_string()),
                Some("2026-05-07".to_string()),
                Some("2026".to_string()),
            )
        );
    }
}
