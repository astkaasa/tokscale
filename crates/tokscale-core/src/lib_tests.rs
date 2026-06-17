use super::{
    aggregate_model_usage_entries, apply_pricing_if_available, dedupe_latest_trae_messages,
    message_cache, normalize_model_for_grouping, parse_all_messages_with_pricing,
    parse_local_clients, parsed_to_unified, pricing, retain_for_requested_clients, scanner,
    select_local_parse_pricing, unified_to_parsed, ClientId, GroupBy, LocalParseOptions,
    TokenBreakdown, UnifiedMessage, UNKNOWN_WORKSPACE_LABEL,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;

fn make_workspace_message(
    client: &str,
    model_id: &str,
    provider_id: &str,
    session_id: &str,
    cost: f64,
    workspace_key: Option<&str>,
    workspace_label: Option<&str>,
) -> UnifiedMessage {
    let mut msg = UnifiedMessage::new(
        client,
        model_id,
        provider_id,
        session_id,
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        cost,
    );
    msg.set_workspace(
        workspace_key.map(str::to_string),
        workspace_label.map(str::to_string),
    );
    msg
}

fn make_trae_message(
    session_id: &str,
    timestamp: i64,
    dedup_key: Option<&str>,
    cost: f64,
) -> UnifiedMessage {
    UnifiedMessage::new_with_dedup(
        "trae",
        "gpt-5.2",
        "openai",
        session_id,
        timestamp,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        cost,
        dedup_key.map(str::to_string),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_opencode_sqlite_payload(
    created_ms: f64,
    completed_ms: f64,
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
    cost: f64,
) -> String {
    format!(
        r#"{{
                "role": "assistant",
                "modelID": "claude-sonnet-4",
                "providerID": "anthropic",
                "cost": {cost},
                "tokens": {{
                    "input": {input},
                    "output": {output},
                    "reasoning": {reasoning},
                    "cache": {{ "read": {cache_read}, "write": {cache_write} }}
                }},
                "time": {{ "created": {created_ms}, "completed": {completed_ms} }},
                "mode": "build"
            }}"#
    )
}

fn create_opencode_sqlite_db(db_path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                data TEXT NOT NULL
            );",
    )
    .unwrap();
    conn
}

fn create_hermes_sqlite_db(db_path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                model TEXT,
                started_at REAL NOT NULL,
                message_count INTEGER DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                billing_provider TEXT,
                estimated_cost_usd REAL,
                actual_cost_usd REAL
            );",
    )
    .unwrap();
    conn
}

fn create_zed_sqlite_db(db_path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                data_type TEXT NOT NULL,
                data BLOB NOT NULL
            );",
    )
    .unwrap();
    conn
}

fn insert_zed_thread(conn: &rusqlite::Connection, id: &str, model: &str) {
    let payload = format!(
        r#"{{
                "version": "0.3.0",
                "title": "Test thread",
                "updated_at": "2026-05-01T12:30:00Z",
                "request_token_usage": {{
                    "turn-1": {{
                        "input_tokens": 42,
                        "output_tokens": 7,
                        "cache_creation_input_tokens": 3,
                        "cache_read_input_tokens": 5
                    }}
                }},
                "model": {{
                    "provider": "zed.dev",
                    "model": "{model}"
                }},
                "imported": false
            }}"#
    );
    conn.execute(
            "INSERT INTO threads (id, summary, updated_at, data_type, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, "Test thread", "2026-05-01T12:30:00Z", "json", payload.as_bytes()],
        )
        .unwrap();
}

fn insert_hermes_session(
    conn: &rusqlite::Connection,
    id: &str,
    model: &str,
    message_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    actual_cost_usd: f64,
) {
    conn.execute(
            "INSERT INTO sessions (
                id, source, model, started_at, message_count,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                billing_provider, estimated_cost_usd, actual_cost_usd
            ) VALUES (?1, 'cli', ?2, 1775001102.0, ?3, ?4, ?5, 0, 0, 0, 'anthropic', NULL, ?6)",
            rusqlite::params![
                id,
                model,
                message_count,
                input_tokens,
                output_tokens,
                actual_cost_usd
            ],
        )
        .unwrap();
}

#[test]
fn test_normalize_model_for_grouping() {
    assert_eq!(
        normalize_model_for_grouping("claude-opus-4-5-20251101"),
        "claude-opus-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-sonnet-4-5-20250929"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-sonnet-4-20250514"),
        "claude-sonnet-4"
    );

    assert_eq!(
        normalize_model_for_grouping("claude-opus-4.5"),
        "claude-opus-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-sonnet-4.5"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-opus-4.6"),
        "claude-opus-4-6"
    );
    assert_eq!(
        normalize_model_for_grouping("anthropic/claude-4-6-sonnet"),
        "claude-sonnet-4-6"
    );
    assert_eq!(
        normalize_model_for_grouping("anthropic/claude-4-5-haiku"),
        "claude-haiku-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("anthropic/claude-4-6-opus"),
        "claude-opus-4-6"
    );

    assert_eq!(normalize_model_for_grouping("gpt-5.2"), "gpt-5.2");
    assert_eq!(normalize_model_for_grouping("gpt-5.4(xhigh)"), "gpt-5.4");
    assert_eq!(normalize_model_for_grouping("gpt-5.4(high)"), "gpt-5.4");
    assert_eq!(normalize_model_for_grouping("gpt-5.4(minimal)"), "gpt-5.4");
    assert_eq!(normalize_model_for_grouping("gpt-5.4(auto)"), "gpt-5.4");
    assert_eq!(normalize_model_for_grouping("gpt-5.4(none)"), "gpt-5.4");
    assert_eq!(
        normalize_model_for_grouping("gpt-5.4(weirdgarbage)"),
        "gpt-5.4(weirdgarbage)"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-sonnet-4.5(high)"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        normalize_model_for_grouping("gemini-3-pro(auto)"),
        "gemini-3-pro"
    );
    assert_eq!(
        normalize_model_for_grouping("gemini-2.5-pro"),
        "gemini-2.5-pro"
    );

    assert_eq!(
        normalize_model_for_grouping("claude-opus-4-5-high"),
        "claude-opus-4-5-high"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-opus-4-5-thinking-high"),
        "claude-opus-4-5-thinking-high"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-sonnet-4-5-high"),
        "claude-sonnet-4-5-high"
    );

    assert_eq!(
        normalize_model_for_grouping("claude-4-sonnet"),
        "claude-4-sonnet"
    );
    assert_eq!(
        normalize_model_for_grouping("claude-4-opus-thinking"),
        "claude-4-opus-thinking"
    );

    assert_eq!(normalize_model_for_grouping("big-pickle"), "big-pickle");
    assert_eq!(normalize_model_for_grouping("grok-code"), "grok-code");

    assert_eq!(
        normalize_model_for_grouping("claude-opus-4.5-20251101"),
        "claude-opus-4-5"
    );
}

#[test]
fn test_group_by_from_str_valid_values() {
    assert_eq!(GroupBy::from_str("model").unwrap(), GroupBy::Model);
    assert_eq!(
        GroupBy::from_str("client,model").unwrap(),
        GroupBy::ClientModel
    );
    assert_eq!(
        GroupBy::from_str("client-model").unwrap(),
        GroupBy::ClientModel
    );
    assert_eq!(
        GroupBy::from_str("client,provider,model").unwrap(),
        GroupBy::ClientProviderModel
    );
    assert_eq!(
        GroupBy::from_str("client-provider-model").unwrap(),
        GroupBy::ClientProviderModel
    );
    assert_eq!(
        GroupBy::from_str("workspace,model").unwrap(),
        GroupBy::WorkspaceModel
    );
    assert_eq!(
        GroupBy::from_str("workspace-model").unwrap(),
        GroupBy::WorkspaceModel
    );
    assert_eq!(GroupBy::from_str("session").unwrap(), GroupBy::Session);
    assert_eq!(
        GroupBy::from_str("session,model").unwrap(),
        GroupBy::Session
    );
    assert_eq!(
        GroupBy::from_str("session-model").unwrap(),
        GroupBy::Session
    );
    assert_eq!(
        GroupBy::from_str("client,session").unwrap(),
        GroupBy::ClientSession
    );
    assert_eq!(
        GroupBy::from_str("client,session,model").unwrap(),
        GroupBy::ClientSession
    );
    assert_eq!(
        GroupBy::from_str("client-session-model").unwrap(),
        GroupBy::ClientSession
    );
    assert!(GroupBy::from_str("unknown").is_err());
}

#[test]
fn test_group_by_default_is_client_model() {
    assert_eq!(GroupBy::default(), GroupBy::ClientModel);
}

#[test]
fn test_group_by_display_round_trips_with_from_str() {
    let variants = [
        GroupBy::Model,
        GroupBy::ClientModel,
        GroupBy::ClientProviderModel,
        GroupBy::WorkspaceModel,
        GroupBy::Session,
        GroupBy::ClientSession,
    ];

    for variant in variants {
        let rendered = variant.to_string();
        let parsed = GroupBy::from_str(&rendered).unwrap();
        assert_eq!(parsed, variant);
    }
}

#[test]
fn test_group_by_from_str_whitespace_handling() {
    assert_eq!(
        GroupBy::from_str("client, model").unwrap(),
        GroupBy::ClientModel
    );
    assert_eq!(GroupBy::from_str(" model ").unwrap(), GroupBy::Model);
    assert_eq!(
        GroupBy::from_str("client , provider , model").unwrap(),
        GroupBy::ClientProviderModel
    );
    assert_eq!(
        GroupBy::from_str("workspace, model").unwrap(),
        GroupBy::WorkspaceModel
    );
}

