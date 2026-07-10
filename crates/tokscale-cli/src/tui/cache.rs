//! TUI data caching for instant startup.
//!
//! This module provides disk-based caching for TUI data to enable instant UI display
//! while fresh data loads in the background.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokscale_core::{sessions, GroupBy, ModelPerformance};

use crate::ClientFilter;

use super::data::{
    AgentUsage, DailyModelInfo, DailySourceInfo, DailyUsage, HourlyModelInfo, HourlyUsage,
    ModelUsage, TokenBreakdown, UsageData,
};

/// Cache staleness threshold: 5 minutes.
const CACHE_STALE_THRESHOLD_MS: u64 = 5 * 60 * 1000;
const CACHE_SCHEMA_VERSION: u32 = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReportScope {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
}

impl CacheReportScope {
    pub fn new(since: Option<String>, until: Option<String>, year: Option<String>) -> Self {
        Self { since, until, year }
    }
}

/// Single source of truth for the `group_by` value used to key the TUI
/// cache. The cache file's `groupBy` field is compared verbatim against
/// this on load (`cache.rs::load_cache`), so any code path that writes
/// the cache must use this exact value, NOT `GroupBy::default()`.
///
/// Historical bug: an older cache writer keyed on `GroupBy::default()`
/// (= `ClientModel`) while the TUI loaded with the hard-coded
/// `GroupBy::Model`, so the next TUI launch's cache could be
/// invalidated and the "show cached data while refreshing" contract
/// never triggered. Anchoring both ends on this constant prevents the
/// two from drifting again — change here ⇒ change everywhere.
///
/// The value matches the TUI's runtime default (`App.group_by` in
/// `app.rs`) so swapping `GroupBy::Model` → `TUI_DEFAULT_GROUP_BY` is
/// purely a refactor with no user-visible presentation change.
pub const TUI_DEFAULT_GROUP_BY: GroupBy = GroupBy::Model;

/// Get the cache directory path.
fn cache_dir() -> Option<PathBuf> {
    Some(crate::paths::get_cache_dir())
}

/// Get the cache file path
fn cache_file() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("tui-data-cache.json"))
}

/// Cached TUI data structure (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedTUIData {
    #[serde(default)]
    schema_version: u32,
    timestamp: u64,
    enabled_clients: Vec<String>,
    #[serde(default)]
    group_by: Option<String>,
    #[serde(default)]
    report_scope: CacheReportScope,
    data: CachedUsageData,
}

