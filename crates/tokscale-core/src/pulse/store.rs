use std::fs::{self, File, OpenOptions};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::weread::{self, WeReadSyncState};
use super::{
    AiPulse, AiQuotaMetric, AiQuotaSource, AiWorkInput, AiWorkPeriodInput, PulseFreshness,
    PulseSnapshotV1, SourceHealth, PULSE_SCHEMA_VERSION,
};

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

    let snapshot = match latest.as_ref() {
        Some(latest) => match reconcile_ai_sources(snapshot, latest, weread) {
            Some(snapshot) => snapshot,
            None => return Ok(SaveOutcome::Superseded(Some(latest.clone()))),
        },
        None => snapshot.clone(),
    };

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
        Ok(SaveOutcome::Committed(snapshot))
    } else {
        Ok(SaveOutcome::Superseded(load_latest_locked()))
    }
}

pub(crate) fn recover_pending() -> Result<()> {
    let _lock = acquire_transaction_lock()?;
    recover_pending_locked()
}

pub(crate) fn acquire_transaction_lock() -> Result<File> {
    let pulse_dir = pulse_dir();
    crate::fs_atomic::ensure_private_dir(&pulse_dir).context("failed to create Pulse directory")?;
    crate::fs_atomic::repair_private_dir(&pulse_dir.join("history"));
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

    let generations = ["local-ai-usage", "subscription-usage-cache"]
        .map(|source_id| ai_source_generation(incoming, latest, source_id));

    if generations.contains(&SourceGeneration::Newer) {
        return false;
    }
    if generations.contains(&SourceGeneration::Older) {
        return true;
    }

    incoming.generated_at < latest.generated_at
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceGeneration {
    Older,
    Equal,
    Newer,
}

fn ai_source_generation(
    incoming: &PulseSnapshotV1,
    latest: &PulseSnapshotV1,
    source_id: &str,
) -> SourceGeneration {
    let incoming = ai_source(incoming, source_id);
    let latest = ai_source(latest, source_id);
    let incoming_is_present = incoming.is_some_and(ai_source_is_present);
    let latest_is_present = latest.is_some_and(ai_source_is_present);

    match (incoming_is_present, latest_is_present) {
        (true, false) => return SourceGeneration::Newer,
        (false, true) => return SourceGeneration::Older,
        (false, false) => return SourceGeneration::Equal,
        (true, true) => {}
    }

    match (
        incoming.and_then(|source| source.observed_at),
        latest.and_then(|source| source.observed_at),
    ) {
        (Some(incoming), Some(latest)) if incoming < latest => SourceGeneration::Older,
        (Some(incoming), Some(latest)) if incoming > latest => SourceGeneration::Newer,
        (None, Some(_)) => SourceGeneration::Older,
        (Some(_), None) => SourceGeneration::Newer,
        _ => SourceGeneration::Equal,
    }
}

fn reconcile_ai_sources(
    incoming: &PulseSnapshotV1,
    latest: &PulseSnapshotV1,
    weread: &WeReadSyncState,
) -> Option<PulseSnapshotV1> {
    if incoming.period.start != latest.period.start
        || incoming.period.end_exclusive != latest.period.end_exclusive
    {
        return Some(incoming.clone());
    }

    let local_from_incoming = source_from_incoming(incoming, latest, "local-ai-usage");
    let quota_from_incoming = source_from_incoming(incoming, latest, "subscription-usage-cache");
    if local_from_incoming && quota_from_incoming {
        return Some(incoming.clone());
    }
    if !local_from_incoming && !quota_from_incoming {
        return None;
    }

    let local = if local_from_incoming {
        incoming
    } else {
        latest
    };
    let quota = if quota_from_incoming {
        incoming
    } else {
        latest
    };
    let local_observed_at = ai_source(local, "local-ai-usage")?.observed_at;
    let quota_observed_at = ai_source(quota, "subscription-usage-cache")?.observed_at;
    let input = ai_input_from_sources(&local.ai, &quota.ai);
    let merged = PulseSnapshotV1::from_inputs_with_source_observed_at(
        input,
        local_observed_at,
        quota_observed_at,
        weread.clone(),
    );

    (merged.period.start == incoming.period.start
        && merged.period.end_exclusive == incoming.period.end_exclusive)
        .then_some(merged)
}

fn source_from_incoming(
    incoming: &PulseSnapshotV1,
    latest: &PulseSnapshotV1,
    source_id: &str,
) -> bool {
    match ai_source_generation(incoming, latest, source_id) {
        SourceGeneration::Newer => true,
        SourceGeneration::Older => false,
        SourceGeneration::Equal => incoming.generated_at >= latest.generated_at,
    }
}

fn ai_input_from_sources(local: &AiPulse, quota: &AiPulse) -> AiWorkInput {
    let current = match (local.total_tokens, local.total_cost, local.active_days) {
        (Some(total_tokens), Some(total_cost), Some(active_days)) => Some(AiWorkPeriodInput {
            total_tokens,
            total_cost,
            active_days,
            peak_day: local.peak_day,
            peak_day_tokens: local.peak_day_tokens.unwrap_or_default(),
            leading_model: local.leading_model.clone(),
            leading_provider: local.leading_provider.clone(),
        }),
        _ => None,
    };
    let previous = match (local.previous_total_tokens, local.previous_total_cost) {
        (Some(total_tokens), Some(total_cost)) => Some(AiWorkPeriodInput {
            total_tokens,
            total_cost,
            active_days: 0,
            peak_day: None,
            peak_day_tokens: 0,
            leading_model: None,
            leading_provider: None,
        }),
        _ => None,
    };
    let quota_count = quota
        .provider_count
        .max(if quota.max_used_percent.is_some() {
            1
        } else {
            0
        });
    let quota_sources = (0..quota_count)
        .map(|index| AiQuotaSource {
            provider: if index == 0 {
                quota
                    .max_provider
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string())
            } else {
                format!("provider-{}", index + 1)
            },
            metrics: if index == 0 {
                quota
                    .max_used_percent
                    .map(|used_percent| AiQuotaMetric {
                        label: quota
                            .max_metric_label
                            .clone()
                            .unwrap_or_else(|| "usage".to_string()),
                        used_percent,
                    })
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            },
        })
        .collect();

    AiWorkInput {
        current,
        previous,
        quota_sources,
        observed_at: None,
    }
}

