use super::App;
use crate::commands::usage::UsageOutput;
use crate::tui::codex_login::{
    cancel_codex_login_child, run_codex_login_worker, CodexLoginChildSlot,
};
use crate::tui::navigation::Tab;
use crate::tui::privacy::looks_like_email;
use crate::tui::pulse_state::AiSourceObservedAt;
use crate::tui::ui::dialog::ConfirmDialog;

impl App {
    pub fn fetch_subscription_usage(&mut self) {
        if self.usage_job.is_running() {
            return; // already fetching
        }
        self.usage_fetch_attempted = true;
        self.usage_fetch_diagnostics.clear();
        self.status_message = Some("Fetching usage data...".into());
        self.status_message_time = Some(std::time::Instant::now());
        #[cfg(test)]
        let usage_fetcher = self.usage_fetcher;
        self.usage_job.start(move || {
            #[cfg(test)]
            let results = usage_fetcher();
            #[cfg(not(test))]
            let results = crate::commands::usage::fetch_all_report_with_intent(
                crate::commands::usage::UsageFetchIntent::TuiSurface,
            );
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
        if self.current_tab != Tab::Pulse {
            return;
        }
        if let Some(status) = self.pulse.maybe_fetch_weread_on_entry(&self.settings) {
            self.set_status(status);
        }
    }

    pub(crate) fn poll_weread_fetch(&mut self) {
        if let Some(update) = self.pulse.poll_weread_fetch() {
            if update.loaded {
                self.clamp_selection();
            }
            match self.pulse.rebuild_snapshot_after_weread(
                &self.data,
                &self.subscription_usage,
                self.pulse_ai_observed_at,
                self.pulse_data_provenance.can_seed_global_snapshot(),
            ) {
                Ok(()) => {
                    self.pulse_ai_observed_at = self.pulse.ai_observed_at();
                    self.set_status(update.status);
                }
                Err(error) => {
                    let message = format!("Pulse snapshot save failed: {error}");
                    self.set_status(&message);
                }
            }
        }
    }

    pub(crate) fn rebuild_pulse_snapshot(&mut self) -> anyhow::Result<()> {
        if !self.pulse_data_provenance.can_seed_global_snapshot() {
            return Ok(());
        }

        let result = self.pulse.rebuild_snapshot(
            &self.data,
            &self.subscription_usage,
            self.pulse_ai_observed_at,
            true,
        );
        if result.is_ok() {
            self.pulse_ai_observed_at = self.pulse.ai_observed_at();
        }
        result
    }

    pub(crate) fn rebuild_pulse_snapshot_preserving_quota(&mut self) -> anyhow::Result<()> {
        if !self.pulse_data_provenance.can_seed_global_snapshot() {
            return Ok(());
        }

        let result = self.pulse.rebuild_snapshot(
            &self.data,
            &[],
            AiSourceObservedAt {
                local: self.pulse_ai_observed_at.local,
                quota: None,
            },
            true,
        );
        if result.is_ok() {
            self.pulse_ai_observed_at = self.pulse.ai_observed_at();
        }
        result
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
        let child_slot = CodexLoginChildSlot::default();
        self.codex_login_child = Some(std::sync::Arc::clone(&child_slot));
        self.set_status("Starting Codex login...");
        std::thread::spawn(move || run_codex_login_worker(tx, child_slot));
    }

    pub fn dismiss_codex_login(&mut self) {
        if self.codex_login_rx.is_some() {
            self.kill_codex_login_child();
            self.codex_login_rx = None;
            self.codex_login_lines.clear();
            self.codex_login_outcome = None;
            self.set_status("Codex login cancelled");
            return;
        }

        self.codex_login_lines.clear();
        self.codex_login_outcome = None;
        self.set_status("Codex login panel dismissed");
    }

    /// Kills any in-flight `codex login` child process. Called on dismiss and
    /// on TUI exit so a dangling login cannot keep holding the OAuth port.
    pub fn kill_codex_login_child(&mut self) {
        let Some(slot) = self.codex_login_child.take() else {
            return;
        };
        cancel_codex_login_child(&slot);
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
        if self.subscription_usage.iter().any(|usage| {
            usage
                .account
                .as_ref()
                .is_some_and(|account| account.id == account_id && account.is_active)
        }) {
            self.set_status("Switch Codex accounts before removing the current account");
            return;
        }

        let account_label = self.codex_account_label(account_id);
        let dialog = ConfirmDialog::codex_remove(
            account_id.to_string(),
            account_label,
            self.confirmed_codex_remove_account_id.clone(),
        );
        self.dialog_stack.show(Box::new(dialog));
        self.set_status("Confirm Codex account removal");
    }

    pub fn confirm_selected_codex_account_switch(&mut self) {
        let Some(output) = self.subscription_usage.get(self.selected_index) else {
            self.set_status("No usage account selected");
            return;
        };

        if output.provider != "Codex" {
            self.set_status("Codex switch only supports Codex accounts");
            return;
        }

        let Some(account_id) = output.account.as_ref().map(|account| account.id.clone()) else {
            self.set_status("Select a saved Codex account to use");
            return;
        };

        self.confirm_codex_account_switch(&account_id);
    }

    pub fn confirm_selected_codex_account_removal(&mut self) {
        let Some(output) = self.subscription_usage.get(self.selected_index) else {
            self.set_status("No usage account selected");
            return;
        };

        if output.provider != "Codex" {
            self.set_status("Codex removal only supports Codex accounts");
            return;
        }

        let Some(account) = output.account.as_ref() else {
            self.set_status("Select a saved Codex account to remove");
            return;
        };
        if account.is_active {
            self.set_status("Switch Codex accounts before removing the current account");
            return;
        }
        let account_id = account.id.clone();

        self.confirm_codex_account_removal(&account_id);
    }

    pub fn confirm_codex_rate_limit_reset(&mut self, account_id: &str) {
        if self.codex_reset_job.is_running() {
            self.set_status("Codex reset already in progress");
            return;
        }

        let Some(output) = self.subscription_usage.iter().find(|usage| {
            usage.provider == "Codex"
                && usage
                    .account
                    .as_ref()
                    .is_some_and(|account| account.id == account_id)
        }) else {
            self.set_status("Codex account not found");
            return;
        };

        let available = output
            .reset_credits
            .as_ref()
            .map(|credits| credits.available_count)
            .unwrap_or(0);
        if available == 0 {
            self.set_status("No Codex reset credits available");
            return;
        }

        let mut account_label = self.codex_account_label(account_id);
        account_label.push_str(&format!(" - {available} reset"));
        if available != 1 {
            account_label.push('s');
        }
        if let Some(expiry) = output.reset_credits.as_ref().and_then(|credits| {
            credits
                .credits
                .iter()
                .find_map(|credit| credit.expires_at.as_ref())
        }) {
            account_label.push_str(&format!(
                " - {}",
                crate::commands::usage::helpers::format_reset_time(expiry)
                    .replace("resets", "expires")
            ));
        }

        let dialog = ConfirmDialog::codex_reset(
            account_id.to_string(),
            account_label,
            self.confirmed_codex_reset_account_id.clone(),
        );
        self.dialog_stack.show(Box::new(dialog));
        self.set_status("Confirm Codex reset credit use");
    }

    pub fn confirm_selected_codex_rate_limit_reset(&mut self) {
        let Some(output) = self.subscription_usage.get(self.selected_index) else {
            self.set_status("No usage account selected");
            return;
        };

        if output.provider != "Codex" {
            self.set_status("Codex reset only supports Codex accounts");
            return;
        }

        let Some(account_id) = output.account.as_ref().map(|account| account.id.clone()) else {
            self.set_status("Select a saved Codex account to reset");
            return;
        };

        self.confirm_codex_rate_limit_reset(&account_id);
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
            return;
        }

        let account_id = self.confirmed_codex_reset_account_id.borrow_mut().take();
        if let Some(account_id) = account_id {
            self.reset_codex_rate_limits(&account_id);
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
                self.sort_codex_subscription_usage();
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
                    self.sort_codex_subscription_usage();
                } else {
                    self.clear_active_codex_accounts();
                }
                self.persist_subscription_usage_cache();
                let display = info.label.as_deref().unwrap_or(&info.id);
                self.set_status(&format!(
                    "Stopped tracking Codex account: {display} (codex CLI login unchanged)"
                ));
            }
            Err(e) => {
                self.set_status(&format!("Codex account removal failed: {e}"));
            }
        }
    }

    pub fn reset_codex_rate_limits(&mut self, account_id: &str) {
        if self.codex_reset_job.is_running() {
            self.set_status("Codex reset already in progress");
            return;
        }

        let account_id = account_id.to_string();
        self.set_status("Resetting Codex limits...");
        self.codex_reset_job.start(move || {
            crate::commands::usage::codex::consume_rate_limit_reset_credit(&account_id)
                .map_err(|error| error.to_string())
        });
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

    pub(crate) fn clear_active_codex_accounts(&mut self) {
        for usage in &mut self.subscription_usage {
            if usage.provider == "Codex" {
                if let Some(account) = &mut usage.account {
                    account.is_active = false;
                }
            }
        }
    }

    pub(crate) fn sort_codex_subscription_usage(&mut self) {
        let mut codex_outputs = self
            .subscription_usage
            .iter()
            .filter(|usage| usage.provider == "Codex")
            .cloned()
            .collect::<Vec<_>>();
        if codex_outputs.len() < 2 {
            return;
        }

        codex_outputs.sort_by(compare_codex_usage_outputs);
        let mut sorted = codex_outputs.into_iter();
        for usage in &mut self.subscription_usage {
            if usage.provider == "Codex" {
                if let Some(next) = sorted.next() {
                    *usage = next;
                }
            }
        }
    }
}

