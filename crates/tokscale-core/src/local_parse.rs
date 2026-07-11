use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::clients::{ClientCounts, ClientId};
use crate::reports::{LocalParseOptions, ParsedMessage, ParsedMessages, TokenBreakdown};
use crate::sessions::UnifiedMessage;
use crate::{message_cache, pricing, scanner, sessions};

pub(crate) fn retain_for_requested_clients(
    client: &str,
    model_id: &str,
    provider_id: &str,
    requested: &HashSet<&str>,
) -> bool {
    requested.contains(client)
        || (requested.contains("claude") && client.starts_with("cc-mirror/"))
        || (requested.contains("synthetic")
            && sessions::synthetic::matches_synthetic_filter(client, model_id, provider_id))
}

pub(crate) const UNKNOWN_WORKSPACE_LABEL: &str = "Unknown workspace";
pub(crate) const UNKNOWN_WORKSPACE_GROUP_KEY: &str = "\0unknown-workspace";

pub fn get_home_dir_string(home_dir_option: &Option<String>) -> Result<String, String> {
    home_dir_option
        .clone()
        .or_else(|| std::env::var("HOME").ok())
        .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().into_owned()))
        .ok_or_else(|| {
            "HOME directory not specified and could not determine home directory".to_string()
        })
}

#[cfg(test)]
pub(crate) fn parse_all_messages_with_pricing(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
) -> Vec<UnifiedMessage> {
    parse_all_messages_with_pricing_with_env_strategy(
        home_dir,
        clients,
        pricing,
        true,
        &scanner::ScannerSettings::default(),
    )
}

