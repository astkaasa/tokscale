//! Pi (badlogic/pi-mono) session parser
//!
//! Parses JSONL files from ~/.pi/agent/sessions/<encoded-cwd>/*.jsonl

use super::utils::file_modified_timestamp_ms;
use super::{normalize_workspace_key, workspace_label_from_key, UnifiedMessage};
use crate::TokenBreakdown;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Pi session header (first line of JSONL)
#[derive(Debug, Deserialize)]
pub struct PiSessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: String,
    pub cwd: Option<String>,
}

/// Pi session entry (subsequent lines of JSONL)
#[derive(Debug, Deserialize)]
pub struct PiSessionEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: Option<String>,
    pub timestamp: Option<String>,
    pub message: Option<PiMessage>,
}

#[derive(Debug, Deserialize)]
pub struct PiMessage {
    pub role: Option<String>,
    pub usage: Option<PiUsage>,
    pub model: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiUsage {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
}

fn fallback_dedup_key(session_id: &str, assistant_ordinal: u64, entry_json: &str) -> String {
    let content_digest = Sha256::digest(entry_json.as_bytes());
    format!("pi:{session_id}:assistant:{assistant_ordinal}:{content_digest:x}")
}

/// Parse a Pi JSONL session file
pub fn parse_pi_file(path: &Path) -> Vec<UnifiedMessage> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let fallback_timestamp = file_modified_timestamp_ms(path);

    let reader = BufReader::new(file);
    let mut messages: Vec<UnifiedMessage> = Vec::with_capacity(64);
    let mut buffer = Vec::with_capacity(4096);

    let mut session_id: Option<String> = None;
    let mut workspace_key: Option<String> = None;
    let mut workspace_label: Option<String> = None;
    let mut assistant_ordinal = 0_u64;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if session_id.is_none() {
            buffer.clear();
            buffer.extend_from_slice(trimmed.as_bytes());
            let header = match simd_json::from_slice::<PiSessionHeader>(&mut buffer) {
                Ok(h) => h,
                Err(_) => return Vec::new(),
            };

            if header.entry_type != "session" {
                return Vec::new();
            }
            session_id = Some(header.id);
            workspace_key = header.cwd.as_deref().and_then(normalize_workspace_key);
            workspace_label = workspace_key.as_deref().and_then(workspace_label_from_key);
            continue;
        }

        buffer.clear();
        buffer.extend_from_slice(trimmed.as_bytes());
        let entry = match simd_json::from_slice::<PiSessionEntry>(&mut buffer) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.entry_type != "message" {
            continue;
        }

        let message = match entry.message {
            Some(m) => m,
            None => continue,
        };

        if message.role.as_deref() != Some("assistant") {
            continue;
        }
        let current_assistant_ordinal = assistant_ordinal;
        assistant_ordinal = assistant_ordinal.saturating_add(1);

        let usage = match message.usage {
            Some(u) => u,
            None => continue,
        };

        let model = match message.model {
            Some(m) => m,
            None => continue,
        };

        let provider = match message.provider {
            Some(p) => p,
            None => continue,
        };

        let timestamp = entry
            .timestamp
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(fallback_timestamp);
        let session_id = session_id.clone().unwrap_or_else(|| "unknown".to_string());
        let dedup_key = entry
            .id
            .filter(|id| !id.trim().is_empty())
            .map(|id| format!("pi:{session_id}:{id}"))
            .unwrap_or_else(|| fallback_dedup_key(&session_id, current_assistant_ordinal, trimmed));

        let mut unified = UnifiedMessage::new_with_dedup(
            "pi",
            model,
            provider,
            session_id,
            timestamp,
            TokenBreakdown {
                input: usage.input.unwrap_or(0).max(0),
                output: usage.output.unwrap_or(0).max(0),
                cache_read: usage.cache_read.unwrap_or(0).max(0),
                cache_write: usage.cache_write.unwrap_or(0).max(0),
                reasoning: 0,
            },
            0.0,
            Some(dedup_key),
        );
        unified.set_workspace(workspace_key.clone(), workspace_label.clone());
        messages.push(unified);
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::io::Write;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::NamedTempFile;

