use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use serde_json::{json, Map, Value};

use super::cache;
use super::model::{
    local_date_from_unix_seconds, now_millis, week_start_for, WeReadBookRef, WeReadCategory,
    WeReadDay, WeReadFocusBook, WeReadMonthly, WeReadNotebookSummary, WeReadNotesSummary,
    WeReadShelfSummary, WeReadState, WeReadStatus, WeReadWeekly, SKILL_VERSION,
};

const GATEWAY_URL: &str = "https://i.weread.qq.com/api/agent/gateway";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn fetch_current(api_key: &str) -> Result<WeReadState> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(anyhow!("WEREAD_API_KEY is not set"));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start WeRead runtime")?;

    let mut state = runtime.block_on(fetch_with_key(api_key))?;
    state.last_refresh_ms = Some(now_millis());
    state.status = WeReadStatus::Fresh;
    state.error = None;
    let _ = cache::save(&state);
    Ok(state)
}

async fn fetch_with_key(api_key: &str) -> Result<WeReadState> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("failed to build WeRead HTTP client")?;

    let weekly = request(
        &client,
        api_key,
        "/readdata/detail",
        &[("mode", json!("weekly"))],
    )
    .await
    .context("weekly reading fetch failed")?;
    let monthly = request(
        &client,
        api_key,
        "/readdata/detail",
        &[("mode", json!("monthly"))],
    )
    .await
    .context("monthly reading fetch failed")?;
    let shelf = request(&client, api_key, "/shelf/sync", &[])
        .await
        .context("shelf fetch failed")?;
    let notebooks = request(&client, api_key, "/user/notebooks", &[("count", json!(20))])
        .await
        .context("notebook fetch failed")?;

    Ok(WeReadState {
        weekly: Some(normalize_weekly(&weekly)?),
        monthly: Some(normalize_monthly(&monthly)),
        shelf: Some(normalize_shelf(&shelf)),
        notes: Some(normalize_notes(&notebooks)),
        status: WeReadStatus::Fresh,
        last_refresh_ms: None,
        error: None,
    })
}

async fn request(
    client: &reqwest::Client,
    api_key: &str,
    api_name: &str,
    params: &[(&str, Value)],
) -> Result<Value> {
    let mut body = Map::new();
    body.insert("api_name".to_string(), Value::String(api_name.to_string()));
    body.insert(
        "skill_version".to_string(),
        Value::String(SKILL_VERSION.to_string()),
    );
    for (key, value) in params {
        body.insert((*key).to_string(), value.clone());
    }

    let response = client
        .post(GATEWAY_URL)
        .bearer_auth(api_key)
        .json(&Value::Object(body))
        .send()
        .await
        .with_context(|| format!("WeRead gateway request failed for {api_name}"))?;

    let status = response.status();
    let value: Value = response
        .json()
        .await
        .with_context(|| format!("WeRead gateway returned non-JSON for {api_name}"))?;

    if let Some(message) = upgrade_message(&value) {
        bail!("WeRead skill upgrade required: {message}");
    }

    if !status.is_success() {
        bail!("WeRead gateway returned HTTP {}", status.as_u16());
    }

    if let Some(errcode) = value.get("errcode").and_then(Value::as_i64) {
        if errcode != 0 {
            let message = value
                .get("errmsg")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown gateway error");
            bail!("WeRead gateway error {errcode}: {message}");
        }
    }

    Ok(value.get("data").cloned().unwrap_or(value))
}

fn upgrade_message(value: &Value) -> Option<String> {
    let info = value
        .get("upgrade_info")
        .or_else(|| value.get("data").and_then(|data| data.get("upgrade_info")))?;
    info.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some(info.to_string()))
}

pub(crate) fn normalize_weekly(value: &Value) -> Result<WeReadWeekly> {
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

pub(crate) fn normalize_monthly(value: &Value) -> WeReadMonthly {
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

pub(crate) fn normalize_shelf(value: &Value) -> WeReadShelfSummary {
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

pub(crate) fn normalize_notes(value: &Value) -> WeReadNotesSummary {
    let mut top_books = value
        .get("books")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(notebook_summary)
        .collect::<Vec<_>>();
    top_books.sort_by(|a, b| {
        b.total_notes
            .cmp(&a.total_notes)
            .then_with(|| a.title.cmp(&b.title))
    });
    top_books.truncate(5);

    WeReadNotesSummary {
        total_books: value_u32_field(value, "totalBookCount"),
        total_notes: value_u32_field(value, "totalNoteCount"),
        top_books,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "totalBookCount": 1,
            "totalNoteCount": 7,
            "books": [{
                "bookId": "1",
                "book": {"title": "Marked", "author": "A"},
                "reviewCount": 2,
                "noteCount": 3,
                "bookmarkCount": 2
            }]
        });

        let notes = normalize_notes(&value);

        assert_eq!(notes.total_books, 1);
        assert_eq!(notes.total_notes, 7);
        assert_eq!(notes.top_books[0].total_notes, 7);
    }
}