pub(crate) fn parse_all_messages_with_pricing_with_env_strategy(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
    use_env_roots: bool,
    scanner_settings: &scanner::ScannerSettings,
) -> Vec<UnifiedMessage> {
    #[derive(Debug)]
    struct CachedParseOutcome {
        messages: Vec<UnifiedMessage>,
        cache_entry: Option<message_cache::CachedSourceEntry>,
        invalidate_cache: bool,
    }

    fn apply_pricing_to_messages(
        messages: &mut [UnifiedMessage],
        pricing: Option<&pricing::PricingService>,
    ) {
        for message in messages {
            message.refresh_derived_fields();
            apply_pricing_if_available(message, pricing);
        }
    }

    fn cached_messages(
        cached: &message_cache::CachedSourceEntry,
        pricing: Option<&pricing::PricingService>,
    ) -> Vec<UnifiedMessage> {
        let mut messages = cached.messages.clone();
        apply_pricing_to_messages(&mut messages, pricing);
        messages
    }

    fn parse_full_log_source(
        path: &Path,
        pricing: Option<&pricing::PricingService>,
        is_headless: bool,
    ) -> CachedParseOutcome {
        let fallback_timestamp = sessions::utils::file_modified_timestamp_ms(path);
        let parsed = sessions::codex::parse_codex_file_incremental(
            path,
            0,
            sessions::codex::CodexParseState::default(),
        );
        let messages = finalize_codex_messages(
            parsed.messages.clone(),
            pricing,
            is_headless,
            &parsed.fallback_timestamp_indices,
            fallback_timestamp,
        );
        if !parsed.parse_succeeded {
            return CachedParseOutcome {
                messages,
                cache_entry: None,
                invalidate_cache: false,
            };
        }

        if parsed.unresolved_model_events {
            return CachedParseOutcome {
                messages,
                cache_entry: None,
                invalidate_cache: false,
            };
        }

        let cache_entry = build_codex_cache_entry(
            path,
            parsed.messages,
            parsed.consumed_offset,
            parsed.state,
            parsed.fallback_timestamp_indices,
        );

        CachedParseOutcome {
            messages,
            cache_entry,
            invalidate_cache: false,
        }
    }

    fn finalize_codex_messages(
        mut messages: Vec<UnifiedMessage>,
        pricing: Option<&pricing::PricingService>,
        is_headless: bool,
        fallback_timestamp_indices: &[usize],
        fallback_timestamp: i64,
    ) -> Vec<UnifiedMessage> {
        for index in fallback_timestamp_indices {
            if let Some(message) = messages.get_mut(*index) {
                message.set_timestamp(fallback_timestamp);
            }
        }
        apply_pricing_to_messages(&mut messages, pricing);
        for message in &mut messages {
            apply_headless_agent(message, is_headless);
        }
        messages
    }

    fn build_codex_cache_entry(
        path: &Path,
        raw_messages: Vec<UnifiedMessage>,
        consumed_offset: u64,
        state: sessions::codex::CodexParseState,
        fallback_timestamp_indices: Vec<usize>,
    ) -> Option<message_cache::CachedSourceEntry> {
        let fingerprint = message_cache::SourceFingerprint::from_path(path)?;
        if fingerprint.size != consumed_offset {
            return None;
        }

        let codex_incremental =
            message_cache::build_codex_incremental_cache(path, consumed_offset, state)?;

        Some(message_cache::CachedSourceEntry::new(
            path,
            fingerprint,
            raw_messages,
            fallback_timestamp_indices,
            Some(codex_incremental),
        ))
    }

    fn load_or_parse_source_with_fingerprint_and_policy<F, FingerprintFn>(
        path: &Path,
        source_cache: &message_cache::SourceMessageCache,
        pricing: Option<&pricing::PricingService>,
        fingerprint_from_path: FingerprintFn,
        parse: F,
    ) -> CachedParseOutcome
    where
        F: Fn(&Path) -> (Vec<UnifiedMessage>, bool),
        FingerprintFn: Fn(&Path) -> Option<message_cache::SourceFingerprint>,
    {
        let Some(fingerprint) = fingerprint_from_path(path) else {
            let (mut messages, _) = parse(path);
            apply_pricing_to_messages(&mut messages, pricing);
            return CachedParseOutcome {
                messages,
                cache_entry: None,
                invalidate_cache: false,
            };
        };

        if let Some(cached) = source_cache.get(path) {
            if cached.fingerprint == fingerprint && !cached.messages.is_empty() {
                return CachedParseOutcome {
                    messages: cached_messages(cached, pricing),
                    cache_entry: None,
                    invalidate_cache: false,
                };
            }
        }

        let (mut messages, cacheable) = parse(path);
        let cache_entry = if messages.is_empty() || !cacheable {
            None
        } else {
            Some(message_cache::CachedSourceEntry::new(
                path,
                fingerprint,
                messages.clone(),
                Vec::new(),
                None,
            ))
        };
        apply_pricing_to_messages(&mut messages, pricing);

        CachedParseOutcome {
            messages,
            cache_entry,
            invalidate_cache: !cacheable,
        }
    }

    fn load_or_parse_source_with_fingerprint<F, FingerprintFn>(
        path: &Path,
        source_cache: &message_cache::SourceMessageCache,
        pricing: Option<&pricing::PricingService>,
        fingerprint_from_path: FingerprintFn,
        parse: F,
    ) -> CachedParseOutcome
    where
        F: Fn(&Path) -> Vec<UnifiedMessage>,
        FingerprintFn: Fn(&Path) -> Option<message_cache::SourceFingerprint>,
    {
        load_or_parse_source_with_fingerprint_and_policy(
            path,
            source_cache,
            pricing,
            fingerprint_from_path,
            |path| (parse(path), true),
        )
    }

    fn load_or_parse_source<F>(
        path: &Path,
        source_cache: &message_cache::SourceMessageCache,
        pricing: Option<&pricing::PricingService>,
        parse: F,
    ) -> CachedParseOutcome
    where
        F: Fn(&Path) -> Vec<UnifiedMessage>,
    {
        load_or_parse_source_with_fingerprint(
            path,
            source_cache,
            pricing,
            message_cache::SourceFingerprint::from_path,
            parse,
        )
    }

    fn load_or_parse_sqlite_source<F>(
        path: &Path,
        source_cache: &message_cache::SourceMessageCache,
        pricing: Option<&pricing::PricingService>,
        parse: F,
    ) -> CachedParseOutcome
    where
        F: Fn(&Path) -> Vec<UnifiedMessage>,
    {
        load_or_parse_source_with_fingerprint(
            path,
            source_cache,
            pricing,
            message_cache::SourceFingerprint::from_sqlite_path,
            parse,
        )
    }

    fn load_or_parse_codex_source(
        path: &Path,
        source_cache: &message_cache::SourceMessageCache,
        pricing: Option<&pricing::PricingService>,
        headless_roots: &[PathBuf],
    ) -> CachedParseOutcome {
        let is_headless = is_headless_path(path, headless_roots);
        let Some(fingerprint) = message_cache::SourceFingerprint::from_path(path) else {
            return parse_full_log_source(path, pricing, is_headless);
        };
        let fallback_timestamp = sessions::utils::file_modified_timestamp_ms(path);

        if let Some(cached) = source_cache.get(path) {
            let reparse_from_start = |invalidate_cache: bool| {
                let mut outcome = parse_full_log_source(path, pricing, is_headless);
                outcome.invalidate_cache = invalidate_cache && outcome.cache_entry.is_none();
                outcome
            };

            if cached.fingerprint == fingerprint {
                if message_cache::codex_cache_entry_matches_fingerprint(cached, &fingerprint) {
                    return CachedParseOutcome {
                        messages: finalize_codex_messages(
                            cached.messages.clone(),
                            pricing,
                            is_headless,
                            &cached.fallback_timestamp_indices,
                            fallback_timestamp,
                        ),
                        cache_entry: None,
                        invalidate_cache: false,
                    };
                }

                return reparse_from_start(true);
            }

            if let Some(codex_incremental) = cached.codex_incremental.as_ref() {
                if fingerprint.size > codex_incremental.consumed_offset
                    && message_cache::codex_prefix_matches(path, codex_incremental)
                {
                    let parsed = sessions::codex::parse_codex_file_incremental(
                        path,
                        codex_incremental.consumed_offset,
                        codex_incremental.state.clone(),
                    );
                    if parsed.parse_succeeded && !parsed.unresolved_model_events {
                        let mut raw_messages = cached.messages.clone();
                        let mut fallback_timestamp_indices =
                            cached.fallback_timestamp_indices.clone();
                        let existing_len = raw_messages.len();
                        fallback_timestamp_indices.extend(
                            parsed
                                .fallback_timestamp_indices
                                .iter()
                                .map(|index| existing_len + index),
                        );
                        raw_messages.extend(parsed.messages.clone());
                        let cache_entry = build_codex_cache_entry(
                            path,
                            raw_messages.clone(),
                            parsed.consumed_offset,
                            parsed.state,
                            fallback_timestamp_indices.clone(),
                        );
                        if let Some(cache_entry) = cache_entry {
                            let messages = finalize_codex_messages(
                                raw_messages,
                                pricing,
                                is_headless,
                                &fallback_timestamp_indices,
                                fallback_timestamp,
                            );

                            return CachedParseOutcome {
                                messages,
                                cache_entry: Some(cache_entry),
                                invalidate_cache: false,
                            };
                        }
                    }
                }
            }

            return reparse_from_start(true);
        }

        parse_full_log_source(path, pricing, is_headless)
    }

    let scan_result = scanner::scan_all_clients_with_scanner_settings(
        home_dir,
        clients,
        use_env_roots,
        scanner_settings,
    );
    let headless_roots = scanner::headless_roots_with_env_strategy(home_dir, use_env_roots);
    let mut source_cache = message_cache::SourceMessageCache::load();
    source_cache.prune_missing_files();
    let mut all_messages: Vec<UnifiedMessage> = Vec::new();
    let include_all = clients.is_empty();
    let include_synthetic = include_all || clients.iter().any(|c| c == "synthetic");

    // Parse OpenCode: prefer SQLite, collapse forked SQLite history there, then
    // suppress legacy JSON overlap by message identity.
    let mut opencode_seen: HashSet<String> = HashSet::new();

    for db_path in &scan_result.opencode_dbs {
        let CachedParseOutcome {
            messages,
            cache_entry,
            ..
        } = load_or_parse_sqlite_source(db_path, &source_cache, pricing, |path| {
            sessions::opencode::parse_opencode_sqlite(path)
        });

        // Dedup across channel-suffixed dbs: the same session can end up in
        // both `opencode.db` and `opencode-<channel>.db` if the user
        // switches channels mid-session. `discover_opencode_dbs` returns
        // paths in sorted order, so the first-seen copy is deterministic.
        all_messages.extend(messages.into_iter().filter(|message| {
            message
                .dedup_key
                .as_ref()
                .is_none_or(|key| opencode_seen.insert(key.clone()))
        }));

        if let Some(entry) = cache_entry {
            source_cache.insert(entry);
        }
    }

    let opencode_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::OpenCode)
        .par_iter()
        .filter_map(|path| {
            Some(load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::opencode::parse_opencode_file(path)
                    .into_iter()
                    .collect()
            }))
        })
        .collect();
    for outcome in opencode_outcomes {
        all_messages.extend(outcome.messages.into_iter().filter(|message| {
            message
                .dedup_key
                .as_ref()
                .is_none_or(|key| opencode_seen.insert(key.clone()))
        }));
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let claude_home = PathBuf::from(home_dir);
    let claude_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Claude)
        .par_iter()
        .map(|path| {
            load_or_parse_source_with_fingerprint(
                path,
                &source_cache,
                pricing,
                |path| {
                    message_cache::SourceFingerprint::from_claude_code_path_with_home(
                        path,
                        Some(&claude_home),
                    )
                },
                |path| sessions::claudecode::parse_claude_file_with_home(path, Some(&claude_home)),
            )
        })
        .collect();
    let mut claude_messages_raw: Vec<(String, UnifiedMessage)> = Vec::new();
    for outcome in claude_outcomes {
        claude_messages_raw.extend(outcome.messages.into_iter().map(|msg| {
            let dedup_key = msg.dedup_key.clone().unwrap_or_default();
            (dedup_key, msg)
        }));
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let mut seen_keys: HashSet<String> = HashSet::new();
    let claude_messages: Vec<UnifiedMessage> = claude_messages_raw
        .into_iter()
        .filter(|(key, _)| key.is_empty() || seen_keys.insert(key.clone()))
        .map(|(_, msg)| msg)
        .collect();
    all_messages.extend(claude_messages);

    let codex_outcomes: Vec<(PathBuf, CachedParseOutcome)> = scan_result
        .get(ClientId::Codex)
        .par_iter()
        .map(|path| {
            (
                path.clone(),
                load_or_parse_codex_source(path, &source_cache, pricing, &headless_roots),
            )
        })
        .collect();
    let mut codex_seen: HashSet<String> = HashSet::new();
    for (path, outcome) in codex_outcomes {
        all_messages.extend(
            outcome
                .messages
                .into_iter()
                .filter(|message| should_keep_deduped_message(&mut codex_seen, message)),
        );
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        } else if outcome.invalidate_cache {
            source_cache.remove(&path);
        }
    }

    let copilot_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Copilot)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::copilot::parse_copilot_file(path)
            })
        })
        .collect();
    for outcome in copilot_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let gemini_outcomes: Vec<(PathBuf, CachedParseOutcome)> = scan_result
        .get(ClientId::Gemini)
        .par_iter()
        .map(|path| {
            let outcome = load_or_parse_source_with_fingerprint_and_policy(
                path,
                &source_cache,
                pricing,
                message_cache::SourceFingerprint::from_path,
                |path| {
                    let parsed = sessions::gemini::parse_gemini_file_with_cache_status(path);
                    (parsed.messages, parsed.cacheable)
                },
            );
            (path.clone(), outcome)
        })
        .collect();
    for (path, outcome) in gemini_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        } else if outcome.invalidate_cache {
            source_cache.remove(&path);
        }
    }

    let cursor_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Cursor)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::cursor::parse_cursor_file(path)
            })
        })
        .collect();
    for outcome in cursor_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let warp_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Warp)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::warp::parse_warp_file(path)
            })
        })
        .collect();
    for outcome in warp_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let amp_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Amp)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::amp::parse_amp_file(path)
            })
        })
        .collect();
    for outcome in amp_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let codebuff_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Codebuff)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::codebuff::parse_codebuff_file(path)
            })
        })
        .collect();
    for outcome in codebuff_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let droid_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Droid)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::droid::parse_droid_file(path)
            })
        })
        .collect();
    for outcome in droid_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let openclaw_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::OpenClaw)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::openclaw::parse_openclaw_transcript(path)
            })
        })
        .collect();
    for outcome in openclaw_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let pi_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Pi)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::pi::parse_pi_file(path)
            })
        })
        .collect();
    for outcome in pi_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    // gjc (gajae-code) JSONL sessions. Binding note N1: this cached cluster
    // MUST obtain messages via the non-repricing parser and apply the A1
    // Hermes guard explicitly (reprice only when the embedded usage.cost.total
    // was absent, i.e. cost <= 0.0). Routing through load_or_parse_source /
    // apply_pricing_to_messages / cached_messages would reprice unconditionally
    // and overwrite gjc's authoritative embedded cost, silently downgrading to
    // A2 on the dominant cached path. Message-level dedup via
    // should_keep_deduped_message collapses depth-1/depth-2 replays.
    let mut gjc_seen: HashSet<String> = HashSet::new();
    let gjc_messages: Vec<UnifiedMessage> = scan_result
        .get(ClientId::Gjc)
        .par_iter()
        .flat_map(|path| {
            sessions::gjc::parse_gjc_file(path)
                .into_iter()
                .map(|mut msg| {
                    if msg.cost <= 0.0 {
                        apply_pricing_if_available(&mut msg, pricing);
                    }
                    msg
                })
                .collect::<Vec<_>>()
        })
        .collect();
    all_messages.extend(
        gjc_messages
            .into_iter()
            .filter(|message| should_keep_deduped_message(&mut gjc_seen, message)),
    );

    let kimi_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Kimi)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::kimi::parse_kimi_file(path)
            })
        })
        .collect();
    for outcome in kimi_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    // Parse Qwen files
    let qwen_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Qwen)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::qwen::parse_qwen_file(path)
            })
        })
        .collect();
    for outcome in qwen_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let roocode_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::RooCode)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::roocode::parse_roocode_file(path)
            })
        })
        .collect();
    for outcome in roocode_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let kilocode_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::KiloCode)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::kilocode::parse_kilocode_file(path)
            })
        })
        .collect();
    for outcome in kilocode_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let cline_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Cline)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::cline::parse_cline_file(path)
            })
        })
        .collect();
    for outcome in cline_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let mux_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Mux)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::mux::parse_mux_file(path)
            })
        })
        .collect();
    for outcome in mux_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    // Kilo CLI: SQLite database
    if let Some(db_path) = &scan_result.kilo_db {
        let kilo_messages: Vec<UnifiedMessage> = sessions::kilo::parse_kilo_sqlite(db_path)
            .into_iter()
            .map(|mut msg| {
                apply_pricing_if_available(&mut msg, pricing);
                msg
            })
            .collect();
        all_messages.extend(kilo_messages);
    }

    let mut hermes_seen: HashSet<String> = HashSet::new();
    for db_path in scan_result.hermes_db_paths() {
        let hermes_messages = parse_hermes_sqlite_with_pricing(&db_path, pricing);
        all_messages.extend(
            hermes_messages
                .into_iter()
                .filter(|message| should_keep_deduped_message(&mut hermes_seen, message)),
        );
    }

    if let Some(db_path) = &scan_result.goose_db {
        let goose_messages: Vec<UnifiedMessage> = sessions::goose::parse_goose_sqlite(db_path)
            .into_iter()
            .map(|mut msg| {
                apply_pricing_if_available(&mut msg, pricing);
                msg
            })
            .collect();
        all_messages.extend(goose_messages);
    }

    for db_path in scan_result.zed_db_paths() {
        let outcome = load_or_parse_sqlite_source(&db_path, &source_cache, pricing, |path| {
            sessions::zed::parse_zed_sqlite(path)
        });
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    let kiro_outcomes: Vec<CachedParseOutcome> = scan_result
        .get(ClientId::Kiro)
        .par_iter()
        .map(|path| {
            load_or_parse_source(path, &source_cache, pricing, |path| {
                sessions::kiro::parse_kiro_file(path)
            })
        })
        .collect();
    for outcome in kiro_outcomes {
        all_messages.extend(outcome.messages);
        if let Some(entry) = outcome.cache_entry {
            source_cache.insert(entry);
        }
    }

    if let Some(db_path) = &scan_result.kiro_db {
        let kiro_db_messages: Vec<UnifiedMessage> = sessions::kiro::parse_kiro_sqlite(db_path)
            .into_iter()
            .map(|mut msg| {
                apply_pricing_if_available(&mut msg, pricing);
                msg
            })
            .collect();
        all_messages.extend(kiro_db_messages);
    }

    for source in &scan_result.crush_dbs {
        let crush_messages: Vec<UnifiedMessage> =
            sessions::crush::parse_crush_sqlite(&source.db_path)
                .into_iter()
                .map(|mut msg| {
                    msg.set_workspace(source.workspace_key.clone(), source.workspace_label.clone());
                    apply_pricing_if_available(&mut msg, pricing);
                    msg
                })
                .collect();
        all_messages.extend(crush_messages);
    }

    let antigravity_messages: Vec<UnifiedMessage> = scan_result
        .get(ClientId::Antigravity)
        .par_iter()
        .flat_map(|path| {
            sessions::antigravity::parse_antigravity_file(path)
                .into_iter()
                .map(|mut msg| {
                    apply_pricing_if_available(&mut msg, pricing);
                    msg
                })
                .collect::<Vec<_>>()
        })
        .collect();
    all_messages.extend(antigravity_messages);

    // Trae API dump uses exact dollar_float totals, so pricing lookup is not needed.
    let trae_messages: Vec<UnifiedMessage> = scan_result
        .get(ClientId::Trae)
        .par_iter()
        .flat_map(|path| sessions::trae::parse_trae_file("trae", path))
        .collect();
    let deduped_trae_messages = dedupe_latest_trae_messages(trae_messages);
    all_messages.extend(deduped_trae_messages);

    if include_synthetic {
        if let Some(db_path) = &scan_result.synthetic_db {
            let outcome = load_or_parse_sqlite_source(db_path, &source_cache, pricing, |path| {
                sessions::synthetic::parse_octofriend_sqlite(path)
            });
            all_messages.extend(outcome.messages);
            if let Some(entry) = outcome.cache_entry {
                source_cache.insert(entry);
            }
        }
    }

    // Filter BEFORE normalization so retain_for_requested_clients can see
    // original model/provider prefixes (e.g. "accounts/fireworks/models/…")
    // that is_synthetic_gateway relies on for gateway detection.
    if !include_all {
        let requested: HashSet<&str> = clients.iter().map(String::as_str).collect();
        all_messages.retain(|msg| {
            retain_for_requested_clients(&msg.client, &msg.model_id, &msg.provider_id, &requested)
        });
    }

    if include_synthetic {
        for msg in &mut all_messages {
            sessions::synthetic::normalize_synthetic_gateway_fields(
                &mut msg.model_id,
                &mut msg.provider_id,
            );
        }
    }

    source_cache.save_if_dirty();

    all_messages
}

