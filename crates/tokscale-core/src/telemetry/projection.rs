use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{
    CheckedIngest, EventCost, EventIdentity, ObservedTelemetryEvent, SourceDescriptor,
    SourceObservation, TelemetryError, TelemetryEventInput, TelemetrySourceKind, TelemetryStore,
};
use crate::sessions::UnifiedMessage;

pub const LEGACY_PROJECTION_VERSION: &str = "legacy-unified-v1";

/// Privacy-safe outcome for the temporary dual-run comparison.
///
/// Deliberately carries no paths, source ids, dates, models, totals, or raw
/// errors, so callers may surface it without exposing personal telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryParityStatus {
    Matched,
    Mismatch,
    NoEligibleEvents,
}

#[derive(Debug, Clone)]
pub struct TelemetryReconcileResult {
    pub messages: Vec<UnifiedMessage>,
    pub parity: TelemetryParityStatus,
}

#[derive(Debug)]
struct ProjectedSource {
    descriptor: SourceDescriptor,
    events: BTreeMap<String, ObservedTelemetryEvent>,
}

/// Ingest identity-safe legacy parser output, then return durable history plus
/// current messages that are not yet safe to persist.
///
/// This adapter is intentionally non-authoritative because the legacy flat
/// vector no longer retains physical source success/failure information. A
/// later source-aware adapter may mark events missing; this bridge never does.
pub fn reconcile_legacy_messages(
    store: &TelemetryStore,
    current_messages: Vec<UnifiedMessage>,
    requested_clients: &[String],
    observed_at_ms: i64,
) -> Result<TelemetryReconcileResult, TelemetryError> {
    let mut fallback = Vec::new();
    let mut projected_fallback = Vec::new();
    let mut sources = BTreeMap::<String, ProjectedSource>::new();

    for message in current_messages {
        let Some((source_namespace, identity)) = identity_for_message(&message) else {
            fallback.push(message);
            continue;
        };
        if !message_is_ledger_safe(&message) {
            fallback.push(message);
            continue;
        }

        let cost = projected_cost(&message);
        let input =
            TelemetryEventInput::from_message(&message, identity, cost, LEGACY_PROJECTION_VERSION);
        let event_key = super::event_id(&input.identity);
        let source = sources
            .entry(source_namespace.to_string())
            .or_insert_with(|| ProjectedSource {
                descriptor: SourceDescriptor {
                    source_id: format!("legacy-projection:{source_namespace}"),
                    source_kind: TelemetrySourceKind::LocalParser,
                    client: source_namespace.to_string(),
                    source_ref: "legacy-normalized-stream".to_string(),
                    source_location: None,
                    parser_id: "legacy-unified-projection".to_string(),
                    parser_version: LEGACY_PROJECTION_VERSION.to_string(),
                    authoritative: false,
                },
                events: BTreeMap::new(),
            });
        source
            .events
            .insert(event_key, ObservedTelemetryEvent::new(input));
        projected_fallback.push(message);
    }

    let parity = if sources.is_empty() {
        TelemetryParityStatus::NoEligibleEvents
    } else {
        store.ingest_checked(
            LEGACY_PROJECTION_VERSION,
            observed_at_ms,
            observed_at_ms,
            |context| {
                let mut matched = true;
                for source in sources.values() {
                    let events = source.events.values().cloned().collect::<Vec<_>>();
                    let projected = events
                        .iter()
                        .map(|observed| observed.event.clone())
                        .collect::<Vec<_>>();
                    context.commit_source(
                        &source.descriptor,
                        SourceObservation::Complete(events),
                        observed_at_ms,
                    )?;
                    matched &= context.projected_source_matches(&source.descriptor, &projected)?;
                }

                let outcome = if matched {
                    CheckedIngest::Commit(TelemetryParityStatus::Matched)
                } else {
                    CheckedIngest::Rollback(TelemetryParityStatus::Mismatch)
                };
                Ok::<_, TelemetryError>(outcome)
            },
        )?
    };

    if parity == TelemetryParityStatus::Mismatch {
        fallback.extend(projected_fallback);
        sort_messages(&mut fallback);
        return Ok(TelemetryReconcileResult {
            messages: fallback,
            parity,
        });
    }

    let requested = requested_clients.iter().map(String::as_str).collect();
    let mut history = store.load_messages(&super::TelemetryQuery::all_history())?;
    history.retain(|message| matches_requested(message, &requested));
    remove_durable_fallback_duplicates(&history, &mut fallback);
    history.extend(fallback);
    sort_messages(&mut history);

    Ok(TelemetryReconcileResult {
        messages: history,
        parity,
    })
}

