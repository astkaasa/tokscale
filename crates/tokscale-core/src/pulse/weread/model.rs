use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SKILL_VERSION: &str = "1.0.4";
pub const WEEKLY_STALE_MS: u64 = 15 * 60 * 1000;
const PREVIOUS_WEEK_STALE_MS: u64 = 24 * 60 * 60 * 1000;
const MONTHLY_STALE_MS: u64 = 60 * 60 * 1000;
const SHELF_STALE_MS: u64 = 6 * 60 * 60 * 1000;
const NOTEBOOKS_STALE_MS: u64 = 60 * 60 * 1000;
pub const UPGRADE_REQUIRED_PREFIX: &str = "WeRead skill upgrade required:";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetFreshness {
    Fresh,
    Stale,
    #[default]
    Missing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetCoverage {
    Complete,
    Partial,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceIssueCode {
    AuthMissing,
    UpgradeRequired,
    TransportError,
    GatewayError,
    InvalidResponse,
    NormalizationError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClassification {
    Retryable,
    AfterAuthentication,
    AfterUpgrade,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIssue {
    pub code: SourceIssueCode,
    pub message: String,
    pub retry: RetryClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_skill_version: Option<String>,
}

impl SourceIssue {
    pub fn new(
        code: SourceIssueCode,
        message: impl Into<String>,
        retry: RetryClassification,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retry,
            blocked_skill_version: None,
        }
    }

    pub fn upgrade_required(message: impl Into<String>) -> Self {
        Self {
            code: SourceIssueCode::UpgradeRequired,
            message: message.into(),
            retry: RetryClassification::AfterUpgrade,
            blocked_skill_version: Some(SKILL_VERSION.to_string()),
        }
    }

    pub fn blocks_current_skill(&self) -> bool {
        self.code == SourceIssueCode::UpgradeRequired
            && self.blocked_skill_version.as_deref() == Some(SKILL_VERSION)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: Deserialize<'de>"))]
pub struct Dataset<T> {
    #[serde(default)]
    pub value: Option<T>,
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub freshness: DatasetFreshness,
    #[serde(default)]
    pub coverage: DatasetCoverage,
    #[serde(default)]
    pub issue: Option<SourceIssue>,
}

impl<T> Default for Dataset<T> {
    fn default() -> Self {
        Self {
            value: None,
            observed_at: None,
            freshness: DatasetFreshness::Missing,
            coverage: DatasetCoverage::Unknown,
            issue: None,
        }
    }
}

impl<T> Dataset<T> {
    pub fn success(value: T, observed_at: DateTime<Utc>, coverage: DatasetCoverage) -> Self {
        Self {
            value: Some(value),
            observed_at: Some(observed_at),
            freshness: DatasetFreshness::Fresh,
            coverage,
            issue: None,
        }
    }

    pub fn retain_with_issue(&mut self, issue: SourceIssue, now: DateTime<Utc>) {
        self.issue = Some(issue);
        self.refresh_freshness_at(now);
    }

    pub fn refresh_freshness_at(&mut self, now: DateTime<Utc>) {
        self.refresh_freshness_at_with_stale_ms(now, WEEKLY_STALE_MS);
    }

    fn refresh_freshness_at_with_stale_ms(&mut self, now: DateTime<Utc>, stale_ms: u64) {
        self.freshness = match (&self.value, self.observed_at) {
            (Some(_), Some(observed_at)) => {
                let age_ms = now
                    .signed_duration_since(observed_at)
                    .num_milliseconds()
                    .max(0) as u64;
                if age_ms > stale_ms {
                    DatasetFreshness::Stale
                } else {
                    DatasetFreshness::Fresh
                }
            }
            _ => DatasetFreshness::Missing,
        };
    }

    fn from_legacy(
        value: Option<T>,
        observed_at: Option<DateTime<Utc>>,
        coverage: DatasetCoverage,
        now: DateTime<Utc>,
    ) -> Self {
        let mut dataset = Self {
            value,
            observed_at,
            freshness: DatasetFreshness::Missing,
            coverage,
            issue: None,
        };
        dataset.refresh_freshness_at(now);
        dataset
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeReadDatasets {
    #[serde(default)]
    pub current_week: Dataset<WeReadWeekly>,
    #[serde(default)]
    pub previous_week: Dataset<WeReadWeekly>,
    #[serde(default)]
    pub current_month: Dataset<WeReadMonthly>,
    #[serde(default)]
    pub shelf: Dataset<WeReadShelfSummary>,
    #[serde(default)]
    pub notebooks: Dataset<WeReadNotesSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeReadSyncState {
    pub datasets: WeReadDatasets,
    pub status: WeReadStatus,
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
}

impl Default for WeReadSyncState {
    fn default() -> Self {
        Self {
            datasets: WeReadDatasets::default(),
            status: WeReadStatus::AuthMissing,
            last_attempt_at: None,
        }
    }
}

impl WeReadSyncState {
    pub fn from_legacy(state: WeReadState, coverage: DatasetCoverage, now: DateTime<Utc>) -> Self {
        let observed_at = state.last_refresh_ms.and_then(datetime_from_millis);
        let mut result = Self {
            datasets: WeReadDatasets {
                current_week: Dataset::from_legacy(state.weekly, observed_at, coverage, now),
                previous_week: Dataset::default(),
                current_month: Dataset::from_legacy(state.monthly, observed_at, coverage, now),
                shelf: Dataset::from_legacy(state.shelf, observed_at, coverage, now),
                notebooks: Dataset::from_legacy(state.notes, observed_at, coverage, now),
            },
            status: state.status,
            last_attempt_at: observed_at,
        };

        if let Some(message) = state.error {
            let issue = if message.starts_with(UPGRADE_REQUIRED_PREFIX) {
                SourceIssue::upgrade_required(message)
            } else {
                SourceIssue::new(
                    SourceIssueCode::GatewayError,
                    message,
                    RetryClassification::Retryable,
                )
            };
            result.datasets.current_week.issue = Some(issue);
        }
        result.status = result.derive_status();
        result
    }

    pub fn into_legacy(self) -> WeReadState {
        let error = self.primary_issue().map(|issue| issue.message.clone());
        let last_refresh_ms = self
            .datasets
            .current_week
            .observed_at
            .map(datetime_to_millis);

        WeReadState {
            weekly: self.datasets.current_week.value,
            monthly: self.datasets.current_month.value,
            shelf: self.datasets.shelf.value,
            notes: self.datasets.notebooks.value,
            status: self.status,
            last_refresh_ms,
            error,
        }
    }

    pub fn refresh_freshness_at(&mut self, now: DateTime<Utc>) {
        self.datasets.current_week.refresh_freshness_at(now);
        self.datasets
            .previous_week
            .refresh_freshness_at_with_stale_ms(now, PREVIOUS_WEEK_STALE_MS);
        self.datasets
            .current_month
            .refresh_freshness_at_with_stale_ms(now, MONTHLY_STALE_MS);
        self.datasets
            .shelf
            .refresh_freshness_at_with_stale_ms(now, SHELF_STALE_MS);
        self.datasets
            .notebooks
            .refresh_freshness_at_with_stale_ms(now, NOTEBOOKS_STALE_MS);
        self.status = self.derive_status();
    }

    pub fn derive_status(&self) -> WeReadStatus {
        let datasets = [
            dataset_health(&self.datasets.current_week),
            dataset_health(&self.datasets.previous_week),
            dataset_health(&self.datasets.current_month),
            dataset_health(&self.datasets.shelf),
            dataset_health(&self.datasets.notebooks),
        ];

        if datasets.iter().any(|health| health.upgrade_blocked) {
            return WeReadStatus::UpgradeRequired;
        }

        if datasets.iter().any(|health| health.auth_missing) {
            return WeReadStatus::AuthMissing;
        }

        let has_data = datasets.iter().any(|health| health.has_value);
        if !has_data {
            return if datasets.iter().any(|health| health.has_issue) {
                WeReadStatus::Error
            } else {
                WeReadStatus::AuthMissing
            };
        }

        if !datasets.iter().any(|health| health.fresh) {
            return WeReadStatus::Stale;
        }

        if datasets.iter().any(|health| {
            !health.has_value
                || !health.fresh
                || health.has_issue
                || health.coverage != DatasetCoverage::Complete
        }) {
            WeReadStatus::Partial
        } else {
            WeReadStatus::Fresh
        }
    }

    pub fn clear_obsolete_upgrade_issues(&mut self) {
        for issue in [
            &mut self.datasets.current_week.issue,
            &mut self.datasets.previous_week.issue,
            &mut self.datasets.current_month.issue,
            &mut self.datasets.shelf.issue,
            &mut self.datasets.notebooks.issue,
        ] {
            if issue.as_ref().is_some_and(|issue| {
                issue.code == SourceIssueCode::UpgradeRequired && !issue.blocks_current_skill()
            }) {
                *issue = None;
            }
        }
        self.status = self.derive_status();
    }

    pub fn blocks_current_skill_upgrade(&self) -> bool {
        [
            self.datasets.current_week.issue.as_ref(),
            self.datasets.previous_week.issue.as_ref(),
            self.datasets.current_month.issue.as_ref(),
            self.datasets.shelf.issue.as_ref(),
            self.datasets.notebooks.issue.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(SourceIssue::blocks_current_skill)
    }

    pub(crate) fn primary_issue(&self) -> Option<&SourceIssue> {
        let issues = [
            self.datasets.current_week.issue.as_ref(),
            self.datasets.previous_week.issue.as_ref(),
            self.datasets.current_month.issue.as_ref(),
            self.datasets.shelf.issue.as_ref(),
            self.datasets.notebooks.issue.as_ref(),
        ];

        if self.status == WeReadStatus::UpgradeRequired {
            if let Some(issue) = issues
                .iter()
                .copied()
                .flatten()
                .find(|issue| issue.blocks_current_skill())
            {
                return Some(issue);
            }
        }
        if self.status == WeReadStatus::AuthMissing {
            if let Some(issue) = issues
                .iter()
                .copied()
                .flatten()
                .find(|issue| issue.code == SourceIssueCode::AuthMissing)
            {
                return Some(issue);
            }
        }

        issues.into_iter().flatten().next()
    }

    pub fn mark_auth_missing(&mut self, now: DateTime<Utc>) {
        self.datasets.current_week.retain_with_issue(
            SourceIssue::new(
                SourceIssueCode::AuthMissing,
                "env.WEREAD_API_KEY is not configured",
                RetryClassification::AfterAuthentication,
            ),
            now,
        );
        self.last_attempt_at = Some(now);
        self.status = self.derive_status();
    }
}

#[derive(Clone, Copy)]
struct DatasetHealth {
    has_value: bool,
    fresh: bool,
    coverage: DatasetCoverage,
    has_issue: bool,
    auth_missing: bool,
    upgrade_blocked: bool,
}

fn dataset_health<T>(dataset: &Dataset<T>) -> DatasetHealth {
    DatasetHealth {
        has_value: dataset.value.is_some(),
        fresh: dataset.freshness == DatasetFreshness::Fresh,
        coverage: dataset.coverage,
        has_issue: dataset.issue.is_some(),
        auth_missing: dataset
            .issue
            .as_ref()
            .is_some_and(|issue| issue.code == SourceIssueCode::AuthMissing),
        upgrade_blocked: dataset
            .issue
            .as_ref()
            .is_some_and(SourceIssue::blocks_current_skill),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeReadStatus {
    AuthMissing,
    Loading,
    Fresh,
    Partial,
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
            WeReadStatus::Partial => "partial",
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
        if self.status == WeReadStatus::UpgradeRequired {
            return false;
        }
        if self.weekly.is_none() {
            return true;
        }
        match self.last_refresh_ms {
            Some(last) => now_ms.saturating_sub(last) > WEEKLY_STALE_MS,
            None => true,
        }
    }

    pub fn mark_error(&mut self, message: String) {
        self.error = Some(message);
        self.status = WeReadStatus::Error;
    }

    pub fn mark_sync_failure(&mut self, message: String) {
        if message.starts_with(UPGRADE_REQUIRED_PREFIX) {
            self.error = Some(message);
            self.status = WeReadStatus::UpgradeRequired;
        } else {
            self.mark_error(message);
        }
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
    #[serde(default, rename = "sampledNotebooks", alias = "topBooks")]
    pub top_books: Vec<WeReadNotebookSummary>,
}

impl WeReadNotesSummary {
    pub fn sampled_notebooks(&self) -> &[WeReadNotebookSummary] {
        &self.top_books
    }
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

pub fn datetime_from_millis(millis: u64) -> Option<DateTime<Utc>> {
    i64::try_from(millis)
        .ok()
        .and_then(|millis| Utc.timestamp_millis_opt(millis).single())
}

pub fn datetime_to_millis(datetime: DateTime<Utc>) -> u64 {
    u64::try_from(datetime.timestamp_millis()).unwrap_or(0)
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

    #[test]
    fn classifies_upgrade_required_sync_failures() {
        let mut state = WeReadState::empty(WeReadStatus::Fresh);

        state.mark_sync_failure(format!("{UPGRADE_REQUIRED_PREFIX} install 1.0.4"));

        assert_eq!(state.status, WeReadStatus::UpgradeRequired);
        assert_eq!(
            state.error.as_deref(),
            Some("WeRead skill upgrade required: install 1.0.4")
        );
    }

    #[test]
    fn auth_missing_keeps_cached_data_but_owns_source_status() {
        let now = Utc::now();
        let mut state = WeReadSyncState::default();
        state.datasets.shelf = Dataset::success(
            WeReadShelfSummary {
                books: 1,
                albums: 0,
                has_mp: false,
                visible_items: 1,
                private_items: 0,
                recent: Vec::new(),
            },
            now,
            DatasetCoverage::Complete,
        );

        state.mark_auth_missing(now);

        assert_eq!(state.status, WeReadStatus::AuthMissing);
        assert!(state.datasets.shelf.value.is_some());
    }

    #[test]
    fn mixed_fresh_and_stale_datasets_are_partial() {
        let now = Utc::now();
        let start = week_start_for(Local::now().date_naive());
        let weekly = |period_start| WeReadWeekly {
            period_start,
            period_end: period_start
                .checked_add_signed(ChronoDuration::days(6))
                .unwrap(),
            read_days: 1,
            total_seconds: 60,
            day_average_seconds: 8,
            compare_ratio: None,
            days: std::array::from_fn(|index| {
                WeReadDay::new(
                    period_start
                        .checked_add_signed(ChronoDuration::days(index as i64))
                        .unwrap(),
                    u32::from(index == 0) * 60,
                )
            }),
            focus: None,
        };
        let mut state = WeReadSyncState {
            datasets: WeReadDatasets {
                current_week: Dataset::success(
                    weekly(start),
                    now - ChronoDuration::minutes(16),
                    DatasetCoverage::Complete,
                ),
                previous_week: Dataset::success(
                    weekly(start - ChronoDuration::days(7)),
                    now,
                    DatasetCoverage::Complete,
                ),
                current_month: Dataset::success(
                    WeReadMonthly {
                        read_days: 1,
                        total_seconds: 60,
                        day_average_seconds: 60,
                        prefer_category_word: None,
                        categories: Vec::new(),
                    },
                    now,
                    DatasetCoverage::Complete,
                ),
                shelf: Dataset::success(
                    WeReadShelfSummary {
                        books: 0,
                        albums: 0,
                        has_mp: false,
                        visible_items: 0,
                        private_items: 0,
                        recent: Vec::new(),
                    },
                    now,
                    DatasetCoverage::Complete,
                ),
                notebooks: Dataset::success(
                    WeReadNotesSummary {
                        total_books: 0,
                        total_notes: 0,
                        top_books: Vec::new(),
                    },
                    now,
                    DatasetCoverage::Complete,
                ),
            },
            status: WeReadStatus::Fresh,
            last_attempt_at: Some(now),
        };

        state.refresh_freshness_at(now);

        assert_eq!(
            state.datasets.current_week.freshness,
            DatasetFreshness::Stale
        );
        assert_eq!(state.datasets.shelf.freshness, DatasetFreshness::Fresh);
        assert_eq!(state.status, WeReadStatus::Partial);
    }

    #[test]
    fn primary_issue_matches_blocking_status() {
        let mut state = WeReadSyncState::default();
        state.datasets.current_week.issue = Some(SourceIssue::new(
            SourceIssueCode::GatewayError,
            "gateway unavailable",
            RetryClassification::Retryable,
        ));
        state.datasets.current_month.issue = Some(SourceIssue::upgrade_required("upgrade"));
        state.status = state.derive_status();

        assert_eq!(state.status, WeReadStatus::UpgradeRequired);
        assert_eq!(
            state.primary_issue().map(|issue| issue.code),
            Some(SourceIssueCode::UpgradeRequired)
        );

        state.datasets.current_month.issue = None;
        state.datasets.shelf.issue = Some(SourceIssue::new(
            SourceIssueCode::AuthMissing,
            "authenticate",
            RetryClassification::AfterAuthentication,
        ));
        state.status = state.derive_status();

        assert_eq!(state.status, WeReadStatus::AuthMissing);
        assert_eq!(
            state.primary_issue().map(|issue| issue.code),
            Some(SourceIssueCode::AuthMissing)
        );
    }
}
