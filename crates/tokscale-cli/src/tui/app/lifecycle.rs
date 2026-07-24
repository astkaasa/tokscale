use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use ratatui::style::Color;

use crate::client_filter::ResolvedClientSelection;
use crate::commands::usage::{
    LoadedSubscriptionCache, UsageCacheIdentity, UsageFetchDiagnostic, UsageFetchDiagnosticKind,
    UsageFetchDiagnosticSeverity, UsageOutput,
};
use crate::tui::background_job::{BackgroundJob, BackgroundJobPoll};
use crate::tui::codex_login::{CodexLoginEvent, CodexLoginOutcome};
use crate::tui::data::{DataLoader, UsageData, UsageObservation};
use crate::tui::drilldown_state::DrilldownView;
use crate::tui::navigation::{ChartGranularity, Tab};
use crate::tui::pulse_state::{AiSourceObservedAt, PulseState};
use crate::tui::settings::Settings;
use crate::tui::themes::Theme;
use crate::tui::ui::dialog::DialogStack;
use crate::tui::ui::widgets::get_provider_shade;

#[cfg(test)]
use super::test_usage_fetcher;
use super::{App, PulseDataProvenance, RefreshTrigger, TuiConfig};

fn merge_usage_refresh(
    previous: &[UsageOutput],
    mut fresh: Vec<UsageOutput>,
    diagnostics: &[UsageFetchDiagnostic],
) -> (Vec<UsageOutput>, Vec<UsageCacheIdentity>) {
    let mut retained = Vec::new();
    for output in previous {
        let already_replaced = fresh
            .iter()
            .any(|candidate| same_usage_identity(candidate, output));
        let failed = diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == UsageFetchDiagnosticSeverity::Error
                && diagnostic_matches_output(diagnostic, output)
        });
        if failed && !already_replaced {
            retained.push(UsageCacheIdentity::from_output(output));
            fresh.push(output.clone());
        }
    }
    (fresh, retained)
}

fn same_usage_identity(left: &UsageOutput, right: &UsageOutput) -> bool {
    left.provider.eq_ignore_ascii_case(&right.provider)
        && match (&left.account, &right.account) {
            (Some(left), Some(right)) => left.id == right.id,
            (None, None) => true,
            _ => false,
        }
}

fn diagnostic_matches_output(diagnostic: &UsageFetchDiagnostic, output: &UsageOutput) -> bool {
    diagnostic.provider.eq_ignore_ascii_case(&output.provider)
        && diagnostic.account.as_ref().is_none_or(|failed_account| {
            output
                .account
                .as_ref()
                .is_some_and(|account| account.id == failed_account.id)
        })
}

fn persist_subscription_cache(
    data: &[UsageOutput],
    partial: bool,
    stale_identities: &[UsageCacheIdentity],
) -> Option<chrono::DateTime<Utc>> {
    #[cfg(not(test))]
    {
        crate::commands::usage::save_cache_with_provenance(data, partial, stale_identities)
    }

    #[cfg(test)]
    {
        let _ = (data, partial, stale_identities);
        Some(Utc::now())
    }
}

fn clear_subscription_cache() {
    #[cfg(not(test))]
    crate::commands::usage::clear_cache();
}

fn quota_refresh_has_errors(diagnostics: &[UsageFetchDiagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == UsageFetchDiagnosticSeverity::Error)
}

fn quota_provenance_is_degraded(diagnostics: &[UsageFetchDiagnostic]) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == UsageFetchDiagnosticSeverity::Error
            || matches!(
                diagnostic.kind,
                UsageFetchDiagnosticKind::CachedDataStale
                    | UsageFetchDiagnosticKind::CacheWriteFailed
            )
    })
}

fn cache_diagnostics(cache: &LoadedSubscriptionCache) -> Vec<UsageFetchDiagnostic> {
    if cache.is_fresh && !cache.partial && cache.stale_identities.is_empty() {
        return Vec::new();
    }

    let message = match (
        cache.is_fresh,
        cache.partial || !cache.stale_identities.is_empty(),
    ) {
        (false, true) => "Cached usage is stale and has partial provider coverage",
        (false, false) => "Cached usage is stale",
        (true, true) => "Cached usage has partial provider coverage",
        (true, false) => return Vec::new(),
    };
    vec![UsageFetchDiagnostic::with_kind(
        "Local quota cache",
        None,
        UsageFetchDiagnosticKind::CachedDataStale,
        UsageFetchDiagnosticSeverity::Warning,
        message,
    )]
}