fn sort_messages(messages: &mut [UnifiedMessage]) {
    messages.sort_unstable_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.client.cmp(&right.client))
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.model_id.cmp(&right.model_id))
    });
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FallbackFingerprint {
    client: String,
    provider_id: String,
    model_id: String,
    session_id: Option<String>,
    workspace_key: Option<String>,
    workspace_label: Option<String>,
    occurred_at_ms: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    duration_ms: Option<i64>,
    message_count: i32,
    agent: Option<String>,
    is_turn_start: bool,
}

fn remove_durable_fallback_duplicates(
    history: &[UnifiedMessage],
    fallback: &mut Vec<UnifiedMessage>,
) {
    let mut durable = HashMap::<FallbackFingerprint, usize>::new();
    for message in history {
        *durable.entry(fallback_fingerprint(message)).or_default() += 1;
    }
    fallback.retain(|message| {
        let fingerprint = fallback_fingerprint(message);
        let Some(remaining) = durable.get_mut(&fingerprint) else {
            return true;
        };
        if *remaining == 0 {
            return true;
        }
        *remaining -= 1;
        false
    });
}

fn fallback_fingerprint(message: &UnifiedMessage) -> FallbackFingerprint {
    FallbackFingerprint {
        client: message.client.clone(),
        provider_id: message.provider_id.clone(),
        model_id: message.model_id.clone(),
        // Old Cursor caches used a filename-derived account in session ids,
        // while archive imports use an explicit account scope. The remaining
        // immutable row fields are sufficient for multiset reconciliation.
        session_id: (message.client != "cursor").then(|| message.session_id.clone()),
        workspace_key: message.workspace_key.clone(),
        workspace_label: message.workspace_label.clone(),
        occurred_at_ms: message.timestamp,
        input: message.tokens.input,
        output: message.tokens.output,
        cache_read: message.tokens.cache_read,
        cache_write: message.tokens.cache_write,
        reasoning: message.tokens.reasoning,
        duration_ms: message.duration_ms,
        message_count: message.message_count,
        agent: message.agent.clone(),
        is_turn_start: message.is_turn_start,
    }
}

pub(crate) fn reconcile_legacy_messages_best_effort(
    store_path: Option<&Path>,
    current_messages: Vec<UnifiedMessage>,
    requested_clients: &[String],
) -> Vec<UnifiedMessage> {
    let Some(store_path) = store_path else {
        return current_messages;
    };
    let fallback = current_messages.clone();
    let observed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default();
    let store = match TelemetryStore::open(store_path.to_path_buf()) {
        Ok(store) => store,
        Err(_) => {
            tracing::warn!(
                issue_code = "telemetry_store_unavailable",
                "durable telemetry is unavailable; using current parser output"
            );
            return fallback;
        }
    };

    match reconcile_legacy_messages(&store, current_messages, requested_clients, observed_at_ms) {
        Ok(result) => {
            if result.parity == TelemetryParityStatus::Mismatch {
                tracing::warn!(
                    issue_code = "telemetry_projection_mismatch",
                    "durable telemetry parity check failed; using current parser output"
                );
                return fallback;
            }
            result.messages
        }
        Err(_) => {
            tracing::warn!(
                issue_code = "telemetry_ingest_failed",
                "durable telemetry ingest failed; using current parser output"
            );
            fallback
        }
    }
}

fn matches_requested(message: &UnifiedMessage, requested: &HashSet<&str>) -> bool {
    requested.is_empty()
        || crate::local_parse::retain_for_requested_clients(
            &message.client,
            &message.model_id,
            &message.provider_id,
            requested,
        )
}

fn message_is_ledger_safe(message: &UnifiedMessage) -> bool {
    message.timestamp >= 0
        && message.message_count >= 0
        && message.tokens.input >= 0
        && message.tokens.output >= 0
        && message.tokens.cache_read >= 0
        && message.tokens.cache_write >= 0
        && message.tokens.reasoning >= 0
        && !message.client.trim().is_empty()
        && !message.model_id.trim().is_empty()
        && !message.provider_id.trim().is_empty()
        && !message.session_id.trim().is_empty()
}

fn projected_cost(message: &UnifiedMessage) -> EventCost {
    if message.cost.is_finite() && message.cost > 0.0 {
        EventCost::Estimated {
            amount: message.cost,
            currency: "USD".to_string(),
            pricing_source: Some("legacy-finalized".to_string()),
            pricing_key: None,
        }
    } else {
        EventCost::Unknown
    }
}

