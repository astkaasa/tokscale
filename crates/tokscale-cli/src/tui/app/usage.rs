use crate::tui::codex_login::run_codex_login_worker;
use crate::tui::navigation::Tab;
use crate::tui::privacy::looks_like_email;
use crate::tui::ui::dialog::ConfirmDialog;

use super::App;

impl App {
    pub fn fetch_subscription_usage(&mut self) {
        if self.usage_job.is_running() {
            return; // already fetching
        }
        self.usage_fetch_attempted = true;
        self.status_message = Some("Fetching usage data...".into());
        self.status_message_time = Some(std::time::Instant::now());
        #[cfg(test)]
        let usage_fetcher = self.usage_fetcher;
        self.usage_job.start(move || {
            #[cfg(test)]
            let results = usage_fetcher();
            #[cfg(not(test))]
            let results = crate::commands::usage::fetch_all();
            results
        });
    }

    pub fn is_fetching_usage(&self) -> bool {
        self.usage_job.is_running()
    }

    pub fn refresh_usage(&mut self) {
        if self.usage_job.is_running() {
            self.set_status("Refresh already in progress");
        } else {
            self.fetch_subscription_usage();
        }
    }

    pub fn is_fetching_weread(&self) -> bool {
        self.pulse.is_fetching_weread()
    }

    pub fn refresh_weread(&mut self) {
        if let Some(status) = self.pulse.refresh_weread(&self.settings) {
            self.set_status(status);
        }
    }

    pub(crate) fn maybe_fetch_usage_on_entry(&mut self) {
        if self.current_tab == Tab::Usage
            && !self.usage_fetch_attempted
            && !self.usage_job.is_running()
        {
            self.fetch_subscription_usage();
        }
    }

    pub(crate) fn maybe_fetch_weread_on_entry(&mut self) {
        if let Some(status) = self.pulse.maybe_fetch_weread_on_entry(&self.settings) {
            self.set_status(status);
        }
    }

    pub(crate) fn poll_weread_fetch(&mut self) {
        if let Some(update) = self.pulse.poll_weread_fetch() {
            if update.loaded {
                self.clamp_selection();
            }
            self.set_status(update.status);
        }
    }

    pub fn is_codex_login_running(&self) -> bool {
        self.codex_login_rx.is_some()
    }

    pub fn should_show_codex_login_panel(&self) -> bool {
        self.is_codex_login_running()
            || self.codex_login_outcome.is_some()
            || !self.codex_login_lines.is_empty()
    }

    pub fn start_codex_login(&mut self) {
        if self.codex_login_rx.is_some() {
            self.set_status("Codex login already in progress");
            return;
        }

        self.codex_login_lines.clear();
        self.codex_login_outcome = None;

        let (tx, rx) = std::sync::mpsc::channel();
        self.codex_login_rx = Some(rx);
        self.set_status("Starting Codex login...");
        std::thread::spawn(move || run_codex_login_worker(tx));
    }

    pub fn dismiss_codex_login(&mut self) {
        if self.codex_login_rx.is_none() {
            self.codex_login_lines.clear();
            self.codex_login_outcome = None;
            self.set_status("Codex login panel dismissed");
        }
    }

    pub fn confirm_codex_account_switch(&mut self, account_id: &str) {
        if self.subscription_usage.iter().any(|usage| {
            usage
                .account
                .as_ref()
                .is_some_and(|account| account.id == account_id && account.is_active)
        }) {
            self.set_status("Codex account already active");
            return;
        }

        let account_label = self.codex_account_label(account_id);
        let dialog = ConfirmDialog::codex_switch(
            account_id.to_string(),
            account_label,
            self.confirmed_codex_use_account_id.clone(),
        );
        self.dialog_stack.show(Box::new(dialog));
        self.set_status("Confirm Codex account switch");
    }

