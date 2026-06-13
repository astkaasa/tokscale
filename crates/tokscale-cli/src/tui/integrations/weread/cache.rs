use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::model::{now_millis, WeReadState, WeReadStatus};

const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedWeReadPulse {
    schema_version: u32,
    timestamp: u64,
    data: CachedWeReadData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedWeReadData {
    #[serde(default)]
    weekly: Option<super::model::WeReadWeekly>,
    #[serde(default)]
    monthly: Option<super::model::WeReadMonthly>,
    #[serde(default)]
    shelf: Option<super::model::WeReadShelfSummary>,
    #[serde(default)]
    notes: Option<super::model::WeReadNotesSummary>,
}

fn cache_file() -> PathBuf {
    crate::paths::get_cache_dir().join("weread-pulse-cache.json")
}

pub(crate) fn load() -> Option<WeReadState> {
    let path = cache_file();
    let file = File::open(path).ok()?;
    let cached: CachedWeReadPulse = serde_json::from_reader(BufReader::new(file)).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }

    let mut state = WeReadState {
        weekly: cached.data.weekly,
        monthly: cached.data.monthly,
        shelf: cached.data.shelf,
        notes: cached.data.notes,
        status: WeReadStatus::Fresh,
        last_refresh_ms: Some(cached.timestamp),
        error: None,
    };

    state.status = if state.is_stale_at(now_millis()) {
        WeReadStatus::Stale
    } else {
        WeReadStatus::Fresh
    };
    Some(state)
}

pub(crate) fn save(state: &WeReadState) -> std::io::Result<()> {
    let path = cache_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let cached = CachedWeReadPulse {
        schema_version: CACHE_SCHEMA_VERSION,
        timestamp: state.last_refresh_ms.unwrap_or_else(now_millis),
        data: CachedWeReadData {
            weekly: state.weekly.clone(),
            monthly: state.monthly.clone(),
            shelf: state.shelf.clone(),
            notes: state.notes.clone(),
        },
    };

    let file = File::create(path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), &cached)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    #[test]
    #[serial]
    fn load_misses_invalid_schema() {
        let dir = tempfile::tempdir().unwrap();
        let prev = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("TOKSCALE_CONFIG_DIR", dir.path());
        }

        let path = cache_file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"schemaVersion":999,"timestamp":1,"data":{"weekly":null}}"#,
        )
        .unwrap();

        assert!(load().is_none());

        unsafe {
            match prev {
                Some(value) => env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }
}