pub(crate) fn dedupe_latest_trae_messages(
    mut messages: Vec<UnifiedMessage>,
) -> Vec<UnifiedMessage> {
    let mut latest_by_session: HashMap<String, UnifiedMessage> = HashMap::new();

    for message in messages.drain(..) {
        let session_id = message.session_id.clone();
        match latest_by_session.get_mut(&session_id) {
            Some(existing) => {
                let should_replace = message.timestamp > existing.timestamp
                    || (message.timestamp == existing.timestamp
                        && message.dedup_key.as_ref().is_some_and(|key| {
                            existing
                                .dedup_key
                                .as_ref()
                                .is_none_or(|existing_key| key > existing_key)
                        }));
                if should_replace {
                    *existing = message;
                }
            }
            None => {
                let _ = latest_by_session.insert(session_id, message);
            }
        }
    }

    let mut deduped: Vec<UnifiedMessage> = latest_by_session.into_values().collect();
    deduped.sort_unstable_by(|a, b| {
        a.session_id
            .cmp(&b.session_id)
            .then_with(|| a.timestamp.cmp(&b.timestamp))
    });
    deduped
}

fn filter_unified_messages(
    messages: Vec<UnifiedMessage>,
    options: &LocalParseOptions,
) -> Vec<UnifiedMessage> {
    let mut filtered = messages;

    if let Some(year) = &options.year {
        let year_prefix = format!("{}-", year);
        filtered.retain(|m| m.date.starts_with(&year_prefix));
    }

    if let Some(since) = &options.since {
        filtered.retain(|m| m.date.as_str() >= since.as_str());
    }

    if let Some(until) = &options.until {
        filtered.retain(|m| m.date.as_str() <= until.as_str());
    }

    filtered
}

