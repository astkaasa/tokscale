use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, Utc};
use serde_json::{json, Map, Value};

use super::cache;
use super::model::{
    local_date_from_unix_seconds, week_start_for, Dataset, DatasetCoverage, RetryClassification,
    SourceIssue, SourceIssueCode, WeReadBookRef, WeReadCategory, WeReadDay, WeReadFocusBook,
    WeReadMonthly, WeReadNotebookSummary, WeReadNotesSummary, WeReadShelfSummary, WeReadState,
    WeReadSyncState, WeReadWeekly, SKILL_VERSION, UPGRADE_REQUIRED_PREFIX,
};

const GATEWAY_URL: &str = "https://i.weread.qq.com/api/agent/gateway";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const NOTEBOOK_PAGE_SIZE: u64 = 20;

type GatewayResult = std::result::Result<Value, SourceIssue>;
type GatewayFuture<'a> = Pin<Box<dyn Future<Output = GatewayResult> + Send + 'a>>;

trait GatewayTransport: Sync {
    fn send<'a>(&'a self, api_key: &'a str, body: Value) -> GatewayFuture<'a>;
}

struct HttpGateway {
    client: reqwest::Client,
}

impl HttpGateway {
    fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .context("failed to build WeRead HTTP client")?,
        })
    }
}

impl GatewayTransport for HttpGateway {
    fn send<'a>(&'a self, api_key: &'a str, body: Value) -> GatewayFuture<'a> {
        Box::pin(send_request(&self.client, api_key, body))
    }
}

pub fn fetch_current(api_key: &str) -> Result<WeReadState> {
    sync_current(api_key).map(WeReadSyncState::into_legacy)
}

pub fn sync_current(api_key: &str) -> Result<WeReadSyncState> {
    let state = sync_current_unpersisted(api_key)?;
    cache::save_sync(&state).context("failed to persist WeRead cache")?;
    Ok(state)
}

pub fn sync_current_unpersisted(api_key: &str) -> Result<WeReadSyncState> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(anyhow!("WEREAD_API_KEY is not set"));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start WeRead runtime")?;

    let gateway = HttpGateway::new()?;
    let previous = cache::load_sync().unwrap_or_default();
    if previous.blocks_current_skill_upgrade() {
        return Ok(previous);
    }
    Ok(runtime.block_on(sync_with_gateway(&gateway, api_key, previous, Utc::now())))
}