#[test]
fn test_model_usage_performance_uses_only_timed_positive_token_messages() {
    let mut timed = make_workspace_message(
        "opencode",
        "gpt-5.4",
        "openai",
        "session-1",
        0.0,
        None,
        None,
    );
    timed.tokens = TokenBreakdown {
        input: 100,
        output: 50,
        cache_read: 25,
        cache_write: 0,
        reasoning: 25,
    };
    timed.duration_ms = Some(400);

    let mut untimed = make_workspace_message(
        "opencode",
        "gpt-5.4",
        "openai",
        "session-2",
        0.0,
        None,
        None,
    );
    untimed.tokens = TokenBreakdown {
        input: 300,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
    };

    let entries = aggregate_model_usage_entries(vec![timed, untimed], &GroupBy::ClientModel);

    assert_eq!(entries.len(), 1);
    let performance = &entries[0].performance;
    assert_eq!(performance.total_duration_ms, 400);
    assert_eq!(performance.timed_tokens, 200);
    assert_eq!(performance.sample_count, 1);
    assert_eq!(performance.ms_per_1k_tokens, Some(2000.0));
    assert!((performance.token_coverage - 0.4).abs() < f64::EPSILON);
}

#[test]
fn test_model_usage_performance_is_null_without_duration_samples() {
    let entries = aggregate_model_usage_entries(
        vec![make_workspace_message(
            "claude",
            "claude-sonnet-4-5",
            "anthropic",
            "session-1",
            0.0,
            None,
            None,
        )],
        &GroupBy::ClientModel,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].performance.ms_per_1k_tokens, None);
    assert_eq!(entries[0].performance.total_duration_ms, 0);
    assert_eq!(entries[0].performance.timed_tokens, 0);
    assert_eq!(entries[0].performance.token_coverage, 0.0);
}

#[test]
fn test_workspace_model_grouping_merges_same_workspace_and_model() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-1",
                1.25,
                Some("/repo-a"),
                Some("repo-a"),
            ),
            make_workspace_message(
                "qwen",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-2",
                2.75,
                Some("/repo-a"),
                Some("repo-a"),
            ),
        ],
        &GroupBy::WorkspaceModel,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model, "claude-sonnet-4-5");
    assert_eq!(entries[0].workspace_key.as_deref(), Some("/repo-a"));
    assert_eq!(entries[0].workspace_label.as_deref(), Some("repo-a"));
    assert_eq!(entries[0].cost, 4.0);
    assert_eq!(entries[0].message_count, 2);
    assert_eq!(entries[0].merged_clients.as_deref(), Some("claude, qwen"));
}

#[test]
fn test_model_grouping_merges_anthropic_prefixed_claude_variant_with_canonical_model() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "anthropic/claude-4-6-sonnet",
                "anthropic",
                "session-1",
                1.25,
                Some("/repo-a"),
                Some("repo-a"),
            ),
            make_workspace_message(
                "claude",
                "claude-sonnet-4-6",
                "anthropic",
                "session-2",
                2.75,
                Some("/repo-b"),
                Some("repo-b"),
            ),
        ],
        &GroupBy::ClientModel,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model, "claude-sonnet-4-6");
    assert_eq!(entries[0].input, 20);
    assert_eq!(entries[0].output, 10);
    assert_eq!(entries[0].cost, 4.0);
    assert_eq!(entries[0].message_count, 2);
}

#[test]
fn test_workspace_model_grouping_separates_different_workspaces() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-1",
                1.0,
                Some("/repo-a"),
                Some("repo-a"),
            ),
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-2",
                2.0,
                Some("/repo-b"),
                Some("repo-b"),
            ),
        ],
        &GroupBy::WorkspaceModel,
    );

    assert_eq!(entries.len(), 2);
    let labels: HashSet<_> = entries
        .iter()
        .map(|entry| entry.workspace_label.as_deref().unwrap())
        .collect();
    assert_eq!(labels, HashSet::from(["repo-a", "repo-b"]));
}

#[test]
fn test_workspace_model_grouping_uses_unknown_bucket_without_workspace_metadata() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-1",
                1.0,
                None,
                None,
            ),
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-2",
                "2.0".parse().unwrap(),
                None,
                None,
            ),
        ],
        &GroupBy::WorkspaceModel,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].workspace_key, None);
    assert_eq!(
        entries[0].workspace_label.as_deref(),
        Some(UNKNOWN_WORKSPACE_LABEL)
    );
    assert_eq!(entries[0].message_count, 2);
    assert_eq!(entries[0].cost, 3.0);
}

#[test]
fn test_parsed_round_trip_preserves_workspace_metadata() {
    let mut unified = UnifiedMessage::new(
        "qwen",
        "qwen3.5-plus",
        "qwen",
        "session-1",
        1_742_390_400_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 2,
            cache_write: 0,
            reasoning: 1,
        },
        1.25,
    );
    unified.set_workspace(
        Some("//server/share/demo-workspace".to_string()),
        Some("demo-workspace".to_string()),
    );
    unified.duration_ms = Some(2500);

    let parsed = unified_to_parsed(&unified);
    let round_tripped = parsed_to_unified(&parsed, 2.5);

    assert_eq!(
        round_tripped.workspace_key.as_deref(),
        Some("//server/share/demo-workspace")
    );
    assert_eq!(
        round_tripped.workspace_label.as_deref(),
        Some("demo-workspace")
    );
    assert_eq!(round_tripped.cost, 2.5);
    assert_eq!(round_tripped.duration_ms, Some(2500));
}

#[test]
fn test_workspace_model_grouping_keeps_real_unknown_workspace_separate() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-1",
                1.0,
                Some("unknown-workspace"),
                Some("unknown-workspace"),
            ),
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-2",
                2.0,
                None,
                None,
            ),
        ],
        &GroupBy::WorkspaceModel,
    );

    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| {
        entry.workspace_key.as_deref() == Some("unknown-workspace")
            && entry.workspace_label.as_deref() == Some("unknown-workspace")
            && (entry.cost - 1.0).abs() < f64::EPSILON
    }));
    assert!(entries.iter().any(|entry| {
        entry.workspace_key.is_none()
            && entry.workspace_label.as_deref() == Some(UNKNOWN_WORKSPACE_LABEL)
            && (entry.cost - 2.0).abs() < f64::EPSILON
    }));
}

#[test]
fn test_session_grouping_merges_same_session_and_model() {
    // Two messages with the same session_id + same model — should collapse
    // into one row regardless of the client that produced them, because
    // GroupBy::Session keys on (session_id, model) only.
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-shared",
                1.25,
                None,
                None,
            ),
            make_workspace_message(
                "amp",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-shared",
                2.75,
                None,
                None,
            ),
        ],
        &GroupBy::Session,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_deref(), Some("session-shared"));
    assert_eq!(entries[0].model, "claude-sonnet-4-5");
    assert!((entries[0].cost - 4.0).abs() < f64::EPSILON);
    assert_eq!(entries[0].message_count, 2);
    assert!(entries[0].workspace_key.is_none());
    assert!(entries[0].workspace_label.is_none());
    // Session grouping does not merge_clients into a comma list.
    assert!(entries[0].merged_clients.is_none());
}

#[test]
fn test_session_grouping_separates_different_sessions() {
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message("codex", "gpt-5", "openai", "session-a", 1.0, None, None),
            make_workspace_message("codex", "gpt-5", "openai", "session-b", 2.0, None, None),
        ],
        &GroupBy::Session,
    );

    assert_eq!(entries.len(), 2);
    let session_ids: HashSet<_> = entries
        .iter()
        .map(|e| e.session_id.as_deref().unwrap())
        .collect();
    assert_eq!(session_ids, HashSet::from(["session-a", "session-b"]));
}

#[test]
fn test_client_session_grouping_keeps_clients_separate() {
    // Same session_id seen by two different clients (unusual in practice
    // but possible if parsers collide on an id space). ClientSession
    // must yield two rows; Session would yield one (covered above).
    let entries = aggregate_model_usage_entries(
        vec![
            make_workspace_message(
                "claude",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-shared",
                1.0,
                None,
                None,
            ),
            make_workspace_message(
                "amp",
                "claude-sonnet-4-5-20250929",
                "anthropic",
                "session-shared",
                3.0,
                None,
                None,
            ),
        ],
        &GroupBy::ClientSession,
    );

    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry.session_id.as_deref(), Some("session-shared"));
        assert!(entry.merged_clients.is_none());
    }
    let by_client: HashSet<_> = entries.iter().map(|e| e.client.as_str()).collect();
    assert_eq!(by_client, HashSet::from(["claude", "amp"]));
}

#[test]
fn test_non_session_grouping_does_not_populate_session_id() {
    // Defensive: only Session/ClientSession variants should set the
    // session_id field on ModelUsage — every other group_by must leave
    // it None so the camelCase JSON output omits it via
    // `skip_serializing_if = "Option::is_none"`.
    for group_by in &[
        GroupBy::Model,
        GroupBy::ClientModel,
        GroupBy::ClientProviderModel,
        GroupBy::WorkspaceModel,
    ] {
        let entries = aggregate_model_usage_entries(
            vec![make_workspace_message(
                "codex",
                "gpt-5",
                "openai",
                "session-x",
                1.0,
                None,
                None,
            )],
            group_by,
        );
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].session_id.is_none(),
            "session_id leaked into {:?} grouping",
            group_by
        );
    }
}

#[test]
fn test_retain_for_requested_clients_keeps_original_client_matches() {
    let requested: HashSet<&str> = HashSet::from(["opencode"]);
    assert!(retain_for_requested_clients(
        "opencode",
        "gpt-4o",
        "anthropic",
        &requested
    ));
    assert!(!retain_for_requested_clients(
        "claude",
        "gpt-4o",
        "anthropic",
        &requested
    ));
}

#[test]
fn test_retain_for_requested_clients_accepts_synthetic_gateway_traffic() {
    let requested: HashSet<&str> = HashSet::from(["synthetic"]);
    assert!(retain_for_requested_clients(
        "opencode",
        "hf:deepseek-ai/DeepSeek-V3-0324",
        "unknown",
        &requested
    ));
    assert!(retain_for_requested_clients(
        "synthetic",
        "deepseek-v3-0324",
        "synthetic",
        &requested
    ));
    assert!(!retain_for_requested_clients(
        "opencode",
        "gpt-4o",
        "anthropic",
        &requested
    ));
}