fn compare_codex_usage_outputs(a: &UsageOutput, b: &UsageOutput) -> std::cmp::Ordering {
    let active_order = codex_usage_is_active(b).cmp(&codex_usage_is_active(a));
    if active_order != std::cmp::Ordering::Equal {
        return active_order;
    }

    codex_usage_sort_key(a)
        .cmp(&codex_usage_sort_key(b))
        .then_with(|| codex_usage_account_id(a).cmp(codex_usage_account_id(b)))
}

fn codex_usage_is_active(output: &UsageOutput) -> bool {
    output
        .account
        .as_ref()
        .is_some_and(|account| account.is_active)
}

fn codex_usage_sort_key(output: &UsageOutput) -> String {
    output
        .account
        .as_ref()
        .map(|account| {
            account
                .label_name()
                .unwrap_or(account.id.as_str())
                .to_lowercase()
        })
        .unwrap_or_else(|| output.display_name().to_lowercase())
}

fn codex_usage_account_id(output: &UsageOutput) -> &str {
    output
        .account
        .as_ref()
        .map(|account| account.id.as_str())
        .unwrap_or_default()
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

pub(crate) fn codex_reset_outcome_label(
    result: &crate::commands::usage::codex::RateLimitResetConsumeResult,
) -> String {
    match result.code.as_str() {
        "reset" => match result.windows_reset {
            Some(1) => "reset 1 window".to_string(),
            Some(count) => format!("reset {count} windows"),
            None => "reset complete".to_string(),
        },
        "already_redeemed" => "credit already redeemed".to_string(),
        "nothing_to_reset" => "nothing to reset".to_string(),
        "no_credit" => "no credit available".to_string(),
        "" => "unknown response".to_string(),
        other => other.to_string(),
    }
}
