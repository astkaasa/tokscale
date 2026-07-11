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
    load_cache, save_cached_data, CacheReportScope, CacheResult, TUI_DEFAULT_GROUP_BY,
};
pub(crate) use data::{DataLoader, UsageData, UsageObservation};
pub(crate) use event::{Event, EventHandler};
pub(crate) use themes::{Theme, ThemePreference};

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

use crate::client_filter::ResolvedClientSelection;

fn decide_initial_data(
    load_result: CacheResult,
    requested_provenance: PulseDataProvenance,
) -> (Option<UsageObservation>, bool, PulseDataProvenance) {
    let (cached_observation, provenance) = match load_result {
        CacheResult::Fresh(observation) => (Some(observation), requested_provenance),
        CacheResult::Stale(observation) => (Some(observation), requested_provenance.as_stale()),
        CacheResult::StaleSubset(observation) => {
            (Some(observation), PulseDataProvenance::Unverified)
        }
        CacheResult::Miss => (None, PulseDataProvenance::Unverified),
    };

    (cached_observation, true, provenance)
}

struct BackgroundLoadResult {
    result: Result<UsageObservation>,
    provenance: PulseDataProvenance,
}

fn apply_background_load_result(app: &mut App, message: BackgroundLoadResult) {
    app.set_background_loading(false);
    match message.result {
        Ok(observation) => match app.update_data(observation, message.provenance) {
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

    // Resolve once so the cache key, provenance, and initial background
    // scan all use the same filter set and stable scanner ordering.
    let selection = ResolvedClientSelection::from_configured(clients.as_deref());

    // Single file read: load cache and check freshness in one pass.
    // The key MUST be `cache::TUI_DEFAULT_GROUP_BY` so TUI cache readers
    // and writers stay on the same grouping contract. Hard-coding a
    // different value here would silently invalidate otherwise fresh
    // cache entries.
    let initial_group_by = TUI_DEFAULT_GROUP_BY;
    let initial_report_scope = background_cache_scope(&since, &until, &year);
    let requested_provenance = PulseDataProvenance::from_scan_scope(
        &selection.filters,
        &initial_group_by,
        &initial_report_scope,
    );
    let cache_result = load_cache(&selection.filters, &initial_group_by, &initial_report_scope);
    let (cached_observation, needs_background_load, cached_data_provenance) =
        decide_initial_data(cache_result, requested_provenance);

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
        cached_observation,
        cached_data_provenance,
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
        let bg_clients = selection.scan_clients.clone();
        let bg_include_synthetic = selection.include_synthetic;
        let bg_since = since.clone();
        let bg_until = until.clone();
        let bg_year = year.clone();
        let bg_enabled_clients = selection.filters.clone();
        let bg_group_by = app.group_by.borrow().clone();
        let bg_report_scope = background_cache_scope(&since, &until, &year);

        thread::spawn(move || {
            let loader = background_data_loader(bg_since, bg_until, bg_year);
            let result = loader.load(&bg_clients, &bg_group_by, bg_include_synthetic);

            if let Ok(ref observation) = result {
                save_cached_data(
                    observation,
                    &bg_enabled_clients,
                    &bg_group_by,
                    &bg_report_scope,
                );
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
                if let Ok(ref observation) = result {
                    save_cached_data(observation, &enabled_clients, &group_by, &report_scope);
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

    fn observation_at(observed_at: chrono::DateTime<chrono::Utc>) -> UsageObservation {
        UsageObservation {
            data: UsageData::default(),
            observed_at,
        }
    }

    #[test]
    fn launches_with_stale_cache_renders_immediately() {
        let observed_at = chrono::Utc::now();
        let (cached_observation, needs_background_load, provenance) = decide_initial_data(
            CacheResult::Stale(observation_at(observed_at)),
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert_eq!(
            cached_observation
                .as_ref()
                .map(|observation| observation.observed_at),
            Some(observed_at)
        );
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::VerifiedDefaultScopeStale);
        assert!(!provenance.can_seed_global_snapshot());
    }

    #[test]
    fn strict_subset_cache_cannot_claim_global_pulse_provenance() {
        let observed_at = chrono::Utc::now();
        let (cached_observation, needs_background_load, provenance) = decide_initial_data(
            CacheResult::StaleSubset(observation_at(observed_at)),
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert_eq!(
            cached_observation
                .as_ref()
                .map(|observation| observation.observed_at),
            Some(observed_at)
        );
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::Unverified);
        assert!(!provenance.can_seed_global_snapshot());
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
                result: Ok(observation_at(chrono::Utc::now())),
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
        let (cached_observation, needs_background_load, provenance) = decide_initial_data(
            CacheResult::Miss,
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert!(cached_observation.is_none());
        assert!(needs_background_load);
        assert_eq!(provenance, PulseDataProvenance::Unverified);
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