#[test]
fn test_retain_for_requested_clients_preserves_kilo_split() {
    let kilocode_only: HashSet<&str> = HashSet::from(["kilocode"]);
    assert!(retain_for_requested_clients(
        "kilocode",
        "gpt-5",
        "openai",
        &kilocode_only
    ));
    assert!(!retain_for_requested_clients(
        "kilo",
        "gpt-5",
        "openai",
        &kilocode_only
    ));

    let kilo_only: HashSet<&str> = HashSet::from(["kilo"]);
    assert!(retain_for_requested_clients(
        "kilo", "gpt-5", "openai", &kilo_only
    ));
    assert!(!retain_for_requested_clients(
        "kilocode", "gpt-5", "openai", &kilo_only
    ));
}

#[test]
fn test_cursor_parse_path_reprices_zero_cost_composer_1_5_rows() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let cursor_cache_dir = temp_dir.path().join(".config/tokscale/cursor-cache");
    std::fs::create_dir_all(&cursor_cache_dir).unwrap();

    let csv = r#"Date,Kind,Model,Max Mode,Input (w/ Cache Write),Input (w/o Cache Write),Cache Read,Output Tokens,Total Tokens,Cost
"2026-03-04T12:00:00.000Z","Included","Composer 1.5","No","1200","1000","5000","2000","8000","0""#;
    std::fs::write(cursor_cache_dir.join("usage.csv"), csv).unwrap();

    let pricing = pricing::PricingService::new(HashMap::new(), HashMap::new());
    let messages = parse_all_messages_with_pricing(
        temp_dir.path().to_str().unwrap(),
        &["cursor".to_string()],
        Some(&pricing),
    );

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].client, "cursor");
    assert_eq!(messages[0].model_id, "Composer 1.5");
    assert!(messages[0].cost > 0.0);
}

fn write_kimi_repeated_status_fixture(source_home: &std::path::Path) {
    let session_dir = source_home.join(".kimi/sessions/group-1/session-1");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
            session_dir.join("wire.jsonl"),
            r#"{"type": "metadata", "protocol_version": "1.3"}
{"timestamp": 1770983410.0, "message": {"type": "StatusUpdate", "payload": {"token_usage": {"input_other": 10, "output": 1, "input_cache_read": 0, "input_cache_creation": 0}, "message_id": "msg-progressive"}}}
{"timestamp": 1770983420.0, "message": {"type": "StatusUpdate", "payload": {"token_usage": {"input_other": 20, "output": 2, "input_cache_read": 0, "input_cache_creation": 0}, "message_id": "msg-progressive"}}}
{"timestamp": 1770983430.0, "message": {"type": "StatusUpdate", "payload": {"token_usage": {"input_other": 5, "output": 1, "input_cache_read": 0, "input_cache_creation": 0}, "message_id": "msg-distinct"}}}
{"timestamp": 1770983440.0, "message": {"type": "StatusUpdate", "payload": {"token_usage": {"input_other": 7, "output": 1, "input_cache_read": 0, "input_cache_creation": 0}}}}
{"timestamp": 1770983450.0, "message": {"type": "StatusUpdate", "payload": {"token_usage": {"input_other": 8, "output": 1, "input_cache_read": 0, "input_cache_creation": 0}}}}"#,
        )
        .unwrap();
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_with_pricing_kimi_deduplicates_repeated_status_updates() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_kimi_repeated_status_fixture(source_home.path());

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["kimi".to_string()],
            None,
        );

        assert_eq!(messages.len(), 4);
        assert_eq!(messages.iter().map(|m| m.tokens.input).sum::<i64>(), 40);
        assert_eq!(messages.iter().map(|m| m.tokens.output).sum::<i64>(), 5);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_local_clients_kimi_deduplicates_repeated_status_updates() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_kimi_repeated_status_fixture(source_home.path());

        let parsed = parse_local_clients(LocalParseOptions {
            home_dir: Some(source_home.path().to_str().unwrap().to_string()),
            use_env_roots: false,
            clients: Some(vec!["kimi".to_string()]),
            since: None,
            until: None,
            year: None,
            scanner_settings: scanner::ScannerSettings::default(),
        })
        .unwrap();

        assert_eq!(parsed.counts.get(ClientId::Kimi), 4);
        assert_eq!(parsed.messages.len(), 4);
        assert_eq!(parsed.messages.iter().map(|m| m.input).sum::<i64>(), 40);
        assert_eq!(parsed.messages.iter().map(|m| m.output).sum::<i64>(), 5);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_source_cache_refreshes_stale_date_on_cache_hit() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let message_dir = source_home
            .path()
            .join(".local/share/opencode/storage/message/project-1");
        std::fs::create_dir_all(&message_dir).unwrap();
        let path = message_dir.join("msg_001.json");
        std::fs::write(
                &path,
                r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
            )
            .unwrap();

        let fingerprint = message_cache::SourceFingerprint::from_path(&path).unwrap();
        let mut stale_message = UnifiedMessage::new(
            "opencode",
            "accounts/fireworks/models/deepseek-v3-0324",
            "fireworks",
            "session-1",
            1_733_011_200_000,
            TokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            0.0,
        );
        stale_message.date = "1900-01-01".to_string();

        let mut cache = message_cache::SourceMessageCache::default();
        cache.insert(message_cache::CachedSourceEntry::new(
            &path,
            fingerprint,
            vec![stale_message],
            Vec::new(),
            None,
        ));
        cache.save_if_dirty();

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );

        assert_eq!(messages.len(), 1);
        assert_ne!(messages[0].date, "1900-01-01");
        assert_eq!(
            messages[0].date,
            UnifiedMessage::new(
                "opencode",
                "accounts/fireworks/models/deepseek-v3-0324",
                "fireworks",
                "session-1",
                1_733_011_200_000,
                TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                0.0,
            )
            .date
        );
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn test_empty_parse_results_are_not_cached_for_optional_file_sources() {
    use std::os::unix::fs::PermissionsExt;

    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let message_dir = source_home
            .path()
            .join(".local/share/opencode/storage/message/project-1");
        std::fs::create_dir_all(&message_dir).unwrap();
        let path = message_dir.join("msg_001.json");
        std::fs::write(
                &path,
                r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
            )
            .unwrap();

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&path, permissions).unwrap();

        let first_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert!(first_messages.is_empty());

        let cache = message_cache::SourceMessageCache::load();
        assert!(cache.get(&path).is_none());

        let mut readable_permissions = std::fs::metadata(&path).unwrap().permissions();
        readable_permissions.set_mode(0o644);
        std::fs::set_permissions(&path, readable_permissions).unwrap();

        let second_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(second_messages.len(), 1);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_empty_cache_hits_are_reparsed_for_optional_file_sources() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let message_dir = source_home
            .path()
            .join(".local/share/opencode/storage/message/project-1");
        std::fs::create_dir_all(&message_dir).unwrap();
        let path = message_dir.join("msg_001.json");
        std::fs::write(
                &path,
                r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
            )
            .unwrap();

        let fingerprint = message_cache::SourceFingerprint::from_path(&path).unwrap();
        let mut cache = message_cache::SourceMessageCache::default();
        cache.insert(message_cache::CachedSourceEntry::new(
            &path,
            fingerprint,
            Vec::new(),
            Vec::new(),
            None,
        ));
        cache.save_if_dirty();

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(messages.len(), 1);

        let loaded = message_cache::SourceMessageCache::load();
        let repaired_entry = loaded.get(&path).unwrap();
        assert_eq!(repaired_entry.messages.len(), 1);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_sqlite_source_cache_invalidates_on_wal_change() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let db_dir = source_home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode=WAL;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY,
                     session_id TEXT NOT NULL,
                     data TEXT NOT NULL
                 );",
        )
        .unwrap();

        let row_one = r#"{
                "role": "assistant",
                "modelID": "claude-sonnet-4",
                "providerID": "anthropic",
                "tokens": { "input": 100, "output": 50, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
                "time": { "created": 1700000000000.0 }
            }"#;
        let row_two = r#"{
                "role": "assistant",
                "modelID": "claude-sonnet-4",
                "providerID": "anthropic",
                "tokens": { "input": 120, "output": 60, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
                "time": { "created": 1700000001000.0 }
            }"#;

        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params!["msg-1", "session-1", row_one],
        )
        .unwrap();

        let first_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(first_messages.len(), 1);

        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params!["msg-2", "session-1", row_two],
        )
        .unwrap();
        assert!(db_path.with_extension("db-wal").exists());

        let refreshed_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(refreshed_messages.len(), 2);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_dedups_across_channel_suffixed_opencode_dbs() {
    // Regression guard: a session that appears in both `opencode.db` and
    // `opencode-<channel>.db` (e.g. the user switches channels mid-session)
    // must only be counted once.
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let db_dir = source_home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&db_dir).unwrap();

        let schema = "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY,
                     session_id TEXT NOT NULL,
                     data TEXT NOT NULL
                 );";
        let row = |input: u64, ts: u64| {
            format!(
                r#"{{
                        "role": "assistant",
                        "modelID": "claude-sonnet-4",
                        "providerID": "anthropic",
                        "tokens": {{ "input": {input}, "output": 10, "reasoning": 0, "cache": {{ "read": 0, "write": 0 }} }},
                        "time": {{ "created": {ts}.0 }}
                    }}"#
            )
        };

        let default_db = db_dir.join("opencode.db");
        let conn = rusqlite::Connection::open(&default_db).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "shared-msg",
                "session-shared",
                row(100, 1_700_000_000_000u64)
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "latest-only",
                "session-latest",
                row(200, 1_700_000_001_000u64)
            ],
        )
        .unwrap();
        drop(conn);

        let stable_db = db_dir.join("opencode-stable.db");
        let conn = rusqlite::Connection::open(&stable_db).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "shared-msg",
                "session-shared",
                row(100, 1_700_000_000_000u64)
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "stable-only",
                "session-stable",
                row(300, 1_700_000_002_000u64)
            ],
        )
        .unwrap();
        drop(conn);

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(
            messages.len(),
            3,
            "expected 3 unique messages (shared + latest-only + stable-only), got {}",
            messages.len()
        );
        let mut ids: Vec<String> = messages
            .iter()
            .filter_map(|m| m.dedup_key.clone())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["latest-only", "shared-msg", "stable-only"]);

        let messages_warm = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );
        assert_eq!(
            messages_warm.len(),
            3,
            "warm cache must also dedup shared message across channel dbs"
        );
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_with_pricing_opencode_sqlite_deduplicates_forked_history() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let db_dir = source_home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("opencode.db");
        let conn = create_opencode_sqlite_db(&db_path);

        let msg_a = build_opencode_sqlite_payload(
            1_700_000_000_000.0,
            1_700_000_000_500.0,
            100,
            50,
            0,
            10,
            5,
            0.01,
        );
        let msg_b = build_opencode_sqlite_payload(
            1_700_000_001_000.0,
            1_700_000_001_500.0,
            200,
            80,
            10,
            20,
            0,
            0.02,
        );
        let msg_c = build_opencode_sqlite_payload(
            1_700_000_002_000.0,
            1_700_000_002_500.0,
            300,
            120,
            15,
            0,
            0,
            0.03,
        );

        for (id, session_id, payload) in [
            ("root_a", "root", msg_a.as_str()),
            ("root_b", "root", msg_b.as_str()),
            ("fork_a_copy", "fork", msg_a.as_str()),
            ("fork_b_copy", "fork", msg_b.as_str()),
            ("fork_c_new", "fork", msg_c.as_str()),
        ] {
            conn.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, session_id, payload],
            )
            .unwrap();
        }
        drop(conn);

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["opencode".to_string()],
            None,
        );

        assert_eq!(messages.len(), 3);
        assert_eq!(messages.iter().map(|m| m.tokens.input).sum::<i64>(), 600);
        assert_eq!(messages.iter().map(|m| m.tokens.output).sum::<i64>(), 250);
        assert_eq!(messages.iter().map(|m| m.cost).sum::<f64>(), 0.06);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_local_clients_opencode_sqlite_counts_deduplicated_forked_history() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let db_dir = source_home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("opencode.db");
        let conn = create_opencode_sqlite_db(&db_path);

        let msg_a = build_opencode_sqlite_payload(
            1_700_000_000_000.0,
            1_700_000_000_500.0,
            100,
            50,
            0,
            10,
            5,
            0.01,
        );
        let msg_b = build_opencode_sqlite_payload(
            1_700_000_001_000.0,
            1_700_000_001_500.0,
            200,
            80,
            10,
            20,
            0,
            0.02,
        );
        let msg_c = build_opencode_sqlite_payload(
            1_700_000_002_000.0,
            1_700_000_002_500.0,
            300,
            120,
            15,
            0,
            0,
            0.03,
        );

        for (id, session_id, payload) in [
            ("root_a", "root", msg_a.as_str()),
            ("root_b", "root", msg_b.as_str()),
            ("fork_a_copy", "fork", msg_a.as_str()),
            ("fork_b_copy", "fork", msg_b.as_str()),
            ("fork_c_new", "fork", msg_c.as_str()),
        ] {
            conn.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, session_id, payload],
            )
            .unwrap();
        }
        drop(conn);

        let parsed = parse_local_clients(LocalParseOptions {
            home_dir: Some(source_home.path().to_str().unwrap().to_string()),
            use_env_roots: false,
            clients: Some(vec!["opencode".to_string()]),
            since: None,
            until: None,
            year: None,
            scanner_settings: scanner::ScannerSettings::default(),
        })
        .unwrap();

        assert_eq!(parsed.counts.get(ClientId::OpenCode), 3);
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages.iter().map(|m| m.input).sum::<i64>(), 600);
        assert_eq!(parsed.messages.iter().map(|m| m.output).sum::<i64>(), 250);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