fn ai_source<'a>(snapshot: &'a PulseSnapshotV1, source_id: &str) -> Option<&'a SourceHealth> {
    snapshot
        .sources
        .iter()
        .find(|source| source.id == source_id)
}

fn ai_source_is_present(source: &SourceHealth) -> bool {
    source.freshness != PulseFreshness::Missing
}

fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Pulse snapshot path has no parent",
        )
    })?;
    crate::fs_atomic::ensure_private_dir(parent)?;
    crate::fs_atomic::atomic_write_private(path, content)
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

    #[cfg(unix)]
    #[test]
    #[serial]
    fn save_repairs_pulse_directories_and_files_to_private_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let history_dir = pulse_dir().join("history");
        fs::create_dir_all(&history_dir).unwrap();
        fs::set_permissions(&pulse_dir(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&history_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let snapshot =
            PulseSnapshotV1::from_inputs(AiWorkInput::default(), WeReadSyncState::default());

        save(&snapshot, &WeReadSyncState::default()).unwrap();

        for path in [pulse_dir(), history_dir] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o700
            );
        }
        for path in [
            latest_path(),
            history_path(&snapshot),
            transaction_lock_path(),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o600
            );
        }
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
    fn crossed_ai_source_generations_merge_without_regression() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let newer = Utc::now();
        let older = newer - chrono::Duration::minutes(1);
        let incoming = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(200, 20.0),
            Some(newer),
            Some(older),
            WeReadSyncState::default(),
        );
        let durable = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 80.0),
            Some(older),
            Some(newer),
            WeReadSyncState::default(),
        );

        assert!(!snapshot_is_superseded(&incoming, &durable));
        save(&durable, &WeReadSyncState::default()).unwrap();

        let outcome = save(&incoming, &WeReadSyncState::default()).unwrap();

        let SaveOutcome::Committed(merged) = outcome else {
            panic!("expected independent AI generations to merge");
        };
        assert_eq!(merged.ai.total_tokens, Some(200));
        assert_eq!(merged.ai.max_used_percent, Some(80.0));
        assert_eq!(
            ai_source(&merged, "local-ai-usage").unwrap().observed_at,
            Some(newer)
        );
        assert_eq!(
            ai_source(&merged, "subscription-usage-cache")
                .unwrap()
                .observed_at,
            Some(newer)
        );
        assert!(merged.validate_evidence_refs());
    }

    #[test]
    #[serial]
    fn fresh_local_usage_retains_durable_quota_when_incoming_quota_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let newer = Utc::now();
        let older = newer - chrono::Duration::minutes(10);
        let durable = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 80.0),
            Some(older),
            Some(older),
            WeReadSyncState::default(),
        );
        let incoming =
            PulseSnapshotV1::from_inputs(ai_input(newer, 250), WeReadSyncState::default());
        save(&durable, &WeReadSyncState::default()).unwrap();

        let outcome = save(&incoming, &WeReadSyncState::default()).unwrap();

        let SaveOutcome::Committed(merged) = outcome else {
            panic!("expected fresh local usage to merge with durable quota");
        };
        assert_eq!(merged.ai.total_tokens, Some(250));
        assert_eq!(merged.ai.max_used_percent, Some(80.0));
        assert_eq!(
            ai_source(&merged, "local-ai-usage").unwrap().observed_at,
            Some(newer)
        );
        assert_eq!(
            ai_source(&merged, "subscription-usage-cache")
                .unwrap()
                .observed_at,
            Some(older)
        );
        assert_eq!(load_latest().unwrap().snapshot_id, merged.snapshot_id);
    }

    #[test]
    #[serial]
    fn equal_ai_generation_allows_the_other_source_to_advance() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let original = Utc::now() - chrono::Duration::minutes(1);
        let newer_quota = Utc::now();
        let durable = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 20.0),
            Some(original),
            Some(original),
            WeReadSyncState::default(),
        );
        let incoming = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 80.0),
            Some(original),
            Some(newer_quota),
            WeReadSyncState::default(),
        );

        assert!(!snapshot_is_superseded(&incoming, &durable));
        save(&durable, &WeReadSyncState::default()).unwrap();

        let outcome = save(&incoming, &WeReadSyncState::default()).unwrap();

        assert!(matches!(outcome, SaveOutcome::Committed(_)));
        assert_eq!(load_latest().unwrap().ai.max_used_percent, Some(80.0));
    }

    #[test]
    #[serial]
    fn equal_ai_generations_reject_delayed_older_writer() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let observed_at = Utc::now();
        let latest = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(200, 80.0),
            Some(observed_at),
            Some(observed_at),
            WeReadSyncState::default(),
        );
        let mut delayed = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 20.0),
            Some(observed_at),
            Some(observed_at),
            WeReadSyncState::default(),
        );
        delayed.generated_at = latest.generated_at - chrono::Duration::seconds(1);
        save(&latest, &WeReadSyncState::default()).unwrap();

        let outcome = save(&delayed, &WeReadSyncState::default()).unwrap();

        let SaveOutcome::Superseded(Some(durable)) = outcome else {
            panic!("expected delayed equal-generation writer to be superseded");
        };
        assert_eq!(durable.snapshot_id, latest.snapshot_id);
        assert_eq!(load_latest().unwrap().ai.total_tokens, Some(200));
        assert_eq!(load_latest().unwrap().ai.max_used_percent, Some(80.0));
    }

    #[test]
    fn present_ai_source_cannot_regress_to_missing() {
        let observed_at = Utc::now();
        let durable = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(100, 20.0),
            Some(observed_at),
            Some(observed_at),
            WeReadSyncState::default(),
        );
        let incoming =
            PulseSnapshotV1::from_inputs(ai_input(observed_at, 200), WeReadSyncState::default());

        assert!(snapshot_is_superseded(&incoming, &durable));
    }

    #[test]
    fn legacy_observation_on_missing_source_does_not_block_replacement() {
        let observed_at = Utc::now();
        let durable =
            PulseSnapshotV1::from_inputs(ai_input(observed_at, 100), WeReadSyncState::default());
        let incoming = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_input_with_quota(200, 20.0),
            Some(observed_at),
            Some(observed_at - chrono::Duration::minutes(1)),
            WeReadSyncState::default(),
        );

        assert!(!snapshot_is_superseded(&incoming, &durable));
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

    fn ai_input_with_quota(total_tokens: u64, used_percent: f64) -> AiWorkInput {
        AiWorkInput {
            current: Some(AiWorkPeriodInput {
                total_tokens,
                total_cost: total_tokens as f64 / 100.0,
                active_days: 1,
                peak_day: None,
                peak_day_tokens: total_tokens,
                leading_model: Some("test-model".to_string()),
                leading_provider: Some("test-provider".to_string()),
            }),
            quota_sources: vec![crate::pulse::AiQuotaSource {
                provider: "test-provider".to_string(),
                metrics: vec![crate::pulse::AiQuotaMetric {
                    label: "weekly".to_string(),
                    used_percent,
                }],
            }],
            ..AiWorkInput::default()
        }
    }
}