async fn sync_with_gateway<G: GatewayTransport>(
    gateway: &G,
    api_key: &str,
    mut previous: WeReadSyncState,
    observed_at: DateTime<Utc>,
) -> WeReadSyncState {
    let current_body = request_body("/readdata/detail", &[("mode", json!("weekly"))]);
    let current_raw = match gateway.send(api_key, current_body).await {
        Ok(value) => value,
        Err(issue) => {
            previous
                .datasets
                .current_week
                .retain_with_issue(issue, observed_at);
            previous.last_attempt_at = Some(observed_at);
            previous.refresh_freshness_at(observed_at);
            return previous;
        }
    };
    let current_week = match normalize_weekly_endpoint(&current_raw) {
        Ok(value) => value,
        Err(issue) => {
            previous
                .datasets
                .current_week
                .retain_with_issue(issue, observed_at);
            previous.last_attempt_at = Some(observed_at);
            previous.refresh_freshness_at(observed_at);
            return previous;
        }
    };

    let previous_base_time = current_raw
        .get("baseTime")
        .and_then(Value::as_i64)
        .map(|base| base.saturating_sub(7 * 24 * 60 * 60));
    let mut previous_params = vec![("mode", json!("weekly"))];
    if let Some(base_time) = previous_base_time {
        previous_params.push(("baseTime", json!(base_time)));
    }

    let previous_body = request_body("/readdata/detail", &previous_params);
    let month_body = request_body("/readdata/detail", &[("mode", json!("monthly"))]);
    let shelf_body = request_body("/shelf/sync", &[]);
    let notebooks_body = request_body("/user/notebooks", &[("count", json!(NOTEBOOK_PAGE_SIZE))]);

    let (previous_result, month_result, shelf_result, notebooks_result) = tokio::join!(
        gateway.send(api_key, previous_body),
        gateway.send(api_key, month_body),
        gateway.send(api_key, shelf_body),
        gateway.send(api_key, notebooks_body),
    );

    let previous_result = previous_result.and_then(|value| normalize_weekly_endpoint(&value));
    let month_result = month_result.and_then(|value| normalize_monthly_endpoint(&value));
    let shelf_result = shelf_result.and_then(|value| normalize_shelf_endpoint(&value));
    let notebooks_result = notebooks_result.and_then(|value| normalize_notes_endpoint(&value));

    let upgrade_blocked = [
        previous_result.as_ref().err(),
        month_result.as_ref().err(),
        shelf_result.as_ref().err(),
        notebooks_result.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    .any(SourceIssue::blocks_current_skill);

    if upgrade_blocked {
        retain_result_issue(
            &mut previous.datasets.previous_week,
            &previous_result,
            observed_at,
        );
        retain_result_issue(
            &mut previous.datasets.current_month,
            &month_result,
            observed_at,
        );
        retain_result_issue(&mut previous.datasets.shelf, &shelf_result, observed_at);
        retain_result_issue(
            &mut previous.datasets.notebooks,
            &notebooks_result,
            observed_at,
        );
        previous.last_attempt_at = Some(observed_at);
        previous.refresh_freshness_at(observed_at);
        return previous;
    }

    previous.datasets.current_week =
        Dataset::success(current_week, observed_at, DatasetCoverage::Complete);
    merge_result(
        &mut previous.datasets.previous_week,
        previous_result.map(|value| (value, DatasetCoverage::Complete)),
        observed_at,
    );
    merge_result(
        &mut previous.datasets.current_month,
        month_result.map(|value| (value, DatasetCoverage::Complete)),
        observed_at,
    );
    merge_result(
        &mut previous.datasets.shelf,
        shelf_result.map(|value| (value, DatasetCoverage::Complete)),
        observed_at,
    );
    merge_result(
        &mut previous.datasets.notebooks,
        notebooks_result,
        observed_at,
    );
    previous.last_attempt_at = Some(observed_at);
    previous.refresh_freshness_at(observed_at);
    previous
}

fn request_body(api_name: &str, params: &[(&str, Value)]) -> Value {
    let mut body = Map::new();
    body.insert("api_name".to_string(), Value::String(api_name.to_string()));
    body.insert(
        "skill_version".to_string(),
        Value::String(SKILL_VERSION.to_string()),
    );
    for (key, value) in params {
        body.insert((*key).to_string(), value.clone());
    }
    Value::Object(body)
}

async fn send_request(client: &reqwest::Client, api_key: &str, body: Value) -> GatewayResult {
    let api_name = body
        .get("api_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown endpoint")
        .to_string();

    let response = client
        .post(GATEWAY_URL)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            issue(
                SourceIssueCode::TransportError,
                format!("WeRead request failed for {api_name}: {error}"),
                RetryClassification::Retryable,
                api_key,
            )
        })?;

    let status = response.status();
    if let Some(issue) = http_auth_issue(status, &api_name, api_key) {
        return Err(issue);
    }
    let value: Value = response.json().await.map_err(|_| {
        issue(
            SourceIssueCode::InvalidResponse,
            format!("WeRead returned invalid JSON for {api_name}"),
            RetryClassification::Retryable,
            api_key,
        )
    })?;

    if let Some(message) = upgrade_message(&value) {
        return Err(SourceIssue::upgrade_required(sanitize_message(
            &format!("{UPGRADE_REQUIRED_PREFIX} {message}"),
            api_key,
        )));
    }

    if !status.is_success() {
        return Err(issue(
            SourceIssueCode::GatewayError,
            format!(
                "WeRead gateway returned HTTP {} for {api_name}",
                status.as_u16()
            ),
            RetryClassification::Retryable,
            api_key,
        ));
    }

    if let Some(errcode) = value.get("errcode").and_then(Value::as_i64) {
        if errcode != 0 {
            let message = value
                .get("errmsg")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown gateway error");
            return Err(issue(
                SourceIssueCode::GatewayError,
                format!("WeRead gateway error {errcode} for {api_name}: {message}"),
                RetryClassification::Retryable,
                api_key,
            ));
        }
    }

    Ok(value.get("data").cloned().unwrap_or(value))
}

fn http_auth_issue(
    status: reqwest::StatusCode,
    api_name: &str,
    api_key: &str,
) -> Option<SourceIssue> {
    if status != reqwest::StatusCode::UNAUTHORIZED && status != reqwest::StatusCode::FORBIDDEN {
        return None;
    }

    Some(issue(
        SourceIssueCode::AuthMissing,
        format!(
            "WeRead authentication failed with HTTP {} for {api_name}",
            status.as_u16()
        ),
        RetryClassification::AfterAuthentication,
        api_key,
    ))
}

fn upgrade_message(value: &Value) -> Option<String> {
    let info = find_upgrade_info(value)?;
    info.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some("install the current WeRead connector".to_string()))
}

fn find_upgrade_info(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => {
            if let Some(info) = object.get("upgrade_info").filter(|info| !info.is_null()) {
                return Some(info);
            }
            object.values().find_map(find_upgrade_info)
        }
        Value::Array(values) => values.iter().find_map(find_upgrade_info),
        _ => None,
    }
}

fn issue(
    code: SourceIssueCode,
    message: String,
    retry: RetryClassification,
    secret: &str,
) -> SourceIssue {
    SourceIssue::new(code, sanitize_message(&message, secret), retry)
}