fn write_codex_forked_history_fixture(source_home: &std::path::Path) {
    let codex_dir = source_home.join(".codex/sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
            codex_dir.join("parent.jsonl"),
            concat!(
                r#"{"timestamp":"2026-04-30T10:00:00Z","type":"session_meta","payload":{"id":"parent-session","source":"interactive","model_provider":"openai","cwd":"/Users/alice/root"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65},"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65}}}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130},"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65}}}}"#,
                "\n"
            ),
        )
        .unwrap();
    std::fs::write(
            codex_dir.join("fork.jsonl"),
            concat!(
                r#"{"timestamp":"2026-04-30T10:01:00Z","type":"session_meta","payload":{"id":"fork-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","cwd":"/Users/alice/root-worktree"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:01:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130}}}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:01:02Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:01:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65},"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65}}}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:01:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130},"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65}}}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T10:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"cached_input_tokens":22,"output_tokens":33,"total_tokens":143},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"total_tokens":13}}}}"#,
                "\n"
            ),
        )
        .unwrap();
}

fn write_codex_parent_replay_fixture(source_home: &std::path::Path) {
    let codex_dir = source_home.join(".codex/sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
            codex_dir.join("parent.jsonl"),
            concat!(
                r#"{"timestamp":"2026-05-24T20:00:00Z","type":"session_meta","payload":{"id":"019e5b00-0000-7000-8000-000000000001","source":"vscode","model_provider":"openai","cwd":"/repo"}}"#,
                "\n",
                r#"{"timestamp":"2026-05-24T20:00:01Z","type":"turn_context","payload":{"turn_id":"019e5b00-0001-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
                "\n",
                r#"{"timestamp":"2026-05-24T20:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110},"last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}"#,
                "\n",
                r#"{"timestamp":"2026-05-24T20:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"output_tokens":13,"total_tokens":143},"last_token_usage":{"input_tokens":30,"output_tokens":3,"total_tokens":33}}}}"#,
                "\n"
            ),
        )
        .unwrap();

    for (filename, child_id, child_turn_id, timestamp) in [
        (
            "child-a.jsonl",
            "019e5c03-1e99-7000-8000-000000000001",
            "019e5c03-6425-7000-8000-000000000001",
            "2026-05-24T21:00:00Z",
        ),
        (
            "child-b.jsonl",
            "019e5c04-1e99-7000-8000-000000000001",
            "019e5c04-6425-7000-8000-000000000001",
            "2026-05-24T22:00:00Z",
        ),
    ] {
        std::fs::write(
                codex_dir.join(filename),
                format!(
                    concat!(
                        r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"{child_id}","forked_from_id":"019e5b00-0000-7000-8000-000000000001","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"019e5b00-0000-7000-8000-000000000001","depth":1}}}}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo"}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"019e5b00-0000-7000-8000-000000000001","source":"vscode","model_provider":"openai","cwd":"/repo"}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{"turn_id":"019e5b00-0001-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"output_tokens":10,"total_tokens":110}},"last_token_usage":{{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":130,"output_tokens":13,"total_tokens":143}},"last_token_usage":{{"input_tokens":30,"output_tokens":3,"total_tokens":33}}}}}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"task_started","turn_id":"{child_turn_id}"}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{"turn_id":"{child_turn_id}","model":"gpt-5.5","cwd":"/repo"}}}}"#,
                        "\n",
                        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":140,"output_tokens":14,"total_tokens":154}},"last_token_usage":{{"input_tokens":10,"output_tokens":1,"total_tokens":11}}}}}}}}"#,
                        "\n",
                    ),
                    timestamp = timestamp,
                    child_id = child_id,
                    child_turn_id = child_turn_id,
                ),
            )
            .unwrap();
    }
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_with_pricing_codex_deduplicates_forked_history() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_codex_forked_history_fixture(source_home.path());

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.input)
                .sum::<i64>(),
            88
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.cache_read)
                .sum::<i64>(),
            22
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.output)
                .sum::<i64>(),
            33
        );
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_with_pricing_codex_deduplicates_parent_replay_across_forks() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_codex_parent_replay_fixture(source_home.path());

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        // Parent contributes its two turns. The two forks each replay the
        // parent history (skipped) and then emit one own turn that lands on
        // the identical cumulative total (140/14). Sibling forks sharing a
        // cumulative total is the signature of a replayed row, so the
        // fork-parent-scoped dedup key collapses them into one. Real fork
        // fan-out replays the same upstream totals into 10-100+ siblings;
        // two distinct turns reaching a byte-identical cumulative vector by
        // chance does not happen in practice because the cumulative encodes
        // each fork's divergent context size.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages.iter().map(|m| m.tokens.input).sum::<i64>(), 140);
        assert_eq!(messages.iter().map(|m| m.tokens.output).sum::<i64>(), 14);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

