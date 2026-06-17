#![deny(clippy::all)]

mod aggregator;
mod cc_mirror;
pub mod clients;
pub mod fs_atomic;
mod local_parse;
pub mod mcp;
mod message_cache;
mod parser;
pub mod paths;
pub mod pricing;
mod provider_identity;
pub mod pulse;
mod reports;
pub mod scanner;
pub mod sessionize;
pub mod sessions;

pub use aggregator::*;
pub use clients::{ClientCounts, ClientDef, ClientId, PathRoot};
#[cfg(test)]
pub(crate) use local_parse::{
    apply_pricing_if_available, dedupe_latest_trae_messages, parse_all_messages_with_pricing,
    retain_for_requested_clients, select_local_parse_pricing, unified_to_parsed,
};
pub use local_parse::{
    get_home_dir_string, parse_local_clients, parse_local_unified_messages,
    parse_local_unified_messages_with_pricing, parsed_to_unified,
};
pub(crate) use local_parse::{
    load_pricing_for_local_parse, parse_all_messages_with_pricing_with_env_strategy,
    UNKNOWN_WORKSPACE_GROUP_KEY, UNKNOWN_WORKSPACE_LABEL,
};
pub use parser::*;
#[cfg(test)]
pub(crate) use reports::aggregate_model_usage_entries;
pub use reports::*;
pub use scanner::*;
pub use sessionize::{
    compute_daily_active_time, compute_time_metrics, sessionize, SessionInterval, TimeMetrics,
    DEFAULT_IDLE_GAP_MS,
};
pub use sessions::UnifiedMessage;

/// Strip a CLIProxyAPI-style `(level)` reasoning-effort suffix from a model id.
///
/// Mirrors <https://help.router-for.me/configuration/thinking>: the proxy
/// strips the parentheses before routing, so for pricing lookups we treat the
/// suffix as cosmetic and resolve to the base model. Accepts the level set the
/// proxy documents (case-insensitive — callers pass the lowercased id):
/// `minimal`, `low`, `medium`, `high`, `xhigh`, `auto`, `none`. Numeric
/// thinking budgets are intentionally not handled here.
pub(crate) fn strip_parenthesized_reasoning_tier(model_id: &str) -> Option<&str> {
    let without_closing_paren = model_id.strip_suffix(')')?;
    let (base_model, tier) = without_closing_paren.rsplit_once('(')?;

    if base_model.is_empty() || base_model.trim() != base_model {
        return None;
    }

    if !matches!(
        tier,
        "minimal" | "low" | "medium" | "high" | "xhigh" | "auto" | "none"
    ) {
        return None;
    }

    Some(base_model)
}

pub fn normalize_model_for_grouping(model_id: &str) -> String {
    let mut name = model_id.to_lowercase();

    if let Some(base_model) = strip_parenthesized_reasoning_tier(&name) {
        name = base_model.to_string();
    }
    if name.len() > 9 {
        let potential_date = &name[name.len() - 8..];
        if potential_date.chars().all(|c| c.is_ascii_digit())
            && name.as_bytes()[name.len() - 9] == b'-'
        {
            name = name[..name.len() - 9].to_string();
        }
    }

    if name.contains("claude") {
        let chars: Vec<char> = name.chars().collect();
        let mut result = String::with_capacity(name.len());
        for i in 0..chars.len() {
            if chars[i] == '.'
                && i > 0
                && i < chars.len() - 1
                && chars[i - 1].is_ascii_digit()
                && chars[i + 1].is_ascii_digit()
            {
                result.push('-');
            } else {
                result.push(chars[i]);
            }
        }
        name = result;
    }

    if let Some(canonical) = normalize_anthropic_prefixed_claude_model(&name) {
        name = canonical;
    }

    name
}

fn normalize_anthropic_prefixed_claude_model(model_id: &str) -> Option<String> {
    let rest = model_id.strip_prefix("anthropic/claude-")?;
    let mut parts = rest.split('-');
    let major = parts.next()?;
    let minor = parts.next()?;
    let family = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    if !matches!(family, "opus" | "sonnet" | "haiku") {
        return None;
    }

    Some(format!("claude-{family}-{major}-{minor}"))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
