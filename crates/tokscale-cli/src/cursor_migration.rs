use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokscale_core::telemetry::{
    CheckedIngest, EventCost, EventIdentity, ObservedTelemetryEvent, SourceDescriptor,
    SourceObservation, TelemetryEventInput, TelemetrySourceHealth, TelemetrySourceKind,
    TelemetrySourceStatus, TelemetrySourceTotals, TelemetryStore,
};
use tokscale_core::{TokenBreakdown, UnifiedMessage};

use crate::integrations::cursor_archive::{
    archive_cursor_csv, CursorAccountResolution, CursorArchiveManifest, CursorArchiveRequest,
};

const CURSOR_ARCHIVE_PARSER_VERSION: &str = "cursor-csv-v1";
const CURSOR_ARCHIVE_REJECTED_ROWS_ISSUE: &str = "cursor_archive_rejected_rows";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CursorMigrationStatus {
    NoData,
    Complete,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorMigrationSummary {
    pub status: CursorMigrationStatus,
    pub archived_files: usize,
    pub imported_events: usize,
    pub attention_files: usize,
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    account_key: String,
    resolution: CursorAccountResolution,
    digest: String,
    selected: bool,
}

#[derive(Debug)]
struct PreparedCursorSource {
    source: SourceDescriptor,
    observation: SourceObservation,
    expected_totals: Option<TelemetrySourceTotals>,
}

#[derive(Debug)]
struct PreparedCursorMigration {
    sources: Vec<PreparedCursorSource>,
    archived_files: usize,
    attention_files: usize,
}

#[derive(Debug, Default)]
struct CursorAccountIndex {
    active_account_id: String,
    account_ids: Vec<String>,
}

pub(crate) fn migrate_default_cursor_history_best_effort() {
    let config_dir = crate::paths::get_config_dir();
    let cache_dir = config_dir.join("cursor-cache");
    let archive_root = config_dir.join("archive/cursor");
    let ledger_path = crate::paths::telemetry_store_path();
    let accounts = load_cursor_account_index(&config_dir);

    if default_migration_is_current(&cache_dir, &archive_root, &ledger_path, accounts.as_ref()) {
        return;
    }

    if migrate_cursor_history(&cache_dir, &archive_root, &ledger_path, accounts.as_ref()).is_err() {
        tracing::warn!(
            issue_code = "cursor_history_migration_failed",
            "Cursor history migration failed; original CSV files were preserved"
        );
    }
}

fn default_migration_is_current(
    cache_dir: &Path,
    archive_root: &Path,
    ledger_path: &Path,
    accounts: Option<&CursorAccountIndex>,
) -> bool {
    if !ledger_path.is_file() {
        return false;
    }
    let candidates = match discover_candidates(cache_dir, accounts) {
        Ok(candidates) if candidates.is_empty() => return true,
        Ok(mut candidates) => {
            resolve_duplicate_candidates(&mut candidates);
            candidates
        }
        Err(_) => return false,
    };

    let manifest_dir = archive_root.join("manifests");
    let manifests = match fs::read_dir(manifest_dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<CursorArchiveManifest>(&bytes).ok())
            .collect::<Vec<_>>(),
        Err(_) => return false,
    };
    let store = match TelemetryStore::open(ledger_path.to_path_buf()) {
        Ok(store) => store,
        Err(_) => return false,
    };

    candidates.into_iter().all(|candidate| {
        let mut heads = manifests.iter().filter(|manifest| {
            manifest.object_sha256 == candidate.digest
                && manifest.account_key == candidate.account_key
                && !manifests.iter().any(|successor| {
                    successor.object_sha256 == candidate.digest
                        && successor.account_key == candidate.account_key
                        && successor.supersedes.as_deref() == Some(manifest.manifest_id.as_str())
                })
        });
        let Some(manifest) = heads.next() else {
            return false;
        };
        if heads.next().is_some()
            || manifest.account_resolution != candidate.resolution
            || manifest.selected_for_import != candidate.selected
        {
            return false;
        }
        if !manifest.selected_for_import {
            return true;
        }
        store
            .source_health(&format!("cursor-archive:{}", manifest.manifest_id))
            .ok()
            .flatten()
            .is_some_and(|health| {
                let expected_status = if manifest.rejected_rows == 0 {
                    TelemetrySourceStatus::Ready
                } else {
                    TelemetrySourceStatus::Error
                };
                let issue_is_current = manifest.rejected_rows == 0
                    || health.issue_code.as_deref() == Some(CURSOR_ARCHIVE_REJECTED_ROWS_ISSUE);
                health.status == expected_status
                    && issue_is_current
                    && health.source_kind == TelemetrySourceKind::ArchiveImport
                    && health.client == "cursor"
                    && health.parser_id == "cursor-csv"
                    && health.parser_version == CURSOR_ARCHIVE_PARSER_VERSION
            })
    })
}

