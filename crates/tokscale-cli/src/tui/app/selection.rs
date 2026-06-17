use ratatui::layout::Rect;
use tokscale_core::ClientId;

use crate::client_filter::ClientFilter;
use crate::tui::interaction::{ClickAction, ClickArea};
use crate::tui::navigation::{
    ChartGranularity, OverviewMode, SortDirection, SortField, Tab, TimelineGranularity,
};
use crate::tui::settings::Settings;
use crate::tui::themes::Theme;
use crate::tui::ui::dialog::ClientPickerDialog;

use super::App;

impl App {
    pub fn handle_resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
    }

    pub(crate) fn set_max_visible_items(&mut self, max_visible_items: usize) {
        self.max_visible_items = max_visible_items.max(1);
        self.clamp_selection();
    }

    /// Clamp selection and scroll offset to valid bounds after data/resize changes.
    pub(crate) fn clamp_selection(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            self.selected_index = 0;
            self.scroll_offset = 0;
            return;
        }
        self.selected_index = self.selected_index.min(len.saturating_sub(1));
        let max_scroll = len.saturating_sub(self.max_visible_items);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    pub fn clear_click_areas(&mut self) {
        self.click_areas.clear();
    }

    pub fn add_click_area(&mut self, rect: Rect, action: ClickAction) {
        self.click_areas.push(ClickArea { rect, action });
    }

    pub(crate) fn reset_selection(&mut self) {
        self.scroll_offset = 0;
        self.selected_index = 0;
        self.selected_daily_detail_date = None;
        self.daily_list_selected_index = 0;
        self.daily_list_scroll_offset = 0;
        self.drilldown = None;
    }

    pub(crate) fn switch_tab(&mut self, target: Tab) {
        self.persist_current_sort();

        self.current_tab = target;
        self.drilldown = None;
        if target != Tab::Daily {
            self.selected_daily_detail_date = None;
        }

        let (field, dir) = self
            .tab_sort_state
            .get(&target)
            .copied()
            .unwrap_or_else(|| Self::default_sort_for_tab(target));
        self.sort_field = field;
        self.sort_direction = dir;

        self.maybe_fetch_usage_on_entry();
        self.maybe_fetch_weread_on_entry();
    }

    pub(crate) fn default_sort_for_tab(tab: Tab) -> (SortField, SortDirection) {
        if matches!(tab, Tab::Pulse | Tab::Daily | Tab::Hourly | Tab::Minutely) {
            (SortField::Date, SortDirection::Descending)
        } else {
            (SortField::Cost, SortDirection::Descending)
        }
    }

    pub(crate) fn tab_visible(settings: &Settings, tab: Tab) -> bool {
        match tab {
            Tab::Minutely => settings.minutely_tab_enabled,
            _ => true,
        }
    }

    pub(crate) fn is_tab_visible(&self, tab: Tab) -> bool {
        Self::tab_visible(&self.settings, tab)
    }

    pub(crate) fn visible_workspaces(&self) -> Vec<Tab> {
        Tab::workspaces()
            .iter()
            .copied()
            .filter(|t| self.is_tab_visible(*t))
            .collect()
    }

    pub(crate) fn next_visible_tab(&self) -> Tab {
        let tabs = self.visible_workspaces();
        if tabs.is_empty() {
            return self.current_tab;
        }
        let next_index = tabs
            .iter()
            .position(|tab| *tab == self.current_tab)
            .map(|index| (index + 1) % tabs.len())
            .unwrap_or(0);
        tabs[next_index]
    }

    pub(crate) fn prev_visible_tab(&self) -> Tab {
        let tabs = self.visible_workspaces();
        if tabs.is_empty() {
            return self.current_tab;
        }
        let prev_index = tabs
            .iter()
            .position(|tab| *tab == self.current_tab)
            .map(|index| index.checked_sub(1).unwrap_or(tabs.len() - 1))
            .unwrap_or(tabs.len() - 1);
        tabs[prev_index]
    }

    pub(crate) fn persist_current_sort(&mut self) {
        self.tab_sort_state
            .insert(self.current_tab, (self.sort_field, self.sort_direction));
    }

    pub(crate) fn move_selection_up(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = len - 1;
            self.scroll_offset = len.saturating_sub(self.max_visible_items);
        } else {
            self.selected_index -= 1;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            }
        }
    }

    pub(crate) fn move_selection_down(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        let max_index = len - 1;
        if self.selected_index >= max_index {
            self.selected_index = 0;
            self.scroll_offset = 0;
        } else {
            self.selected_index += 1;
            if self.selected_index >= self.scroll_offset + self.max_visible_items {
                self.scroll_offset = self.selected_index - self.max_visible_items + 1;
            }
        }
    }

    pub(crate) fn move_page_up(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        let jump = (self.max_visible_items / 2).max(1);
        self.selected_index = self.selected_index.saturating_sub(jump);
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        }
    }

    pub(crate) fn move_page_down(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        let jump = (self.max_visible_items / 2).max(1);
        let max_index = len - 1;
        self.selected_index = (self.selected_index + jump).min(max_index);
        if self.selected_index >= self.scroll_offset + self.max_visible_items {
            self.scroll_offset = self.selected_index - self.max_visible_items + 1;
        }
    }

    pub(crate) fn move_to_top(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn move_to_bottom(&mut self) {
        let len = self.get_current_list_len();
        if len == 0 {
            return;
        }
        self.selected_index = len - 1;
        self.scroll_offset = len.saturating_sub(self.max_visible_items);
    }

    pub(crate) fn get_current_list_len(&self) -> usize {
        if self.is_drilldown_active() {
            return self.drilldown_list_len();
        }

        match self.current_tab {
            Tab::Overview => self.overview_model_len(),
            Tab::Pulse => 0,
            Tab::Models => self.data.models.len(),
            Tab::Daily if self.is_daily_detail_active() => {
                self.get_sorted_daily_detail_rows().len()
            }
            Tab::Daily => match self.timeline_granularity {
                TimelineGranularity::Day => self.data.daily.len(),
                TimelineGranularity::Hour => self.data.hourly.len(),
            },
            Tab::Hourly => self.data.hourly.len(),
            Tab::Minutely => self.data.minutely.len(),
            Tab::Usage => self.subscription_usage.len(),
        }
    }

    pub(crate) fn set_sort(&mut self, field: SortField) {
        let (current_field, current_direction) = self.active_sort_state();
        let next_direction = if current_field == field {
            match current_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            }
        } else {
            SortDirection::Descending
        };

        self.sort_field = field;
        self.sort_direction = next_direction;
        if let Some(drilldown) = self.drilldown.as_mut() {
            drilldown.sort_field = field;
            drilldown.sort_direction = next_direction;
        } else {
            self.persist_current_sort();
        }
        if self.is_drilldown_active()
            || (self.current_tab == Tab::Daily && self.is_daily_detail_active())
        {
            self.selected_index = 0;
            self.scroll_offset = 0;
        } else {
            self.reset_selection();
        }
        self.set_status(&format!(
            "Sorted by {:?} {:?}",
            self.sort_field, self.sort_direction
        ));
    }

    pub(crate) fn active_sort_state(&self) -> (SortField, SortDirection) {
        self.drilldown
            .as_ref()
            .map(|state| (state.sort_field, state.sort_direction))
            .unwrap_or((self.sort_field, self.sort_direction))
    }

    pub(crate) fn toggle_overview_mode(&mut self) {
        if self.current_tab != Tab::Overview {
            return;
        }

        self.overview_mode = match self.overview_mode {
            OverviewMode::All => OverviewMode::Today,
            OverviewMode::Today => OverviewMode::All,
        };
        self.reset_selection();
        self.clamp_selection();

        match self.overview_mode {
            OverviewMode::All => self.set_status("Overview: all time"),
            OverviewMode::Today => self.set_status("Overview: today"),
        }
    }

    pub(crate) fn set_chart_granularity(&mut self, granularity: ChartGranularity) {
        if self.current_tab != Tab::Overview {
            return;
        }
        self.chart_granularity = granularity;
        self.set_status(&format!(
            "Overview chart: {}",
            granularity.title_label().to_lowercase()
        ));
    }

    pub(crate) fn set_timeline_granularity(&mut self, granularity: TimelineGranularity) {
        if self.current_tab != Tab::Daily {
            return;
        }
        self.timeline_granularity = granularity;
        self.selected_daily_detail_date = None;
        self.drilldown = None;
        self.sort_field = SortField::Date;
        self.sort_direction = SortDirection::Descending;
        self.reset_selection();
        self.set_status(&format!("Timeline: {}", granularity.title_label()));
    }

    pub(crate) fn jump_to_today(&mut self) {
        if self.current_tab != Tab::Daily {
            return;
        }
        self.selected_daily_detail_date = None;
        self.drilldown = None;

        let today = chrono::Local::now().date_naive();
        let (today_index, total_len) = {
            let sorted_daily = self.get_sorted_daily();
            (
                sorted_daily.iter().position(|d| d.date == today),
                sorted_daily.len(),
            )
        };

        if let Some(index) = today_index {
            self.selected_index = index;

            if self.max_visible_items > 0 {
                let max_scroll = total_len.saturating_sub(self.max_visible_items);
                self.scroll_offset = index
                    .saturating_sub(self.max_visible_items / 2)
                    .min(max_scroll);
            } else {
                self.scroll_offset = 0;
            }

            self.set_status("Jumped to today's usage");
        } else {
            self.set_status("No usage recorded for today");
        }
    }

    pub(crate) fn cycle_theme(&mut self) {
        let new_theme = self.theme.name.next();
        self.theme = Theme::from_name_for_current_terminal(new_theme);
        self.dialog_stack.set_theme(self.theme.clone());
        self.settings.set_theme(new_theme);
        if let Err(e) = self.settings.save() {
            self.set_status(&format!(
                "Theme: {} (save failed: {})",
                new_theme.as_str(),
                e
            ));
        } else {
            self.set_status(&format!("Theme: {}", new_theme.as_str()));
        }
    }

    pub(crate) fn open_client_picker(&mut self) {
        let dialog = ClientPickerDialog::new(
            self.enabled_clients.clone(),
            self.dialog_needs_reload.clone(),
        );
        self.dialog_stack.show(Box::new(dialog));
    }

    /// Project the unified `HashSet<ClientFilter>` into the
    /// `Vec<ClientId>` shape that `tokscale_core` scanners still consume.
    /// `ClientFilter::Synthetic` does not have a `ClientId` and is
    /// excluded from this projection — use [`Self::include_synthetic`]
    /// for that signal.
    pub fn scan_clients(&self) -> Vec<ClientId> {
        let mut out: Vec<ClientId> = self
            .enabled_clients
            .borrow()
            .iter()
            .filter_map(|f| f.to_client_id())
            .collect();
        // Stable order for downstream cache key + log output. Sort by the
        // declaration index in ClientId::ALL so the projection mirrors
        // the canonical ordering used elsewhere.
        out.sort_by_key(|c| *c as usize);
        out
    }

    /// Whether the user has Synthetic enabled. Boundary helper for code
    /// paths that still take a separate `bool include_synthetic` argument.
    pub fn include_synthetic(&self) -> bool {
        self.enabled_clients
            .borrow()
            .contains(&ClientFilter::Synthetic)
    }

    pub(crate) fn open_group_by_picker(&mut self) {
        use crate::tui::ui::dialog::GroupByPickerDialog;
        let dialog =
            GroupByPickerDialog::new(self.group_by.clone(), self.dialog_needs_reload.clone());
        self.dialog_stack.show(Box::new(dialog));
    }
}
