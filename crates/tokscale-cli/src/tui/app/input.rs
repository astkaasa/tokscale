use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::tui::drilldown_state::DrilldownView;
use crate::tui::interaction::ClickAction;
use crate::tui::navigation::{
    ChartGranularity, HourlyViewMode, SortField, Tab, TimelineGranularity,
};

use super::App;

impl App {
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return true;
        }

        if self.dialog_stack.is_active() {
            self.dialog_stack.handle_key(key.code);
            self.consume_confirmed_codex_account_action();
            return false;
        }

        if key.code == KeyCode::Esc
            && self.current_tab == Tab::Usage
            && self.should_show_codex_login_panel()
        {
            self.dismiss_codex_login();
            return false;
        }

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return true;
            }
            KeyCode::Tab => {
                let next = self.next_visible_tab();
                self.switch_tab(next);
                self.reset_selection();
            }
            KeyCode::BackTab => {
                let prev = self.prev_visible_tab();
                self.switch_tab(prev);
                self.reset_selection();
            }
            KeyCode::Left
                if self.current_tab == Tab::Overview
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.scroll_overview_chart_left();
            }
            KeyCode::Right
                if self.current_tab == Tab::Overview
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.scroll_overview_chart_right();
            }
            KeyCode::Left => {
                let prev = self.prev_visible_tab();
                self.switch_tab(prev);
                self.reset_selection();
            }
            KeyCode::Right => {
                let next = self.next_visible_tab();
                self.switch_tab(next);
                self.reset_selection();
            }
            KeyCode::Up => {
                self.move_selection_up();
            }
            KeyCode::Down => {
                self.move_selection_down();
            }
            KeyCode::PageUp => {
                self.move_page_up();
            }
            KeyCode::PageDown => {
                self.move_page_down();
            }
            KeyCode::Home
                if self.current_tab == Tab::Overview
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.scroll_overview_chart_to_start();
            }
            KeyCode::End
                if self.current_tab == Tab::Overview
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.scroll_overview_chart_to_end();
            }
            KeyCode::Home => {
                self.move_to_top();
            }
            KeyCode::End => {
                self.move_to_bottom();
            }
            KeyCode::Char('c') => {
                self.set_sort(SortField::Cost);
            }
            KeyCode::Char('t') if self.current_tab == Tab::Overview => {
                self.toggle_overview_mode();
            }
            KeyCode::Char('T') if self.current_tab == Tab::Overview => {
                self.set_sort(SortField::Tokens);
            }
            KeyCode::Char('t') => {
                self.set_sort(SortField::Tokens);
            }
            KeyCode::Char('d')
                if matches!(self.drilldown_view(), Some(DrilldownView::Model(_))) =>
            {
                self.set_sort(SortField::Date);
            }
            KeyCode::Char('d') if self.is_drilldown_active() => {}
            KeyCode::Char('d') if self.current_tab != Tab::Daily => {
                self.set_sort(SortField::Date);
            }
            KeyCode::Char('j') => {
                self.jump_to_today();
            }
            KeyCode::Char('r') => {
                if self.current_tab == Tab::Usage {
                    self.refresh_usage();
                } else if self.current_tab == Tab::Pulse {
                    self.refresh_weread();
                } else if self.background_loading {
                    self.set_status("Refresh already in progress");
                } else {
                    self.needs_reload = true;
                }
            }
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.toggle_auto_refresh();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.increase_refresh_interval();
            }
            KeyCode::Char('-') => {
                self.decrease_refresh_interval();
            }
            KeyCode::Char('y') => {
                self.copy_selected_to_clipboard();
            }
            KeyCode::Char('e') => {
                self.export_to_json();
            }
            KeyCode::Char('s') => {
                self.open_client_picker();
            }
            KeyCode::Char('D') if self.current_tab == Tab::Overview => {
                self.set_chart_granularity(ChartGranularity::Daily);
            }
            KeyCode::Char('W') if self.current_tab == Tab::Overview => {
                self.set_chart_granularity(ChartGranularity::Weekly);
            }
            KeyCode::Char('M') if self.current_tab == Tab::Overview => {
                self.set_chart_granularity(ChartGranularity::Monthly);
            }
            KeyCode::Char('d') if self.current_tab == Tab::Daily => {
                self.set_timeline_granularity(TimelineGranularity::Day);
            }
            KeyCode::Char('h') if self.current_tab == Tab::Daily => {
                self.set_timeline_granularity(TimelineGranularity::Hour);
            }
            KeyCode::Char('v') if self.current_tab == Tab::Hourly => {
                self.hourly_view_mode = match self.hourly_view_mode {
                    HourlyViewMode::Table => HourlyViewMode::Profile,
                    HourlyViewMode::Profile => HourlyViewMode::Table,
                };
                self.reset_selection();
            }
            KeyCode::Char('g') => {
                self.open_group_by_picker();
            }
            KeyCode::Char('a') if self.current_tab == Tab::Usage => {
                self.start_codex_login();
            }
            KeyCode::Char('m') if self.current_tab == Tab::Usage => {
                self.toggle_usage_email_privacy();
            }
            KeyCode::Char('x') if self.current_tab == Tab::Usage => {
                self.confirm_selected_codex_rate_limit_reset();
            }
            KeyCode::Enter if self.is_drilldown_active() => {
                self.open_selected_drilldown_child();
            }
            KeyCode::Enter if matches!(self.current_tab, Tab::Overview | Tab::Models) => {
                self.open_selected_model_detail();
            }
            KeyCode::Enter
                if self.current_tab == Tab::Daily
                    && self.timeline_granularity == TimelineGranularity::Day =>
            {
                self.open_selected_period_detail();
            }
            KeyCode::Esc | KeyCode::Backspace if self.is_drilldown_active() => {
                self.close_drilldown();
            }
            _ => {}
        }
        false
    }
    pub fn handle_mouse_event(&mut self, event: MouseEvent) {
        if self.dialog_stack.is_active() {
            self.dialog_stack.handle_mouse(event);
            self.consume_confirmed_codex_account_action();
            return;
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let x = event.column;
                let y = event.row;

                let action = self
                    .click_areas
                    .iter()
                    .rev()
                    .find(|area| {
                        x >= area.rect.x
                            && x < area.rect.x + area.rect.width
                            && y >= area.rect.y
                            && y < area.rect.y + area.rect.height
                    })
                    .map(|area| area.action.clone());

                if let Some(action) = action {
                    match action {
                        ClickAction::Tab(tab) => {
                            self.switch_tab(tab);
                            self.reset_selection();
                        }
                        ClickAction::Sort(field) => {
                            self.set_sort(field);
                        }
                        ClickAction::OverviewChartGranularity(granularity) => {
                            self.set_chart_granularity(granularity);
                        }
                        ClickAction::TimelineGranularity(granularity) => {
                            self.set_timeline_granularity(granularity);
                        }
                        ClickAction::OpenModelDetail(key) => {
                            self.open_model_detail(key);
                        }
                        ClickAction::OpenPeriodDetail(key) => {
                            self.open_period_detail(key);
                        }
                        ClickAction::UsageRefresh => {
                            self.refresh_usage();
                        }
                        ClickAction::CodexStartLogin => {
                            self.start_codex_login();
                        }
                        ClickAction::CodexDismissLogin => {
                            self.dismiss_codex_login();
                        }
                        ClickAction::UsageToggleEmailPrivacy => {
                            self.toggle_usage_email_privacy();
                        }
                        ClickAction::WeReadRefresh => {
                            self.refresh_weread();
                        }
                        ClickAction::UsageSelect { index } => {
                            self.selected_index = index;
                        }
                        ClickAction::CodexUseAccount { account_id } => {
                            self.confirm_codex_account_switch(&account_id);
                        }
                        ClickAction::CodexRemoveAccount { account_id } => {
                            self.confirm_codex_account_removal(&account_id);
                        }
                        ClickAction::CodexResetAccount { account_id } => {
                            self.confirm_codex_rate_limit_reset(&account_id);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                self.move_selection_up();
            }
            MouseEventKind::ScrollDown => {
                self.move_selection_down();
            }
            _ => {}
        }
    }

    pub fn toggle_usage_email_privacy(&mut self) {
        self.hide_usage_emails = !self.hide_usage_emails;
        if self.hide_usage_emails {
            self.set_status("Usage emails hidden");
        } else {
            self.set_status("Usage emails visible");
        }
    }
}