fn identity_for_message(message: &UnifiedMessage) -> Option<(&'static str, EventIdentity)> {
    let dedup_key = message
        .dedup_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())?;
    let namespace = immutable_namespace(&message.client)?;

    if namespace == "cursor" {
        let (account, digest, occurrence) = parse_cursor_dedup_key(dedup_key)?;
        return Some((
            namespace,
            EventIdentity::raw_record(namespace, account, digest, occurrence),
        ));
    }

    Some((
        namespace,
        EventIdentity::native(namespace, message.session_id.clone(), dedup_key),
    ))
}

fn immutable_namespace(client: &str) -> Option<&'static str> {
    match client {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        value if value.starts_with("cc-mirror/") => Some("claude"),
        "opencode" => Some("opencode"),
        "cursor" => Some("cursor"),
        "copilot" => Some("copilot"),
        "codebuff" => Some("codebuff"),
        "gjc" => Some("gjc"),
        "pi" => Some("pi"),
        "kimi" => Some("kimi"),
        "kilo" => Some("kilo"),
        "kiro" => Some("kiro"),
        "antigravity" => Some("antigravity"),
        "synthetic" | "octofriend" => Some("synthetic"),
        // These parsers currently emit cumulative or mutable snapshots. They
        // need explicit SnapshotBucket adapters before durable ingestion.
        "trae" | "warp" | "mux" | "goose" | "hermes" | "droid" | "zed" | "crush" => None,
        _ => None,
    }
}