fn write_codex_twin_token_count_fixture(source_home: &std::path::Path) {
    // Single session with two turns whose `last_token_usage` deltas are
    // byte-identical but emitted at different timestamps. The fork-dedup
    // key includes the cumulative total, so both turns must survive even
    // when a user happens to send two turns producing the same per-turn
    // delta.
    let codex_dir = source_home.join(".codex/sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
            codex_dir.join("twin-deltas.jsonl"),
            concat!(
                r#"{"timestamp":"2026-04-30T11:00:00Z","type":"session_meta","payload":{"id":"twin-session","source":"interactive","model_provider":"openai","cwd":"/Users/alice/root"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T11:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T11:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                "\n",
                r#"{"timestamp":"2026-04-30T11:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":4,"output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                "\n"
            ),
        )
        .unwrap();
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_with_pricing_codex_keeps_twin_token_counts_at_distinct_timestamps() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_codex_twin_token_count_fixture(source_home.path());

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(
            messages.len(),
            2,
            "two turns with identical token deltas at distinct timestamps must both survive dedup",
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.input)
                .sum::<i64>(),
            16,
            "input tokens normalize cache_read out of input: 2 turns × (10 - 2) = 16",
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.output)
                .sum::<i64>(),
            6,
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.tokens.cache_read)
                .sum::<i64>(),
            4,
        );
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_local_clients_codex_counts_deduplicated_forked_history() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        write_codex_forked_history_fixture(source_home.path());

        let parsed = parse_local_clients(LocalParseOptions {
            home_dir: Some(source_home.path().to_str().unwrap().to_string()),
            use_env_roots: false,
            clients: Some(vec!["codex".to_string()]),
            since: None,
            until: None,
            year: None,
            scanner_settings: scanner::ScannerSettings::default(),
        })
        .unwrap();

        assert_eq!(parsed.counts.get(ClientId::Codex), 3);
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(
            parsed
                .messages
                .iter()
                .map(|message| message.input)
                .sum::<i64>(),
            88
        );
        assert_eq!(
            parsed
                .messages
                .iter()
                .map(|message| message.cache_read)
                .sum::<i64>(),
            22
        );
        assert_eq!(
            parsed
                .messages
                .iter()
                .map(|message| message.output)
                .sum::<i64>(),
            33
        );
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_codex_cache_reparses_from_zero_when_incremental_prefix_is_stale() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let codex_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let path = codex_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n"
                ),
            )
            .unwrap();

        let initial_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(initial_messages.len(), 1);
        assert_eq!(initial_messages[0].model_id, "gpt-5.4");
        assert!(message_cache::SourceMessageCache::load()
            .get(&path)
            .and_then(|entry| entry.codex_incremental.as_ref())
            .is_some());

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
                    "\n"
                ),
            )
            .unwrap();

        let warm_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(warm_messages, fresh_messages);
        assert_eq!(warm_messages.len(), 2);
        assert!(warm_messages
            .iter()
            .all(|message| message.model_id == "gpt-5.5"));
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_source_cache_keeps_untimestamped_rows_in_sync_after_append() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let codex_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let path = codex_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n"
                ),
            )
            .unwrap();

        let first_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(first_messages.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
                concat!(
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        file.flush().unwrap();

        let warm_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(warm_messages, fresh_messages);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_source_cache_matches_cold_parse_after_malformed_json_append() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let codex_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let path = codex_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":999""#,
                    "\n"
                ),
            )
            .unwrap();

        let initial_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(initial_messages.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
                concat!(
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        file.flush().unwrap();

        let warm_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert!(message_cache::SourceMessageCache::load()
            .get(&path)
            .is_none());

        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(warm_messages, fresh_messages);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_exact_hit_codex_cache_repairs_fallback_timestamps_without_incremental_state() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let session_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n"
                ),
            )
            .unwrap();

        let expected = crate::sessions::codex::parse_codex_file(&path);
        assert_eq!(expected.len(), 1);

        let fingerprint = message_cache::SourceFingerprint::from_path(&path).unwrap();
        let mut stale_message = expected[0].clone();
        stale_message.timestamp = 0;
        stale_message.date = "1900-01-01".to_string();

        let mut cache = message_cache::SourceMessageCache::default();
        cache.insert(message_cache::CachedSourceEntry::new(
            &path,
            fingerprint,
            vec![stale_message],
            vec![0],
            None,
        ));
        cache.save_if_dirty();

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(messages, expected);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_codex_cache_repairs_fallback_timestamps_after_source_mtime_change() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let session_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        let contents = concat!(
            r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
            "\n"
        );
        std::fs::write(&path, contents).unwrap();

        let initial_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(initial_messages.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, contents).unwrap();

        let warm_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(warm_messages, fresh_messages);
        assert_ne!(warm_messages[0].timestamp, initial_messages[0].timestamp);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_full_log_parse_preserves_valid_messages_before_invalid_line_error() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let session_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");

        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        file.write_all(&[0xff, b'\n']).unwrap();
        file.flush().unwrap();

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].model_id, "gpt-5.4");

        let cache = message_cache::SourceMessageCache::load();
        assert!(cache.get(&path).is_none());
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_codex_cache_does_not_persist_unknown_before_later_turn_context() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let session_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"session_meta","payload":{"source":"interactive","model_provider":"openai"}}"#,
                    "\n",
                    r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                    "\n"
                ),
            )
            .unwrap();

        let initial_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(initial_messages.len(), 1);
        assert_eq!(initial_messages[0].model_id, "unknown");
        assert!(message_cache::SourceMessageCache::load()
            .get(&path)
            .is_none());

        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
                concat!(
                    r#"{"timestamp":"2026-04-27T10:00:04Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        file.flush().unwrap();

        let resumed_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(resumed_messages, fresh_messages);
        assert_eq!(resumed_messages.len(), 1);
        assert_eq!(resumed_messages[0].model_id, "gpt-5.5");

        std::env::set_var("HOME", cache_home.path());
        assert!(message_cache::SourceMessageCache::load()
            .get(&path)
            .is_some());
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_codex_cache_skips_non_newline_terminated_resume_prefix() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let fresh_cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let session_dir = source_home.path().join(".codex/sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        std::fs::write(
                &path,
                concat!(
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#
                ),
            )
            .unwrap();

        let initial_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );
        assert_eq!(initial_messages.len(), 1);
        assert!(message_cache::SourceMessageCache::load()
            .get(&path)
            .is_none());

        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
                concat!(
                    "\n",
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        file.flush().unwrap();

        let warm_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        std::env::set_var("HOME", fresh_cache_home.path());
        let fresh_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["codex".to_string()],
            None,
        );

        assert_eq!(warm_messages, fresh_messages);
        assert_eq!(warm_messages.len(), 2);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_source_cache_does_not_reuse_priced_cost_without_pricing_service() {
    let temp_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_home.path());
    {
        let cursor_cache_dir = source_home.path().join(".config/tokscale/cursor-cache");
        std::fs::create_dir_all(&cursor_cache_dir).unwrap();

        let csv = r#"Date,Kind,Model,Max Mode,Input (w/ Cache Write),Input (w/o Cache Write),Cache Read,Output Tokens,Total Tokens,Cost
"2026-03-04T12:00:00.000Z","Included","Composer 1.5","No","1200","1000","5000","2000","8000","0""#;
        std::fs::write(cursor_cache_dir.join("usage.csv"), csv).unwrap();

        let mut litellm = HashMap::new();
        litellm.insert(
            "Composer 1.5".into(),
            pricing::ModelPricing {
                input_cost_per_token: Some(0.001),
                output_cost_per_token: Some(0.002),
                cache_read_input_token_cost: Some(0.0005),
                ..Default::default()
            },
        );
        let pricing = pricing::PricingService::new(litellm, HashMap::new());

        let repriced_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["cursor".to_string()],
            Some(&pricing),
        );
        assert_eq!(repriced_messages.len(), 1);
        assert!(repriced_messages[0].cost > 0.0);

        let cached_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["cursor".to_string()],
            None,
        );

        assert_eq!(cached_messages.len(), 1);
        assert_eq!(cached_messages[0].cost, 0.0);
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn test_apply_pricing_if_available_keeps_existing_cost_without_pricing() {
    let mut msg = UnifiedMessage::new_with_agent(
        "roocode",
        "gpt-4o",
        "provider",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.42,
        Some("planner".to_string()),
    );

    apply_pricing_if_available(&mut msg, None);

    assert_eq!(msg.cost, 0.42);
}

#[test]
fn test_apply_pricing_if_available_overrides_cost_when_pricing_exists() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "gpt-4o".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "codex",
        "gpt-4o",
        "provider",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.02);
}

#[test]
fn test_apply_pricing_if_available_applies_zed_hosted_markup() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "claude-sonnet-4-5".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "zed",
        "claude-sonnet-4-5",
        crate::sessions::zed::ZED_HOSTED_PROVIDER,
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert!((msg.cost - 0.022).abs() < 1e-12);
}

#[test]
fn test_apply_pricing_if_available_skips_zed_markup_for_non_zed_client() {
    // Non-zed client with provider_id "zed.dev" must not receive the +10%
    // markup. The multiplier is gated on (client == "zed" AND provider).
    let mut litellm = HashMap::new();
    litellm.insert(
        "claude-sonnet-4-5".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "claudecode",
        "claude-sonnet-4-5",
        crate::sessions::zed::ZED_HOSTED_PROVIDER,
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    // 10 * 0.001 + 5 * 0.002 = 0.020, no markup.
    assert!((msg.cost - 0.020).abs() < 1e-12);
}

#[test]
fn test_apply_pricing_if_available_skips_zed_markup_for_byok_provider() {
    // A Zed message whose provider_id is the upstream provider directly
    // (BYOK / non-hosted path) must not be marked up — the user is paying
    // the upstream API directly, not through Zed.
    let mut litellm = HashMap::new();
    litellm.insert(
        "claude-sonnet-4-5".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "zed",
        "claude-sonnet-4-5",
        "anthropic",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert!((msg.cost - 0.020).abs() < 1e-12);
}

#[test]
fn test_apply_pricing_if_available_uses_reasoning_for_gemini() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "gemini-2.5-pro".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "gemini",
        "gemini-2.5-pro",
        "google",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 7,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.034);
}

#[test]
fn test_apply_pricing_if_available_uses_cache_read_pricing_for_gemini() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "gemini-2.5-pro".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            cache_read_input_token_cost: Some(0.0001),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "gemini",
        "gemini-2.5-pro",
        "google",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 7,
            cache_write: 0,
            reasoning: 3,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.0267);
}

#[test]
fn test_apply_pricing_if_available_uses_market_rate_for_free_variant() {
    let mut openrouter = HashMap::new();
    openrouter.insert(
        "z-ai/glm-4.7".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(HashMap::new(), openrouter);

    let mut msg = UnifiedMessage::new(
        "opencode",
        "glm-4.7-free",
        "modal",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.02);
}

#[test]
fn test_apply_pricing_if_available_prefers_provider_aware_match() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "xai/grok-code-fast-1-0825".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    litellm.insert(
        "azure_ai/grok-code-fast-1".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.01),
            output_cost_per_token: Some(0.02),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "opencode",
        "grok-code",
        "azure",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.2);
}