fn should_publish_quota_pulse(fresh_count: usize, partial: bool, cache_persisted: bool) -> bool {
    fresh_count > 0 && !partial && cache_persisted
}

impl App {
    #[cfg(test)]
    pub fn new_with_cached_data(config: TuiConfig, cached_data: Option<UsageData>) -> Result<Self> {
        let cached_observation = cached_data.map(|data| UsageObservation {
            data,
            observed_at: Utc::now(),
        });
        Self::build_with_cached_data(
            config,
            cached_observation,
            PulseDataProvenance::Unverified,
            true,
            None,
        )
    }

    pub(crate) fn new_with_cached_data_and_provenance(
        config: TuiConfig,
        cached_observation: Option<UsageObservation>,
        provenance: PulseDataProvenance,
    ) -> Result<Self> {
        Self::build_with_cached_data(config, cached_observation, provenance, true, None)
    }

    pub(crate) fn new_surface_with_cached_data(
        config: TuiConfig,
        cached_data: Option<UsageData>,
        settings: Settings,
    ) -> Result<Self> {
        let cached_observation = cached_data.map(|data| UsageObservation {
            data,
            observed_at: Utc::now(),
        });
        Self::build_with_cached_data(
            config,
            cached_observation,
            PulseDataProvenance::Unverified,
            false,
            Some(settings),
        )
    }