    fn create_test_file(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    fn set_modified_time(file: &NamedTempFile, seconds_since_epoch: u64) {
        let modified = UNIX_EPOCH + Duration::from_secs(seconds_since_epoch);
        file.as_file()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn test_parse_pi_jsonl_valid_assistant_message() {
        // given
        let content = r#"{"type":"session","id":"pi_ses_001","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}
{"type":"message","id":"msg_001","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"assistant","model":"claude-3-5-sonnet","provider":"anthropic","usage":{"input":100,"output":50,"cacheRead":10,"cacheWrite":5,"totalTokens":165}}}"#;
        let file = create_test_file(content);

        // when
        let messages = parse_pi_file(file.path());

        // then
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].client, "pi");
        assert_eq!(messages[0].session_id, "pi_ses_001");
        assert_eq!(messages[0].model_id, "claude-3-5-sonnet");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(messages[0].tokens.input, 100);
        assert_eq!(messages[0].tokens.output, 50);
        assert_eq!(messages[0].tokens.cache_read, 10);
        assert_eq!(messages[0].tokens.cache_write, 5);
        assert_eq!(
            messages[0].dedup_key.as_deref(),
            Some("pi:pi_ses_001:msg_001")
        );
        assert_eq!(messages[0].workspace_key, Some("/tmp".to_string()));
        assert_eq!(messages[0].workspace_label, Some("tmp".to_string()));
    }

    #[test]
    fn test_parse_pi_skips_non_assistant_messages() {
        // given
        let content = r#"{"type":"session","id":"pi_ses_002","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}
{"type":"message","timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","model":"claude-3-5-sonnet","provider":"anthropic","usage":{"input":100,"output":50,"cacheRead":0,"cacheWrite":0,"totalTokens":150}}}"#;
        let file = create_test_file(content);

        // when
        let messages = parse_pi_file(file.path());

        // then
        assert!(messages.is_empty());
    }

    #[test]
    fn test_parse_pi_skips_missing_usage() {
        // given
        let content = r#"{"type":"session","id":"pi_ses_003","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}
{"type":"message","timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"assistant","model":"claude-3-5-sonnet","provider":"anthropic"}}"#;
        let file = create_test_file(content);

        // when
        let messages = parse_pi_file(file.path());

        // then
        assert!(messages.is_empty());
    }

    #[test]
    fn test_parse_pi_skips_malformed_json_lines() {
        // given
        let content = r#"{"type":"session","id":"pi_ses_004","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}
not valid json
{"type":"message","timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"assistant","model":"gpt-4o-mini","provider":"openai","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15}}}"#;
        let file = create_test_file(content);

        // when
        let messages = parse_pi_file(file.path());

        // then
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].model_id, "gpt-4o-mini");
        assert_eq!(messages[0].provider_id, "openai");
    }

    #[test]
    fn test_missing_identity_is_stable_after_append_changes_file_mtime() {
        let content = r#"{"type":"session","id":"pi_ses_fallback","cwd":"/tmp"}
{"type":"message","message":{"role":"assistant","model":"gpt-5","provider":"openai","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0}}}"#;
        let mut file = create_test_file(content);
        set_modified_time(&file, 1_700_000_000);

        let before_append = parse_pi_file(file.path());
        assert_eq!(before_append.len(), 1);

        let appended = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","provider":"openai","usage":{"input":20,"output":10,"cacheRead":0,"cacheWrite":0}}}"#;
        write!(file, "\n{appended}").unwrap();
        file.flush().unwrap();
        set_modified_time(&file, 1_700_000_060);

        let after_append = parse_pi_file(file.path());
        assert_eq!(after_append.len(), 2);
        assert_eq!(before_append[0].timestamp, 1_700_000_000_000);
        assert_eq!(after_append[0].timestamp, 1_700_000_060_000);
        assert_eq!(before_append[0].dedup_key, after_append[0].dedup_key);
    }

    #[test]
    fn test_adjacent_missing_identity_messages_have_distinct_dedup_keys() {
        let content = r#"{"type":"session","id":"pi_ses_adjacent","cwd":"/tmp"}
{"type":"message","message":{"role":"assistant","model":"gpt-5","provider":"openai","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0}}}
{"type":"message","message":{"role":"assistant","model":"gpt-5","provider":"openai","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0}}}"#;
        let file = create_test_file(content);

        let messages = parse_pi_file(file.path());

        assert_eq!(messages.len(), 2);
        assert_ne!(messages[0].dedup_key, messages[1].dedup_key);
    }
}