#[test]
fn test_apply_pricing_if_available_uses_nested_reseller_exact_match() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "gpt-4".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        },
    );
    litellm.insert(
        "azure/openai/gpt-4".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.01),
            output_cost_per_token: Some(0.02),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "opencode",
        "gpt-4",
        "azure",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.2);
}

#[test]
fn test_apply_pricing_if_available_keeps_scoped_fireworks_cost_without_exact_pricing() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "fireworks_ai/accounts/fireworks/models/deepseek-r1-0528-distill-qwen3-8b".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.0000002),
            output_cost_per_token: Some(0.0000002),
            ..Default::default()
        },
    );

    let mut openrouter = HashMap::new();
    openrouter.insert(
        "deepseek/deepseek-v4-pro".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.000001),
            output_cost_per_token: Some(0.000002),
            ..Default::default()
        },
    );

    let pricing = pricing::PricingService::new(litellm, openrouter);
    let mut msg = UnifiedMessage::new(
        "opencode",
        "accounts/fireworks/models/deepseek-v4-pro",
        "fireworks",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.123,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.123);
}

#[test]
fn test_apply_pricing_if_available_prefers_provider_specific_exact_match_over_plain_exact() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "gemini-2.5-pro".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            cache_creation_input_token_cost: None,
            ..Default::default()
        },
    );

    let mut openrouter = HashMap::new();
    openrouter.insert(
        "google/gemini-2.5-pro".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            cache_creation_input_token_cost: Some(0.01),
            ..Default::default()
        },
    );

    let pricing = pricing::PricingService::new(litellm, openrouter);

    let mut msg = UnifiedMessage::new(
        "opencode",
        "gemini-2.5-pro",
        "google",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 3,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.05);
}

#[test]
fn test_apply_pricing_if_available_normalizes_openai_codex_provider() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "openai/gpt-5.2-preview".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.01),
            output_cost_per_token: Some(0.02),
            ..Default::default()
        },
    );
    litellm.insert(
        "google/gpt-5.2-preview-max".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.1),
            output_cost_per_token: Some(0.2),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "openclaw",
        "gpt-5.2",
        "openai-codex",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.2);
}

#[test]
fn test_apply_pricing_if_available_prices_claude_code_gpt_5_3_codex() {
    let pricing = pricing::PricingService::new(HashMap::new(), HashMap::new());

    let mut msg = UnifiedMessage::new(
        "claude",
        "gpt-5.3-codex",
        "openai",
        "session-1",
        1_776_000_000_000,
        TokenBreakdown {
            input: 1_000_000,
            output: 100_000,
            cache_read: 50_000,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    let expected = 1.75 + 1.4 + 0.00875;
    assert!((msg.cost - expected).abs() < 1e-12);
}

#[test]
fn test_apply_pricing_if_available_prices_claude_code_minimax_model() {
    let mut litellm = HashMap::new();
    litellm.insert(
        "minimax/minimax-m2.1".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.01),
            output_cost_per_token: Some(0.02),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(litellm, HashMap::new());

    let mut msg = UnifiedMessage::new(
        "claude",
        "MiniMax-M2.1",
        "minimax",
        "session-1",
        1_776_000_000_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    assert_eq!(msg.cost, 0.2);
}

#[test]
fn test_apply_pricing_if_available_prices_kimi_k2p6_alias() {
    let mut openrouter = HashMap::new();
    openrouter.insert(
        "moonshotai/kimi-k2.6".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(9.5e-7),
            output_cost_per_token: Some(0.000004),
            ..Default::default()
        },
    );
    let pricing = pricing::PricingService::new(HashMap::new(), openrouter);

    let mut msg = UnifiedMessage::new(
        "kimi",
        "k2p6",
        "kimi-for-coding",
        "session-1",
        1_776_000_000_000,
        TokenBreakdown {
            input: 1_000_000,
            output: 250_000,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(&pricing));

    let expected = 1_000_000.0 * 9.5e-7 + 250_000.0 * 0.000004;
    assert!((msg.cost - expected).abs() < 1e-12);
    assert!(msg.cost > 0.0);
}

#[test]
fn test_select_local_parse_pricing_prefers_fresh_service_for_new_models() {
    let mut fresh_litellm = HashMap::new();
    fresh_litellm.insert(
        "gpt-5.4".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.000002),
            output_cost_per_token: Some(0.00001),
            ..Default::default()
        },
    );
    let fresh = Arc::new(pricing::PricingService::new(fresh_litellm, HashMap::new()));
    let stale = pricing::PricingService::new(HashMap::new(), HashMap::new());
    let selected = select_local_parse_pricing(Ok(Arc::clone(&fresh)), || Some(stale)).unwrap();

    let mut msg = UnifiedMessage::new(
        "opencode",
        "gpt-5.4",
        "openai",
        "session-1",
        1_733_011_200_000,
        TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        },
        0.0,
    );

    apply_pricing_if_available(&mut msg, Some(selected.as_ref()));

    assert!(msg.cost > 0.0);
}

#[test]
fn test_select_local_parse_pricing_falls_back_to_stale_cache_on_fetch_error() {
    let mut stale_litellm = HashMap::new();
    stale_litellm.insert(
        "gpt-5.2".into(),
        pricing::ModelPricing {
            input_cost_per_token: Some(0.00000175),
            output_cost_per_token: Some(0.000014),
            ..Default::default()
        },
    );
    let stale = pricing::PricingService::new(stale_litellm, HashMap::new());

    let selected =
        select_local_parse_pricing(Err("network failed".to_string()), || Some(stale)).unwrap();

    assert!(selected.lookup_with_source("gpt-5.2", None).is_some());
}

#[test]
fn test_select_local_parse_pricing_does_not_evaluate_stale_fallback_on_fresh_success() {
    let fresh = Arc::new(pricing::PricingService::new(HashMap::new(), HashMap::new()));
    let mut stale_called = false;

    let selected = select_local_parse_pricing(Ok(Arc::clone(&fresh)), || {
        stale_called = true;
        None
    })
    .unwrap();

    assert!(Arc::ptr_eq(&selected, &fresh));
    assert!(!stale_called);
}

#[test]
fn test_dedupe_latest_trae_messages_keeps_latest_timestamp_for_session() {
    let messages = vec![
        make_trae_message(
            "session-stable",
            1_700_000_002_000,
            Some("trae:session-stable:1_700_000_002"),
            0.2,
        ),
        make_trae_message(
            "session-stable",
            1_700_000_003_000,
            Some("trae:session-stable:1_700_000_003"),
            0.3,
        ),
        make_trae_message(
            "session-other",
            1_700_000_001_000,
            Some("trae:session-other:1_700_000_001"),
            0.1,
        ),
    ];

    let deduped = dedupe_latest_trae_messages(messages);

    assert_eq!(deduped.len(), 2);
    let stable = deduped
        .iter()
        .find(|msg| msg.session_id == "session-stable")
        .expect("session-stable should remain after dedupe");
    assert_eq!(stable.timestamp, 1_700_000_003_000);
    assert_eq!(stable.cost, 0.3);
    assert_eq!(
        stable.dedup_key.as_deref(),
        Some("trae:session-stable:1_700_000_003")
    );
}

#[test]
fn test_dedupe_latest_trae_messages_tiebreaks_by_dedup_key() {
    let messages = vec![
        make_trae_message(
            "session-stable",
            1_700_000_010_000,
            Some("dedupe-key-a"),
            0.2,
        ),
        make_trae_message(
            "session-stable",
            1_700_000_010_000,
            Some("dedupe-key-z"),
            0.4,
        ),
        make_trae_message(
            "session-stable",
            1_700_000_009_000,
            Some("dedupe-key-m"),
            0.1,
        ),
    ];

    let deduped = dedupe_latest_trae_messages(messages);

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].timestamp, 1_700_000_010_000);
    assert_eq!(deduped[0].dedup_key.as_deref(), Some("dedupe-key-z"));
    assert_eq!(deduped[0].cost, 0.4);
}

#[test]
fn test_parse_all_messages_with_pricing_keeps_gateway_message_under_synthetic_filter() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let message_dir = temp_dir
        .path()
        .join(".local/share/opencode/storage/message/project-1");
    std::fs::create_dir_all(&message_dir).unwrap();
    std::fs::write(
            message_dir.join("msg_001.json"),
            r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"hf:deepseek-ai/DeepSeek-V3-0324","providerID":"unknown","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
        )
        .unwrap();

    let pricing = pricing::PricingService::new(HashMap::new(), HashMap::new());
    let messages = parse_all_messages_with_pricing(
        temp_dir.path().to_str().unwrap(),
        &["synthetic".to_string()],
        Some(&pricing),
    );

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].client, "opencode");
    assert_eq!(messages[0].model_id, "deepseek-v3-0324");
    assert_eq!(messages[0].provider_id, "synthetic");
}

#[test]
fn test_parse_local_clients_preserves_gateway_message_client_counts() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let message_dir = temp_dir
        .path()
        .join(".local/share/opencode/storage/message/project-1");
    std::fs::create_dir_all(&message_dir).unwrap();
    std::fs::write(
            message_dir.join("msg_001.json"),
            r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
        )
        .unwrap();

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["opencode".to_string(), "synthetic".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();

    assert_eq!(parsed.counts.get(ClientId::OpenCode), 1);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].client, "opencode");
    assert_eq!(parsed.messages[0].model_id, "deepseek-v3-0324");
    assert_eq!(parsed.messages[0].provider_id, "fireworks");
}

#[test]
fn test_parse_all_messages_fireworks_provider_kept_under_synthetic_only_filter() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let message_dir = temp_dir
        .path()
        .join(".local/share/opencode/storage/message/project-1");
    std::fs::create_dir_all(&message_dir).unwrap();
    std::fs::write(
            message_dir.join("msg_001.json"),
            r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0.1,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
        )
        .unwrap();

    let pricing = pricing::PricingService::new(HashMap::new(), HashMap::new());
    let messages = parse_all_messages_with_pricing(
        temp_dir.path().to_str().unwrap(),
        &["synthetic".to_string()],
        Some(&pricing),
    );

    assert_eq!(
        messages.len(),
        1,
        "fireworks gateway message must not be dropped when filtering for synthetic"
    );
    assert_eq!(messages[0].client, "opencode");
    assert_eq!(messages[0].model_id, "deepseek-v3-0324");
    assert_eq!(messages[0].provider_id, "fireworks");
}