    fn build_with_cached_data(
        config: TuiConfig,
        cached_observation: Option<UsageObservation>,
        pulse_data_provenance: PulseDataProvenance,
        fetch_on_entry: bool,
        settings_override: Option<Settings>,
    ) -> Result<Self> {
        let settings = settings_override.unwrap_or_else(Settings::load);
        let theme_preference = config.theme.unwrap_or(settings.ui_theme);
        let theme = Theme::for_current_terminal_with_preference(theme_preference);

        let enabled_clients =
            ResolvedClientSelection::from_configured(config.clients.as_deref()).filters;

        let auto_refresh_interval = if config.refresh > 0 {
            Duration::from_secs(config.refresh)
        } else if let Some(interval) = settings.get_auto_refresh_interval() {
            interval
        } else {
            Duration::from_secs(30)
        };

        let auto_refresh = config.refresh > 0 || settings.auto_refresh_enabled;

        let overview_mode = Self::initial_overview_mode(&config.since, &config.until, &config.year);

        let data_loader = DataLoader::with_filters(config.since, config.until, config.year);

        let (data, cached_ai_observed_at) = match cached_observation {
            Some(observation) => (observation.data, Some(observation.observed_at)),
            None => (UsageData::default(), None),
        };
        let has_data = !data.models.is_empty()
            || !data.daily.is_empty()
            || !data.agents.is_empty()
            || data.total_tokens > 0
            || data.total_cost > 0.0;
        let dialog_stack = DialogStack::new(theme.clone());
        let dialog_needs_reload = Rc::new(RefCell::new(false));
        let confirmed_codex_use_account_id = Rc::new(RefCell::new(None));
        let confirmed_codex_remove_account_id = Rc::new(RefCell::new(None));
        let confirmed_codex_reset_account_id = Rc::new(RefCell::new(None));
        let requested_tab = config.initial_tab.unwrap_or(Tab::Overview);
        let current_tab = requested_tab;
        let (sort_field, sort_direction) = Self::default_sort_for_tab(current_tab);
        let timeline_granularity = config.initial_timeline_granularity.unwrap_or_default();
        let pulse = if fetch_on_entry {
            PulseState::new(&settings)
        } else {
            PulseState::empty_for_surface()
        };
        let (subscription_usage, subscription_observed_at, usage_cache_diagnostics) = {
            #[cfg(not(test))]
            {
                if fetch_on_entry {
                    match crate::commands::usage::load_cache_for_tui() {
                        Some(cache) => {
                            let diagnostics = cache_diagnostics(&cache);
                            (cache.data, Some(cache.observed_at), diagnostics)
                        }
                        None => (Vec::new(), None, Vec::new()),
                    }
                } else {
                    (Vec::new(), None, Vec::new())
                }
            }
            #[cfg(test)]
            {
                (Vec::new(), None, Vec::new())
            }
        };
        let pulse_ai_observed_at = AiSourceObservedAt {
            local: pulse_data_provenance
                .can_seed_global_snapshot()
                .then_some(cached_ai_observed_at)
                .flatten()
                .filter(|_| has_data),
            quota: (!subscription_usage.is_empty())
                .then_some(subscription_observed_at)
                .flatten(),
        };

        let mut app = Self {
            should_quit: false,
            current_tab,
            theme,
            theme_preference,
            settings,
            data,
            data_loader,
            enabled_clients: Rc::new(RefCell::new(enabled_clients)),
            group_by: Rc::new(RefCell::new(crate::tui::cache::TUI_DEFAULT_GROUP_BY)),
            sort_field,
            sort_direction,
            tab_sort_state: HashMap::new(),
            chart_granularity: ChartGranularity::default(),
            overview_chart_scroll_offset: usize::MAX,
            timeline_granularity,
            overview_mode,
            scroll_offset: 0,
            selected_index: 0,
            max_visible_items: 20,
            drilldown: None,
            auto_refresh,
            auto_refresh_interval,
            last_auto_refresh: Instant::now(),
            last_refresh: Instant::now(),
            status_message: if has_data {
                Some("Loaded from cache".to_string())
            } else {
                None
            },
            status_message_time: if has_data { Some(Instant::now()) } else { None },
            terminal_width: 80,
            terminal_height: 24,
            click_areas: Vec::new(),
            spinner_frame: 0,
            background_loading: false,
            needs_reload: false,
            dialog_stack,
            dialog_needs_reload,
            model_shade_map: HashMap::new(),
            subscription_usage,
            account_activities: HashMap::new(),
            account_activity_error: None,
            expanded_usage_account_id: None,
            codex_login_lines: Vec::new(),
            codex_login_outcome: None,
            confirmed_codex_use_account_id,
            confirmed_codex_remove_account_id,
            confirmed_codex_reset_account_id,
            hide_usage_emails: true,
            usage_fetch_attempted: false,
            usage_fetch_diagnostics: usage_cache_diagnostics,
            usage_job: BackgroundJob::default(),
            usage_refresh_is_background: false,
            last_quota_sample: Instant::now(),
            last_codex_activity_fetch: None,
            codex_reset_job: BackgroundJob::default(),
            pulse,
            pulse_data_provenance,
            pulse_ai_observed_at,
            render_reference_now: None,
            #[cfg(test)]
            usage_fetcher: test_usage_fetcher,
            codex_login_rx: None,
            codex_login_child: None,
            data_version: 0,
        };
        app.build_model_shade_map();
        if fetch_on_entry {
            #[cfg(not(test))]
            app.reload_account_activities();
            app.maybe_fetch_usage_on_entry();
            #[cfg(not(test))]
            app.maybe_start_background_quota_sampling();
            app.maybe_fetch_weread_on_entry();
        }
        Ok(app)
    }

    pub fn set_background_loading(&mut self, loading: bool) {
        self.background_loading = loading;
        // Don't set data.loading - let cached data remain visible during background refresh
    }

    pub fn update_data(
        &mut self,
        observation: UsageObservation,
        provenance: PulseDataProvenance,
    ) -> Result<()> {
        let UsageObservation { data, observed_at } = observation;
        self.pulse_data_provenance = provenance;
        self.pulse_ai_observed_at.local =
            provenance.can_seed_global_snapshot().then_some(observed_at);
        let quota_degraded = !self.subscription_usage.is_empty()
            && quota_provenance_is_degraded(&self.usage_fetch_diagnostics);
        self.data = data;
        self.data_version = self.data_version.saturating_add(1);
        self.last_refresh = Instant::now();
        self.build_model_shade_map();
        let pulse_result = if quota_degraded {
            self.rebuild_pulse_snapshot_preserving_quota()
        } else {
            self.rebuild_pulse_snapshot()
        };
        if quota_degraded {
            self.mark_quota_source_degraded();
        }

        if let Some(DrilldownView::Period(key)) = self.drilldown_view().cloned() {
            if !self
                .data
                .daily
                .iter()
                .any(|day| day.date >= key.start && day.date <= key.end)
            {
                self.close_drilldown();
            }
        }

        self.clamp_selection();
        pulse_result
    }