fn sanitize_message(message: &str, secret: &str) -> String {
    let redacted = if secret.trim().is_empty() {
        message.to_string()
    } else {
        message.replace(secret.trim(), "[redacted]")
    };
    let compact = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(240).collect()
}

#[derive(Clone, Copy)]
enum ResponseFieldKind {
    UnixSeconds,
    UnsignedInteger,
    Object,
    Array,
    BooleanFlag,
}

impl ResponseFieldKind {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::UnixSeconds => value
                .as_i64()
                .and_then(local_date_from_unix_seconds)
                .is_some(),
            Self::UnsignedInteger => value
                .as_u64()
                .is_some_and(|value| u32::try_from(value).is_ok()),
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::BooleanFlag => value.is_boolean() || matches!(value.as_i64(), Some(0 | 1)),
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::UnixSeconds => "a valid Unix timestamp",
            Self::UnsignedInteger => "an unsigned 32-bit integer",
            Self::Object => "an object",
            Self::Array => "an array",
            Self::BooleanFlag => "a boolean or 0/1 integer",
        }
    }
}

fn validate_required_fields(
    value: &Value,
    endpoint: &str,
    fields: &[(&str, ResponseFieldKind)],
) -> std::result::Result<(), SourceIssue> {
    let Some(object) = value.as_object() else {
        return Err(SourceIssue::new(
            SourceIssueCode::InvalidResponse,
            format!("WeRead {endpoint} response must be an object"),
            RetryClassification::Retryable,
        ));
    };

    for (field, kind) in fields {
        let Some(field_value) = object.get(*field) else {
            return Err(SourceIssue::new(
                SourceIssueCode::InvalidResponse,
                format!("WeRead {endpoint} response is missing required field `{field}`"),
                RetryClassification::Retryable,
            ));
        };
        if !kind.accepts(field_value) {
            return Err(SourceIssue::new(
                SourceIssueCode::NormalizationError,
                format!(
                    "WeRead {endpoint} response field `{field}` must be {}",
                    kind.expected()
                ),
                RetryClassification::Manual,
            ));
        }
    }

    Ok(())
}

fn validate_optional_field(
    value: &Value,
    endpoint: &str,
    field: &str,
    kind: ResponseFieldKind,
) -> std::result::Result<(), SourceIssue> {
    if value.get(field).is_some_and(|value| !kind.accepts(value)) {
        return Err(SourceIssue::new(
            SourceIssueCode::NormalizationError,
            format!(
                "WeRead {endpoint} response field `{field}` must be {}",
                kind.expected()
            ),
            RetryClassification::Manual,
        ));
    }
    Ok(())
}

fn normalize_weekly_endpoint(value: &Value) -> std::result::Result<WeReadWeekly, SourceIssue> {
    validate_required_fields(
        value,
        "weekly",
        &[
            ("baseTime", ResponseFieldKind::UnixSeconds),
            ("readDays", ResponseFieldKind::UnsignedInteger),
            ("totalReadTime", ResponseFieldKind::UnsignedInteger),
            ("dayAverageReadTime", ResponseFieldKind::UnsignedInteger),
        ],
    )?;
    validate_optional_field(value, "weekly", "readTimes", ResponseFieldKind::Object)?;
    normalize_weekly(value).map_err(|error| {
        SourceIssue::new(
            SourceIssueCode::NormalizationError,
            format!("WeRead weekly response could not be normalized: {error}"),
            RetryClassification::Manual,
        )
    })
}

fn normalize_monthly_endpoint(value: &Value) -> std::result::Result<WeReadMonthly, SourceIssue> {
    validate_required_fields(
        value,
        "monthly",
        &[
            ("readDays", ResponseFieldKind::UnsignedInteger),
            ("totalReadTime", ResponseFieldKind::UnsignedInteger),
            ("dayAverageReadTime", ResponseFieldKind::UnsignedInteger),
        ],
    )?;
    Ok(normalize_monthly(value))
}

fn normalize_shelf_endpoint(value: &Value) -> std::result::Result<WeReadShelfSummary, SourceIssue> {
    validate_required_fields(
        value,
        "shelf",
        &[
            ("books", ResponseFieldKind::Array),
            ("albums", ResponseFieldKind::Array),
        ],
    )?;
    Ok(normalize_shelf(value))
}

fn normalize_notes_endpoint(
    value: &Value,
) -> std::result::Result<(WeReadNotesSummary, DatasetCoverage), SourceIssue> {
    validate_required_fields(
        value,
        "notebooks",
        &[
            ("totalBookCount", ResponseFieldKind::UnsignedInteger),
            ("totalNoteCount", ResponseFieldKind::UnsignedInteger),
            ("hasMore", ResponseFieldKind::BooleanFlag),
            ("books", ResponseFieldKind::Array),
        ],
    )?;
    let coverage = if bool_field(value, "hasMore") {
        DatasetCoverage::Partial
    } else {
        DatasetCoverage::Complete
    };
    Ok((normalize_notes(value), coverage))
}