#[test]
fn test_parse_local_clients_fireworks_provider_kept_under_synthetic_only_filter() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let message_dir = temp_dir
        .path()
        .join(".local/share/opencode/storage/message/project-1");
    std::fs::create_dir_all(&message_dir).unwrap();
    std::fs::write(
            message_dir.join("msg_001.json"),
            r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0.1,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
        )
        .unwrap();

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["synthetic".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();

    assert_eq!(
        parsed.messages.len(),
        1,
        "fireworks gateway message must not be dropped when filtering for synthetic only"
    );
    assert_eq!(parsed.messages[0].client, "opencode");
    assert_eq!(parsed.messages[0].model_id, "deepseek-v3-0324");
    assert_eq!(parsed.messages[0].provider_id, "fireworks");
}

#[test]
fn test_parse_local_clients_honors_scanner_settings_opencode_db_paths() {
    // Regression guard: `parse_local_clients` used to call
    // `scan_all_clients_with_env_strategy`, which silently dropped
    // `options.scanner_settings`. Users with
    // `scanner.opencodeDbPaths` pointing at an OPENCODE_DB outside the
    // XDG data dir would see no rows through client listing paths even
    // though local report commands honored the same config.
    let temp_dir = tempfile::TempDir::new().unwrap();
    // Deliberately do not create ~/.local/share/opencode so nothing
    // is auto-discoverable; the only db the scanner can find must
    // come from `scanner_settings`.
    let outside_dir = temp_dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let external_db = outside_dir.join("opencode.db");

    let conn = rusqlite::Connection::open(&external_db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
             CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 data TEXT NOT NULL
             );",
    )
    .unwrap();
    conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "ext-msg-1",
                "ext-session",
                r#"{
                    "role": "assistant",
                    "modelID": "claude-sonnet-4",
                    "providerID": "anthropic",
                    "tokens": { "input": 42, "output": 7, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
                    "time": { "created": 1700000000000.0 }
                }"#
            ],
        )
        .unwrap();
    drop(conn);

    // Without scanner_settings: no rows (nothing auto-discoverable).
    let parsed_default = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["opencode".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();
    assert_eq!(parsed_default.counts.get(ClientId::OpenCode), 0);
    assert!(parsed_default.messages.is_empty());

    // With scanner_settings pointing at the external db: the user
    // row must show up.
    let parsed_with_settings = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["opencode".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            opencode_db_paths: vec![external_db.clone()],
            ..Default::default()
        },
    })
    .unwrap();
    assert_eq!(
        parsed_with_settings.counts.get(ClientId::OpenCode),
        1,
        "scanner.opencodeDbPaths must reach the parse_local_clients path"
    );
    assert_eq!(parsed_with_settings.messages.len(), 1);
    assert_eq!(parsed_with_settings.messages[0].client, "opencode");
    assert_eq!(parsed_with_settings.messages[0].model_id, "claude-sonnet-4");
}

#[test]
fn test_parse_local_clients_honors_scanner_extra_scan_paths_for_hermes_profile_db() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let profile_dir = temp_dir.path().join(".hermes/profiles/director_planning");
    std::fs::create_dir_all(&profile_dir).unwrap();
    let profile_db = profile_dir.join("state.db");
    let conn = create_hermes_sqlite_db(&profile_db);
    insert_hermes_session(
        &conn,
        "hermes-extra-session",
        "claude-sonnet-4",
        2,
        100,
        25,
        0.07,
    );
    drop(conn);

    let parsed_default = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["hermes".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();
    assert_eq!(parsed_default.counts.get(ClientId::Hermes), 0);
    assert!(parsed_default.messages.is_empty());

    let mut extra_scan_paths = std::collections::BTreeMap::new();
    extra_scan_paths.insert("hermes".to_string(), vec![profile_dir]);
    let parsed_with_settings = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["hermes".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        },
    })
    .unwrap();

    assert_eq!(parsed_with_settings.counts.get(ClientId::Hermes), 2);
    assert_eq!(parsed_with_settings.messages.len(), 1);
    assert_eq!(parsed_with_settings.messages[0].client, "hermes");
    assert_eq!(
        parsed_with_settings.messages[0].agent.as_deref(),
        Some("Hermes Agent")
    );
    assert_eq!(
        parsed_with_settings.messages[0].session_id,
        "hermes-extra-session"
    );
    assert_eq!(parsed_with_settings.messages[0].model_id, "claude-sonnet-4");
    assert_eq!(parsed_with_settings.messages[0].input, 100);
    assert_eq!(parsed_with_settings.messages[0].output, 25);
}

#[test]
fn test_parse_local_clients_honors_scanner_extra_scan_paths_for_zed_threads_db() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let windows_threads_dir = temp_dir.path().join("AppData/Local/Zed/threads");
    std::fs::create_dir_all(&windows_threads_dir).unwrap();
    let threads_db = windows_threads_dir.join("threads.db");
    let conn = create_zed_sqlite_db(&threads_db);
    insert_zed_thread(&conn, "zed-extra-thread", "claude-sonnet-4-5");
    drop(conn);

    let parsed_default = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["zed".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();
    assert_eq!(parsed_default.counts.get(ClientId::Zed), 0);
    assert!(parsed_default.messages.is_empty());

    let mut extra_scan_paths = std::collections::BTreeMap::new();
    extra_scan_paths.insert("zed".to_string(), vec![windows_threads_dir]);
    let parsed_with_settings = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["zed".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        },
    })
    .unwrap();

    assert_eq!(parsed_with_settings.counts.get(ClientId::Zed), 1);
    assert_eq!(parsed_with_settings.messages.len(), 1);
    assert_eq!(parsed_with_settings.messages[0].client, "zed");
    assert_eq!(
        parsed_with_settings.messages[0].session_id,
        "zed-extra-thread"
    );
    assert_eq!(
        parsed_with_settings.messages[0].model_id,
        "claude-sonnet-4-5"
    );
    assert_eq!(parsed_with_settings.messages[0].input, 42);
    assert_eq!(parsed_with_settings.messages[0].output, 7);
}

#[test]
fn test_local_parse_includes_antigravity_cache_rows() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let sessions_dir = temp_dir
        .path()
        .join(".config/tokscale/antigravity-cache/sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(
            sessions_dir.join("ag-local.jsonl"),
            r#"{"type":"usage","sessionId":"ag-local","modelId":"model_placeholder_m84","timestamp":1711200000000,"input":12,"output":4,"cacheRead":2,"cacheWrite":0,"reasoning":1,"responseId":"resp-ag"}
"#,
        )
        .unwrap();

    let clients = vec!["antigravity".to_string(), "synthetic".to_string()];

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_string_lossy().to_string()),
        use_env_roots: false,
        clients: Some(clients),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();

    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].client, "antigravity");
    assert_eq!(parsed.messages[0].model_id, "model_placeholder_m84");
    assert_eq!(parsed.messages[0].input, 12);
    assert_eq!(parsed.messages[0].output, 4);
    assert_eq!(parsed.messages[0].cache_read, 2);
    assert_eq!(parsed.messages[0].reasoning, 1);
}

#[test]
fn test_parse_local_clients_dedups_zed_threads_across_default_and_extra_dbs() {
    let temp_dir = tempfile::TempDir::new().unwrap();

    // Place threads.db at the default platform path so the scanner finds it
    // as `zed_db` AND we also pass it via extraScanPaths.
    let default_threads_dir = temp_dir.path().join(".local/share/zed/threads");
    std::fs::create_dir_all(&default_threads_dir).unwrap();
    let default_db = default_threads_dir.join("threads.db");
    let conn = create_zed_sqlite_db(&default_db);
    insert_zed_thread(&conn, "shared-zed-thread", "claude-sonnet-4-5");
    drop(conn);

    // Point extraScanPaths.zed at the same directory — dedup should prevent
    // the thread from appearing twice.
    let mut extra_scan_paths = std::collections::BTreeMap::new();
    extra_scan_paths.insert("zed".to_string(), vec![default_threads_dir.clone()]);
    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["zed".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        },
    })
    .unwrap();

    // Should see exactly 1 message, not 2 (deduped by canonicalize).
    assert_eq!(parsed.counts.get(ClientId::Zed), 1);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].session_id, "shared-zed-thread");
}

#[test]
fn test_parse_local_clients_zed_extra_scan_paths_nonexistent_dir_is_silent() {
    let temp_dir = tempfile::TempDir::new().unwrap();

    let mut extra_scan_paths = std::collections::BTreeMap::new();
    extra_scan_paths.insert(
        "zed".to_string(),
        vec![temp_dir.path().join("does/not/exist")],
    );
    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["zed".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        },
    })
    .unwrap();

    assert_eq!(parsed.counts.get(ClientId::Zed), 0);
    assert!(parsed.messages.is_empty());
}

#[test]
fn test_parse_local_clients_dedups_hermes_sessions_across_default_and_extra_dbs() {
    let temp_dir = tempfile::TempDir::new().unwrap();

    let default_dir = temp_dir.path().join(".hermes");
    std::fs::create_dir_all(&default_dir).unwrap();
    let default_db = default_dir.join("state.db");
    let default_conn = create_hermes_sqlite_db(&default_db);
    insert_hermes_session(
        &default_conn,
        "shared-hermes-session",
        "claude-sonnet-4",
        2,
        100,
        25,
        0.07,
    );
    drop(default_conn);

    let profile_dir = temp_dir.path().join(".hermes/profiles/director_planning");
    std::fs::create_dir_all(&profile_dir).unwrap();
    let profile_db = profile_dir.join("state.db");
    let profile_conn = create_hermes_sqlite_db(&profile_db);
    insert_hermes_session(
        &profile_conn,
        "shared-hermes-session",
        "claude-sonnet-4",
        9,
        999,
        999,
        9.99,
    );
    drop(profile_conn);

    let mut extra_scan_paths = std::collections::BTreeMap::new();
    extra_scan_paths.insert("hermes".to_string(), vec![profile_db]);
    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["hermes".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        },
    })
    .unwrap();

    assert_eq!(parsed.counts.get(ClientId::Hermes), 2);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].session_id, "shared-hermes-session");
    assert_eq!(parsed.messages[0].input, 100);
    assert_eq!(parsed.messages[0].output, 25);
}