    pub fn confirm_codex_account_removal(&mut self, account_id: &str) {
        let account_label = self.codex_account_label(account_id);
        let dialog = ConfirmDialog::codex_remove(
            account_id.to_string(),
            account_label,
            self.confirmed_codex_remove_account_id.clone(),
        );
        self.dialog_stack.show(Box::new(dialog));
        self.set_status("Confirm Codex account removal");
    }

    pub(crate) fn consume_confirmed_codex_account_action(&mut self) {
        let account_id = self.confirmed_codex_use_account_id.borrow_mut().take();
        if let Some(account_id) = account_id {
            self.use_codex_account(&account_id);
            return;
        }

        let account_id = self.confirmed_codex_remove_account_id.borrow_mut().take();
        if let Some(account_id) = account_id {
            self.remove_codex_account(&account_id);
        }
    }

    pub(crate) fn codex_account_label(&self, account_id: &str) -> String {
        self.subscription_usage
            .iter()
            .find_map(|usage| {
                let account = usage.account.as_ref()?;
                if account.id != account_id {
                    return None;
                }

                let label = usage
                    .account_display_name()
                    .unwrap_or_else(|| account.display_name());
                if self.hide_usage_emails && looks_like_email(&label) {
                    Some(format!("Account {}", account.short_id()))
                } else {
                    Some(label)
                }
            })
            .unwrap_or_else(|| short_account_id(account_id))
    }

    pub fn use_codex_account(&mut self, account_id: &str) {
        match crate::commands::usage::codex::switch_active_account(account_id) {
            Ok(info) => {
                self.mark_active_codex_account(&info.id);
                if let Some(index) = self.subscription_usage.iter().position(|usage| {
                    usage
                        .account
                        .as_ref()
                        .is_some_and(|account| account.id == info.id)
                }) {
                    self.selected_index = index;
                    if self.selected_index < self.scroll_offset {
                        self.scroll_offset = self.selected_index;
                    } else if self.selected_index >= self.scroll_offset + self.max_visible_items {
                        self.scroll_offset = self
                            .selected_index
                            .saturating_sub(self.max_visible_items.saturating_sub(1));
                    }
                }
                self.persist_subscription_usage_cache();
                let display = info.label.as_deref().unwrap_or(&info.id);
                self.set_status(&format!("Active Codex account: {display}"));
            }
            Err(e) => {
                self.set_status(&format!("Codex account switch failed: {e}"));
            }
        }
    }

    pub fn remove_codex_account(&mut self, account_id: &str) {
        match crate::commands::usage::codex::remove_account(account_id) {
            Ok(info) => {
                self.subscription_usage.retain(|usage| {
                    usage.account.as_ref().map(|account| account.id.as_str())
                        != Some(info.id.as_str())
                });
                self.clamp_selection();
                if let Some(active) = crate::commands::usage::codex::list_accounts()
                    .into_iter()
                    .find(|account| account.is_active)
                {
                    self.mark_active_codex_account(&active.id);
                }
                self.persist_subscription_usage_cache();
                let display = info.label.as_deref().unwrap_or(&info.id);
                self.set_status(&format!("Removed Codex account: {display}"));
            }
            Err(e) => {
                self.set_status(&format!("Codex account removal failed: {e}"));
            }
        }
    }

    pub(crate) fn persist_subscription_usage_cache(&self) {
        if self.subscription_usage.is_empty() {
            crate::commands::usage::clear_cache();
        } else {
            crate::commands::usage::save_cache(&self.subscription_usage);
        }
    }

    pub(crate) fn mark_active_codex_account(&mut self, active_account_id: &str) {
        for usage in &mut self.subscription_usage {
            if usage.provider == "Codex" {
                if let Some(account) = &mut usage.account {
                    account.is_active = account.id == active_account_id;
                }
            }
        }
    }
}

fn short_account_id(account_id: &str) -> String {
    let id = account_id.trim();
    if id.is_empty() {
        return "Account unknown".to_string();
    }

    let char_count = id.chars().count();
    if char_count <= 12 {
        return format!("Account {id}");
    }

    let head: String = id.chars().take(6).collect();
    let tail: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("Account {head}...{tail}")
}
