use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::ClientFilter;

use super::background_job::BackgroundJob;
use super::codex_login::{CodexLoginChildSlot, CodexLoginEvent, CodexLoginOutcome};
use super::data::{DataLoader, UsageData};
use super::drilldown_state::DrilldownState;
pub(crate) use super::drilldown_state::{
    DrilldownView, ModelDetailKey, ModelDetailPeriodRow, PeriodDetailKey, PeriodDetailModelRow,
    PeriodGranularity,
};
pub(crate) use super::interaction::ClickAction;
use super::interaction::ClickArea;
pub(crate) use super::navigation::{
    ChartGranularity, OverviewMode, SortDirection, SortField, Tab, TimelineGranularity,
};
use super::pulse_state::PulseState;
use super::settings::Settings;
use super::themes::{Theme, ThemePreference};
use super::ui::dialog::DialogStack;

/// Configuration for TUI initialization
pub struct TuiConfig {
    pub theme: Option<ThemePreference>,
    pub refresh: u64,
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub initial_tab: Option<Tab>,
    pub initial_timeline_granularity: Option<TimelineGranularity>,
}

#[cfg(test)]
type UsageFetcher = fn() -> crate::commands::usage::UsageFetchReport;

#[cfg(test)]
fn test_usage_fetcher() -> crate::commands::usage::UsageFetchReport {
    crate::commands::usage::UsageFetchReport::default()
}

pub struct App {
    pub should_quit: bool,
    pub current_tab: Tab,
    pub theme: Theme,
    theme_preference: ThemePreference,
    pub settings: Settings,
    pub data: UsageData,
    pub data_loader: DataLoader,

    /// Set of clients currently selected in the source picker. The
    /// `Synthetic` variant is part of the same set so dialog code can
    /// uniformly toggle/inspect every option without a separate boolean.
    /// Code that talks to `tokscale_core` (which still expects a
    /// `Vec<ClientId>` plus a `bool include_synthetic`) projects this set
    /// at the boundary via `App::scan_clients` and `App::include_synthetic`.
    pub enabled_clients: Rc<RefCell<HashSet<ClientFilter>>>,
    pub group_by: Rc<RefCell<tokscale_core::GroupBy>>,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    tab_sort_state: HashMap<Tab, (SortField, SortDirection)>,
    pub chart_granularity: ChartGranularity,
    /// `usize::MAX` means "pin to the newest visible point"; the chart renderer
    /// clamps it once the terminal width and data length are known.
    pub overview_chart_scroll_offset: usize,
    pub timeline_granularity: TimelineGranularity,
    pub overview_mode: OverviewMode,

    pub scroll_offset: usize,
    pub selected_index: usize,
    pub max_visible_items: usize,
    pub drilldown: Option<DrilldownState>,

    pub auto_refresh: bool,
    pub auto_refresh_interval: Duration,
    pub last_auto_refresh: Instant,
    pub last_refresh: Instant,

    pub status_message: Option<String>,
    pub status_message_time: Option<Instant>,

    pub terminal_width: u16,
    pub terminal_height: u16,

    pub click_areas: Vec<ClickArea>,

    pub spinner_frame: usize,

    pub background_loading: bool,

    pub needs_reload: bool,

    pub dialog_stack: DialogStack,

    pub dialog_needs_reload: Rc<RefCell<bool>>,

    pub model_shade_map: HashMap<String, Color>,

    pub subscription_usage: Vec<crate::commands::usage::UsageOutput>,

    pub codex_login_lines: Vec<String>,
    pub(crate) codex_login_outcome: Option<CodexLoginOutcome>,
    confirmed_codex_use_account_id: Rc<RefCell<Option<String>>>,
    confirmed_codex_remove_account_id: Rc<RefCell<Option<String>>>,
    confirmed_codex_reset_account_id: Rc<RefCell<Option<String>>>,
    pub hide_usage_emails: bool,

    pub usage_fetch_attempted: bool,
    pub usage_fetch_diagnostics: Vec<crate::commands::usage::UsageFetchDiagnostic>,
    usage_job: BackgroundJob<crate::commands::usage::UsageFetchReport>,
    codex_reset_job:
        BackgroundJob<Result<crate::commands::usage::codex::RateLimitResetConsumeResult, String>>,
    pub pulse: PulseState,
    #[cfg(test)]
    usage_fetcher: UsageFetcher,
    codex_login_rx: Option<std::sync::mpsc::Receiver<CodexLoginEvent>>,
    codex_login_child: Option<CodexLoginChildSlot>,

    data_version: u64,
}

mod actions;
pub(crate) use actions::RefreshTrigger;
mod drilldown;
mod input;
mod lifecycle;
mod selection;
mod usage;
mod views;

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