fn merge_result<T>(
    dataset: &mut Dataset<T>,
    result: std::result::Result<(T, DatasetCoverage), SourceIssue>,
    observed_at: DateTime<Utc>,
) {
    match result {
        Ok((value, coverage)) => *dataset = Dataset::success(value, observed_at, coverage),
        Err(issue) => dataset.retain_with_issue(issue, observed_at),
    }
}

fn retain_result_issue<T, U>(
    dataset: &mut Dataset<T>,
    result: &std::result::Result<U, SourceIssue>,
    observed_at: DateTime<Utc>,
) {
    if let Err(issue) = result {
        dataset.retain_with_issue(issue.clone(), observed_at);
    }
}

pub fn normalize_weekly(value: &Value) -> Result<WeReadWeekly> {
    let today = Local::now().date_naive();
    let period_start = value
        .get("baseTime")
        .and_then(Value::as_i64)
        .and_then(local_date_from_unix_seconds)
        .unwrap_or_else(|| week_start_for(today));
    let period_end = period_start
        .checked_add_signed(ChronoDuration::days(6))
        .unwrap_or(period_start);

    let mut seconds_by_date = BTreeMap::<NaiveDate, u32>::new();
    if let Some(read_times) = value.get("readTimes").and_then(Value::as_object) {
        for (timestamp, seconds) in read_times {
            let Some(timestamp) = timestamp.parse::<i64>().ok() else {
                continue;
            };
            let Some(date) = local_date_from_unix_seconds(timestamp) else {
                continue;
            };
            let seconds = value_u32(seconds);
            seconds_by_date.insert(date, seconds);
        }
    }

    let days: Vec<WeReadDay> = (0..7)
        .filter_map(|offset| period_start.checked_add_signed(ChronoDuration::days(offset)))
        .map(|date| WeReadDay::new(date, *seconds_by_date.get(&date).unwrap_or(&0)))
        .collect();
    let days: [WeReadDay; 7] = days
        .try_into()
        .map_err(|_| anyhow!("failed to build weekly WeRead buckets"))?;

    Ok(WeReadWeekly {
        period_start,
        period_end,
        read_days: value_u32_field(value, "readDays") as u8,
        total_seconds: value_u32_field(value, "totalReadTime"),
        day_average_seconds: value_u32_field(value, "dayAverageReadTime"),
        compare_ratio: value.get("compare").and_then(Value::as_f64),
        focus: focus_book(value),
        days,
    })
}

pub fn normalize_monthly(value: &Value) -> WeReadMonthly {
    let mut categories = value
        .get("preferCategory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|category| {
            let title = category
                .get("categoryTitle")
                .or_else(|| category.get("parentCategoryTitle"))
                .and_then(Value::as_str)?
                .trim();
            if title.is_empty() {
                return None;
            }
            Some(WeReadCategory {
                title: title.to_string(),
                reading_seconds: value_u32_field(category, "readingTime"),
                reading_count: value_u32_field(category, "readingCount"),
                weight: category.get("val").and_then(Value::as_f64).unwrap_or(0.0),
            })
        })
        .take(5)
        .collect::<Vec<_>>();

    let max_reading_seconds = categories
        .iter()
        .map(|category| category.reading_seconds)
        .max()
        .unwrap_or(0);
    if max_reading_seconds > 0 {
        for category in &mut categories {
            if category.weight <= 0.0 {
                category.weight = category.reading_seconds as f64 / max_reading_seconds as f64;
            }
        }
    }

    WeReadMonthly {
        read_days: value_u32_field(value, "readDays") as u16,
        total_seconds: value_u32_field(value, "totalReadTime"),
        day_average_seconds: value_u32_field(value, "dayAverageReadTime"),
        prefer_category_word: value
            .get("preferCategoryWord")
            .and_then(Value::as_str)
            .map(str::to_string),
        categories,
    }
}

pub fn normalize_shelf(value: &Value) -> WeReadShelfSummary {
    let books = value
        .get("books")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let albums = value
        .get("albums")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_mp = value.get("mp").is_some_and(|mp| !mp.is_null());

    let book_count = books.len() as u32;
    let album_count = albums.len() as u32;
    let visible_items = book_count + album_count + u32::from(has_mp);
    let private_books = books
        .iter()
        .filter(|book| value_u32_field(book, "secret") == 1)
        .count() as u32;
    let private_albums = albums
        .iter()
        .filter(|album| {
            album
                .get("albumInfoExtra")
                .is_some_and(|extra| value_u32_field(extra, "secret") == 1)
        })
        .count() as u32;
    let private_items = private_books + private_albums + u32::from(has_mp);

    let mut recent = books
        .iter()
        .filter_map(book_ref_from_shelf_book)
        .chain(albums.iter().filter_map(book_ref_from_album))
        .collect::<Vec<_>>();
    recent.sort_by(|a, b| b.last_read_time.cmp(&a.last_read_time));
    recent.truncate(5);

    WeReadShelfSummary {
        books: book_count,
        albums: album_count,
        has_mp,
        visible_items,
        private_items,
        recent,
    }
}