    fn mark_quota_source_degraded(&mut self) {
        let issue_code = if self
            .usage_fetch_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == UsageFetchDiagnosticKind::CacheWriteFailed)
        {
            "cache_write_failed"
        } else if self
            .usage_fetch_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == UsageFetchDiagnosticSeverity::Error)
        {
            "partial_refresh"
        } else {
            "stale_cache"
        };
        let Some(snapshot) = self.pulse.snapshot.as_mut() else {
            return;
        };
        snapshot.mark_source_degraded("subscription-usage-cache", issue_code);
    }

    pub fn build_model_shade_map(&mut self) {
        self.model_shade_map = crate::tui::colors::build_model_shade_map(&self.data.models);
    }

    pub fn model_color_for(&self, provider: &str, model: &str) -> Color {
        let provider = crate::tui::colors::provider_color_key(provider, model);
        let lookup_key = crate::tui::colors::model_shade_key(&provider, model);
        let color = self
            .model_shade_map
            .get(&lookup_key)
            .copied()
            .unwrap_or_else(|| get_provider_shade(&provider, 0));
        self.theme.color(color)
    }

    pub fn has_visible_data(&self) -> bool {
        !self.data.models.is_empty()
            || !self.data.daily.is_empty()
            || !self.data.agents.is_empty()
            || self.data.total_tokens > 0
            || self.data.total_cost > 0.0
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.data.error = error;
    }

    pub fn on_tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 20;

        if let Some(status_time) = self.status_message_time {
            if status_time.elapsed() > Duration::from_secs(3) {
                self.status_message = None;
                self.status_message_time = None;
            }
        }

        if self.auto_refresh && self.last_auto_refresh.elapsed() >= self.auto_refresh_interval {
            self.last_auto_refresh = Instant::now();
            self.refresh_current_surface(RefreshTrigger::Auto);
        }

        self.maybe_sample_quota_in_background();

        if *self.dialog_needs_reload.borrow() {
            *self.dialog_needs_reload.borrow_mut() = false;
            self.needs_reload = true;
        }

        match self.usage_job.poll() {
            Some(BackgroundJobPoll::Ready(report)) => {
                let fresh_count = report.outputs.len();
                let partial = quota_refresh_has_errors(&report.diagnostics);
                let (usage, retained_identities) = merge_usage_refresh(
                    &self.subscription_usage,
                    report.outputs,
                    &report.diagnostics,
                );
                let retained_count = retained_identities.len();
                self.subscription_usage = usage;
                self.usage_fetch_diagnostics = report.diagnostics;
                let mut cache_persisted = false;
                if fresh_count > 0 {
                    match persist_subscription_cache(
                        &self.subscription_usage,
                        partial,
                        &retained_identities,
                    ) {
                        Some(observed_at) => {
                            cache_persisted = true;
                            if !partial {
                                self.pulse_ai_observed_at.quota = Some(observed_at);
                            }
                        }
                        None => {
                            self.usage_fetch_diagnostics
                                .push(UsageFetchDiagnostic::with_kind(
                                    "Local quota cache",
                                    None,
                                    UsageFetchDiagnosticKind::CacheWriteFailed,
                                    UsageFetchDiagnosticSeverity::Warning,
                                    "Refreshed usage could not be persisted",
                                ));
                        }
                    }
                } else if self.usage_fetch_diagnostics.is_empty() {
                    clear_subscription_cache();
                    self.pulse_ai_observed_at.quota = None;
                }

                let publish_pulse =
                    should_publish_quota_pulse(fresh_count, partial, cache_persisted)
                        || (fresh_count == 0 && self.usage_fetch_diagnostics.is_empty());
                let quota_degraded = partial || (!cache_persisted && fresh_count > 0);
                let pulse_result = if publish_pulse {
                    self.rebuild_pulse_snapshot()
                } else if quota_degraded {
                    self.rebuild_pulse_snapshot_preserving_quota()
                } else {
                    Ok(())
                };
                if quota_degraded {
                    self.mark_quota_source_degraded();
                }
                self.reload_account_activities();
                self.clamp_selection();
                let usage_status = if fresh_count > 0 {
                    if self.usage_fetch_diagnostics.is_empty() {
                        Some("Usage data loaded".into())
                    } else {
                        Some(format!(
                            "Usage data loaded with {} issue{}",
                            self.usage_fetch_diagnostics.len(),
                            if self.usage_fetch_diagnostics.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ))
                    }
                } else if retained_count > 0 {
                    Some(format!(
                        "Usage refresh failed; kept cached data ({} issue{})",
                        self.usage_fetch_diagnostics.len(),
                        if self.usage_fetch_diagnostics.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ))
                } else {
                    if let Some(diagnostic) = self.usage_fetch_diagnostics.first() {
                        Some(format!("Usage fetch failed: {}", diagnostic.display_name()))
                    } else {
                        Some("No usage data available".into())
                    }
                };
                if !self.usage_refresh_is_background || pulse_result.is_err() {
                    self.status_message = match pulse_result {
                        Ok(()) => usage_status,
                        Err(error) => Some(format!("Pulse snapshot save failed: {error}")),
                    };
                    self.status_message_time = Some(std::time::Instant::now());
                }
                self.usage_refresh_is_background = false;
            }
            Some(BackgroundJobPoll::Disconnected) => {
                self.usage_fetch_diagnostics = vec![UsageFetchDiagnostic::new(
                    "Quota providers",
                    None,
                    "Usage refresh worker stopped",
                )];
                self.mark_quota_source_degraded();
                if !self.usage_refresh_is_background {
                    self.status_message = Some("Usage fetch failed".into());
                    self.status_message_time = Some(std::time::Instant::now());
                }
                self.usage_refresh_is_background = false;
            }
            None => {}
        }

        match self.codex_reset_job.poll() {
            Some(BackgroundJobPoll::Ready(Ok(result))) => {
                self.status_message = Some(format!(
                    "Codex reset credit: {}",
                    crate::tui::app::usage::codex_reset_outcome_label(&result)
                ));
                self.status_message_time = Some(std::time::Instant::now());
                self.fetch_subscription_usage();
            }
            Some(BackgroundJobPoll::Ready(Err(error))) => {
                self.status_message = Some(format!("Codex reset failed: {error}"));
                self.status_message_time = Some(std::time::Instant::now());
            }
            Some(BackgroundJobPoll::Disconnected) => {
                self.status_message = Some("Codex reset failed".into());
                self.status_message_time = Some(std::time::Instant::now());
            }
            None => {}
        }

        self.poll_weread_fetch();
        self.poll_codex_login();
    }

    pub(crate) fn poll_codex_login(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(rx) = &self.codex_login_rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let mut finished = false;
        for event in events {
            match event {
                CodexLoginEvent::Output(line) => {
                    self.codex_login_lines.push(line);
                    const MAX_LOGIN_LINES: usize = 12;
                    if self.codex_login_lines.len() > MAX_LOGIN_LINES {
                        let drain_count = self.codex_login_lines.len() - MAX_LOGIN_LINES;
                        self.codex_login_lines.drain(0..drain_count);
                    }
                }
                CodexLoginEvent::Finished(outcome) => {
                    finished = true;
                    match &outcome {
                        CodexLoginOutcome::Imported(info) => {
                            let display = info.label.as_deref().unwrap_or(&info.id);
                            self.set_status(&format!("Imported Codex account: {display}"));
                        }
                        CodexLoginOutcome::Failed(error) => {
                            self.set_status(&format!("Codex login failed: {error}"));
                        }
                    }
                    self.codex_login_outcome = Some(outcome);
                }
            }
        }

        if disconnected && !finished && self.codex_login_outcome.is_none() {
            self.codex_login_outcome = Some(CodexLoginOutcome::Failed(
                "login worker stopped".to_string(),
            ));
            self.set_status("Codex login failed: login worker stopped");
            finished = true;
        }

        if finished {
            self.codex_login_rx = None;
            self.codex_login_child = None;
            if matches!(
                self.codex_login_outcome,
                Some(CodexLoginOutcome::Imported(_))
            ) {
                self.codex_login_lines.clear();
                self.codex_login_outcome = None;
                self.refresh_usage();
            }
        }
    }
}