/// Serializable version of UsageData
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedUsageData {
    models: Vec<CachedModelUsage>,
    #[serde(default)]
    agents: Vec<CachedAgentUsage>,
    daily: Vec<CachedDailyUsage>,
    #[serde(default)]
    hourly: Vec<CachedHourlyUsage>,
    total_tokens: u64,
    total_cost: f64,
    current_streak: u32,
    longest_streak: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedTokenBreakdown {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedModelUsage {
    model: String,
    provider: String,
    client: String,
    #[serde(default)]
    workspace_key: Option<String>,
    #[serde(default)]
    workspace_label: Option<String>,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    performance: ModelPerformance,
    session_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedAgentUsage {
    agent: String,
    clients: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailyModelInfo {
    provider: String,
    display_name: String,
    color_key: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    messages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailySourceInfo {
    tokens: CachedTokenBreakdown,
    cost: f64,
    models: Vec<(String, CachedDailyModelInfo)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailyUsage {
    date: String, // NaiveDate serialized as string
    tokens: CachedTokenBreakdown,
    cost: f64,
    source_breakdown: Vec<(String, CachedDailySourceInfo)>,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    turn_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedHourlyModelInfo {
    provider: String,
    display_name: String,
    color_key: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedHourlyUsage {
    datetime: String, // NaiveDateTime as "YYYY-MM-DD HH:MM:SS"
    tokens: CachedTokenBreakdown,
    cost: f64,
    clients: Vec<String>,
    models: Vec<(String, CachedHourlyModelInfo)>,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    turn_count: u32,
}

impl From<&TokenBreakdown> for CachedTokenBreakdown {
    fn from(t: &TokenBreakdown) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
            reasoning: t.reasoning,
        }
    }
}

impl From<CachedTokenBreakdown> for TokenBreakdown {
    fn from(t: CachedTokenBreakdown) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
            reasoning: t.reasoning,
        }
    }
}

impl From<&ModelUsage> for CachedModelUsage {
    fn from(m: &ModelUsage) -> Self {
        Self {
            model: m.model.clone(),
            provider: m.provider.clone(),
            client: m.client.clone(),
            workspace_key: m.workspace_key.clone(),
            workspace_label: m.workspace_label.clone(),
            tokens: (&m.tokens).into(),
            cost: m.cost,
            performance: m.performance.clone(),
            session_count: m.session_count,
        }
    }
}

impl From<CachedModelUsage> for ModelUsage {
    fn from(m: CachedModelUsage) -> Self {
        Self {
            model: m.model,
            provider: m.provider,
            client: m.client,
            workspace_key: m.workspace_key,
            workspace_label: m.workspace_label,
            tokens: m.tokens.into(),
            cost: m.cost,
            performance: m.performance,
            session_count: m.session_count,
        }
    }
}

impl From<&AgentUsage> for CachedAgentUsage {
    fn from(a: &AgentUsage) -> Self {
        Self {
            agent: a.agent.clone(),
            clients: a.clients.clone(),
            tokens: (&a.tokens).into(),
            cost: a.cost,
            message_count: a.message_count,
        }
    }
}

impl From<CachedAgentUsage> for AgentUsage {
    fn from(a: CachedAgentUsage) -> Self {
        Self {
            agent: a.agent,
            clients: a.clients,
            tokens: a.tokens.into(),
            cost: a.cost,
            message_count: a.message_count,
        }
    }
}

impl From<&DailyModelInfo> for CachedDailyModelInfo {
    fn from(d: &DailyModelInfo) -> Self {
        Self {
            provider: d.provider.clone(),
            display_name: d.display_name.clone(),
            color_key: d.color_key.clone(),
            tokens: (&d.tokens).into(),
            cost: d.cost,
            messages: d.messages,
        }
    }
}

fn daily_model_info_from_cached(key: &str, value: CachedDailyModelInfo) -> DailyModelInfo {
    DailyModelInfo {
        provider: value.provider,
        display_name: if value.display_name.is_empty() {
            key.to_string()
        } else {
            value.display_name
        },
        color_key: if value.color_key.is_empty() {
            key.to_string()
        } else {
            value.color_key
        },
        tokens: value.tokens.into(),
        cost: value.cost,
        messages: value.messages,
    }
}

impl From<&DailySourceInfo> for CachedDailySourceInfo {
    fn from(source: &DailySourceInfo) -> Self {
        Self {
            tokens: (&source.tokens).into(),
            cost: source.cost,
            models: source
                .models
                .iter()
                .map(|(key, value)| (key.clone(), value.into()))
                .collect(),
        }
    }
}

impl From<CachedDailySourceInfo> for DailySourceInfo {
    fn from(source: CachedDailySourceInfo) -> Self {
        Self {
            tokens: source.tokens.into(),
            cost: source.cost,
            models: source
                .models
                .into_iter()
                .map(|(key, value)| {
                    let model_info = daily_model_info_from_cached(&key, value);
                    (key, model_info)
                })
                .collect(),
        }
    }
}

impl From<&HourlyModelInfo> for CachedHourlyModelInfo {
    fn from(h: &HourlyModelInfo) -> Self {
        Self {
            provider: h.provider.clone(),
            display_name: h.display_name.clone(),
            color_key: h.color_key.clone(),
            tokens: (&h.tokens).into(),
            cost: h.cost,
        }
    }
}

fn hourly_model_info_from_cached(key: &str, value: CachedHourlyModelInfo) -> HourlyModelInfo {
    HourlyModelInfo {
        provider: value.provider,
        display_name: if value.display_name.is_empty() {
            key.to_string()
        } else {
            value.display_name
        },
        color_key: if value.color_key.is_empty() {
            key.to_string()
        } else {
            value.color_key
        },
        tokens: value.tokens.into(),
        cost: value.cost,
    }
}

impl From<&HourlyUsage> for CachedHourlyUsage {
    fn from(h: &HourlyUsage) -> Self {
        Self {
            datetime: h.datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
            tokens: (&h.tokens).into(),
            cost: h.cost,
            clients: h.clients.iter().cloned().collect(),
            models: h
                .models
                .iter()
                .map(|(k, v)| (k.clone(), v.into()))
                .collect(),
            message_count: h.message_count,
            turn_count: h.turn_count,
        }
    }
}

impl TryFrom<CachedHourlyUsage> for HourlyUsage {
    type Error = chrono::ParseError;

    fn try_from(h: CachedHourlyUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDateTime;
        Ok(Self {
            datetime: NaiveDateTime::parse_from_str(&h.datetime, "%Y-%m-%d %H:%M:%S")?,
            tokens: h.tokens.into(),
            cost: h.cost,
            clients: h.clients.into_iter().collect(),
            models: h
                .models
                .into_iter()
                .map(|(key, value)| {
                    let model_info = hourly_model_info_from_cached(&key, value);
                    (key, model_info)
                })
                .collect(),
            message_count: h.message_count,
            turn_count: h.turn_count,
        })
    }
}

impl From<&DailyUsage> for CachedDailyUsage {
    fn from(d: &DailyUsage) -> Self {
        Self {
            date: d.date.to_string(),
            tokens: (&d.tokens).into(),
            cost: d.cost,
            source_breakdown: d
                .source_breakdown
                .iter()
                .map(|(key, value)| (key.clone(), value.into()))
                .collect(),
            message_count: d.message_count,
            turn_count: d.turn_count,
        }
    }
}

impl TryFrom<CachedDailyUsage> for DailyUsage {
    type Error = chrono::ParseError;

    fn try_from(d: CachedDailyUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        Ok(Self {
            date: NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")?,
            tokens: d.tokens.into(),
            cost: d.cost,
            source_breakdown: d
                .source_breakdown
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
            message_count: d.message_count,
            turn_count: d.turn_count,
        })
    }
}

impl From<&UsageData> for CachedUsageData {
    fn from(u: &UsageData) -> Self {
        Self {
            models: u.models.iter().map(|m| m.into()).collect(),
            agents: u.agents.iter().map(|a| a.into()).collect(),
            daily: u.daily.iter().map(|d| d.into()).collect(),
            hourly: u.hourly.iter().map(|h| h.into()).collect(),
            total_tokens: u.total_tokens,
            total_cost: u.total_cost,
            current_streak: u.current_streak,
            longest_streak: u.longest_streak,
        }
    }
}

impl TryFrom<CachedUsageData> for UsageData {
    type Error = chrono::ParseError;

    fn try_from(u: CachedUsageData) -> Result<Self, Self::Error> {
        let daily: Result<Vec<DailyUsage>, _> = u.daily.into_iter().map(|d| d.try_into()).collect();
        let hourly: Result<Vec<HourlyUsage>, _> =
            u.hourly.into_iter().map(|h| h.try_into()).collect();
        Ok(Self {
            models: u.models.into_iter().map(|m| m.into()).collect(),
            agents: normalize_cached_agents(u.agents),
            daily: daily?,
            hourly: hourly?,
            total_tokens: u.total_tokens,
            total_cost: u.total_cost,
            loading: false,
            error: None,
            current_streak: u.current_streak,
            longest_streak: u.longest_streak,
        })
    }
}

fn normalize_cached_agents(agents: Vec<CachedAgentUsage>) -> Vec<AgentUsage> {
    let mut merged: BTreeMap<String, AgentUsage> = BTreeMap::new();
    let mut clients_by_agent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for cached in agents {
        let normalized_agent = normalize_cached_agent_name(&cached.agent, &cached.clients);
        let entry = merged
            .entry(normalized_agent.clone())
            .or_insert_with(|| AgentUsage {
                agent: normalized_agent.clone(),
                clients: String::new(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                message_count: 0,
            });

        let tokens: TokenBreakdown = cached.tokens.into();
        entry.tokens.input = entry.tokens.input.saturating_add(tokens.input);
        entry.tokens.output = entry.tokens.output.saturating_add(tokens.output);
        entry.tokens.cache_read = entry.tokens.cache_read.saturating_add(tokens.cache_read);
        entry.tokens.cache_write = entry.tokens.cache_write.saturating_add(tokens.cache_write);
        entry.tokens.reasoning = entry.tokens.reasoning.saturating_add(tokens.reasoning);
        entry.cost += cached.cost;
        entry.message_count = entry.message_count.saturating_add(cached.message_count);

        let client_set = clients_by_agent.entry(normalized_agent).or_default();
        for client in cached
            .clients
            .split(", ")
            .filter(|client| !client.is_empty())
        {
            client_set.insert(client.to_string());
        }
    }

    let mut agents = merged.into_values().collect::<Vec<_>>();
    for agent in &mut agents {
        if let Some(clients) = clients_by_agent.get(&agent.agent) {
            agent.clients = clients.iter().cloned().collect::<Vec<_>>().join(", ");
        }
    }
    agents
}

fn normalize_cached_agent_name(agent: &str, clients: &str) -> String {
    if clients.split(", ").any(|client| client == "opencode") {
        sessions::normalize_opencode_agent_name(agent)
    } else {
        sessions::normalize_agent_name(agent)
    }
}

/// Result of loading the TUI cache — combines staleness check with data loading
/// to avoid double file I/O (previously is_cache_stale + load_cached_data both parsed the file).
pub enum CacheResult {
    /// Cache exists, is fresh (within TTL), and clients match exactly
    Fresh(UsageData),
    /// Cache exists, clients match exactly, but needs background refresh
    Stale(UsageData),
    /// Cache exists for a strict subset of the requested clients.
    ///
    /// This remains useful for immediate TUI rendering, but callers must not
    /// treat it as a complete data source for a global Pulse snapshot.
    StaleSubset(UsageData),
    /// Cache missing, unreadable, unparseable, or clients don't match
    Miss,
}

/// How the cached client set relates to the currently enabled client set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientMatch {
    /// Cached clients are exactly the currently enabled clients
    Exact,
    /// Cached clients are a strict subset of the currently enabled clients.
    /// The cached data is still valid — it just doesn't cover the new clients yet.
    Subset,
    /// No usable overlap (superset, disjoint, or synthetic flag mismatch)
    Mismatch,
}
/// Load cached TUI data from disk with a single read/parse.
/// Returns Fresh/Stale/Miss so the caller can decide whether to
/// display cached data immediately and/or trigger a background refresh.
///
/// `enabled_clients` is the unified `HashSet<ClientFilter>`; the on-disk
/// `enabledClients` list uses the same canonical ids, including `synthetic`.
pub fn load_cache(
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
    report_scope: &CacheReportScope,
) -> CacheResult {
    load_cache_with_observed_at(enabled_clients, group_by, report_scope).0
}

/// Load cached TUI data together with the time the cache was written.
///
/// Pulse uses this timestamp as the AI data generation. Using the cache read
/// time would let an older cache appear newer each time the process starts.
pub fn load_cache_with_observed_at(
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
    report_scope: &CacheReportScope,
) -> (CacheResult, Option<DateTime<Utc>>) {
    let Some(cache_path) = cache_file() else {
        return (CacheResult::Miss, None);
    };
    let cached: Option<CachedTUIData> = match File::open(&cache_path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).ok()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => None,
    };
    let Some(cached) = cached else {
        return (CacheResult::Miss, None);
    };
    if cached.schema_version != CACHE_SCHEMA_VERSION {
        return (CacheResult::Miss, None);
    }
    let cached_group_by = cached
        .group_by
        .as_deref()
        .and_then(|value: &str| value.parse::<GroupBy>().ok());

    if cached_group_by.as_ref() != Some(group_by) {
        return (CacheResult::Miss, None);
    }

    if &cached.report_scope != report_scope {
        return (CacheResult::Miss, None);
    }

    // Check how cached clients relate to enabled clients
    let client_match = check_client_match(enabled_clients, &cached.enabled_clients);

    if client_match == ClientMatch::Mismatch {
        return (CacheResult::Miss, None);
    }
    // Convert cached data to UsageData
    let data = match cached.data.try_into() {
        Ok(d) => d,
        Err(_) => return (CacheResult::Miss, None),
    };

    let observed_at = i64::try_from(cached.timestamp)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis);
    let Some(observed_at) = observed_at else {
        return (CacheResult::Miss, None);
    };

    if client_match == ClientMatch::Subset {
        return (CacheResult::StaleSubset(data), Some(observed_at));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cache_age = now.saturating_sub(cached.timestamp);
    let result = if cache_age > CACHE_STALE_THRESHOLD_MS {
        CacheResult::Stale(data)
    } else {
        CacheResult::Fresh(data)
    };
    (result, Some(observed_at))
}

/// Determine how the cached client set relates to the currently enabled set.
///
/// - `Exact`    — same clients
/// - `Subset`   — cached clients ⊆ enabled clients (e.g. update added a new client),
///   and cached doesn't carry data the user doesn't want
/// - `Mismatch` — anything else (superset, disjoint, unwanted synthetic data)
fn check_client_match(
    enabled_clients: &HashSet<ClientFilter>,
    cached_clients: &[String],
) -> ClientMatch {
    // Every cached client must exist in the enabled set. Compare on the
    // canonical lowercase id so we don't have to round-trip through
    // ClientId for clients that map 1:1.
    for cached_client_str in cached_clients {
        let in_enabled = enabled_clients
            .iter()
            .any(|f| f.as_filter_str() == cached_client_str);
        if !in_enabled {
            return ClientMatch::Mismatch;
        }
    }

    if enabled_clients.len() == cached_clients.len() {
        ClientMatch::Exact
    } else {
        ClientMatch::Subset
    }
}

/// Save TUI data to disk cache.
///
/// The cache stores the same canonical client ids used by `ClientFilter`,
/// including `synthetic`.
pub fn save_cached_data(
    data: &UsageData,
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
    report_scope: &CacheReportScope,
) {
    let Some(cache_path) = cache_file() else {
        return;
    };

    // Ensure cache directory exists
    if let Some(dir) = cache_path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut clients_vec: Vec<String> = enabled_clients
        .iter()
        .map(|f| f.as_filter_str().to_string())
        .collect();
    // Sort so the cache key is deterministic across runs / HashSet
    // iteration order — otherwise unrelated runs would invalidate each
    // other's caches just because the JSON ordering shuffled.
    clients_vec.sort();

    let cached = CachedTUIData {
        schema_version: CACHE_SCHEMA_VERSION,
        timestamp,
        enabled_clients: clients_vec,
        group_by: Some(group_by.to_string()),
        report_scope: report_scope.clone(),
        data: data.into(),
    };

    // INVARIANT: All cache writes use atomic temp-file rename. NEVER delete
    // the canonical cache file before writing — a partial save or process
    // crash between delete and rename would lose the cache. The temp-file
    // pattern makes corruption-on-crash impossible.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let temp_path = cache_path.with_file_name(format!(
        ".{}.{}.{:x}.tmp",
        cache_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("tui-data-cache.json"),
        std::process::id(),
        nanos
    ));
    let file = match File::create(&temp_path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let writer = BufWriter::new(file);

    if serde_json::to_writer(writer, &cached).is_ok() {
        let _ = tokscale_core::fs_atomic::replace_file(&temp_path, &cache_path);
    } else {
        let _ = fs::remove_file(&temp_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};
    use tempfile::TempDir;

    /// Build a unified filter set. Pass `synthetic=true` to include
    /// `ClientFilter::Synthetic` as a set member (the new way to express
    /// "user has synthetic enabled").
    fn make_filters(filters: &[ClientFilter], synthetic: bool) -> HashSet<ClientFilter> {
        let mut set: HashSet<ClientFilter> = filters.iter().copied().collect();
        if synthetic {
            set.insert(ClientFilter::Synthetic);
        }
        set
    }

    fn cached_agent(agent: &str, clients: &str, total_seed: u64) -> CachedAgentUsage {
        CachedAgentUsage {
            agent: agent.to_string(),
            clients: clients.to_string(),
            tokens: CachedTokenBreakdown {
                input: total_seed,
                output: 1,
                cache_read: 2,
                cache_write: 3,
                reasoning: 4,
            },
            cost: total_seed as f64,
            message_count: 1,
        }
    }

    #[test]
    fn test_normalize_cached_agents_merges_opencode_display_variants() {
        let agents = normalize_cached_agents(vec![
            cached_agent("Sisyphus", "opencode", 10),
            cached_agent("\u{200B} Sisyphus   -   Ultraworker", "opencode", 20),
            cached_agent(
                "\u{200B}\u{200B}\u{200B} Prometheus    Plan Builder",
                "opencode",
                30,
            ),
        ]);

        assert_eq!(agents.len(), 2);
        let sisyphus = agents
            .iter()
            .find(|agent| agent.agent == "Sisyphus")
            .unwrap();
        assert_eq!(sisyphus.clients, "opencode");
        assert_eq!(sisyphus.message_count, 2);
        assert_eq!(sisyphus.tokens.input, 30);
        assert!((sisyphus.cost - 30.0).abs() < f64::EPSILON);

        let prometheus = agents
            .iter()
            .find(|agent| agent.agent == "Prometheus")
            .unwrap();
        assert_eq!(prometheus.message_count, 1);
    }

    // ── check_client_match ──────────────────────────────────────────

    #[test]
    fn test_exact_match() {
        let enabled = make_filters(&[ClientFilter::Claude, ClientFilter::Opencode], false);
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Exact,);
    }

    #[test]
    fn test_subset_new_client_added() {
        // Simulates: update added Qwen, cache only has Claude + OpenCode
        let enabled = make_filters(
            &[
                ClientFilter::Claude,
                ClientFilter::Opencode,
                ClientFilter::Qwen,
            ],
            false,
        );
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Subset,);
    }

    #[test]
    fn test_subset_synthetic_added() {
        // Cache was saved without synthetic, now user enables it
        let enabled = make_filters(&[ClientFilter::Claude], true);
        let cached = vec!["claude".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Subset,);
    }

    #[test]
    fn test_mismatch_superset() {
        // Cache has more clients than enabled (user narrowed filter)
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Mismatch,);
    }

    #[test]
    fn test_mismatch_disjoint() {
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["opencode".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Mismatch,);
    }

    #[test]
    fn test_mismatch_unwanted_synthetic() {
        // Cache has synthetic data but user doesn't want it
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["claude".to_string(), "synthetic".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Mismatch,);
    }

    #[test]
    fn test_exact_with_synthetic() {
        let enabled = make_filters(&[ClientFilter::Claude], true);
        let cached = vec!["claude".to_string(), "synthetic".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Exact,);
    }

    #[test]
    fn test_subset_both_new_client_and_synthetic() {
        // Update added new client AND user also enabled synthetic
        let enabled = make_filters(&[ClientFilter::Claude, ClientFilter::Qwen], true);
        let cached = vec!["claude".to_string()];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Subset,);
    }

    #[test]
    fn test_empty_cache_is_subset() {
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached: Vec<String> = vec![];
        assert_eq!(check_client_match(&enabled, &cached), ClientMatch::Subset,);
    }

    #[test]
    #[serial]
    fn load_cache_preserves_strict_subset_provenance() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let cached_clients = make_filters(&[ClientFilter::Claude], false);
        let requested_clients = make_filters(&[ClientFilter::Claude, ClientFilter::Qwen], false);
        save_cached_data(
            &UsageData::default(),
            &cached_clients,
            &GroupBy::Model,
            &CacheReportScope::default(),
        );

        assert!(matches!(
            load_cache(
                &requested_clients,
                &GroupBy::Model,
                &CacheReportScope::default()
            ),
            CacheResult::StaleSubset(_)
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    #[test]
    #[serial]
    fn load_cache_reports_original_write_time() {
        let temp_dir = TempDir::new().unwrap();
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { env::set_var("TOKSCALE_CONFIG_DIR", temp_dir.path()) };

        let clients = make_filters(&[ClientFilter::Claude], false);
        let scope = CacheReportScope::default();
        save_cached_data(&UsageData::default(), &clients, &GroupBy::Model, &scope);

        let path = cache_file().unwrap();
        let mut cached: CachedTUIData =
            serde_json::from_reader(BufReader::new(File::open(&path).unwrap())).unwrap();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 60_000;
        cached.timestamp = timestamp;
        serde_json::to_writer(BufWriter::new(File::create(&path).unwrap()), &cached).unwrap();

        let (result, observed_at) = load_cache_with_observed_at(&clients, &GroupBy::Model, &scope);

        assert!(matches!(result, CacheResult::Fresh(_)));
        assert_eq!(
            observed_at.unwrap().timestamp_millis(),
            i64::try_from(timestamp).unwrap()
        );

        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    #[test]
    #[serial]
    fn load_cache_misses_when_report_scope_differs() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 10,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "groupBy": "model",
  "reportScope": {
    "since": "2026-05-01",
    "until": "2026-05-07",
    "year": null
  },
  "data": {
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        let unfiltered_scope = CacheReportScope::default();
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model, &unfiltered_scope),
            CacheResult::Miss
        ));

        let filtered_scope = CacheReportScope::new(
            Some("2026-05-01".to_string()),
            Some("2026-05-07".to_string()),
            None,
        );
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model, &filtered_scope),
            CacheResult::Fresh(_)
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn save_cached_data_writes_report_scope() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let clients = make_filters(&[ClientFilter::Claude], false);
        let scope = CacheReportScope::new(
            Some("2026-05-01".to_string()),
            Some("2026-05-07".to_string()),
            Some("2026".to_string()),
        );

        save_cached_data(&UsageData::default(), &clients, &GroupBy::Model, &scope);

        let cache_path = cache_file().unwrap();
        let saved: CachedTUIData = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert_eq!(saved.report_scope, scope);
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model, &scope),
            CacheResult::Fresh(_)
        ));
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model, &CacheReportScope::default()),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    #[test]
    #[serial]
    fn outdated_cache_schema_misses() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 8,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model, &CacheReportScope::default()),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_misses_when_group_by_differs() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 10,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "groupBy": "model",
  "data": {
    "models": [],
    "daily": [],
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(
                &clients,
                &GroupBy::WorkspaceModel,
                &CacheReportScope::default()
            ),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_reads_source_breakdown_from_current_schema() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 10,
  "timestamp": 9999999999999,
  "enabledClients": ["claude", "cursor"],
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [{
      "date": "2026-03-18",
      "tokens": {
        "input": 30,
        "output": 15,
        "cacheRead": 0,
        "cacheWrite": 0,
        "reasoning": 0
      },
      "cost": 3.25,
      "sourceBreakdown": [[
        "claude",
        {
          "tokens": {
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 1.25,
          "models": [[
            "claude-sonnet-4-5",
            {
              "provider": "anthropic",
              "displayName": "claude-sonnet-4-5",
              "colorKey": "claude-sonnet-4-5",
              "tokens": {
                "input": 10,
                "output": 5,
                "cacheRead": 0,
                "cacheWrite": 0,
                "reasoning": 0
              },
              "cost": 1.25
            }
          ]]
        }
      ], [
        "cursor",
        {
          "tokens": {
            "input": 20,
            "output": 10,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 2.0,
          "models": [[
            "claude-sonnet-4-5",
            {
              "provider": "anthropic",
              "displayName": "claude-sonnet-4-5",
              "colorKey": "claude-sonnet-4-5",
              "tokens": {
                "input": 20,
                "output": 10,
                "cacheRead": 0,
                "cacheWrite": 0,
                "reasoning": 0
              },
              "cost": 2.0
            }
          ]]
        }
      ]]
    }],
    "totalTokens": 45,
    "totalCost": 3.25,
    "currentStreak": 1,
    "longestStreak": 1
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude, ClientFilter::Cursor], false);
        match load_cache(&clients, &GroupBy::Model, &CacheReportScope::default()) {
            CacheResult::Fresh(data) => {
                assert_eq!(data.daily[0].source_breakdown.len(), 2);
                let cursor = data.daily[0].source_breakdown.get("cursor").unwrap();
                let model = cursor.models.get("claude-sonnet-4-5").unwrap();
                assert_eq!(model.provider, "anthropic");
                assert_eq!(model.tokens.total(), 30);
            }
            other => panic!(
                "expected fresh current-schema cache, got {:?}",
                other_variant_name(&other)
            ),
        }

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn save_cached_data_does_not_delete_destination() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        fs::write(
            &cache_path,
            format!(
                r#"{{
  "schemaVersion": 10,
  "timestamp": {old_timestamp},
  "enabledClients": ["claude"],
  "groupBy": "model",
  "data": {{
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }}
}}"#
            ),
        )
        .unwrap();
        assert!(fs::metadata(&cache_path).is_ok());

        let clients = make_filters(&[ClientFilter::Claude], false);
        save_cached_data(
            &UsageData::default(),
            &clients,
            &GroupBy::Model,
            &CacheReportScope::default(),
        );

        let metadata = fs::metadata(&cache_path).unwrap();
        assert!(metadata.is_file());
        let saved: CachedTUIData = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert!(saved.timestamp >= old_timestamp);

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    fn other_variant_name(result: &CacheResult) -> &'static str {
        match result {
            CacheResult::Fresh(_) => "Fresh",
            CacheResult::Stale(_) => "Stale",
            CacheResult::StaleSubset(_) => "StaleSubset",
            CacheResult::Miss => "Miss",
        }
    }

    /// Regression test for the TUI cache `group_by` mismatch bug.
    ///
    /// Symptom: a normal TUI launch silently dropped the
    /// on-disk cache and showed an empty dashboard until the background
    /// scan finished, even though `~/.config/tokscale/cache/tui-data-cache.json`
    /// existed and was well-formed.
    ///
    /// Root cause: an older cache writer saved the cache with
    /// `GroupBy::default()`
    /// (= `ClientModel`, serialized as `"client,model"`), while the TUI
    /// reader (`tui::run`) loaded with the hard-coded `GroupBy::Model`
    /// (serialized as `"model"`). `cache.rs::load_cache` does a strict
    /// inequality check on the cached vs. requested `group_by`, so the
    /// two never matched and those writers silently invalidated the next
    /// TUI launch's cache.
    ///
    /// Fix: anchor both ends on `TUI_DEFAULT_GROUP_BY`. This test pins
    /// the contract — round-tripping a save→load under the canonical key
    /// must return `Fresh`, never `Miss`.
    #[test]
    #[serial]
    fn cache_round_trip_under_canonical_key_is_fresh() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let enabled = ClientFilter::default_set();
        let scope = CacheReportScope::default();

        // Write with the canonical key.
        save_cached_data(
            &UsageData::default(),
            &enabled,
            &TUI_DEFAULT_GROUP_BY,
            &scope,
        );

        // Read with the canonical key (mirrors what `tui::run` does on
        // launch). The bug would have returned `Miss` here because the
        // historical writer used `GroupBy::default()` (= ClientModel)
        // while the reader used `GroupBy::Model`.
        let result = load_cache(&enabled, &TUI_DEFAULT_GROUP_BY, &scope);
        assert!(
            matches!(result, CacheResult::Fresh(_)),
            "expected Fresh after writing with TUI_DEFAULT_GROUP_BY, got {}",
            other_variant_name(&result)
        );

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    /// Documents the historical bug as a frozen regression: writing with
    /// `GroupBy::default()` (the old writer behavior)
    /// and reading with `TUI_DEFAULT_GROUP_BY` returns `Miss`. If
    /// anyone re-introduces `GroupBy::default()` at any TUI cache write
    /// site, this assertion proves the cache breaks.
    #[test]
    #[serial]
    fn pre_fix_writer_key_misses_under_canonical_reader_key() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let enabled = ClientFilter::default_set();
        let scope = CacheReportScope::default();

        // Pre-fix: writer used `GroupBy::default()`.
        save_cached_data(&UsageData::default(), &enabled, &GroupBy::default(), &scope);

        // Reader uses the canonical key. If `GroupBy::default()` and
        // `TUI_DEFAULT_GROUP_BY` ever coincide (e.g. someone changes
        // `impl Default for GroupBy` to return `Model`), this assertion
        // will start failing, at which point this regression test should
        // be updated accordingly.
        let result = load_cache(&enabled, &TUI_DEFAULT_GROUP_BY, &scope);
        assert!(
            matches!(result, CacheResult::Miss),
            "expected Miss when reader uses TUI_DEFAULT_GROUP_BY and writer used GroupBy::default(), got {}",
            other_variant_name(&result)
        );

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }
}