fn is_headless_path(path: &Path, headless_roots: &[PathBuf]) -> bool {
    headless_roots.iter().any(|root| path.starts_with(root))
}

fn apply_headless_agent(message: &mut UnifiedMessage, is_headless: bool) {
    if is_headless && message.agent.is_none() {
        message.agent = Some("headless".to_string());
    }
}

fn pricing_multiplier(message: &UnifiedMessage) -> f64 {
    // Zed bills hosted models at provider list price + 10%.
    // Source: https://zed.dev/docs/ai/plans-and-usage and https://zed.dev/docs/ai/models
    //
    // The multiplier is keyed on the message's `provider_id`, not on the
    // provenance of the matched LiteLLM pricing row. Today this is safe because
    // tokscale's bundled LiteLLM dataset only carries upstream-provider rows
    // (anthropic, openai, google) for the underlying models. If a future
    // LiteLLM update adds rows under provider `zed.dev` that already include
    // Zed's markup, this function would double-bill — revisit by threading
    // the matched-price provenance through `apply_pricing_if_available`.
    if message.client == "zed"
        && message
            .provider_id
            .eq_ignore_ascii_case(sessions::zed::ZED_HOSTED_PROVIDER)
    {
        1.1
    } else {
        1.0
    }
}

pub(crate) fn apply_pricing_if_available(
    message: &mut UnifiedMessage,
    pricing: Option<&pricing::PricingService>,
) {
    let Some(pricing) = pricing else {
        return;
    };

    let calculated_cost = pricing.calculate_cost_with_provider(
        &message.model_id,
        Some(&message.provider_id),
        &message.tokens,
    ) * pricing_multiplier(message);

    if calculated_cost > 0.0 {
        message.cost = calculated_cost;
    }
}