#[test]
fn test_parse_local_clients_claude_filter_ignores_scanner_settings_opencode_db_paths() {
    // Regression guard for the scanner client-filter bypass: even
    // when `scanner.opencodeDbPaths` pins an external opencode db,
    // a `--clients claude` request must NOT pull in OpenCode rows.
    // Before the fix, the merge ran outside the OpenCode-enabled
    // guard so user-pinned dbs leaked through both `messages` and
    // `counts` (the latter is computed before the message-level
    // client filter, so even the post-filter pipeline could not
    // hide a leaked count).
    let temp_dir = tempfile::TempDir::new().unwrap();

    // Claude session: one assistant message, the only thing the
    // filter should accept.
    let claude_dir = temp_dir.path().join(".claude/projects/myproject");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
            claude_dir.join("conversation.jsonl"),
            r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-3-5-sonnet","usage":{"input_tokens":100,"output_tokens":50}}}
"#,
        )
        .unwrap();

    // External opencode.db that the user has pinned via
    // scanner.opencodeDbPaths. Without the fix, this would leak
    // into the Claude-only result.
    let outside_dir = temp_dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let external_db = outside_dir.join("opencode.db");
    let conn = rusqlite::Connection::open(&external_db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
             CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 data TEXT NOT NULL
             );",
    )
    .unwrap();
    conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "leaked-opencode",
                "should-not-show-up",
                r#"{
                    "role": "assistant",
                    "modelID": "claude-sonnet-4",
                    "providerID": "anthropic",
                    "tokens": { "input": 9999, "output": 9999, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
                    "time": { "created": 1700000000000.0 }
                }"#
            ],
        )
        .unwrap();
    drop(conn);

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["claude".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings {
            opencode_db_paths: vec![external_db.clone()],
            ..Default::default()
        },
    })
    .unwrap();

    assert_eq!(
        parsed.counts.get(ClientId::OpenCode),
        0,
        "OpenCode count must stay zero under a Claude-only filter even \
             when scanner.opencodeDbPaths is set"
    );
    assert_eq!(
        parsed.counts.get(ClientId::Claude),
        1,
        "Claude message must still be counted"
    );
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].client, "claude");
    assert!(
        parsed.messages.iter().all(|m| m.client != "opencode"),
        "no OpenCode messages may leak into a Claude-only result, got {:?}",
        parsed.messages
    );
}

#[test]
fn test_parse_local_clients_claude_transcripts_count_only_usage_metadata() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let transcripts_dir = temp_dir.path().join(".claude/transcripts");
    std::fs::create_dir_all(&transcripts_dir).unwrap();
    std::fs::write(
            transcripts_dir.join("ses_123456789012345678901234567.jsonl"),
            r#"{"type":"user","timestamp":"2026-04-01T10:00:00.000Z","message":{"content":"Transcript prompt"}}
{"type":"assistant","timestamp":"2026-04-01T10:00:01.000Z","requestId":"req_transcript","message":{"id":"msg_transcript","model":"claude-sonnet-4","usage":{"input_tokens":123,"output_tokens":45,"cache_read_input_tokens":67,"cache_creation_input_tokens":8}}}
"#,
        )
        .unwrap();
    std::fs::write(
            transcripts_dir.join("ses_765432109876543210987654321.jsonl"),
            r#"{"type":"user","timestamp":"2026-04-01T10:00:00.000Z","message":{"content":"Transcript prompt"}}
{"type":"tool_use","timestamp":"2026-04-01T10:00:01.000Z","message":{"content":"Run tool"}}
{"type":"tool_result","timestamp":"2026-04-01T10:00:02.000Z","message":{"content":"Tool result"}}
"#,
        )
        .unwrap();

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["claude".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();

    assert_eq!(parsed.counts.get(ClientId::Claude), 1);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].client, "claude");
    assert_eq!(
        parsed.messages[0].session_id,
        "ses_123456789012345678901234567"
    );
    assert_eq!(parsed.messages[0].model_id, "claude-sonnet-4");
    assert_eq!(parsed.messages[0].input, 123);
    assert_eq!(parsed.messages[0].output, 45);
    assert_eq!(parsed.messages[0].cache_read, 67);
    assert_eq!(parsed.messages[0].cache_write, 8);
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_refreshes_cc_mirror_provider_when_variant_metadata_changes() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let variant_dir = source_home.path().join(".cc-mirror/kimi-code");
        let config_dir = source_home.path().join("mirror-configs/kimi-code");
        let project_dir = config_dir.join("projects/project-one");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&variant_dir).unwrap();
        let variant_path = variant_dir.join("variant.json");
        std::fs::write(
            &variant_path,
            format!(
                r#"{{"name":"kimi-code","provider":"kimi","configDir":"{}"}}"#,
                config_dir.display()
            ),
        )
        .unwrap();
        let session_path = project_dir.join("session.jsonl");
        std::fs::write(
                &session_path,
                r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-3-5-sonnet","usage":{"input_tokens":100,"output_tokens":50}}}
"#,
            )
            .unwrap();

        let first_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["claude".to_string()],
            None,
        );
        assert_eq!(first_messages.len(), 1);
        assert_eq!(first_messages[0].client, "cc-mirror/kimi-code");
        assert_eq!(first_messages[0].provider_id, "kimi");

        std::fs::write(
            &variant_path,
            format!(
                r#"{{"name":"kimi-code","provider":"minimax","configDir":"{}"}}"#,
                config_dir.display()
            ),
        )
        .unwrap();

        let refreshed_messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["claude".to_string()],
            None,
        );
        assert_eq!(refreshed_messages.len(), 1);
        assert_eq!(refreshed_messages[0].client, "cc-mirror/kimi-code");
        assert_eq!(refreshed_messages[0].provider_id, "minimax");
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
#[serial_test::serial]
fn test_parse_all_messages_keeps_normal_claude_when_cc_mirror_points_at_claude_config() {
    let cache_home = tempfile::TempDir::new().unwrap();
    let source_home = tempfile::TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", cache_home.path());

    {
        let claude_dir = source_home.path().join(".claude");
        let project_dir = claude_dir.join("projects/project-one");
        std::fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session.jsonl");
        std::fs::write(
                &session_path,
                r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-3-5-sonnet","usage":{"input_tokens":100,"output_tokens":50}}}
"#,
            )
            .unwrap();

        let variant_dir = source_home.path().join(".cc-mirror/plain-mirror");
        std::fs::create_dir_all(&variant_dir).unwrap();
        std::fs::write(
            variant_dir.join("variant.json"),
            format!(
                r#"{{"name":"plain-mirror","provider":"mirror","configDir":"{}"}}"#,
                claude_dir.display()
            ),
        )
        .unwrap();

        let messages = parse_all_messages_with_pricing(
            source_home.path().to_str().unwrap(),
            &["claude".to_string()],
            None,
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].client, "claude");
    }

    match original_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn test_parse_local_clients_amp_partial_ledger_recovers_message_fallback_day() {
    use chrono::TimeZone;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let amp_dir = temp_dir.path().join(".local/share/amp/threads");
    std::fs::create_dir_all(&amp_dir).unwrap();

    let thread_created = chrono::DateTime::parse_from_rfc3339("2026-04-04T12:00:00Z")
        .unwrap()
        .timestamp_millis();
    let ledger_timestamp = chrono::DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
        .unwrap()
        .timestamp_millis();

    let thread = format!(
        r#"{{
                "id": "thread-amp-gap",
                "created": {thread_created},
                "usageLedger": {{
                    "events": [
                        {{
                            "timestamp": "2026-04-08T12:00:00Z",
                            "model": "claude-sonnet-4-0",
                            "credits": 0.75,
                            "tokens": {{ "input": 100, "output": 20 }}
                        }}
                    ]
                }},
                "messages": [
                    {{
                        "role": "assistant",
                        "messageId": 1,
                        "usage": {{
                            "model": "claude-sonnet-4-0",
                            "inputTokens": 100,
                            "outputTokens": 20,
                            "credits": 0.75
                        }}
                    }},
                    {{
                        "role": "assistant",
                        "messageId": 2,
                        "usage": {{
                            "model": "claude-sonnet-4-0",
                            "inputTokens": 50,
                            "outputTokens": 10,
                            "credits": 0.40
                        }}
                    }}
                ]
            }}"#
    );
    std::fs::write(amp_dir.join("T-thread-amp-gap.json"), thread).unwrap();

    let parsed = parse_local_clients(LocalParseOptions {
        home_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        use_env_roots: false,
        clients: Some(vec!["amp".to_string()]),
        since: None,
        until: None,
        year: None,
        scanner_settings: scanner::ScannerSettings::default(),
    })
    .unwrap();

    assert_eq!(parsed.counts.get(ClientId::Amp), 2);
    assert_eq!(parsed.messages.len(), 2);

    let dates: HashSet<String> = parsed.messages.iter().map(|msg| msg.date.clone()).collect();
    let local_date = |timestamp_ms: i64| {
        chrono::Local
            .timestamp_millis_opt(timestamp_ms)
            .single()
            .unwrap()
            .format("%Y-%m-%d")
            .to_string()
    };
    assert!(dates.contains(&local_date(thread_created + 2000)));
    assert!(dates.contains(&local_date(ledger_timestamp)));
}