fn cursor_source_health_matches(
    health: &TelemetrySourceHealth,
    status: TelemetrySourceStatus,
    issue_code: Option<&str>,
    expected_events: usize,
) -> bool {
    health.status == status
        && health.source_kind == TelemetrySourceKind::ArchiveImport
        && health.client == "cursor"
        && health.parser_id == "cursor-csv"
        && health.parser_version == CURSOR_ARCHIVE_PARSER_VERSION
        && health.issue_code.as_deref() == issue_code
        && health.observed_events == expected_events
        && health.present_events == expected_events
        && health.missing_events == 0
}

fn cursor_source_totals_match(
    stored: &TelemetrySourceTotals,
    expected: &TelemetrySourceTotals,
) -> bool {
    stored.event_count == expected.event_count
        && stored.tokens == expected.tokens
        && stored.min_occurred_at_ms == expected.min_occurred_at_ms
        && stored.max_occurred_at_ms == expected.max_occurred_at_ms
        && (stored.cost - expected.cost).abs() <= 1e-9
}

pub(crate) fn run_default_cursor_migration(json: bool) -> Result<()> {
    let config_dir = crate::paths::get_config_dir();
    let cache_dir = config_dir.join("cursor-cache");
    let accounts = load_cursor_account_index(&config_dir);
    let summary = migrate_cursor_history(
        &cache_dir,
        &config_dir.join("archive/cursor"),
        &crate::paths::telemetry_store_path(),
        accounts.as_ref(),
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        match summary.status {
            CursorMigrationStatus::NoData => println!("No Cursor usage CSV files found."),
            CursorMigrationStatus::Complete => println!(
                "Archived {} Cursor CSV file(s); verified {} imported event(s).",
                summary.archived_files, summary.imported_events
            ),
            CursorMigrationStatus::NeedsAttention => println!(
                "Archived {} Cursor CSV file(s); {} require review before import.",
                summary.archived_files, summary.attention_files
            ),
        }
    }
    Ok(())
}

fn migrate_cursor_history(
    cache_dir: &Path,
    archive_root: &Path,
    ledger_path: &Path,
    credentials: Option<&CursorAccountIndex>,
) -> Result<CursorMigrationSummary> {
    let mut candidates = discover_candidates(cache_dir, credentials)?;
    if candidates.is_empty() {
        return Ok(CursorMigrationSummary {
            status: CursorMigrationStatus::NoData,
            archived_files: 0,
            imported_events: 0,
            attention_files: 0,
        });
    }
    resolve_duplicate_candidates(&mut candidates);

    let store = TelemetryStore::open(ledger_path.to_path_buf())
        .context("could not open durable telemetry for Cursor import")?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let PreparedCursorMigration {
        sources,
        archived_files,
        attention_files,
    } = prepare_cursor_migration(&candidates, archive_root)?;
    let imported_events = ingest_prepared_cursor_sources(&store, sources, now_ms)?;

    Ok(CursorMigrationSummary {
        status: if attention_files == 0 {
            CursorMigrationStatus::Complete
        } else {
            CursorMigrationStatus::NeedsAttention
        },
        archived_files,
        imported_events,
        attention_files,
    })
}