pub fn normalize_notes(value: &Value) -> WeReadNotesSummary {
    let books = value
        .get("books")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(notebook_summary)
        .collect::<Vec<_>>();
    let combined_total_notes = books
        .iter()
        .fold(0u32, |total, book| total.saturating_add(book.total_notes));

    WeReadNotesSummary {
        total_books: value_u32_field(value, "totalBookCount"),
        total_notes: value_u32_field(value, "totalNoteCount").max(combined_total_notes),
        top_books: books,
    }
}

fn focus_book(value: &Value) -> Option<WeReadFocusBook> {
    let item = value
        .get("readLongest")
        .and_then(Value::as_array)
        .and_then(|items| items.first())?;
    let read_seconds = value_u32_field(item, "readTime");

    if let Some(book) = item.get("book") {
        return Some(WeReadFocusBook {
            id: value_string_field(book, "bookId")?,
            title: value_string_field(book, "title")?,
            author: value_string_field(book, "author"),
            read_seconds,
        });
    }

    let album = item.get("albumInfo")?;
    Some(WeReadFocusBook {
        id: value_string_field(album, "albumId")?,
        title: value_string_field(album, "name")?,
        author: value_string_field(album, "authorName"),
        read_seconds,
    })
}

fn book_ref_from_shelf_book(value: &Value) -> Option<WeReadBookRef> {
    Some(WeReadBookRef {
        id: value_string_field(value, "bookId")?,
        title: value_string_field(value, "title")?,
        author: value_string_field(value, "author"),
        last_read_time: value.get("readUpdateTime").and_then(Value::as_i64),
    })
}

fn book_ref_from_album(value: &Value) -> Option<WeReadBookRef> {
    let info = value.get("albumInfo")?;
    let extra = value.get("albumInfoExtra");
    Some(WeReadBookRef {
        id: value_string_field(info, "albumId")?,
        title: value_string_field(info, "name")?,
        author: value_string_field(info, "authorName"),
        last_read_time: extra
            .and_then(|v| v.get("lectureReadUpdateTime"))
            .and_then(Value::as_i64)
            .or_else(|| info.get("updateTime").and_then(Value::as_i64)),
    })
}

fn notebook_summary(value: &Value) -> Option<WeReadNotebookSummary> {
    let book = value.get("book").unwrap_or(value);
    let review_count = value_u32_field(value, "reviewCount");
    let note_count = value_u32_field(value, "noteCount");
    let bookmark_count = value_u32_field(value, "bookmarkCount");
    Some(WeReadNotebookSummary {
        id: value_string_field(value, "bookId").or_else(|| value_string_field(book, "bookId"))?,
        title: value_string_field(book, "title")?,
        author: value_string_field(book, "author"),
        total_notes: review_count
            .saturating_add(note_count)
            .saturating_add(bookmark_count),
        review_count,
        note_count,
        bookmark_count,
    })
}

fn value_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn value_u32_field(value: &Value, field: &str) -> u32 {
    value.get(field).map(value_u32).unwrap_or(0)
}