fn parse_hermes_sqlite_with_pricing(
    db_path: &Path,
    pricing: Option<&pricing::PricingService>,
) -> Vec<UnifiedMessage> {
    sessions::hermes::parse_hermes_sqlite(db_path)
        .into_iter()
        .map(|mut msg| {
            if msg.cost <= 0.0 {
                apply_pricing_if_available(&mut msg, pricing);
            }
            msg
        })
        .collect()
}

pub(crate) fn select_local_parse_pricing<F>(
    fresh: Result<Arc<pricing::PricingService>, String>,
    stale: F,
) -> Option<Arc<pricing::PricingService>>
where
    F: FnOnce() -> Option<pricing::PricingService>,
{
    fresh.ok().or_else(|| stale().map(Arc::new))
}

pub(crate) async fn load_pricing_for_local_parse() -> Option<Arc<pricing::PricingService>> {
    if std::env::var("TOKSCALE_PRICING_CACHE_ONLY")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
    {
        return pricing::PricingService::load_cached_any_age().map(Arc::new);
    }

    // Interactive/local views should pick up newly released model pricing as soon
    // as a fresh fetch succeeds, but still remain usable offline by falling back
    // to any cached dataset when the network path fails.
    select_local_parse_pricing(
        pricing::PricingService::get_or_init().await,
        pricing::PricingService::load_cached_any_age,
    )
}

fn resolve_local_parse_request(
    options: &LocalParseOptions,
) -> Result<(String, Vec<String>), String> {
    let home_dir = get_home_dir_string(&options.home_dir)?;
    let clients = options.clients.clone().unwrap_or_else(|| {
        let mut clients: Vec<String> = ClientId::iter()
            .filter(|c| c.parse_local())
            .map(|c| c.as_str().to_string())
            .collect();
        clients.push("synthetic".to_string());
        clients
    });
    Ok((home_dir, clients))
}