fn prepare_cursor_migration(
    candidates: &[Candidate],
    archive_root: &Path,
) -> Result<PreparedCursorMigration> {
    let mut prepared = PreparedCursorMigration {
        sources: Vec::new(),
        archived_files: 0,
        attention_files: 0,
    };

    for candidate in candidates {
        let mut request = CursorArchiveRequest::new(
            &candidate.path,
            &candidate.account_key,
            candidate.resolution,
        );
        request.selected_for_import = candidate.selected;
        let archived =
            archive_cursor_csv(archive_root, request).context("could not archive Cursor CSV")?;
        prepared.archived_files += 1;

        if !archived.manifest.selected_for_import {
            prepared.attention_files += 1;
            continue;
        }

        let source = cursor_archive_source(&archived.manifest);
        if archived.manifest.rejected_rows > 0 {
            prepared.attention_files += 1;
            prepared.sources.push(PreparedCursorSource {
                source,
                observation: SourceObservation::Failed {
                    issue_code: CURSOR_ARCHIVE_REJECTED_ROWS_ISSUE.to_string(),
                },
                expected_totals: None,
            });
            continue;
        }

        let messages = tokscale_core::sessions::cursor::parse_cursor_file_for_account(
            &archived.object_path,
            &archived.manifest.account_key,
        );
        if messages.len() as u64 != archived.manifest.accepted_rows {
            bail!("Cursor archive parser count changed after archival");
        }
        let expected_totals = totals_for_messages(&messages);
        let events = messages
            .iter()
            .map(cursor_observation)
            .collect::<Result<Vec<_>>>()?;
        prepared.sources.push(PreparedCursorSource {
            source,
            observation: SourceObservation::Complete(events),
            expected_totals: Some(expected_totals),
        });
    }

    Ok(prepared)
}

fn ingest_prepared_cursor_sources(
    store: &TelemetryStore,
    sources: Vec<PreparedCursorSource>,
    now_ms: i64,
) -> Result<usize> {
    let imported_events =
        store.ingest_checked(CURSOR_ARCHIVE_PARSER_VERSION, now_ms, now_ms, |context| {
            let mut imported_events = 0;
            for prepared in sources {
                let commit_context = if prepared.expected_totals.is_some() {
                    "could not import Cursor archive"
                } else {
                    "could not record rejected Cursor archive rows"
                };
                let summary = context
                    .commit_source(&prepared.source, prepared.observation, now_ms)
                    .context(commit_context)?;

                let Some(expected) = prepared.expected_totals else {
                    let stored = context
                        .source_totals(&prepared.source.source_id)
                        .context("could not verify rejected Cursor archive totals")?
                        .context("rejected Cursor archive source totals are missing")?;
                    let health = context
                        .source_health(&prepared.source.source_id)
                        .context("could not verify rejected Cursor archive health")?
                        .context("rejected Cursor archive source health is missing")?;
                    if stored.event_count != 0
                        || !cursor_source_health_matches(
                            &health,
                            TelemetrySourceStatus::Error,
                            Some(CURSOR_ARCHIVE_REJECTED_ROWS_ISSUE),
                            0,
                        )
                    {
                        bail!("rejected Cursor archive source state mismatch");
                    }
                    continue;
                };

                if summary.unique_events != expected.event_count {
                    bail!("Cursor archive identity collision detected");
                }
                let stored = context
                    .source_totals(&prepared.source.source_id)
                    .context("could not verify Cursor archive import")?
                    .context("Cursor archive source totals are missing")?;
                if !cursor_source_totals_match(&stored, &expected) {
                    bail!("Cursor archive import parity mismatch");
                }
                let health = context
                    .source_health(&prepared.source.source_id)
                    .context("could not verify Cursor archive source health")?
                    .context("Cursor archive source health is missing")?;
                if !cursor_source_health_matches(
                    &health,
                    TelemetrySourceStatus::Ready,
                    None,
                    expected.event_count,
                ) {
                    bail!("Cursor archive source health mismatch");
                }
                imported_events += stored.event_count;
            }

            Ok(CheckedIngest::Commit(imported_events))
        })?;
    store
        .verify_integrity()
        .context("Cursor archive import failed SQLite integrity verification")?;
    Ok(imported_events)
}