fn value_u32(value: &Value) -> u32 {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

fn bool_field(value: &Value, field: &str) -> bool {
    value.get(field).is_some_and(|value| {
        value.as_bool().unwrap_or(false) || value.as_i64().is_some_and(|value| value != 0)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use chrono::TimeZone;

    use super::super::model::WeReadStatus;
    use super::*;

    #[derive(Default)]
    struct FakeGateway {
        responses: Mutex<HashMap<String, GatewayResult>>,
        bodies: Mutex<Vec<Value>>,
    }

    impl FakeGateway {
        fn with(self, key: &str, response: GatewayResult) -> Self {
            self.responses
                .lock()
                .unwrap()
                .insert(key.to_string(), response);
            self
        }

        fn bodies(&self) -> Vec<Value> {
            self.bodies.lock().unwrap().clone()
        }
    }

    impl GatewayTransport for FakeGateway {
        fn send<'a>(&'a self, _api_key: &'a str, body: Value) -> GatewayFuture<'a> {
            let key = request_key(&body);
            self.bodies.lock().unwrap().push(body);
            let response = self
                .responses
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or_else(|| {
                    Err(SourceIssue::new(
                        SourceIssueCode::GatewayError,
                        format!("missing fake response for {key}"),
                        RetryClassification::Manual,
                    ))
                });
            Box::pin(async move { response })
        }
    }

    fn request_key(body: &Value) -> String {
        match body.get("api_name").and_then(Value::as_str) {
            Some("/readdata/detail") => match body.get("mode").and_then(Value::as_str) {
                Some("monthly") => "month".to_string(),
                Some("weekly") if body.get("baseTime").is_some() => "previous".to_string(),
                Some("weekly") => "current".to_string(),
                _ => "readdata".to_string(),
            },
            Some("/shelf/sync") => "shelf".to_string(),
            Some("/user/notebooks") => "notebooks".to_string(),
            other => other.unwrap_or("unknown").to_string(),
        }
    }

    fn weekly_value(total_seconds: u32, read_days: u8) -> Value {
        json!({
            "baseTime": 1780934400,
            "readTimes": {"1780934400": total_seconds},
            "readDays": read_days,
            "totalReadTime": total_seconds,
            "dayAverageReadTime": total_seconds / 7
        })
    }

    fn month_value(total_seconds: u32) -> Value {
        json!({
            "readDays": 4,
            "totalReadTime": total_seconds,
            "dayAverageReadTime": total_seconds / 30
        })
    }

    fn shelf_value() -> Value {
        json!({"books": [], "albums": []})
    }

    fn notebooks_value(has_more: bool) -> Value {
        json!({
            "totalBookCount": 0,
            "totalNoteCount": 0,
            "hasMore": i32::from(has_more),
            "books": []
        })
    }

    fn observed_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap()
    }

    fn assert_response_issue<T: std::fmt::Debug>(
        result: std::result::Result<T, SourceIssue>,
        code: SourceIssueCode,
        field: &str,
    ) {
        let issue = result.unwrap_err();
        assert_eq!(issue.code, code);
        assert!(issue.message.contains(field));
        assert!(!issue.message.contains("sensitive-response-value"));
    }

    #[test]
    fn unauthorized_http_statuses_require_authentication() {
        for status in [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
        ] {
            let issue = http_auth_issue(status, "/user/notebooks", "secret").unwrap();

            assert_eq!(issue.code, SourceIssueCode::AuthMissing);
            assert_eq!(issue.retry, RetryClassification::AfterAuthentication);
        }
        assert!(http_auth_issue(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "/user/notebooks",
            "secret"
        )
        .is_none());
    }

    #[test]
    fn endpoint_normalizers_accept_minimal_zero_value_responses() {
        assert!(normalize_weekly_endpoint(&weekly_value(0, 0)).is_ok());
        assert!(normalize_monthly_endpoint(&json!({
            "readDays": 0,
            "totalReadTime": 0,
            "dayAverageReadTime": 0
        }))
        .is_ok());
        assert!(normalize_shelf_endpoint(&shelf_value()).is_ok());
        assert!(normalize_notes_endpoint(&notebooks_value(false)).is_ok());
    }

    #[test]
    fn endpoint_normalizers_reject_empty_objects_and_missing_required_fields() {
        assert_response_issue(
            normalize_weekly_endpoint(&json!({})),
            SourceIssueCode::InvalidResponse,
            "baseTime",
        );
        assert_response_issue(
            normalize_monthly_endpoint(&json!({})),
            SourceIssueCode::InvalidResponse,
            "readDays",
        );
        assert_response_issue(
            normalize_shelf_endpoint(&json!({})),
            SourceIssueCode::InvalidResponse,
            "books",
        );
        assert_response_issue(
            normalize_notes_endpoint(&json!({})),
            SourceIssueCode::InvalidResponse,
            "totalBookCount",
        );
        for (endpoint, issue) in [
            ("weekly", normalize_weekly_endpoint(&json!([])).unwrap_err()),
            (
                "monthly",
                normalize_monthly_endpoint(&json!([])).unwrap_err(),
            ),
            ("shelf", normalize_shelf_endpoint(&json!([])).unwrap_err()),
            (
                "notebooks",
                normalize_notes_endpoint(&json!([])).unwrap_err(),
            ),
        ] {
            assert_eq!(issue.code, SourceIssueCode::InvalidResponse);
            assert!(issue.message.contains(endpoint));
        }

        for field in [
            "baseTime",
            "readDays",
            "totalReadTime",
            "dayAverageReadTime",
        ] {
            let mut value = weekly_value(0, 0);
            value.as_object_mut().unwrap().remove(field);
            assert_response_issue(
                normalize_weekly_endpoint(&value),
                SourceIssueCode::InvalidResponse,
                field,
            );
        }
        for field in ["readDays", "totalReadTime", "dayAverageReadTime"] {
            let mut value = month_value(0);
            value.as_object_mut().unwrap().remove(field);
            assert_response_issue(
                normalize_monthly_endpoint(&value),
                SourceIssueCode::InvalidResponse,
                field,
            );
        }
        for field in ["books", "albums"] {
            let mut value = shelf_value();
            value.as_object_mut().unwrap().remove(field);
            assert_response_issue(
                normalize_shelf_endpoint(&value),
                SourceIssueCode::InvalidResponse,
                field,
            );
        }
        for field in ["totalBookCount", "totalNoteCount", "hasMore", "books"] {
            let mut value = notebooks_value(false);
            value.as_object_mut().unwrap().remove(field);
            assert_response_issue(
                normalize_notes_endpoint(&value),
                SourceIssueCode::InvalidResponse,
                field,
            );
        }
    }

    #[test]
    fn endpoint_normalizers_reject_invalid_required_field_types_without_echoing_values() {
        for field in [
            "baseTime",
            "readTimes",
            "readDays",
            "totalReadTime",
            "dayAverageReadTime",
        ] {
            let mut value = weekly_value(0, 0);
            value[field] = json!("sensitive-response-value");
            assert_response_issue(
                normalize_weekly_endpoint(&value),
                SourceIssueCode::NormalizationError,
                field,
            );
        }
        for field in ["readDays", "totalReadTime", "dayAverageReadTime"] {
            let mut value = month_value(0);
            value[field] = json!("sensitive-response-value");
            assert_response_issue(
                normalize_monthly_endpoint(&value),
                SourceIssueCode::NormalizationError,
                field,
            );
        }
        for field in ["books", "albums"] {
            let mut value = shelf_value();
            value[field] = json!("sensitive-response-value");
            assert_response_issue(
                normalize_shelf_endpoint(&value),
                SourceIssueCode::NormalizationError,
                field,
            );
        }
        for field in ["totalBookCount", "totalNoteCount", "hasMore", "books"] {
            let mut value = notebooks_value(false);
            value[field] = json!("sensitive-response-value");
            assert_response_issue(
                normalize_notes_endpoint(&value),
                SourceIssueCode::NormalizationError,
                field,
            );
        }
    }

    #[tokio::test]
    async fn partial_sync_keeps_last_known_dataset_and_flat_request_contract() {
        let old_month = normalize_monthly(&month_value(900));
        let mut previous = WeReadSyncState::default();
        previous.datasets.current_month = Dataset::success(
            old_month,
            observed_at() - ChronoDuration::minutes(5),
            DatasetCoverage::Complete,
        );
        let gateway = FakeGateway::default()
            .with("current", Ok(weekly_value(1_400, 3)))
            .with("previous", Ok(weekly_value(1_000, 2)))
            .with(
                "month",
                Err(SourceIssue::new(
                    SourceIssueCode::TransportError,
                    "month unavailable",
                    RetryClassification::Retryable,
                )),
            )
            .with("shelf", Ok(shelf_value()))
            .with("notebooks", Ok(notebooks_value(true)));

        let state = sync_with_gateway(&gateway, "secret", previous, observed_at()).await;

        assert_eq!(state.status, WeReadStatus::Partial);
        assert_eq!(
            state
                .datasets
                .current_week
                .value
                .as_ref()
                .unwrap()
                .total_seconds,
            1_400
        );
        assert_eq!(
            state
                .datasets
                .current_month
                .value
                .as_ref()
                .unwrap()
                .total_seconds,
            900
        );
        assert_eq!(state.datasets.notebooks.coverage, DatasetCoverage::Partial);
        for body in gateway.bodies() {
            assert_eq!(body["skill_version"], SKILL_VERSION);
            assert!(body.get("params").is_none());
        }
    }

    #[tokio::test]
    async fn malformed_secondary_responses_keep_last_known_datasets_and_mark_partial() {
        let old_month = normalize_monthly(&month_value(900));
        let old_shelf = normalize_shelf(&json!({
            "books": [{
                "bookId": "saved-book",
                "title": "Saved",
                "readUpdateTime": 10
            }],
            "albums": []
        }));
        let old_notebooks = normalize_notes(&json!({
            "totalBookCount": 1,
            "totalNoteCount": 2,
            "books": [{
                "bookId": "saved-book",
                "book": {"title": "Saved"},
                "reviewCount": 1,
                "noteCount": 1,
                "bookmarkCount": 0
            }]
        }));
        let retained_at = observed_at() - ChronoDuration::minutes(5);
        let mut previous = WeReadSyncState::default();
        previous.datasets.current_month =
            Dataset::success(old_month.clone(), retained_at, DatasetCoverage::Complete);
        previous.datasets.shelf =
            Dataset::success(old_shelf.clone(), retained_at, DatasetCoverage::Complete);
        previous.datasets.notebooks = Dataset::success(
            old_notebooks.clone(),
            retained_at,
            DatasetCoverage::Complete,
        );
        let gateway = FakeGateway::default()
            .with("current", Ok(weekly_value(1_400, 3)))
            .with("previous", Ok(weekly_value(1_000, 2)))
            .with("month", Ok(json!({})))
            .with("shelf", Ok(json!({})))
            .with("notebooks", Ok(json!({})));

        let state = sync_with_gateway(&gateway, "secret", previous, observed_at()).await;

        assert_eq!(state.status, WeReadStatus::Partial);
        assert_eq!(
            state.datasets.current_month.value.as_ref(),
            Some(&old_month)
        );
        assert_eq!(state.datasets.shelf.value.as_ref(), Some(&old_shelf));
        assert_eq!(
            state.datasets.notebooks.value.as_ref(),
            Some(&old_notebooks)
        );
        for observed_at in [
            state.datasets.current_month.observed_at,
            state.datasets.shelf.observed_at,
            state.datasets.notebooks.observed_at,
        ] {
            assert_eq!(observed_at, Some(retained_at));
        }
        for issue in [
            state.datasets.current_month.issue.as_ref(),
            state.datasets.shelf.issue.as_ref(),
            state.datasets.notebooks.issue.as_ref(),
        ] {
            assert_eq!(issue.unwrap().code, SourceIssueCode::InvalidResponse);
        }
    }

    #[tokio::test]
    async fn concurrent_upgrade_discards_all_new_values() {
        let mut previous = WeReadSyncState::default();
        previous.datasets.current_week = Dataset::success(
            normalize_weekly(&weekly_value(700, 1)).unwrap(),
            observed_at() - ChronoDuration::minutes(5),
            DatasetCoverage::Complete,
        );
        let gateway = FakeGateway::default()
            .with("current", Ok(weekly_value(1_400, 3)))
            .with("previous", Ok(weekly_value(1_000, 2)))
            .with("month", Err(SourceIssue::upgrade_required("install 1.0.5")))
            .with("shelf", Ok(shelf_value()))
            .with("notebooks", Ok(notebooks_value(false)));

        let state = sync_with_gateway(&gateway, "secret", previous, observed_at()).await;

        assert_eq!(state.status, WeReadStatus::UpgradeRequired);
        assert_eq!(
            state
                .datasets
                .current_week
                .value
                .as_ref()
                .unwrap()
                .total_seconds,
            700
        );
        assert!(state.datasets.shelf.value.is_none());
        assert!(state.blocks_current_skill_upgrade());
    }

    #[tokio::test]
    async fn current_gate_upgrade_stops_before_follow_up_requests() {
        let gateway = FakeGateway::default().with(
            "current",
            Err(SourceIssue::upgrade_required("install 1.0.5")),
        );

        let state = sync_with_gateway(
            &gateway,
            "secret",
            WeReadSyncState::default(),
            observed_at(),
        )
        .await;

        assert_eq!(gateway.bodies().len(), 1);
        assert_eq!(state.status, WeReadStatus::UpgradeRequired);
    }

    #[test]
    fn normalizes_weekly_read_times_to_local_days() {
        let value = json!({
            "baseTime": 1780934400,
            "readTimes": {
                "1780934400": 6089,
                "1781020800": 8813
            },
            "readDays": 2,
            "totalReadTime": 14902,
            "dayAverageReadTime": 3725,
            "compare": 0.35,
            "readLongest": [{
                "book": {"bookId": "b1", "title": "Focus", "author": "A"},
                "readTime": 3600
            }]
        });

        let weekly = normalize_weekly(&value).unwrap();

        assert_eq!(weekly.read_days, 2);
        assert_eq!(weekly.total_seconds, 14902);
        assert_eq!(weekly.days[0].read_seconds, 6089);
        assert!(weekly.days[0].checked_in);
        assert_eq!(weekly.days[1].read_seconds, 8813);
        assert_eq!(weekly.days[2].read_seconds, 0);
        assert_eq!(weekly.focus.as_ref().unwrap().title, "Focus");
    }

    #[test]
    fn shelf_total_counts_books_albums_and_mp() {
        let value = json!({
            "books": [
                {"bookId": "1", "title": "A", "author": "a", "secret": 0, "readUpdateTime": 2},
                {"bookId": "2", "title": "B", "author": "b", "secret": 1, "readUpdateTime": 3}
            ],
            "albums": [{
                "albumInfo": {"albumId": "a1", "name": "Audio", "authorName": "n", "updateTime": 1},
                "albumInfoExtra": {"secret": 1, "lectureReadUpdateTime": 4}
            }],
            "mp": {"enabled": true}
        });

        let shelf = normalize_shelf(&value);

        assert_eq!(shelf.visible_items, 4);
        assert_eq!(shelf.private_items, 3);
        assert_eq!(shelf.recent[0].title, "Audio");
    }

    #[test]
    fn notes_total_uses_review_note_and_bookmark_counts() {
        let value = json!({
            "totalBookCount": 2,
            "totalNoteCount": 3,
            "books": [{
                "bookId": "1",
                "book": {"title": "Marked", "author": "A"},
                "reviewCount": 2,
                "noteCount": 3,
                "bookmarkCount": 2
            }, {
                "bookId": "2",
                "book": {"title": "Second", "author": "B"},
                "reviewCount": 1,
                "noteCount": 1,
                "bookmarkCount": 0
            }]
        });

        let notes = normalize_notes(&value);

        assert_eq!(notes.total_books, 2);
        assert_eq!(notes.total_notes, 9);
        assert_eq!(notes.top_books[0].total_notes, 7);
    }
}
