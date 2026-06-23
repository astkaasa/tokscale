use std::time::{Duration, Instant};

use crate::tui::navigation::{Tab, TimelineGranularity};
use crate::tui::themes::Theme;

use super::App;

impl App {
    pub(crate) fn toggle_auto_refresh(&mut self) {
        self.auto_refresh = !self.auto_refresh;
        self.settings.auto_refresh_enabled = self.auto_refresh;
        let save_result = self.settings.save();
        let msg = if self.auto_refresh {
            format!(
                "Auto-refresh ON ({}s)",
                self.auto_refresh_interval.as_secs()
            )
        } else {
            "Auto-refresh OFF".to_string()
        };
        if let Err(e) = save_result {
            self.set_status(&format!("{} (save failed: {})", msg, e));
        } else {
            self.set_status(&msg);
        }
    }

    pub(crate) fn increase_refresh_interval(&mut self) {
        let ms = self.auto_refresh_interval.as_millis() as u64;
        let new_ms = ms.saturating_add(10_000).min(300_000);
        self.auto_refresh_interval = Duration::from_millis(new_ms);
        self.settings.auto_refresh_ms = new_ms;
        let save_result = self.settings.save();
        let msg = format!("Refresh interval: {}s", new_ms / 1000);
        if let Err(e) = save_result {
            self.set_status(&format!("{} (save failed: {})", msg, e));
        } else {
            self.set_status(&msg);
        }
    }

    pub(crate) fn decrease_refresh_interval(&mut self) {
        let ms = self.auto_refresh_interval.as_millis() as u64;
        let new_ms = ms.saturating_sub(10_000).max(30_000);
        self.auto_refresh_interval = Duration::from_millis(new_ms);
        self.settings.auto_refresh_ms = new_ms;
        let save_result = self.settings.save();
        let msg = format!("Refresh interval: {}s", new_ms / 1000);
        if let Err(e) = save_result {
            self.set_status(&format!("{} (save failed: {})", msg, e));
        } else {
            self.set_status(&msg);
        }
    }

    pub(crate) fn toggle_theme(&mut self) {
        let next = self.theme_preference.toggled();
        self.theme_preference = next;
        self.settings.ui_theme = next;

        let theme = Theme::for_current_terminal_with_preference(next);
        self.theme = theme.clone();
        self.dialog_stack.set_theme(theme);

        let msg = format!("Theme: {}", next.label());
        match self.settings.save() {
            Ok(()) => self.set_status(&msg),
            Err(e) => self.set_status(&format!("{} (save failed: {})", msg, e)),
        }
    }

    pub(crate) fn copy_selected_to_clipboard(&mut self) {
        let text = match self.current_tab {
            Tab::Overview | Tab::Models => self
                .get_sorted_models()
                .get(self.selected_index)
                .map(|m| format!("{}: {} tokens, ${:.4}", m.model, m.tokens.total(), m.cost)),
            Tab::Daily if self.is_daily_detail_active() => self
                .get_sorted_daily_detail_rows()
                .get(self.selected_index)
                .map(|row| {
                    format!(
                        "{} / {}: {} tokens, ${:.4}",
                        row.source,
                        row.model,
                        row.tokens.total(),
                        row.cost
                    )
                }),
            Tab::Daily => match self.timeline_granularity {
                TimelineGranularity::Day => self
                    .get_sorted_daily()
                    .get(self.selected_index)
                    .map(|d| format!("{}: {} tokens, ${:.4}", d.date, d.tokens.total(), d.cost)),
                TimelineGranularity::Hour => {
                    self.get_sorted_hourly().get(self.selected_index).map(|h| {
                        format!(
                            "{}: {} tokens, ${:.4}",
                            h.datetime.format("%Y-%m-%d %H:%M"),
                            h.tokens.total(),
                            h.cost
                        )
                    })
                }
            },
            Tab::Hourly => self.get_sorted_hourly().get(self.selected_index).map(|h| {
                format!(
                    "{}: {} tokens, ${:.4}",
                    h.datetime.format("%Y-%m-%d %H:%M"),
                    h.tokens.total(),
                    h.cost
                )
            }),
            Tab::Minutely => self
                .get_sorted_minutely()
                .get(self.selected_index)
                .map(|m| {
                    format!(
                        "{}: {} tokens, ${:.4}",
                        m.datetime.format("%Y-%m-%d %H:%M"),
                        m.tokens.total(),
                        m.cost
                    )
                }),
            Tab::Pulse | Tab::Usage => None,
        };

        if let Some(text) = text {
            match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text)) {
                Ok(_) => self.set_status("Copied to clipboard"),
                Err(_) => self.set_status("Failed to copy"),
            }
        }
    }

    pub(crate) fn export_to_json(&mut self) {
        let filename = format!(
            "tokscale-export-{}.json",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        );

        match crate::tui::export::build_export_json(&self.data) {
            Ok(json) => match std::fs::write(&filename, json) {
                Ok(_) => self.set_status(&format!("Exported to {}", filename)),
                Err(e) => self.set_status(&format!("Export failed: {}", e)),
            },
            Err(e) => self.set_status(&format!("Export failed: {}", e)),
        }
    }

    pub fn set_status(&mut self, message: &str) {
        self.status_message = Some(message.to_string());
        self.status_message_time = Some(Instant::now());
    }

    pub(crate) fn clear_status(&mut self) {
        self.status_message = None;
        self.status_message_time = None;
    }
}
