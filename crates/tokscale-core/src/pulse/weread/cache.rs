use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::model::{
    DatasetCoverage, WeReadDatasets, WeReadState, WeReadStatus, WeReadSyncState, SKILL_VERSION,
};

const CACHE_SCHEMA_VERSION: u32 = 2;
const LEGACY_CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_FILENAME: &str = "weread-pulse-cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedWeReadPulseV2 {
    schema_version: u32,
    skill_version: String,
    #[serde(default)]
    last_attempt_at: Option<chrono::DateTime<Utc>>,
    datasets: WeReadDatasets,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedWeReadPulseV1 {
    schema_version: u32,
    timestamp: u64,
    data: CachedWeReadDataV1,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedWeReadDataV1 {
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
    crate::paths::get_cache_dir().join(CACHE_FILENAME)
}

pub fn load() -> Option<WeReadState> {
    load_sync().map(WeReadSyncState::into_legacy)
}

pub fn load_sync() -> Option<WeReadSyncState> {
    crate::pulse::store::recover_pending().ok()?;
    load_sync_file(Utc::now())
}

fn load_sync_file(now: chrono::DateTime<Utc>) -> Option<WeReadSyncState> {
    let path = cache_file();
    let file = File::open(&path).ok()?;
    let value: serde_json::Value = serde_json::from_reader(BufReader::new(file)).ok()?;
    let schema_version = value.get("schemaVersion")?.as_u64()? as u32;

    let mut state = match schema_version {
        CACHE_SCHEMA_VERSION => {
            let cached: CachedWeReadPulseV2 = serde_json::from_value(value).ok()?;
            if cached.schema_version != CACHE_SCHEMA_VERSION {
                return None;
            }
            WeReadSyncState {
                datasets: cached.datasets,
                status: WeReadStatus::Fresh,
                last_attempt_at: cached.last_attempt_at,
            }
        }
        LEGACY_CACHE_SCHEMA_VERSION => {
            let cached: CachedWeReadPulseV1 = serde_json::from_value(value).ok()?;
            if cached.schema_version != LEGACY_CACHE_SCHEMA_VERSION {
                return None;
            }
            let legacy = WeReadState {
                weekly: cached.data.weekly,
                monthly: cached.data.monthly,
                shelf: cached.data.shelf,
                notes: cached.data.notes,
                status: WeReadStatus::Fresh,
                last_refresh_ms: Some(cached.timestamp),
                error: None,
            };
            WeReadSyncState::from_legacy(legacy, DatasetCoverage::Unknown, now)
        }
        _ => return None,
    };

    repair_user_only_permissions(&path).ok()?;
    state.clear_obsolete_upgrade_issues();
    state.refresh_freshness_at(now);
    Some(state)
}

pub fn save(state: &WeReadState) -> std::io::Result<()> {
    let sync_state =
        WeReadSyncState::from_legacy(state.clone(), DatasetCoverage::Unknown, Utc::now());
    save_sync(&sync_state)
}

pub fn save_sync(state: &WeReadSyncState) -> std::io::Result<()> {
    let _lock = crate::pulse::store::acquire_transaction_lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    crate::pulse::store::recover_pending_locked()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if is_older_than_cached_locked(state)? {
        return Ok(());
    }
    crate::pulse::store::invalidate_latest_locked()?;
    save_sync_locked(state)?;
    Ok(())
}

pub(crate) fn save_sync_locked(state: &WeReadSyncState) -> std::io::Result<bool> {
    if is_older_than_cached_locked(state)? {
        return Ok(false);
    }

    let cached = CachedWeReadPulseV2 {
        schema_version: CACHE_SCHEMA_VERSION,
        skill_version: SKILL_VERSION.to_string(),
        last_attempt_at: state.last_attempt_at,
        datasets: state.datasets.clone(),
    };
    let content = serde_json::to_vec_pretty(&cached).map_err(std::io::Error::other)?;
    atomic_write(&cache_file(), &content)?;
    Ok(true)
}

pub(crate) fn is_older_than_cached_locked(incoming: &WeReadSyncState) -> std::io::Result<bool> {
    let Some(cached) = load_sync_file(Utc::now()) else {
        return Ok(false);
    };

    Ok(match (generation(incoming), generation(&cached)) {
        (Some(incoming), Some(cached)) => incoming < cached,
        (None, Some(_)) => true,
        _ => false,
    })
}

fn generation(state: &WeReadSyncState) -> Option<chrono::DateTime<Utc>> {
    [
        state.last_attempt_at.as_ref(),
        state.datasets.current_week.observed_at.as_ref(),
        state.datasets.previous_week.observed_at.as_ref(),
        state.datasets.current_month.observed_at.as_ref(),
        state.datasets.shelf.observed_at.as_ref(),
        state.datasets.notebooks.observed_at.as_ref(),
    ]
    .into_iter()
    .flatten()
    .max()
    .cloned()
}

fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WeRead cache path has no parent",
        ));
    };
    fs::create_dir_all(parent)?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = parent.join(format!(
        ".{CACHE_FILENAME}.{}.{nanos:x}.tmp",
        std::process::id()
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let file = options.open(&temp_path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(content)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        crate::fs_atomic::replace_file(&temp_path, path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(unix)]
fn repair_user_only_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn repair_user_only_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::weread::model::datetime_from_millis;
    use crate::pulse::weread::{DatasetFreshness, WeReadMonthly};
    use serial_test::serial;
    use std::env;

    struct ConfigDirGuard(Option<std::ffi::OsString>);

    impl ConfigDirGuard {
        fn set(path: &Path) -> Self {
            let previous = env::var_os("TOKSCALE_CONFIG_DIR");
            unsafe { env::set_var("TOKSCALE_CONFIG_DIR", path) };
            Self(previous)
        }
    }

    impl Drop for ConfigDirGuard {
        fn drop(&mut self) {
            unsafe {
                match self.0.take() {
                    Some(value) => env::set_var("TOKSCALE_CONFIG_DIR", value),
                    None => env::remove_var("TOKSCALE_CONFIG_DIR"),
                }
            }
        }
    }

    fn monthly_state(total_seconds: u32, observed_at: chrono::DateTime<Utc>) -> WeReadSyncState {
        WeReadSyncState {
            datasets: WeReadDatasets {
                current_month: super::super::model::Dataset::success(
                    WeReadMonthly {
                        read_days: 2,
                        total_seconds,
                        day_average_seconds: total_seconds / 2,
                        prefer_category_word: None,
                        categories: Vec::new(),
                    },
                    observed_at,
                    DatasetCoverage::Complete,
                ),
                ..WeReadDatasets::default()
            },
            status: WeReadStatus::Partial,
            last_attempt_at: Some(observed_at),
        }
    }

    #[test]
    #[serial]
    fn migrates_v1_with_unknown_coverage_and_original_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let timestamp = super::super::model::now_millis();
        let path = cache_file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "timestamp": timestamp,
                "data": {
                    "monthly": {
                        "readDays": 3,
                        "totalSeconds": 600,
                        "dayAverageSeconds": 200,
                        "preferCategoryWord": null,
                        "categories": []
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }

        let state = load_sync().unwrap();

        assert_eq!(
            state.datasets.current_month.coverage,
            DatasetCoverage::Unknown
        );
        assert_eq!(
            state.datasets.current_month.observed_at,
            datetime_from_millis(timestamp)
        );
        assert_eq!(
            state.datasets.previous_week.freshness,
            DatasetFreshness::Missing
        );
        let unchanged: serde_json::Value =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(unchanged["schemaVersion"], 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(cache_file()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    #[serial]
    fn saves_v2_atomically_without_secret_or_raw_payload() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let observed_at = Utc::now();
        let mut state = WeReadSyncState::default();
        state.datasets.current_month = super::super::model::Dataset::success(
            WeReadMonthly {
                read_days: 2,
                total_seconds: 300,
                day_average_seconds: 150,
                prefer_category_word: None,
                categories: Vec::new(),
            },
            observed_at,
            DatasetCoverage::Complete,
        );
        state.last_attempt_at = Some(observed_at);

        save_sync(&state).unwrap();

        let path = cache_file();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schemaVersion\": 2"));
        assert!(!text.contains("secret-token"));
        assert!(!text.contains("rawResponse"));
        assert!(fs::read_dir(path.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    #[serial]
    fn load_misses_invalid_schema() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let path = cache_file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"schemaVersion":999,"timestamp":1,"data":{"weekly":null}}"#,
        )
        .unwrap();

        assert!(load().is_none());
    }

    #[test]
    #[serial]
    fn older_slow_generation_cannot_replace_newer_cache() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let newer_at = Utc::now();
        let older = monthly_state(100, newer_at - chrono::Duration::minutes(1));
        let newer = monthly_state(200, newer_at);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let slow_writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            save_sync(&older).unwrap();
        });

        started_rx.recv().unwrap();
        save_sync(&newer).unwrap();
        release_tx.send(()).unwrap();
        slow_writer.join().unwrap();

        let cached = load_sync().unwrap();
        assert_eq!(cached.last_attempt_at, Some(newer_at));
        assert_eq!(
            cached
                .datasets
                .current_month
                .value
                .as_ref()
                .unwrap()
                .total_seconds,
            200
        );
    }
}