fn parse_local_unified_messages_resolved(
    options: LocalParseOptions,
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
    telemetry_store_path: Option<&Path>,
) -> Result<Vec<UnifiedMessage>, String> {
    let messages = parse_all_messages_with_pricing_with_env_strategy(
        home_dir,
        clients,
        pricing,
        options.use_env_roots,
        &options.scanner_settings,
    );
    let messages = crate::telemetry::reconcile_legacy_messages_best_effort(
        telemetry_store_path,
        messages,
        clients,
    );
    Ok(filter_unified_messages(messages, &options))
}
pub fn parse_local_clients(options: LocalParseOptions) -> Result<ParsedMessages, String> {
    let start = Instant::now();

    let home_dir = get_home_dir_string(&options.home_dir)?;

    let clients: Vec<String> = options.clients.clone().unwrap_or_else(|| {
        let mut clients: Vec<String> = ClientId::iter()
            .filter(|c| c.parse_local())
            .map(|c| c.as_str().to_string())
            .collect();
        clients.push("synthetic".to_string());
        clients
    });
    let include_all = clients.is_empty();
    let include_synthetic = include_all || clients.iter().any(|c| c == "synthetic");

    let scan_result = scanner::scan_all_clients_with_scanner_settings(
        &home_dir,
        &clients,
        options.use_env_roots,
        &options.scanner_settings,
    );
    let headless_roots =
        scanner::headless_roots_with_env_strategy(&home_dir, options.use_env_roots);

    let mut messages: Vec<ParsedMessage> = Vec::new();

    // Parse OpenCode: prefer SQLite, collapse forked SQLite history there, then
    // suppress legacy JSON overlap by message identity.
    let mut counts = ClientCounts::new();

    let opencode_count: i32 = {
        let mut seen: HashSet<String> = HashSet::new();
        let mut count: i32 = 0;

        for db_path in &scan_result.opencode_dbs {
            let sqlite_msgs: Vec<(String, ParsedMessage)> =
                sessions::opencode::parse_opencode_sqlite(db_path)
                    .into_iter()
                    .filter_map(|msg| {
                        let key = msg.dedup_key.clone().unwrap_or_default();
                        // Dedup across multiple channel-suffixed dbs: the
                        // same session can end up in both `opencode.db` and
                        // `opencode-<channel>.db` if the user switches
                        // channels mid-session.
                        if !key.is_empty() && !seen.insert(key.clone()) {
                            return None;
                        }
                        Some((key, unified_to_parsed(&msg)))
                    })
                    .collect();
            count += sqlite_msgs.len() as i32;
            for (_key, parsed) in sqlite_msgs {
                messages.push(parsed);
            }
        }

        let json_msgs: Vec<(String, ParsedMessage)> = scan_result
            .get(ClientId::OpenCode)
            .par_iter()
            .filter_map(|path| {
                let msg = sessions::opencode::parse_opencode_file(path)?;
                let key = msg.dedup_key.clone().unwrap_or_default();
                Some((key, unified_to_parsed(&msg)))
            })
            .collect();
        let deduped: Vec<ParsedMessage> = json_msgs
            .into_iter()
            .filter(|(key, _)| key.is_empty() || seen.insert(key.clone()))
            .map(|(_, msg)| msg)
            .collect();
        count += deduped.len() as i32;
        messages.extend(deduped);

        count
    };
    counts.set(ClientId::OpenCode, opencode_count);

    let claude_home = PathBuf::from(&home_dir);
    let claude_msgs_raw: Vec<(String, ParsedMessage)> = scan_result
        .get(ClientId::Claude)
        .par_iter()
        .map_init(std::collections::HashMap::new, |parent_cache, path| {
            sessions::claudecode::parse_claude_file_with_cache_and_home(
                path,
                parent_cache,
                Some(&claude_home),
            )
            .into_iter()
            .map(|msg| {
                let dedup_key = msg.dedup_key.clone().unwrap_or_default();
                (dedup_key, unified_to_parsed(&msg))
            })
            .collect::<Vec<_>>()
        })
        .flatten()
        .collect();

    let mut seen_keys: HashSet<String> = HashSet::new();
    let claude_msgs: Vec<ParsedMessage> = claude_msgs_raw
        .into_iter()
        .filter(|(key, _)| key.is_empty() || seen_keys.insert(key.clone()))
        .map(|(_, msg)| msg)
        .collect();
    let claude_count = claude_msgs.len() as i32;
    counts.set(ClientId::Claude, claude_count);
    messages.extend(claude_msgs);

    let codex_msgs_raw: Vec<UnifiedMessage> = scan_result
        .get(ClientId::Codex)
        .par_iter()
        .flat_map(|path| {
            let is_headless = is_headless_path(path, &headless_roots);
            sessions::codex::parse_codex_file(path)
                .into_iter()
                .map(|mut msg| {
                    apply_headless_agent(&mut msg, is_headless);
                    msg
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut codex_seen: HashSet<String> = HashSet::new();
    let codex_msgs: Vec<ParsedMessage> = codex_msgs_raw
        .into_iter()
        .filter(|message| should_keep_deduped_message(&mut codex_seen, message))
        .map(|message| unified_to_parsed(&message))
        .collect();
    let codex_count = codex_msgs.len() as i32;
    counts.set(ClientId::Codex, codex_count);
    messages.extend(codex_msgs);

    let copilot_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Copilot)
        .par_iter()
        .flat_map(|path| {
            sessions::copilot::parse_copilot_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let copilot_count = copilot_msgs.len() as i32;
    counts.set(ClientId::Copilot, copilot_count);
    messages.extend(copilot_msgs);

    let gemini_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Gemini)
        .par_iter()
        .flat_map(|path| {
            sessions::gemini::parse_gemini_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let gemini_count = gemini_msgs.len() as i32;
    counts.set(ClientId::Gemini, gemini_count);
    messages.extend(gemini_msgs);

    let amp_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Amp)
        .par_iter()
        .flat_map(|path| {
            sessions::amp::parse_amp_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let amp_count = amp_msgs.len() as i32;
    counts.set(ClientId::Amp, amp_count);
    messages.extend(amp_msgs);

    let codebuff_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Codebuff)
        .par_iter()
        .flat_map(|path| {
            sessions::codebuff::parse_codebuff_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let codebuff_count = codebuff_msgs.len() as i32;
    counts.set(ClientId::Codebuff, codebuff_count);
    messages.extend(codebuff_msgs);

    let droid_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Droid)
        .par_iter()
        .flat_map(|path| {
            sessions::droid::parse_droid_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let droid_count = droid_msgs.len() as i32;
    counts.set(ClientId::Droid, droid_count);
    messages.extend(droid_msgs);

    let openclaw_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::OpenClaw)
        .par_iter()
        .flat_map(|path| {
            sessions::openclaw::parse_openclaw_transcript(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let openclaw_count = openclaw_msgs.len() as i32;
    counts.set(ClientId::OpenClaw, openclaw_count);
    messages.extend(openclaw_msgs);

    let pi_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Pi)
        .par_iter()
        .flat_map(|path| {
            sessions::pi::parse_pi_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let pi_count = pi_msgs.len() as i32;
    counts.set(ClientId::Pi, pi_count);
    messages.extend(pi_msgs);

    // gjc (gajae-code) JSONL sessions. This non-cached path produces
    // ParsedMessage (no cost field) and has no pricing service in scope, so
    // the A1 cost guard is a no-op here — cost correctness is enforced on the
    // cached pricing path (see the gjc_outcomes block). What matters here is
    // message-level dedup (codebuff-style key via should_keep_deduped_message)
    // to collapse depth-1/depth-2 replays, mirroring the codex cluster.
    let gjc_msgs_raw: Vec<UnifiedMessage> = scan_result
        .get(ClientId::Gjc)
        .par_iter()
        .flat_map(|path| sessions::gjc::parse_gjc_file(path))
        .collect();
    let mut gjc_seen: HashSet<String> = HashSet::new();
    let gjc_msgs: Vec<ParsedMessage> = gjc_msgs_raw
        .into_iter()
        .filter(|message| should_keep_deduped_message(&mut gjc_seen, message))
        .map(|message| unified_to_parsed(&message))
        .collect();
    let gjc_count = gjc_msgs.len() as i32;
    counts.set(ClientId::Gjc, gjc_count);
    messages.extend(gjc_msgs);

    // Parse Kimi wire.jsonl files in parallel
    let kimi_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Kimi)
        .par_iter()
        .flat_map(|path| {
            sessions::kimi::parse_kimi_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let kimi_count = kimi_msgs.len() as i32;
    counts.set(ClientId::Kimi, kimi_count);
    messages.extend(kimi_msgs);

    // Parse Qwen JSONL files in parallel
    let qwen_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Qwen)
        .par_iter()
        .flat_map(|path| {
            sessions::qwen::parse_qwen_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let qwen_count = qwen_msgs.len() as i32;
    counts.set(ClientId::Qwen, qwen_count);
    messages.extend(qwen_msgs);

    let roocode_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::RooCode)
        .par_iter()
        .flat_map(|path| {
            sessions::roocode::parse_roocode_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let roocode_count = roocode_msgs.len() as i32;
    counts.set(ClientId::RooCode, roocode_count);
    messages.extend(roocode_msgs);

    let kilocode_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::KiloCode)
        .par_iter()
        .flat_map(|path| {
            sessions::kilocode::parse_kilocode_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let kilocode_count = summed_parsed_message_count(&kilocode_msgs);
    counts.set(ClientId::KiloCode, kilocode_count);
    messages.extend(kilocode_msgs);

    let cline_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Cline)
        .par_iter()
        .flat_map(|path| {
            sessions::cline::parse_cline_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let cline_count = summed_parsed_message_count(&cline_msgs);
    counts.set(ClientId::Cline, cline_count);
    messages.extend(cline_msgs);

    let mux_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Mux)
        .par_iter()
        .flat_map(|path| {
            sessions::mux::parse_mux_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let mux_count = summed_parsed_message_count(&mux_msgs);
    counts.set(ClientId::Mux, mux_count);
    messages.extend(mux_msgs);

    // Kilo CLI: SQLite database
    let _kilo_count: i32 = if let Some(db_path) = &scan_result.kilo_db {
        let kilo_msgs: Vec<ParsedMessage> = sessions::kilo::parse_kilo_sqlite(db_path)
            .into_iter()
            .map(|msg| unified_to_parsed(&msg))
            .collect();
        let count = summed_parsed_message_count(&kilo_msgs);
        counts.set(ClientId::Kilo, count);
        messages.extend(kilo_msgs);
        count
    } else {
        0
    };

    let hermes_db_paths = scan_result.hermes_db_paths();
    if !hermes_db_paths.is_empty() {
        let mut hermes_seen: HashSet<String> = HashSet::new();
        let hermes_msgs: Vec<ParsedMessage> = hermes_db_paths
            .iter()
            .flat_map(|db_path| sessions::hermes::parse_hermes_sqlite(db_path))
            .filter(|msg| should_keep_deduped_message(&mut hermes_seen, msg))
            .map(|msg| unified_to_parsed(&msg))
            .collect();
        let count = summed_parsed_message_count(&hermes_msgs);
        counts.set(ClientId::Hermes, count);
        messages.extend(hermes_msgs);
    }

    if let Some(db_path) = &scan_result.goose_db {
        let goose_msgs: Vec<ParsedMessage> = sessions::goose::parse_goose_sqlite(db_path)
            .into_iter()
            .map(|msg| unified_to_parsed(&msg))
            .collect();
        let count = summed_parsed_message_count(&goose_msgs);
        counts.set(ClientId::Goose, count);
        messages.extend(goose_msgs);
    }

    let zed_db_paths = scan_result.zed_db_paths();
    if !zed_db_paths.is_empty() {
        let zed_msgs: Vec<ParsedMessage> = zed_db_paths
            .iter()
            .flat_map(|db_path| sessions::zed::parse_zed_sqlite(db_path))
            .map(|msg| unified_to_parsed(&msg))
            .collect();
        let count = summed_parsed_message_count(&zed_msgs);
        counts.set(ClientId::Zed, count);
        messages.extend(zed_msgs);
    }

    let kiro_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Kiro)
        .par_iter()
        .flat_map(|path| {
            sessions::kiro::parse_kiro_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let kiro_count = summed_parsed_message_count(&kiro_msgs);
    counts.set(ClientId::Kiro, kiro_count);
    messages.extend(kiro_msgs);

    if let Some(db_path) = &scan_result.kiro_db {
        let kiro_db_msgs: Vec<ParsedMessage> = sessions::kiro::parse_kiro_sqlite(db_path)
            .into_iter()
            .map(|msg| unified_to_parsed(&msg))
            .collect();
        let kiro_db_count = summed_parsed_message_count(&kiro_db_msgs);
        counts.add(ClientId::Kiro, kiro_db_count);
        messages.extend(kiro_db_msgs);
    }

    let crush_msgs: Vec<ParsedMessage> = scan_result
        .crush_dbs
        .par_iter()
        .flat_map(|source| {
            sessions::crush::parse_crush_sqlite(&source.db_path)
                .into_iter()
                .map(|mut msg| {
                    msg.set_workspace(source.workspace_key.clone(), source.workspace_label.clone());
                    unified_to_parsed(&msg)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let crush_count = summed_parsed_message_count(&crush_msgs);
    counts.set(ClientId::Crush, crush_count);
    messages.extend(crush_msgs);

    let antigravity_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Antigravity)
        .par_iter()
        .flat_map(|path| {
            sessions::antigravity::parse_antigravity_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let antigravity_count = antigravity_msgs.len() as i32;
    counts.set(ClientId::Antigravity, antigravity_count);
    messages.extend(antigravity_msgs);

    let trae_msgs: Vec<ParsedMessage> = {
        let unique_trae_messages = dedupe_latest_trae_messages(
            scan_result
                .get(ClientId::Trae)
                .par_iter()
                .flat_map(|path| sessions::trae::parse_trae_file("trae", path))
                .collect(),
        );
        unique_trae_messages
            .into_iter()
            .map(|msg| unified_to_parsed(&msg))
            .collect()
    };
    let trae_count = trae_msgs.len() as i32;
    counts.set(ClientId::Trae, trae_count);
    messages.extend(trae_msgs);

    let warp_msgs: Vec<ParsedMessage> = scan_result
        .get(ClientId::Warp)
        .par_iter()
        .flat_map(|path| {
            sessions::warp::parse_warp_file(path)
                .into_iter()
                .map(|msg| unified_to_parsed(&msg))
                .collect::<Vec<_>>()
        })
        .collect();
    let warp_count = summed_parsed_message_count(&warp_msgs);
    counts.set(ClientId::Warp, warp_count);
    messages.extend(warp_msgs);

    if include_synthetic {
        if let Some(db_path) = &scan_result.synthetic_db {
            let synthetic_msgs: Vec<ParsedMessage> =
                sessions::synthetic::parse_octofriend_sqlite(db_path)
                    .into_iter()
                    .map(|msg| unified_to_parsed(&msg))
                    .collect();
            messages.extend(synthetic_msgs);
        }
    }

    // Filter BEFORE normalization (see parse_all_messages_with_pricing).
    if !include_all {
        let requested: HashSet<&str> = clients.iter().map(String::as_str).collect();
        messages.retain(|msg| {
            retain_for_requested_clients(&msg.client, &msg.model_id, &msg.provider_id, &requested)
        });
    }

    if include_synthetic {
        for msg in &mut messages {
            sessions::synthetic::normalize_synthetic_gateway_fields(
                &mut msg.model_id,
                &mut msg.provider_id,
            );
        }
    }

    let filtered = filter_parsed_messages(messages, &options);

    Ok(ParsedMessages {
        messages: filtered,
        counts,
        processing_time_ms: start.elapsed().as_millis() as u32,
    })
}

#[doc(hidden)]
pub async fn parse_local_unified_messages_with_pricing(
    options: LocalParseOptions,
    pricing: Option<&pricing::PricingService>,
) -> Result<Vec<UnifiedMessage>, String> {
    let (home_dir, clients) = resolve_local_parse_request(&options)?;
    parse_local_unified_messages_resolved(options, &home_dir, &clients, pricing, None)
}

pub async fn parse_local_unified_messages(
    options: LocalParseOptions,
) -> Result<Vec<UnifiedMessage>, String> {
    let (home_dir, clients) = resolve_local_parse_request(&options)?;
    let pricing = load_pricing_for_local_parse().await;
    parse_local_unified_messages_resolved(options, &home_dir, &clients, pricing.as_deref(), None)
}

#[doc(hidden)]
pub async fn parse_local_unified_messages_with_telemetry(
    options: LocalParseOptions,
    telemetry_store_path: Option<PathBuf>,
) -> Result<Vec<UnifiedMessage>, String> {
    let (home_dir, clients) = resolve_local_parse_request(&options)?;
    let pricing = load_pricing_for_local_parse().await;
    parse_local_unified_messages_resolved(
        options,
        &home_dir,
        &clients,
        pricing.as_deref(),
        telemetry_store_path.as_deref(),
    )
}

pub(crate) fn unified_to_parsed(msg: &UnifiedMessage) -> ParsedMessage {
    ParsedMessage {
        client: msg.client.clone(),
        model_id: msg.model_id.clone(),
        provider_id: msg.provider_id.clone(),
        session_id: msg.session_id.clone(),
        workspace_key: msg.workspace_key.clone(),
        workspace_label: msg.workspace_label.clone(),
        timestamp: msg.timestamp,
        date: msg.date.clone(),
        input: msg.tokens.input,
        output: msg.tokens.output,
        cache_read: msg.tokens.cache_read,
        cache_write: msg.tokens.cache_write,
        reasoning: msg.tokens.reasoning,
        duration_ms: msg.duration_ms,
        message_count: msg.message_count,
        agent: msg.agent.clone(),
    }
}

fn should_keep_deduped_message(seen_keys: &mut HashSet<String>, message: &UnifiedMessage) -> bool {
    message
        .dedup_key
        .as_ref()
        .is_none_or(|key| seen_keys.insert(key.clone()))
}

fn summed_parsed_message_count(messages: &[ParsedMessage]) -> i32 {
    messages
        .iter()
        .map(|msg| msg.message_count.max(0))
        .sum::<i32>()
}

fn filter_parsed_messages(
    messages: Vec<ParsedMessage>,
    options: &LocalParseOptions,
) -> Vec<ParsedMessage> {
    let mut filtered = messages;

    if let Some(year) = &options.year {
        let year_prefix = format!("{}-", year);
        filtered.retain(|m| m.date.starts_with(&year_prefix));
    }

    if let Some(since) = &options.since {
        filtered.retain(|m| m.date.as_str() >= since.as_str());
    }

    if let Some(until) = &options.until {
        filtered.retain(|m| m.date.as_str() <= until.as_str());
    }

    filtered
}

pub fn parsed_to_unified(msg: &ParsedMessage, cost: f64) -> UnifiedMessage {
    UnifiedMessage {
        client: msg.client.clone(),
        model_id: msg.model_id.clone(),
        provider_id: msg.provider_id.clone(),
        session_id: msg.session_id.clone(),
        workspace_key: msg.workspace_key.clone(),
        workspace_label: msg.workspace_label.clone(),
        timestamp: msg.timestamp,
        date: msg.date.clone(),
        tokens: TokenBreakdown {
            input: msg.input,
            output: msg.output,
            cache_read: msg.cache_read,
            cache_write: msg.cache_write,
            reasoning: msg.reasoning,
        },
        cost,
        duration_ms: msg.duration_ms,
        message_count: msg.message_count,
        agent: msg.agent.clone(),
        dedup_key: None,
        is_turn_start: false,
    }
}