fn discover_candidates(
    cache_dir: &Path,
    credentials: Option<&CursorAccountIndex>,
) -> Result<Vec<Candidate>> {
    let entries = match fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("could not inspect Cursor cache"),
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_usage_csv(path))
        .collect::<Vec<_>>();
    paths.sort_unstable();

    let mut candidates = Vec::new();
    for path in paths {
        let bytes = fs::read(&path).context("could not read Cursor CSV candidate")?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let (account_key, resolution, selected) = resolve_candidate_account(&path, credentials);
        candidates.push(Candidate {
            path,
            account_key,
            resolution,
            digest,
            selected,
        });
    }
    Ok(candidates)
}

fn is_usage_csv(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == "usage.csv" {
        return true;
    }
    name.starts_with("usage.")
        && name.ends_with(".csv")
        && !name.starts_with("usage.backup")
        && name
            .trim_start_matches("usage.")
            .trim_end_matches(".csv")
            .chars()
            .all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
}

fn resolve_candidate_account(
    path: &Path,
    credentials: Option<&CursorAccountIndex>,
) -> (String, CursorAccountResolution, bool) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("usage.csv");
    if name == "usage.csv" {
        let resolution = if credentials
            .map(|store| store.active_account_id.trim())
            .filter(|active| !active.is_empty())
            .is_some()
        {
            CursorAccountResolution::Resolved
        } else {
            CursorAccountResolution::Unknown
        };
        return ("legacy-active".to_string(), resolution, true);
    }

    let stem = name.trim_start_matches("usage.").trim_end_matches(".csv");
    let matching = credentials
        .into_iter()
        .flat_map(|store| store.account_ids.iter())
        .filter(|account| sanitize_account_key(account) == stem)
        .cloned()
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [_account] => (stem.to_string(), CursorAccountResolution::Resolved, true),
        [] => (stem.to_string(), CursorAccountResolution::Unknown, true),
        _ => (
            format!("ambiguous-{stem}"),
            CursorAccountResolution::Ambiguous,
            false,
        ),
    }
}

fn load_cursor_account_index(config_dir: &Path) -> Option<CursorAccountIndex> {
    let path = config_dir.join("cursor-credentials.json");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let active_account_id = value
        .get("activeAccountId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut account_ids = value
        .get("accounts")
        .and_then(serde_json::Value::as_object)
        .map(|accounts| accounts.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    account_ids.sort_unstable();
    Some(CursorAccountIndex {
        active_account_id,
        account_ids,
    })
}

fn resolve_duplicate_candidates(candidates: &mut [Candidate]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        groups
            .entry(candidate.digest.clone())
            .or_default()
            .push(index);
    }

    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let mut by_account = HashMap::<String, Vec<usize>>::new();
        for index in indices {
            by_account
                .entry(candidates[*index].account_key.clone())
                .or_default()
                .push(*index);
        }
        for same_account in by_account.values() {
            let preferred = same_account
                .iter()
                .copied()
                .filter(|index| candidates[*index].selected)
                .min_by_key(|index| {
                    let resolution_rank = match candidates[*index].resolution {
                        CursorAccountResolution::Resolved => 0,
                        CursorAccountResolution::Unknown => 1,
                        CursorAccountResolution::Ambiguous => 2,
                    };
                    (resolution_rank, *index)
                });
            for index in same_account {
                if Some(*index) != preferred {
                    candidates[*index].selected = false;
                }
            }
        }

        let has_resolved = indices.iter().any(|index| {
            candidates[*index].resolution == CursorAccountResolution::Resolved
                && candidates[*index].selected
        });
        if has_resolved {
            for index in indices {
                if candidates[*index].resolution != CursorAccountResolution::Resolved {
                    candidates[*index].selected = false;
                }
            }
            continue;
        }

        let unresolved_accounts = indices
            .iter()
            .filter(|index| {
                candidates[**index].resolution != CursorAccountResolution::Resolved
                    && candidates[**index].selected
            })
            .copied()
            .collect::<Vec<_>>();
        let distinct_unresolved = unresolved_accounts
            .iter()
            .map(|index| candidates[*index].account_key.as_str())
            .collect::<std::collections::HashSet<_>>();
        if distinct_unresolved.len() > 1 {
            for index in unresolved_accounts {
                candidates[index].selected = false;
                candidates[index].resolution = CursorAccountResolution::Ambiguous;
            }
        }
    }
}

