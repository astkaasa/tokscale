use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SKILL_VERSION: &str = "1.0.3";
pub const WEEKLY_STALE_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeReadStatus {
    AuthMissing,
    Loading,
    Fresh,
    Stale,
    Error,
    UpgradeRequired,
}

impl WeReadStatus {
    pub fn label(self) -> &'static str {
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
pub struct WeReadState {
    #[serde(default)]
    pub weekly: Option<WeReadWeekly>,
    #[serde(default)]
    pub monthly: Option<WeReadMonthly>,
    #[serde(default)]
    pub shelf: Option<WeReadShelfSummary>,
    #[serde(default)]
    pub notes: Option<WeReadNotesSummary>,
    pub status: WeReadStatus,
    #[serde(default)]
    pub last_refresh_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Default for WeReadState {
    fn default() -> Self {
        Self::empty(WeReadStatus::AuthMissing)
    }
}

impl WeReadState {
    pub fn empty(status: WeReadStatus) -> Self {
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

    pub fn has_data(&self) -> bool {
        self.weekly.is_some()
            || self.monthly.is_some()
            || self.shelf.is_some()
            || self.notes.is_some()
    }

    pub fn is_stale_at(&self, now_ms: u64) -> bool {
        match self.last_refresh_ms {
            Some(last) => now_ms.saturating_sub(last) > WEEKLY_STALE_MS,
            None => true,
        }
    }

    pub fn mark_error(&mut self, message: String) {
        self.error = Some(message);
        self.status = WeReadStatus::Error;
    }

    pub fn mark_auth_missing(&mut self) {
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
pub struct WeReadWeekly {
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub read_days: u8,
    pub total_seconds: u32,
    pub day_average_seconds: u32,
    pub compare_ratio: Option<f64>,
    pub days: [WeReadDay; 7],
    pub focus: Option<WeReadFocusBook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadDay {
    pub date: NaiveDate,
    pub read_seconds: u32,
    pub checked_in: bool,
}

impl WeReadDay {
    pub fn new(date: NaiveDate, read_seconds: u32) -> Self {
        Self {
            date,
            read_seconds,
            checked_in: read_seconds >= 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadFocusBook {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub read_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadMonthly {
    pub read_days: u16,
    pub total_seconds: u32,
    pub day_average_seconds: u32,
    pub prefer_category_word: Option<String>,
    pub categories: Vec<WeReadCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadCategory {
    pub title: String,
    pub reading_seconds: u32,
    pub reading_count: u32,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadShelfSummary {
    pub books: u32,
    pub albums: u32,
    pub has_mp: bool,
    pub visible_items: u32,
    pub private_items: u32,
    pub recent: Vec<WeReadBookRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadBookRef {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub last_read_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadNotesSummary {
    pub total_books: u32,
    pub total_notes: u32,
    pub top_books: Vec<WeReadNotebookSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeReadNotebookSummary {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub total_notes: u32,
    pub review_count: u32,
    pub note_count: u32,
    pub bookmark_count: u32,
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn local_date_from_unix_seconds(seconds: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(seconds, 0).map(|dt| dt.with_timezone(&Local).date_naive())
}

pub fn week_start_for(date: NaiveDate) -> NaiveDate {
    date.checked_sub_signed(ChronoDuration::days(
        date.weekday().num_days_from_monday() as i64
    ))
    .unwrap_or(date)
}

pub fn format_read_duration(seconds: u32) -> String {
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

pub fn format_compare_ratio(ratio: Option<f64>) -> String {
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
