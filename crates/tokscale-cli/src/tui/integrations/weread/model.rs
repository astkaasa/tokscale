use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const SKILL_VERSION: &str = "1.0.3";
pub(crate) const WEEKLY_STALE_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WeReadStatus {
    AuthMissing,
    Loading,
    Fresh,
    Stale,
    Error,
    UpgradeRequired,
}

impl WeReadStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            WeReadStatus::AuthMissing => "auth missing",
            WeReadStatus::Loading => "syncing",
            WeReadStatus::Fresh => "fresh",
            WeReadStatus::Stale => "stale",
            WeReadStatus::Error => "error",
            WeReadStatus::UpgradeRequired => "upgrade required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadState {
    #[serde(default)]
    pub(crate) weekly: Option<WeReadWeekly>,
    #[serde(default)]
    pub(crate) monthly: Option<WeReadMonthly>,
    #[serde(default)]
    pub(crate) shelf: Option<WeReadShelfSummary>,
    #[serde(default)]
    pub(crate) notes: Option<WeReadNotesSummary>,
    pub(crate) status: WeReadStatus,
    #[serde(default)]
    pub(crate) last_refresh_ms: Option<u64>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

impl Default for WeReadState {
    fn default() -> Self {
        Self::empty(WeReadStatus::AuthMissing)
    }
}

impl WeReadState {
    pub(crate) fn empty(status: WeReadStatus) -> Self {
        Self {
            weekly: None,
            monthly: None,
            shelf: None,
            notes: None,
            status,
            last_refresh_ms: None,
            error: None,
        }
    }

    pub(crate) fn has_data(&self) -> bool {
        self.weekly.is_some()
            || self.monthly.is_some()
            || self.shelf.is_some()
            || self.notes.is_some()
    }

    pub(crate) fn is_stale_at(&self, now_ms: u64) -> bool {
        match self.last_refresh_ms {
            Some(last) => now_ms.saturating_sub(last) > WEEKLY_STALE_MS,
            None => true,
        }
    }

    pub(crate) fn mark_error(&mut self, message: String) {
        self.error = Some(message);
        self.status = if self.has_data() {
            WeReadStatus::Error
        } else {
            WeReadStatus::Error
        };
    }

    pub(crate) fn mark_auth_missing(&mut self) {
        self.error = Some("env.WEREAD_API_KEY is not configured".to_string());
        self.status = if self.has_data() {
            WeReadStatus::Stale
        } else {
            WeReadStatus::AuthMissing
        };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadWeekly {
    pub(crate) period_start: NaiveDate,
    pub(crate) period_end: NaiveDate,
    pub(crate) read_days: u8,
    pub(crate) total_seconds: u32,
    pub(crate) day_average_seconds: u32,
    pub(crate) compare_ratio: Option<f64>,
    pub(crate) days: [WeReadDay; 7],
    pub(crate) focus: Option<WeReadFocusBook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadDay {
    pub(crate) date: NaiveDate,
    pub(crate) read_seconds: u32,
    pub(crate) checked_in: bool,
}

impl WeReadDay {
    pub(crate) fn new(date: NaiveDate, read_seconds: u32) -> Self {
        Self {
            date,
            read_seconds,
            checked_in: read_seconds >= 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadFocusBook {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) author: Option<String>,
    pub(crate) read_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadMonthly {
    pub(crate) read_days: u16,
    pub(crate) total_seconds: u32,
    pub(crate) day_average_seconds: u32,
    pub(crate) prefer_category_word: Option<String>,
    pub(crate) categories: Vec<WeReadCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadCategory {
    pub(crate) title: String,
    pub(crate) reading_seconds: u32,
    pub(crate) reading_count: u32,
    pub(crate) weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadShelfSummary {
    pub(crate) books: u32,
    pub(crate) albums: u32,
    pub(crate) has_mp: bool,
    pub(crate) visible_items: u32,
    pub(crate) private_items: u32,
    pub(crate) recent: Vec<WeReadBookRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadBookRef {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) author: Option<String>,
    pub(crate) last_read_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadNotesSummary {
    pub(crate) total_books: u32,
    pub(crate) total_notes: u32,
    pub(crate) top_books: Vec<WeReadNotebookSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeReadNotebookSummary {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) author: Option<String>,
    pub(crate) total_notes: u32,
    pub(crate) review_count: u32,
    pub(crate) note_count: u32,
    pub(crate) bookmark_count: u32,
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn local_date_from_unix_seconds(seconds: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(seconds, 0).map(|dt| dt.with_timezone(&Local).date_naive())
}

pub(crate) fn week_start_for(date: NaiveDate) -> NaiveDate {
    date.checked_sub_signed(ChronoDuration::days(
        date.weekday().num_days_from_monday() as i64
    ))
    .unwrap_or(date)
}

pub(crate) fn format_read_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let mins = minutes % 60;

    match (hours, mins) {
        (0, 0) if seconds > 0 => "<1m".to_string(),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

pub(crate) fn format_compare_ratio(ratio: Option<f64>) -> String {
    let Some(ratio) = ratio else {
        return "n/a".to_string();
    };
    let pct = ratio * 100.0;
    if pct > 0.0 {
        format!("+{pct:.0}%")
    } else {
        format!("{pct:.0}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_one_minute_as_check_in() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();

        assert!(!WeReadDay::new(date, 59).checked_in);
        assert!(WeReadDay::new(date, 60).checked_in);
    }

    #[test]
    fn formats_read_duration_compactly() {
        assert_eq!(format_read_duration(30), "<1m");
        assert_eq!(format_read_duration(60), "1m");
        assert_eq!(format_read_duration(3600), "1h");
        assert_eq!(format_read_duration(3660), "1h1m");
    }

    #[test]
    fn formats_compare_ratio_as_percent() {
        assert_eq!(format_compare_ratio(Some(0.35)), "+35%");
        assert_eq!(format_compare_ratio(Some(-0.18)), "-18%");
        assert_eq!(format_compare_ratio(None), "n/a");
    }
}
