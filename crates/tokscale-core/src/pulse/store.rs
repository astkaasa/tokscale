use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::weread::{self, WeReadSyncState};
use super::{PulseSnapshotV1, PULSE_SCHEMA_VERSION};

const TRANSACTION_SCHEMA_VERSION: u32 = 1;
const TRANSACTION_LOCK_FILENAME: &str = ".transaction.lock";
const PENDING_TRANSACTION_FILENAME: &str = ".pending-transaction.json";

#[derive(Clone, Debug)]
pub enum SaveOutcome {
    Committed(PulseSnapshotV1),
    Superseded(Option<PulseSnapshotV1>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPulseTransaction {
    schema_version: u32,
    snapshot: PulseSnapshotV1,
    weread: WeReadSyncState,
}

fn pulse_dir() -> PathBuf {
    crate::paths::get_config_dir().join("pulse")
}

pub fn latest_path() -> PathBuf {
    pulse_dir().join("latest.json")
}

pub fn history_path(snapshot: &PulseSnapshotV1) -> PathBuf {
    pulse_dir()
        .join("history")
        .join(format!("{}.json", snapshot.period.start))
}

fn transaction_lock_path() -> PathBuf {
    pulse_dir().join(TRANSACTION_LOCK_FILENAME)
}

fn pending_transaction_path() -> PathBuf {
    pulse_dir().join(PENDING_TRANSACTION_FILENAME)
}

pub fn load_latest() -> Option<PulseSnapshotV1> {
    recover_pending().ok()?;
    load_latest_locked()
}

fn load_latest_locked() -> Option<PulseSnapshotV1> {
    let file = File::open(latest_path()).ok()?;
    let snapshot: PulseSnapshotV1 = serde_json::from_reader(BufReader::new(file)).ok()?;
    (snapshot.schema_version == PULSE_SCHEMA_VERSION).then_some(snapshot)
}

pub fn save(snapshot: &PulseSnapshotV1, weread: &WeReadSyncState) -> Result<SaveOutcome> {
    let _lock = acquire_transaction_lock()?;
    recover_pending_locked()?;
    let latest = load_latest_locked();
    if weread::cache::is_older_than_cached_locked(weread)
        .context("failed to compare WeRead cache generations")?
        || latest
            .as_ref()
            .is_some_and(|latest| snapshot_is_superseded(snapshot, latest))
    {
        return Ok(SaveOutcome::Superseded(latest));
    }

    let transaction = PendingPulseTransaction {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        snapshot: snapshot.clone(),
        weread: weread.clone(),
    };
    let content =
        serde_json::to_vec_pretty(&transaction).context("failed to serialize Pulse transaction")?;
    atomic_write(&pending_transaction_path(), &content)
        .context("failed to write pending Pulse transaction")?;
    let committed = apply_transaction(&transaction)?;
    remove_file_durable(&pending_transaction_path())
        .context("failed to finish Pulse transaction")?;
    if committed {
        Ok(SaveOutcome::Committed(snapshot.clone()))
    } else {
        Ok(SaveOutcome::Superseded(load_latest_locked()))
    }
}

pub(crate) fn recover_pending() -> Result<()> {
    let _lock = acquire_transaction_lock()?;
    recover_pending_locked()
}

pub(crate) fn acquire_transaction_lock() -> Result<File> {
    fs::create_dir_all(pulse_dir()).context("failed to create Pulse directory")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(transaction_lock_path())
        .context("failed to open Pulse transaction lock")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to secure Pulse transaction lock")?;
    }
    FileExt::lock_exclusive(&file).context("failed to lock Pulse transaction")?;
    Ok(file)
}

pub(crate) fn recover_pending_locked() -> Result<()> {
    let path = pending_transaction_path();
    let Ok(file) = File::open(&path) else {
        return Ok(());
    };
    let transaction: PendingPulseTransaction = serde_json::from_reader(BufReader::new(file))
        .context("failed to read pending Pulse transaction")?;
    if transaction.schema_version != TRANSACTION_SCHEMA_VERSION {
        bail!(
            "unsupported pending Pulse transaction schema {}",
            transaction.schema_version
        );
    }
    apply_transaction(&transaction)?;
    remove_file_durable(&path).context("failed to finish recovered Pulse transaction")?;
    Ok(())
}

pub(crate) fn invalidate_latest_locked() -> std::io::Result<()> {
    let path = latest_path();
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "latest Pulse snapshot path has no parent",
        )
    })?;
    match fs::remove_file(&path) {
        Ok(()) => sync_parent_directory(parent),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn apply_transaction(transaction: &PendingPulseTransaction) -> Result<bool> {
    if !weread::cache::save_sync_locked(&transaction.weread)
        .context("failed to write WeRead transaction cache")?
    {
        return Ok(false);
    }
    let content = serde_json::to_vec_pretty(&transaction.snapshot)
        .context("failed to serialize Pulse snapshot")?;
    atomic_write(&history_path(&transaction.snapshot), &content)
        .context("failed to write Pulse history snapshot")?;
    atomic_write(&latest_path(), &content).context("failed to write latest Pulse snapshot")?;
    Ok(true)
}

fn snapshot_is_superseded(incoming: &PulseSnapshotV1, latest: &PulseSnapshotV1) -> bool {
    if incoming.period.start != latest.period.start
        || incoming.period.end_exclusive != latest.period.end_exclusive
    {
        return (incoming.period.start, incoming.period.end_exclusive)
            < (latest.period.start, latest.period.end_exclusive);
    }

    match (ai_generation(incoming), ai_generation(latest)) {
        (Some(incoming), Some(latest)) if latest != incoming => latest > incoming,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        _ => latest.generated_at > incoming.generated_at,
    }
}

fn ai_generation(snapshot: &PulseSnapshotV1) -> Option<chrono::DateTime<chrono::Utc>> {
    snapshot
        .sources
        .iter()
        .filter(|source| {
            matches!(
                source.id.as_str(),
                "local-ai-usage" | "subscription-usage-cache"
            )
        })
        .filter_map(|source| source.observed_at)
        .max()
}

fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Pulse snapshot path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!(
        ".pulse-{}.{}.{nonce:x}.tmp",
        std::process::id(),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("snapshot")
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let file = options.open(&temp)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(content)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        crate::fs_atomic::replace_file(&temp, path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn remove_file_durable(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Pulse transaction path has no parent",
        )
    })?;
    fs::remove_file(path)?;
    sync_parent_directory(parent)
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
    use std::env;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use chrono::Utc;
    use serial_test::serial;

    use super::*;
    use crate::pulse::weread::WeReadSyncState;
    use crate::pulse::{AiWorkInput, AiWorkPeriodInput, PulseSnapshotV1};

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

    #[test]
    #[serial]
    fn saves_latest_and_period_history_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let input = AiWorkInput {
            observed_at: Some(Utc::now()),
            ..AiWorkInput::default()
        };
        let snapshot = PulseSnapshotV1::from_inputs(input, WeReadSyncState::default());

        save(&snapshot, &WeReadSyncState::default()).unwrap();

        assert_eq!(load_latest().unwrap().snapshot_id, snapshot.snapshot_id);
        assert!(history_path(&snapshot).is_file());
        assert!(fs::read_dir(pulse_dir()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    #[serial]
    fn replays_pending_transaction_before_reading_latest() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let snapshot =
            PulseSnapshotV1::from_inputs(AiWorkInput::default(), WeReadSyncState::default());
        let transaction = PendingPulseTransaction {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            snapshot: snapshot.clone(),
            weread: WeReadSyncState::default(),
        };
        let content = serde_json::to_vec_pretty(&transaction).unwrap();
        atomic_write(&pending_transaction_path(), &content).unwrap();

        let loaded = load_latest().unwrap();

        assert_eq!(loaded.snapshot_id, snapshot.snapshot_id);
        assert!(!pending_transaction_path().exists());
        assert!(weread::cache::load_sync().is_some());
    }

    #[test]
    #[serial]
    fn standalone_weread_writer_waits_for_pulse_transaction_lock() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let lock = acquire_transaction_lock().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();

        let writer = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = weread::cache::save_sync(&WeReadSyncState::default());
            finished_tx.send(result).unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());

        drop(lock);
        finished_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
    }

    #[test]
    #[serial]
    fn standalone_weread_writer_invalidates_unpaired_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let snapshot =
            PulseSnapshotV1::from_inputs(AiWorkInput::default(), WeReadSyncState::default());
        save(&snapshot, &WeReadSyncState::default()).unwrap();
        assert!(latest_path().is_file());

        let newer_weread = WeReadSyncState {
            last_attempt_at: Some(Utc::now()),
            ..WeReadSyncState::default()
        };
        weread::cache::save_sync(&newer_weread).unwrap();

        assert!(!latest_path().exists());
        assert!(weread::cache::load_sync().is_some());
    }

    #[test]
    #[serial]
    fn older_standalone_weread_writer_keeps_paired_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let observed_at = Utc::now();
        let newer_weread = WeReadSyncState {
            last_attempt_at: Some(observed_at),
            ..WeReadSyncState::default()
        };
        let snapshot = PulseSnapshotV1::from_inputs(AiWorkInput::default(), newer_weread.clone());
        save(&snapshot, &newer_weread).unwrap();

        let older_weread = WeReadSyncState {
            last_attempt_at: Some(observed_at - chrono::Duration::minutes(1)),
            ..WeReadSyncState::default()
        };
        weread::cache::save_sync(&older_weread).unwrap();

        assert_eq!(load_latest().unwrap().snapshot_id, snapshot.snapshot_id);
        assert_eq!(
            weread::cache::load_sync().unwrap().last_attempt_at,
            Some(observed_at)
        );
    }

    #[test]
    #[serial]
    fn older_ai_generation_is_reported_as_superseded() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let observed_at = Utc::now();
        let newer =
            PulseSnapshotV1::from_inputs(ai_input(observed_at, 200), WeReadSyncState::default());
        let older = PulseSnapshotV1::from_inputs(
            ai_input(observed_at - chrono::Duration::minutes(1), 100),
            WeReadSyncState::default(),
        );
        save(&newer, &WeReadSyncState::default()).unwrap();

        let outcome = save(&older, &WeReadSyncState::default()).unwrap();

        let SaveOutcome::Superseded(Some(durable)) = outcome else {
            panic!("expected the older AI generation to be superseded");
        };
        assert_eq!(durable.snapshot_id, newer.snapshot_id);
        assert_eq!(load_latest().unwrap().ai.total_tokens, Some(200));
    }

    #[test]
    #[serial]
    fn delayed_previous_period_cannot_replace_latest_period() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let observed_at = Utc::now();
        let latest =
            PulseSnapshotV1::from_inputs(ai_input(observed_at, 200), WeReadSyncState::default());
        let mut previous = PulseSnapshotV1::from_inputs(
            ai_input(observed_at - chrono::Duration::minutes(1), 100),
            WeReadSyncState::default(),
        );
        previous.period.start -= chrono::Duration::days(7);
        previous.period.end_exclusive -= chrono::Duration::days(7);
        save(&latest, &WeReadSyncState::default()).unwrap();

        let outcome = save(&previous, &WeReadSyncState::default()).unwrap();

        let SaveOutcome::Superseded(Some(durable)) = outcome else {
            panic!("expected the previous period to be superseded");
        };
        assert_eq!(durable.snapshot_id, latest.snapshot_id);
        assert_eq!(load_latest().unwrap().snapshot_id, latest.snapshot_id);
        assert!(!history_path(&previous).exists());
    }

    fn ai_input(observed_at: chrono::DateTime<Utc>, total_tokens: u64) -> AiWorkInput {
        AiWorkInput {
            observed_at: Some(observed_at),
            current: Some(AiWorkPeriodInput {
                total_tokens,
                total_cost: total_tokens as f64 / 100.0,
                active_days: 1,
                peak_day: None,
                peak_day_tokens: total_tokens,
                leading_model: Some("test-model".to_string()),
                leading_provider: Some("test-provider".to_string()),
            }),
            ..AiWorkInput::default()
        }
    }
}
