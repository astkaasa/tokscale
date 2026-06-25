use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::style::Color;

use crate::client_filter::ClientFilter;
use crate::tui::background_job::{BackgroundJob, BackgroundJobPoll};
use crate::tui::codex_login::{CodexLoginEvent, CodexLoginOutcome};
use crate::tui::data::{DataLoader, UsageData};
use crate::tui::drilldown_state::DrilldownView;
use crate::tui::navigation::{ChartGranularity, Tab};
use crate::tui::pulse_state::PulseState;
use crate::tui::settings::Settings;
use crate::tui::themes::Theme;
use crate::tui::ui::dialog::DialogStack;
use crate::tui::ui::widgets::get_provider_shade;

#[cfg(test)]
use super::test_usage_fetcher;
use super::{App, RefreshTrigger, TuiConfig};

impl App {
    pub fn new_with_cached_data(config: TuiConfig, cached_data: Option<UsageData>) -> Result<Self> {
        Self::build_with_cached_data(config, cached_data, true)
    }

    pub(crate) fn new_surface_with_cached_data(
        config: TuiConfig,
        cached_data: Option<UsageData>,
    ) -> Result<Self> {
        Self::build_with_cached_data(config, cached_data, false)
    }

    fn build_with_cached_data(
        config: TuiConfig,
        cached_data: Option<UsageData>,
        fetch_on_entry: bool,
    ) -> Result<Self> {
        let settings = Settings::load();
        let theme_preference = config.theme.unwrap_or(settings.ui_theme);
        let theme = Theme::for_current_terminal_with_preference(theme_preference);

        let enabled_clients: HashSet<ClientFilter> = if let Some(ref cli_clients) = config.clients {
            // CLI-provided filter list. Each entry is the canonical
            // lowercase id (`opencode`, `claude`, ..., `synthetic`).
            // Unknown ids are dropped silently; the CLI parser already
            // validated against `ClientFilter` so this lookup should be
            // total in practice.
            cli_clients
                .iter()
                .filter_map(|s| ClientFilter::from_filter_str(&s.to_lowercase()))
                .collect()
        } else {
            // No filter → use the canonical default set (every real
            // client, Synthetic opt-in only). This must stay in sync
            // with the cache key used by no-filter TUI launches.
            ClientFilter::default_set()
        };

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

        let data = cached_data.unwrap_or_default();
        let has_data = !data.models.is_empty();
        let dialog_stack = DialogStack::new(theme.clone());
        let dialog_needs_reload = Rc::new(RefCell::new(false));
        let confirmed_codex_use_account_id = Rc::new(RefCell::new(None));
        let confirmed_codex_remove_account_id = Rc::new(RefCell::new(None));
        let confirmed_codex_reset_account_id = Rc::new(RefCell::new(None));
        let requested_tab = config.initial_tab.unwrap_or(Tab::Overview);
        let current_tab = requested_tab;
        let (sort_field, sort_direction) = Self::default_sort_for_tab(current_tab);
        let timeline_granularity = config.initial_timeline_granularity.unwrap_or_default();
        let pulse = PulseState::new(&settings);

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
            subscription_usage: {
                #[cfg(not(test))]
                {
                    crate::commands::usage::load_cache().unwrap_or_default()
                }
                #[cfg(test)]
                {
                    Vec::new()
                }
            },
            codex_login_lines: Vec::new(),
            codex_login_outcome: None,
            confirmed_codex_use_account_id,
            confirmed_codex_remove_account_id,
            confirmed_codex_reset_account_id,
            hide_usage_emails: true,
            usage_fetch_attempted: false,
            usage_job: BackgroundJob::default(),
            codex_reset_job: BackgroundJob::default(),
            pulse,
            #[cfg(test)]
            usage_fetcher: test_usage_fetcher,
            codex_login_rx: None,
            codex_login_child: None,
            data_version: 0,
        };
        app.build_model_shade_map();
        if fetch_on_entry {
            app.maybe_fetch_usage_on_entry();
            app.maybe_fetch_weread_on_entry();
        }
        Ok(app)
    }

    pub fn set_background_loading(&mut self, loading: bool) {
        self.background_loading = loading;
        // Don't set data.loading - let cached data remain visible during background refresh
    }

    pub fn update_data(&mut self, data: UsageData) {
        self.data = data;
        self.data_version = self.data_version.saturating_add(1);
        self.last_refresh = Instant::now();
        self.build_model_shade_map();

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

        if *self.dialog_needs_reload.borrow() {
            *self.dialog_needs_reload.borrow_mut() = false;
            self.needs_reload = true;
        }

        match self.usage_job.poll() {
            Some(BackgroundJobPoll::Ready(results)) => {
                self.subscription_usage = results;
                self.clamp_selection();
                if !self.subscription_usage.is_empty() {
                    crate::commands::usage::save_cache(&self.subscription_usage);
                    self.status_message = Some("Usage data loaded".into());
                } else {
                    crate::commands::usage::clear_cache();
                    self.status_message = Some("No usage data available".into());
                }
                self.status_message_time = Some(std::time::Instant::now());
            }
            Some(BackgroundJobPoll::Disconnected) => {
                self.status_message = Some("Usage fetch failed".into());
                self.status_message_time = Some(std::time::Instant::now());
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