#[cfg(test)]
mod quota_resilience_tests {
    use super::*;
    use crate::commands::usage::UsageAccount;
    use crate::tui::data::{DailyUsage, TokenBreakdown};
    use std::collections::BTreeMap;

    fn app() -> App {
        App::new_with_cached_data(
            TuiConfig {
                theme: None,
                refresh: 0,
                clients: None,
                since: None,
                until: None,
                year: None,
                initial_tab: None,
                initial_timeline_granularity: None,
            },
            None,
        )
        .unwrap()
    }

    fn output(account_id: &str) -> UsageOutput {
        UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: account_id.to_string(),
                label: None,
                is_active: false,
            }),
            plan: None,
            email: None,
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        }
    }

    #[test]
    fn partial_refresh_retains_only_failed_account_with_identity() {
        let fresh = output("fresh");
        let retained = output("retained");
        let diagnostic =
            UsageFetchDiagnostic::new("Codex", retained.account.clone(), "provider unavailable");

        let (merged, stale_identities) = merge_usage_refresh(
            &[fresh.clone(), retained.clone()],
            vec![fresh],
            &[diagnostic],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(
            stale_identities,
            vec![UsageCacheIdentity::from_output(&retained)]
        );
    }

    #[test]
    fn stale_cache_load_creates_degraded_non_error_diagnostic() {
        let cache = LoadedSubscriptionCache {
            data: vec![output("cached")],
            observed_at: Utc::now() - chrono::Duration::minutes(10),
            is_fresh: false,
            partial: true,
            stale_identities: vec![UsageCacheIdentity::from_output(&output("cached"))],
        };

        let diagnostics = cache_diagnostics(&cache);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            UsageFetchDiagnosticKind::CachedDataStale
        );
        assert_eq!(
            diagnostics[0].severity,
            UsageFetchDiagnosticSeverity::Warning
        );
        assert!(quota_provenance_is_degraded(&diagnostics));
    }

    #[test]
    fn pulse_publication_requires_complete_persisted_quota_refresh() {
        assert!(should_publish_quota_pulse(1, false, true));
        assert!(!should_publish_quota_pulse(1, false, false));
        assert!(!should_publish_quota_pulse(1, true, true));
        assert!(!should_publish_quota_pulse(0, false, true));
    }

    #[test]
    fn cache_write_failure_still_publishes_local_pulse_without_untrusted_quota() {
        let mut app = app();
        app.subscription_usage = vec![output("cached")];
        app.usage_fetch_diagnostics = vec![UsageFetchDiagnostic::with_kind(
            "Local quota cache",
            None,
            UsageFetchDiagnosticKind::CacheWriteFailed,
            UsageFetchDiagnosticSeverity::Warning,
            "disk full",
        )];
        let mut data = UsageData::default();
        data.daily.push(DailyUsage {
            date: chrono::Local::now().date_naive(),
            tokens: TokenBreakdown {
                input: 42,
                ..TokenBreakdown::default()
            },
            cost: 1.25,
            source_breakdown: BTreeMap::new(),
            message_count: 1,
            turn_count: 1,
        });

        let observed_at = Utc::now() - chrono::Duration::seconds(5);
        let result = app.update_data(
            UsageObservation { data, observed_at },
            PulseDataProvenance::VerifiedDefaultScopeFresh,
        );

        assert!(result.is_ok());
        let snapshot = app.pulse.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.ai.total_tokens, Some(42));
        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|source| source.id == "local-ai-usage")
                .and_then(|source| source.observed_at),
            Some(observed_at)
        );
        let quota = snapshot
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();
        assert_eq!(quota.freshness, tokscale_core::pulse::PulseFreshness::Stale);
        assert_eq!(quota.issue_code.as_deref(), Some("cache_write_failed"));
        assert!(!snapshot
            .evidence
            .iter()
            .any(|evidence| evidence.source_id == "subscription-usage-cache"));
    }
}