fn cursor_archive_source(manifest: &CursorArchiveManifest) -> SourceDescriptor {
    SourceDescriptor {
        source_id: format!("cursor-archive:{}", manifest.manifest_id),
        source_kind: TelemetrySourceKind::ArchiveImport,
        client: "cursor".to_string(),
        source_ref: manifest.manifest_id.clone(),
        source_location: Some(format!("manifests/{}.json", manifest.manifest_id)),
        parser_id: "cursor-csv".to_string(),
        parser_version: CURSOR_ARCHIVE_PARSER_VERSION.to_string(),
        authoritative: true,
    }
}

fn sanitize_account_key(account: &str) -> String {
    let sanitized = account
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('-');
    let shortened = trimmed.get(..trimmed.len().min(80)).unwrap_or(trimmed);
    if shortened.is_empty() {
        "account".to_string()
    } else {
        shortened.to_string()
    }
}

fn cursor_observation(message: &UnifiedMessage) -> Result<ObservedTelemetryEvent> {
    let dedup_key = message
        .dedup_key
        .as_deref()
        .context("Cursor archive row is missing a stable identity")?;
    let (account, digest, occurrence) = parse_cursor_identity(dedup_key)
        .context("Cursor archive row has an invalid stable identity")?;
    let event = TelemetryEventInput::from_message(
        message,
        EventIdentity::raw_record("cursor", account, digest, occurrence),
        EventCost::reported(message.cost.max(0.0), "USD"),
        CURSOR_ARCHIVE_PARSER_VERSION,
    );
    Ok(ObservedTelemetryEvent::new(event))
}

fn parse_cursor_identity(key: &str) -> Option<(&str, &str, u32)> {
    let rest = key.strip_prefix("cursor:")?;
    let (account_and_digest, occurrence) = rest.rsplit_once(':')?;
    let (account, digest) = account_and_digest.rsplit_once(':')?;
    Some((account, digest, occurrence.parse().ok()?))
}