fn parse_cursor_dedup_key(key: &str) -> Option<(&str, &str, u32)> {
    let rest = key.strip_prefix("cursor:")?;
    let (account_and_digest, occurrence) = rest.rsplit_once(':')?;
    let occurrence = occurrence.parse().ok()?;
    let (account, digest) = account_and_digest.rsplit_once(':')?;
    if account.is_empty() || digest.is_empty() {
        return None;
    }
    Some((account, digest, occurrence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenBreakdown;

    fn store() -> (tempfile::TempDir, TelemetryStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let store = TelemetryStore::open(temp.path().join("telemetry.sqlite")).unwrap();
        (temp, store)
    }

    fn ledger_counts(connection: &rusqlite::Connection) -> (i64, i64, i64, i64) {
        connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM telemetry_ingest_runs),
                    (SELECT COUNT(*) FROM telemetry_sources),
                    (SELECT COUNT(*) FROM telemetry_events),
                    (SELECT COUNT(*) FROM telemetry_event_sources)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap()
    }

    fn message(client: &str, dedup_key: Option<&str>, timestamp: i64) -> UnifiedMessage {
        UnifiedMessage::new_with_dedup(
            client,
            "gpt-5",
            "openai",
            "session-1",
            timestamp,
            TokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 2,
                cache_write: 1,
                reasoning: 0,
            },
            0.25,
            dedup_key.map(str::to_string),
        )
    }

    #[test]
    fn identity_safe_events_survive_an_empty_later_scan() {
        let (_temp, store) = store();
        let first = reconcile_legacy_messages(
            &store,
            vec![message("codex", Some("codex:event-1"), 100)],
            &["codex".to_string()],
            1_000,
        )
        .unwrap();
        assert_eq!(first.parity, TelemetryParityStatus::Matched);
        assert_eq!(first.messages.len(), 1);

        let second =
            reconcile_legacy_messages(&store, Vec::new(), &["codex".to_string()], 2_000).unwrap();
        assert_eq!(second.parity, TelemetryParityStatus::NoEligibleEvents);
        assert_eq!(second.messages.len(), 1);
    }

    #[test]
    fn repeated_native_identity_updates_without_duplication() {
        let (_temp, store) = store();
        let mut corrected = message("codex", Some("codex:event-1"), 100);
        corrected.tokens.input = 42;

        reconcile_legacy_messages(
            &store,
            vec![message("codex", Some("codex:event-1"), 100)],
            &["codex".to_string()],
            1_000,
        )
        .unwrap();
        let result =
            reconcile_legacy_messages(&store, vec![corrected], &["codex".to_string()], 2_000)
                .unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].tokens.input, 42);
    }

    #[test]
    fn parity_mismatch_rolls_back_the_entire_projection_ingest() {
        let (_temp, store) = store();
        reconcile_legacy_messages(
            &store,
            vec![message("opencode", Some("message-1"), 100)],
            &[],
            1_000,
        )
        .unwrap();

        let history_before = store
            .load_messages(&super::super::TelemetryQuery::all_history())
            .unwrap();
        let connection = rusqlite::Connection::open(store.path()).unwrap();
        let counts_before = ledger_counts(&connection);
        connection
            .execute_batch(
                "CREATE TRIGGER remove_projected_codex_mapping
                 AFTER INSERT ON telemetry_event_sources
                 WHEN NEW.source_id = 'legacy-projection:codex'
                 BEGIN
                     DELETE FROM telemetry_event_sources
                     WHERE event_id = NEW.event_id AND source_id = NEW.source_id;
                 END;",
            )
            .unwrap();
        drop(connection);

        let mut corrected_history = message("opencode", Some("message-1"), 100);
        corrected_history.tokens.input = 42;
        let current_messages = vec![
            corrected_history,
            message("codex", Some("codex:event-2"), 200),
        ];
        let result =
            reconcile_legacy_messages(&store, current_messages.clone(), &[], 2_000).unwrap();

        assert_eq!(result.parity, TelemetryParityStatus::Mismatch);
        assert_eq!(result.messages, current_messages);

        let history_after = store
            .load_messages(&super::super::TelemetryQuery::all_history())
            .unwrap();
        assert_eq!(history_after, history_before);

        let connection = rusqlite::Connection::open(store.path()).unwrap();
        let counts_after = ledger_counts(&connection);
        assert_eq!(counts_after, counts_before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM telemetry_ingest_runs WHERE started_at_ms = 2000",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM telemetry_sources
                     WHERE source_id = 'legacy-projection:codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM telemetry_events WHERE client = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn snapshot_and_unkeyed_messages_remain_live_only() {
        let (_temp, store) = store();
        let result = reconcile_legacy_messages(
            &store,
            vec![
                message("trae", Some("trae:session:1"), 100),
                message("codex", None, 200),
            ],
            &[],
            1_000,
        )
        .unwrap();

        assert_eq!(result.parity, TelemetryParityStatus::NoEligibleEvents);
        assert_eq!(result.messages.len(), 2);
        assert!(store
            .load_messages(&super::super::TelemetryQuery::all_history())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unkeyed_cursor_cache_does_not_duplicate_imported_history() {
        let (_temp, store) = store();
        let mut archived = message("cursor", Some("cursor:legacy-active:abcdef:0"), 100);
        archived.session_id = "cursor-legacy-active-date".to_string();
        reconcile_legacy_messages(&store, vec![archived], &[], 1_000).unwrap();

        let mut cached = message("cursor", None, 100);
        cached.session_id = "cursor-active-date".to_string();
        let result = reconcile_legacy_messages(&store, vec![cached], &[], 2_000).unwrap();

        assert_eq!(result.parity, TelemetryParityStatus::NoEligibleEvents);
        assert_eq!(result.messages.len(), 1);
    }

    #[test]
    fn fallback_reconciliation_keeps_structurally_distinct_agent_events() {
        let mut durable = message("codex", None, 100);
        durable.agent = Some("reviewer".to_string());
        let mut fallback = vec![message("codex", None, 100)];
        fallback[0].agent = Some("builder".to_string());

        remove_durable_fallback_duplicates(&[durable], &mut fallback);

        assert_eq!(fallback.len(), 1);
    }

    #[test]
    fn requested_clients_filter_durable_history() {
        let (_temp, store) = store();
        reconcile_legacy_messages(
            &store,
            vec![
                message("codex", Some("codex:event-1"), 100),
                message("opencode", Some("message-2"), 200),
            ],
            &[],
            1_000,
        )
        .unwrap();

        let result =
            reconcile_legacy_messages(&store, Vec::new(), &["codex".to_string()], 2_000).unwrap();
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].client, "codex");
    }

    #[test]
    fn cursor_row_identity_uses_account_digest_and_occurrence() {
        let message = message("cursor", Some("cursor:account:with:colon:abcdef:2"), 100);
        let (_, identity) = identity_for_message(&message).unwrap();

        assert_eq!(
            identity,
            EventIdentity::raw_record("cursor", "account:with:colon", "abcdef", 2)
        );
    }

    #[test]
    fn parity_status_serializes_without_telemetry_details() {
        assert_eq!(
            serde_json::to_string(&TelemetryParityStatus::Mismatch).unwrap(),
            "\"mismatch\""
        );
    }
}