fn totals_for_messages(
    messages: &[UnifiedMessage],
) -> tokscale_core::telemetry::TelemetrySourceTotals {
    let mut tokens = TokenBreakdown::default();
    let mut cost = 0.0;
    for message in messages {
        tokens.input += message.tokens.input;
        tokens.output += message.tokens.output;
        tokens.cache_read += message.tokens.cache_read;
        tokens.cache_write += message.tokens.cache_write;
        tokens.reasoning += message.tokens.reasoning;
        cost += message.cost.max(0.0);
    }
    tokscale_core::telemetry::TelemetrySourceTotals {
        event_count: messages.len(),
        tokens,
        cost,
        min_occurred_at_ms: messages.iter().map(|message| message.timestamp).min(),
        max_occurred_at_ms: messages.iter().map(|message| message.timestamp).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "Date,Model,Input (w/ Cache Write),Input (w/o Cache Write),Cache Read,Output Tokens,Total Tokens,Cost,Cost to you\n2025-02-01,gpt-4o,10,5,0,15,30,$0.10,$0.10\n";

    fn assert_rejected_archive_needs_attention(
        csv: &str,
        expected_accepted: u64,
        expected_rejected: u64,
    ) {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("usage.csv"), csv).unwrap();
        let archive = temp.path().join("archive/cursor");
        let ledger = temp.path().join("data/telemetry.sqlite");

        let result = migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();

        assert_eq!(result.status, CursorMigrationStatus::NeedsAttention);
        assert_eq!(result.imported_events, 0);
        assert_eq!(result.attention_files, 1);
        let manifest = fs::read_dir(archive.join("manifests"))
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .map(|bytes| serde_json::from_slice::<CursorArchiveManifest>(&bytes).unwrap())
            .next()
            .unwrap();
        assert_eq!(manifest.accepted_rows, expected_accepted);
        assert_eq!(manifest.rejected_rows, expected_rejected);

        let store = TelemetryStore::open(&ledger).unwrap();
        let health = store.all_source_health().unwrap();
        assert_eq!(health.len(), 1);
        assert_eq!(
            health[0].status,
            tokscale_core::telemetry::TelemetrySourceStatus::Error
        );
        assert_eq!(
            health[0].issue_code.as_deref(),
            Some(CURSOR_ARCHIVE_REJECTED_ROWS_ISSUE)
        );
        assert_eq!(health[0].source_kind, TelemetrySourceKind::ArchiveImport);
        assert_eq!(health[0].client, "cursor");
        assert_eq!(health[0].parser_id, "cursor-csv");
        assert_eq!(health[0].parser_version, CURSOR_ARCHIVE_PARSER_VERSION);
        assert!(store
            .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
            .unwrap()
            .is_empty());
        assert!(default_migration_is_current(
            &cache, &archive, &ledger, None
        ));
    }

    #[test]
    fn migration_archives_imports_and_is_idempotent() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        let source = cache.join("usage.csv");
        fs::write(&source, CSV).unwrap();
        let archive = temp.path().join("archive/cursor");
        let ledger = temp.path().join("data/telemetry.sqlite");

        let first = migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();
        let second = migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();

        assert_eq!(first.status, CursorMigrationStatus::Complete);
        assert_eq!(first.imported_events, 1);
        assert_eq!(second.imported_events, 1);
        assert!(source.exists());
        let store = TelemetryStore::open(&ledger).unwrap();
        let messages = store
            .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].client, "cursor");
        assert!(default_migration_is_current(
            &cache, &archive, &ledger, None
        ));

        let source_id = store.all_source_health().unwrap()[0].source_id.clone();
        let stale_run = store.start_run("test-stale-parser", 10).unwrap();
        store
            .commit_source(
                &stale_run,
                &SourceDescriptor {
                    source_id,
                    source_kind: TelemetrySourceKind::ArchiveImport,
                    client: "cursor".to_string(),
                    source_ref: "stale-parser-fixture".to_string(),
                    source_location: None,
                    parser_id: "cursor-csv".to_string(),
                    parser_version: "cursor-csv-v0".to_string(),
                    authoritative: false,
                },
                SourceObservation::Complete(Vec::new()),
                10,
            )
            .unwrap();
        store.finish_run(&stale_run, 11).unwrap();
        assert!(!default_migration_is_current(
            &cache, &archive, &ledger, None
        ));

        migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();
        assert!(default_migration_is_current(
            &cache, &archive, &ledger, None
        ));

        fs::write(
            &source,
            format!("{CSV}2025-02-02,gpt-4o-mini,0,0,0,5,5,$0.05,$0.05\n"),
        )
        .unwrap();
        assert!(!default_migration_is_current(
            &cache, &archive, &ledger, None
        ));
    }

    #[test]
    fn failed_checked_ingest_does_not_pollute_existing_ledger() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        let csv_path = cache.join("usage.csv");
        fs::write(&csv_path, CSV).unwrap();
        let archive = temp.path().join("archive/cursor");
        let ledger = temp.path().join("data/telemetry.sqlite");
        migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();

        let store = TelemetryStore::open(&ledger).unwrap();
        let messages_before = store
            .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
            .unwrap();
        let health_before = store.all_source_health().unwrap();

        let first_messages = tokscale_core::sessions::cursor::parse_cursor_file_for_account(
            &csv_path,
            "rollback-first",
        );
        let first_events = first_messages
            .iter()
            .map(cursor_observation)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let first_source_id = "cursor-archive:rollback-first";
        let first = PreparedCursorSource {
            source: SourceDescriptor {
                source_id: first_source_id.to_string(),
                source_kind: TelemetrySourceKind::ArchiveImport,
                client: "cursor".to_string(),
                source_ref: "rollback-first".to_string(),
                source_location: None,
                parser_id: "cursor-csv".to_string(),
                parser_version: CURSOR_ARCHIVE_PARSER_VERSION.to_string(),
                authoritative: true,
            },
            observation: SourceObservation::Complete(first_events),
            expected_totals: Some(totals_for_messages(&first_messages)),
        };

        let second_messages = tokscale_core::sessions::cursor::parse_cursor_file_for_account(
            &csv_path,
            "rollback-second",
        );
        let second_events = second_messages
            .iter()
            .map(cursor_observation)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let mut wrong_totals = totals_for_messages(&second_messages);
        wrong_totals.tokens.input += 1;
        let second_source_id = "cursor-archive:rollback-second";
        let second = PreparedCursorSource {
            source: SourceDescriptor {
                source_id: second_source_id.to_string(),
                source_kind: TelemetrySourceKind::ArchiveImport,
                client: "cursor".to_string(),
                source_ref: "rollback-second".to_string(),
                source_location: None,
                parser_id: "cursor-csv".to_string(),
                parser_version: CURSOR_ARCHIVE_PARSER_VERSION.to_string(),
                authoritative: true,
            },
            observation: SourceObservation::Complete(second_events),
            expected_totals: Some(wrong_totals),
        };

        let error = ingest_prepared_cursor_sources(&store, vec![first, second], 9_999).unwrap_err();
        assert!(error
            .to_string()
            .contains("Cursor archive import parity mismatch"));
        assert_eq!(
            store
                .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
                .unwrap(),
            messages_before
        );
        assert_eq!(store.all_source_health().unwrap(), health_before);
        assert!(store.source_health(first_source_id).unwrap().is_none());
        assert!(store.source_health(second_source_id).unwrap().is_none());
    }

    #[test]
    fn resolved_duplicate_revises_pending_manifests_without_importing_unknown() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        let resolved_source = cache.join("usage.one.csv");
        let unknown_source = cache.join("usage.two.csv");
        fs::write(&resolved_source, CSV).unwrap();
        fs::write(&unknown_source, CSV).unwrap();
        let archive = temp.path().join("archive/cursor");
        let ledger = temp.path().join("data/telemetry.sqlite");

        let pending = migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();
        assert_eq!(pending.status, CursorMigrationStatus::NeedsAttention);
        assert_eq!(pending.imported_events, 0);
        let pending_manifests = fs::read_dir(archive.join("manifests"))
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .map(|bytes| serde_json::from_slice::<CursorArchiveManifest>(&bytes).unwrap())
            .collect::<Vec<_>>();
        let pending_resolved_source = pending_manifests
            .iter()
            .find(|manifest| manifest.account_key == "one")
            .unwrap();
        assert_eq!(
            pending_resolved_source.account_resolution,
            CursorAccountResolution::Ambiguous
        );
        assert!(!pending_resolved_source.selected_for_import);
        let pending_manifest_path = archive
            .join("manifests")
            .join(format!("{}.json", pending_resolved_source.manifest_id));
        let pending_manifest_bytes = fs::read(&pending_manifest_path).unwrap();

        let accounts = CursorAccountIndex {
            active_account_id: "one".to_string(),
            account_ids: vec!["one".to_string()],
        };
        let resolved = migrate_cursor_history(&cache, &archive, &ledger, Some(&accounts)).unwrap();

        assert_eq!(resolved.status, CursorMigrationStatus::NeedsAttention);
        assert_eq!(resolved.archived_files, 2);
        assert_eq!(resolved.imported_events, 1);
        assert_eq!(resolved.attention_files, 1);
        let manifests = fs::read_dir(archive.join("manifests"))
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .map(|bytes| serde_json::from_slice::<CursorArchiveManifest>(&bytes).unwrap())
            .collect::<Vec<_>>();
        let importable = manifests
            .iter()
            .find(|manifest| {
                manifest.account_key == "one"
                    && manifest.account_resolution == CursorAccountResolution::Resolved
                    && manifest.selected_for_import
            })
            .unwrap();
        assert_eq!(
            importable.supersedes.as_deref(),
            Some(pending_resolved_source.manifest_id.as_str())
        );
        assert!(manifests.iter().any(|manifest| {
            manifest.account_key == "two"
                && manifest.account_resolution == CursorAccountResolution::Unknown
                && !manifest.selected_for_import
        }));
        assert_eq!(
            fs::read_dir(archive.join("objects/sha256"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read(&pending_manifest_path).unwrap(),
            pending_manifest_bytes
        );
        assert_eq!(fs::read(&resolved_source).unwrap(), CSV.as_bytes());
        assert_eq!(fs::read(&unknown_source).unwrap(), CSV.as_bytes());

        let store = TelemetryStore::open(&ledger).unwrap();
        assert_eq!(
            store
                .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
                .unwrap()
                .len(),
            1
        );
        assert!(default_migration_is_current(
            &cache,
            &archive,
            &ledger,
            Some(&accounts)
        ));

        let pending_again = migrate_cursor_history(&cache, &archive, &ledger, None).unwrap();
        assert_eq!(pending_again.imported_events, 0);
        assert!(default_migration_is_current(
            &cache, &archive, &ledger, None
        ));
        assert!(!default_migration_is_current(
            &cache,
            &archive,
            &ledger,
            Some(&accounts)
        ));

        let resolved_again =
            migrate_cursor_history(&cache, &archive, &ledger, Some(&accounts)).unwrap();
        assert_eq!(resolved_again.imported_events, 1);
        assert!(default_migration_is_current(
            &cache,
            &archive,
            &ledger,
            Some(&accounts)
        ));
        assert_eq!(
            store
                .load_messages(&tokscale_core::telemetry::TelemetryQuery::all_history())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn partially_rejected_csv_is_not_reported_as_complete() {
        let csv = format!("{CSV}not-a-date,gpt-4o,10,5,0,15,30,$0.10,$0.10\n");
        assert_rejected_archive_needs_attention(&csv, 1, 1);
    }

    #[test]
    fn fully_rejected_csv_is_not_reported_as_complete() {
        let header = CSV.lines().next().unwrap();
        let csv = format!("{header}\nnot-a-date,gpt-4o,10,5,0,15,30,$0.10,$0.10\n");
        assert_rejected_archive_needs_attention(&csv, 0, 1);
    }

    #[test]
    fn duplicate_unresolved_files_are_archived_but_not_imported() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("usage.one.csv"), CSV).unwrap();
        fs::write(cache.join("usage.two.csv"), CSV).unwrap();

        let result = migrate_cursor_history(
            &cache,
            &temp.path().join("archive/cursor"),
            &temp.path().join("data/telemetry.sqlite"),
            None,
        )
        .unwrap();

        assert_eq!(result.status, CursorMigrationStatus::NeedsAttention);
        assert_eq!(result.archived_files, 2);
        assert_eq!(result.imported_events, 0);
        assert_eq!(result.attention_files, 2);
    }

    #[test]
    fn sanitized_account_collision_requires_attention() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache = temp.path().join("cursor-cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("usage.team-one.csv"), CSV).unwrap();
        let credentials = CursorAccountIndex {
            active_account_id: "team/one".to_string(),
            account_ids: vec!["team/one".to_string(), "team-one".to_string()],
        };

        let result = migrate_cursor_history(
            &cache,
            &temp.path().join("archive/cursor"),
            &temp.path().join("data/telemetry.sqlite"),
            Some(&credentials),
        )
        .unwrap();

        assert_eq!(result.status, CursorMigrationStatus::NeedsAttention);
        assert_eq!(result.imported_events, 0);
    }
}
