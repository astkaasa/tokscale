mod amp;
mod claude;
pub mod codex;
mod copilot;
pub mod helpers;
mod kimi;
mod minimax;
mod warp;
mod zai;

use std::collections::HashSet;

use anyhow::Result;

// ── Shared types ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageMetric {
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub remaining_label: Option<String>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageResetCredits {
    pub available_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<UsageResetCredit>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageResetCredit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageCreditStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_credits: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_limit_reached: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageSpendControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub individual_limit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageOutput {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<UsageAccount>,
    pub plan: Option<String>,
    pub email: Option<String>,
    pub metrics: Vec<UsageMetric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<UsageResetCredits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_status: Option<UsageCreditStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_control: Option<UsageSpendControl>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageAccount {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageFetchDiagnostic {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<UsageAccount>,
    #[serde(default)]
    pub kind: UsageFetchDiagnosticKind,
    #[serde(default)]
    pub severity: UsageFetchDiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFetchDiagnosticKind {
    #[default]
    FetchFailed,
    ImportCurrentLoginFailed,
    ProviderPanicked,
    CachedDataStale,
    CacheWriteFailed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFetchDiagnosticSeverity {
    Info,
    Warning,
    #[default]
    Error,
}

impl UsageFetchDiagnostic {
    pub fn new(
        provider: impl Into<String>,
        account: Option<UsageAccount>,
        message: impl Into<String>,
    ) -> Self {
        Self::with_kind(
            provider,
            account,
            UsageFetchDiagnosticKind::FetchFailed,
            UsageFetchDiagnosticSeverity::Error,
            message,
        )
    }

    pub fn with_kind(
        provider: impl Into<String>,
        account: Option<UsageAccount>,
        kind: UsageFetchDiagnosticKind,
        severity: UsageFetchDiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            account,
            kind,
            severity,
            message: message.into(),
        }
    }

    pub fn display_name(&self) -> String {
        match &self.account {
            Some(account) => format!("{} ({})", self.provider, account.display_name()),
            None => self.provider.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageFetchReport {
    pub outputs: Vec<UsageOutput>,
    pub diagnostics: Vec<UsageFetchDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageFetchIntent {
    CliReadOnly,
    TuiSurface,
}

impl UsageFetchReport {
    fn from_outputs(outputs: Vec<UsageOutput>) -> Self {
        Self {
            outputs,
            diagnostics: Vec::new(),
        }
    }

    fn from_error(
        provider: impl Into<String>,
        account: Option<UsageAccount>,
        error: impl std::fmt::Display,
    ) -> Self {
        Self {
            outputs: Vec::new(),
            diagnostics: vec![UsageFetchDiagnostic::new(
                provider,
                account,
                error.to_string(),
            )],
        }
    }

    fn extend(&mut self, other: UsageFetchReport) {
        self.outputs.extend(other.outputs);
        self.diagnostics.extend(other.diagnostics);
    }
}

impl UsageAccount {
    pub fn label_name(&self) -> Option<&str> {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
    }

    pub fn short_id(&self) -> String {
        let id = self.id.trim();
        if id.is_empty() {
            return "unknown".to_string();
        }

        let char_count = id.chars().count();
        if char_count <= 12 {
            return id.to_string();
        }

        let head: String = id.chars().take(6).collect();
        let tail: String = id
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{head}...{tail}")
    }

    pub fn display_name(&self) -> String {
        self.label_name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Account {}", self.short_id()))
    }
}

impl UsageOutput {
    pub fn account_display_name(&self) -> Option<String> {
        let account = self.account.as_ref()?;

        if let Some(label) = account.label_name() {
            return Some(label.to_string());
        }

        if let Some(email) = self
            .email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(email.to_string());
        }

        Some(account.display_name())
    }

    pub fn display_name(&self) -> String {
        match &self.account {
            Some(_) => format!(
                "{} ({})",
                self.provider,
                self.account_display_name().unwrap_or_default()
            ),
            None => self.provider.clone(),
        }
    }
}

// ── Cache ──

const SUBSCRIPTION_CACHE_FUTURE_TOLERANCE_SECS: u64 = 60;
const SUBSCRIPTION_CACHE_TTL_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageCacheIdentity {
    pub(crate) provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) account_id: Option<String>,
}

impl UsageCacheIdentity {
    pub(crate) fn from_output(output: &UsageOutput) -> Self {
        Self {
            provider: output.provider.clone(),
            account_id: output.account.as_ref().map(|account| account.id.clone()),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscriptionCacheDocument {
    timestamp: u64,
    data: Vec<UsageOutput>,
    #[serde(default)]
    partial: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stale_identities: Vec<UsageCacheIdentity>,
}

#[derive(Debug)]
pub(crate) struct LoadedSubscriptionCache {
    pub(crate) data: Vec<UsageOutput>,
    pub(crate) observed_at: chrono::DateTime<chrono::Utc>,
    pub(crate) is_fresh: bool,
    pub(crate) partial: bool,
    pub(crate) stale_identities: Vec<UsageCacheIdentity>,
}

fn cache_path() -> Option<std::path::PathBuf> {
    let config_dir = crate::paths::get_config_dir();
    if tokscale_core::fs_atomic::ensure_private_dir(&config_dir).is_err() {
        return None;
    }

    let dir = crate::paths::get_cache_dir();
    if tokscale_core::fs_atomic::ensure_private_dir(&dir).is_err() {
        return None;
    }

    let path = dir.join("subscription-usage-cache.json");
    tokscale_core::fs_atomic::repair_private_file(&path);
    Some(path)
}

pub fn save_cache(data: &[UsageOutput]) {
    let _ = save_cache_with_observed_at(data);
}

pub(crate) fn save_cache_with_observed_at(
    data: &[UsageOutput],
) -> Option<chrono::DateTime<chrono::Utc>> {
    save_cache_with_provenance(data, false, &[])
}

pub(crate) fn save_cache_with_provenance(
    data: &[UsageOutput],
    partial: bool,
    stale_identities: &[UsageCacheIdentity],
) -> Option<chrono::DateTime<chrono::Utc>> {
    let path = cache_path()?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut seen_identities = HashSet::new();
    let stale_identities = stale_identities
        .iter()
        .filter(|identity| seen_identities.insert((*identity).clone()))
        .cloned()
        .collect();
    let document = SubscriptionCacheDocument {
        timestamp,
        data: data.to_vec(),
        partial,
        stale_identities,
    };
    let Ok(content) = serde_json::to_vec(&document) else {
        return None;
    };
    tokscale_core::fs_atomic::atomic_write_private(&path, &content).ok()?;
    i64::try_from(timestamp)
        .ok()
        .and_then(|timestamp| chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0))
}

pub fn clear_cache() {
    if let Some(path) = cache_path() {
        let _ = std::fs::remove_file(&path);
    }
}

pub fn load_cache_with_observed_at() -> Option<(Vec<UsageOutput>, chrono::DateTime<chrono::Utc>)> {
    let cache = load_cache_for_tui()?;
    if !cache.is_fresh || cache.partial || !cache.stale_identities.is_empty() {
        return None;
    }
    Some((cache.data, cache.observed_at))
}

pub(crate) fn load_cache_for_tui() -> Option<LoadedSubscriptionCache> {
    let path = cache_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let document: SubscriptionCacheDocument = serde_json::from_str(&content).ok()?;
    let timestamp = document.timestamp;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if timestamp > now.saturating_add(SUBSCRIPTION_CACHE_FUTURE_TOLERANCE_SECS) {
        return None;
    }
    let age = now.saturating_sub(timestamp);
    let observed_at = i64::try_from(timestamp)
        .ok()
        .and_then(|timestamp| chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0))?;
    Some(LoadedSubscriptionCache {
        data: document.data,
        observed_at,
        is_fresh: age <= SUBSCRIPTION_CACHE_TTL_SECS,
        partial: document.partial,
        stale_identities: document.stale_identities,
    })
}

// ── Public API ──

type UsageProvider = (&'static str, fn() -> bool, fn() -> UsageFetchReport);

fn fetch_single_provider(provider: &'static str, result: Result<UsageOutput>) -> UsageFetchReport {
    fetch_provider(provider, result.map(|output| vec![output]))
}

fn fetch_provider(provider: &'static str, result: Result<Vec<UsageOutput>>) -> UsageFetchReport {
    match result {
        Ok(outputs) => UsageFetchReport::from_outputs(outputs),
        Err(error) => UsageFetchReport::from_error(provider, None, error),
    }
}

fn fetch_amp() -> UsageFetchReport {
    fetch_single_provider("Amp", amp::fetch())
}

fn fetch_claude() -> UsageFetchReport {
    fetch_single_provider("Claude", claude::fetch())
}

fn fetch_copilot() -> UsageFetchReport {
    fetch_single_provider("Copilot", copilot::fetch())
}

fn fetch_kimi() -> UsageFetchReport {
    fetch_single_provider("Kimi", kimi::fetch())
}

fn fetch_minimax() -> UsageFetchReport {
    fetch_single_provider("MiniMax", minimax::fetch())
}

fn fetch_warp() -> UsageFetchReport {
    fetch_single_provider("Warp/Oz", warp::fetch())
}

fn fetch_zai() -> UsageFetchReport {
    fetch_single_provider("Z.ai", zai::fetch())
}

pub fn fetch_all_report() -> UsageFetchReport {
    fetch_all_report_with_intent(UsageFetchIntent::CliReadOnly)
}

pub fn fetch_all_report_with_intent(intent: UsageFetchIntent) -> UsageFetchReport {
    let codex_fetch = match intent {
        UsageFetchIntent::CliReadOnly => codex::fetch_all_report,
        UsageFetchIntent::TuiSurface => codex::fetch_all_report_importing_current_auth,
    };
    fetch_all_report_with_codex(codex_fetch)
}

fn fetch_all_report_with_codex(codex_fetch: fn() -> UsageFetchReport) -> UsageFetchReport {
    let usage_settings = crate::tui::settings::load_usage_settings();
    let effective_allowlist = effective_usage_provider_allowlist(
        &usage_settings.enabled_providers,
        &usage_settings.excluded_providers,
    );
    let providers: Vec<UsageProvider> = vec![
        ("Claude", claude::has_credentials, fetch_claude),
        ("Codex", codex::has_credentials, codex_fetch),
        ("Z.ai", zai::has_credentials, fetch_zai),
        ("Amp", amp::has_credentials, fetch_amp),
        ("Copilot", copilot::has_credentials, fetch_copilot),
        ("Kimi", kimi::has_credentials, fetch_kimi),
        ("MiniMax", minimax::has_credentials, fetch_minimax),
        ("Warp/Oz", warp::has_credentials, fetch_warp),
    ];

    let active = credentialed_usage_providers(providers, &effective_allowlist);

    if active.is_empty() {
        return UsageFetchReport::default();
    }

    std::thread::scope(|s| {
        let handles = active
            .into_iter()
            .map(|(provider, _, fetch)| (provider, s.spawn(fetch)))
            .collect::<Vec<_>>();

        let mut report = UsageFetchReport::default();
        for (provider, handle) in handles {
            match handle.join() {
                Ok(provider_report) => report.extend(provider_report),
                Err(_) => report.diagnostics.push(UsageFetchDiagnostic::with_kind(
                    provider,
                    None,
                    UsageFetchDiagnosticKind::ProviderPanicked,
                    UsageFetchDiagnosticSeverity::Error,
                    "usage fetch worker panicked",
                )),
            }
        }
        report
    })
}

fn credentialed_usage_providers(
    providers: Vec<UsageProvider>,
    effective_allowlist: &HashSet<String>,
) -> Vec<UsageProvider> {
    let enabled = providers
        .into_iter()
        .filter(|(provider, _, _)| effective_allowlist.contains(&usage_provider_key(provider)))
        .collect::<Vec<_>>();

    enabled
        .into_iter()
        .filter(|(_, has_credentials, _)| has_credentials())
        .collect()
}

fn effective_usage_provider_allowlist(enabled: &[String], excluded: &[String]) -> HashSet<String> {
    let excluded = excluded
        .iter()
        .map(|provider| usage_provider_key(provider))
        .filter(|provider| !provider.is_empty())
        .collect::<HashSet<_>>();

    enabled
        .iter()
        .map(|provider| usage_provider_key(provider))
        .filter(|provider| !provider.is_empty() && !excluded.contains(provider))
        .collect()
}

fn usage_provider_key(provider: &str) -> String {
    provider
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

// ── Light-mode rendering ──

const BAR_WIDTH: usize = 12;
const METRIC_LABEL_WIDTH: usize = 14;
const METRIC_REMAINING_WIDTH: usize = 11;
const METRIC_BAR_WIDTH: usize = BAR_WIDTH + 2;
const METRIC_RESET_WIDTH: usize = 24;
const CARD_WIDTH: usize =
    1 + METRIC_LABEL_WIDTH + METRIC_REMAINING_WIDTH + METRIC_BAR_WIDTH + METRIC_RESET_WIDTH;

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_len - 1).collect();
    format!("{truncated}…")
}

fn render_light(output: &UsageOutput) {
    println!("╭{}╮", "─".repeat(CARD_WIDTH));
    // Provider header
    println!(
        "│ {:<width$}│",
        output.display_name(),
        width = CARD_WIDTH - 1
    );
    for m in &output.metrics {
        let rem = m
            .remaining_label
            .clone()
            .unwrap_or_else(|| format!("{:.0}% left", m.remaining_percent));
        let rem = truncate(&rem, 11);
        let bar = helpers::render_ascii_bar(m.remaining_percent, BAR_WIDTH);
        let reset = m
            .resets_at
            .as_ref()
            .map(|r| helpers::format_reset_time(r))
            .unwrap_or_default();
        let reset = truncate(&reset, METRIC_RESET_WIDTH);
        let label = truncate(&m.label, METRIC_LABEL_WIDTH);
        println!(
            "│ {:<label_width$}{:<remaining_width$}{:<bar_width$}{:<reset_width$}│",
            label,
            rem,
            bar,
            reset,
            label_width = METRIC_LABEL_WIDTH,
            remaining_width = METRIC_REMAINING_WIDTH,
            bar_width = METRIC_BAR_WIDTH,
            reset_width = METRIC_RESET_WIDTH,
        );
    }
    if let Some(ref email) = output.email {
        let email = truncate(email, CARD_WIDTH - 11);
        println!(
            "│ {:<10}{:<width$}│",
            "Account",
            email,
            width = CARD_WIDTH - 11
        );
    }
    if let Some(ref plan) = output.plan {
        let plan = truncate(plan, CARD_WIDTH - 11);
        println!("│ {:<10}{:<width$}│", "Plan", plan, width = CARD_WIDTH - 11);
    }
    if let Some(ref credits) = output.reset_credits {
        println!(
            "│ {:<10}{:<width$}│",
            "Resets",
            format!("{} available", credits.available_count),
            width = CARD_WIDTH - 11
        );
    }
    println!("╰{}╯", "─".repeat(CARD_WIDTH));
}

pub fn run(json: bool, _light: bool) -> Result<()> {
    let report = fetch_all_report();
    if report.outputs.is_empty() {
        if let Some(diagnostic) = report.diagnostics.first() {
            anyhow::bail!(
                "Usage fetch failed for {}: {}",
                diagnostic.display_name(),
                diagnostic.message
            );
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report.outputs)?);
    } else {
        for o in &report.outputs {
            render_light(o);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials_available() -> bool {
        true
    }

    fn credentials_unavailable() -> bool {
        false
    }

    fn credentials_must_not_be_probed() -> bool {
        panic!("disabled provider credentials were probed")
    }

    fn fetch_must_not_run() -> UsageFetchReport {
        panic!("provider fetch ran during selection test")
    }

    fn cache_output() -> UsageOutput {
        UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: "acct-work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            plan: None,
            email: None,
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        }
    }

    #[test]
    fn usage_fetch_intent_exposes_tui_surface_variant() {
        assert!(matches!(
            UsageFetchIntent::TuiSurface,
            UsageFetchIntent::TuiSurface
        ));
    }

    #[test]
    fn usage_fetch_diagnostic_defaults_to_fetch_error() {
        let diagnostic = UsageFetchDiagnostic::new("Codex", None, "failed");

        assert_eq!(diagnostic.kind, UsageFetchDiagnosticKind::FetchFailed);
        assert_eq!(diagnostic.severity, UsageFetchDiagnosticSeverity::Error);
    }

    #[test]
    fn usage_provider_allowlist_normalizes_names_and_applies_legacy_excludes() {
        let enabled = vec![
            "  cOdEx  ".to_string(),
            "WARP / OZ".to_string(),
            "Copilot".to_string(),
        ];
        let excluded = vec!["  coPILOT ".to_string()];

        let effective = effective_usage_provider_allowlist(&enabled, &excluded);

        assert!(effective.contains(&usage_provider_key("Codex")));
        assert!(effective.contains(&usage_provider_key("Warp/Oz")));
        assert!(!effective.contains(&usage_provider_key("Copilot")));
    }

    #[test]
    fn default_usage_provider_policy_only_probes_codex() {
        let settings = crate::tui::settings::UsageSettings::default();
        let effective = effective_usage_provider_allowlist(
            &settings.enabled_providers,
            &settings.excluded_providers,
        );
        let providers: Vec<UsageProvider> = vec![
            ("Codex", credentials_available, fetch_must_not_run),
            ("Claude", credentials_must_not_be_probed, fetch_must_not_run),
            (
                "Copilot",
                credentials_must_not_be_probed,
                fetch_must_not_run,
            ),
        ];

        let selected = credentialed_usage_providers(providers, &effective);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, "Codex");
    }

    #[test]
    fn usage_provider_selection_only_probes_effective_allowlist() {
        let enabled = vec![
            "Codex".to_string(),
            "Copilot".to_string(),
            "Amp".to_string(),
        ];
        let excluded = vec![" copilot ".to_string()];
        let effective = effective_usage_provider_allowlist(&enabled, &excluded);
        let providers: Vec<UsageProvider> = vec![
            ("Codex", credentials_available, fetch_must_not_run),
            (
                "Copilot",
                credentials_must_not_be_probed,
                fetch_must_not_run,
            ),
            ("Claude", credentials_must_not_be_probed, fetch_must_not_run),
            ("Amp", credentials_unavailable, fetch_must_not_run),
        ];

        let selected = credentialed_usage_providers(providers, &effective);
        let selected_names = selected
            .into_iter()
            .map(|(provider, _, _)| provider)
            .collect::<Vec<_>>();

        assert_eq!(selected_names, vec!["Codex"]);
    }

    #[test]
    fn empty_usage_provider_allowlist_skips_all_credential_probes() {
        let effective = effective_usage_provider_allowlist(&[], &[]);
        let providers: Vec<UsageProvider> = vec![
            ("Codex", credentials_must_not_be_probed, fetch_must_not_run),
            ("Claude", credentials_must_not_be_probed, fetch_must_not_run),
        ];

        assert!(credentialed_usage_providers(providers, &effective).is_empty());
    }

    #[test]
    fn usage_output_display_name_includes_account_label() {
        let output = UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: "acct_123".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
            plan: None,
            email: None,
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };

        assert_eq!(output.display_name(), "Codex (work)");
    }

    #[test]
    fn usage_output_display_name_prefers_email_over_account_id() {
        let output = UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: "acct_123".to_string(),
                label: Some("  ".to_string()),
                is_active: false,
            }),
            plan: None,
            email: Some("user@example.com".to_string()),
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };

        assert_eq!(output.display_name(), "Codex (user@example.com)");
    }

    #[test]
    fn usage_output_display_name_masks_long_account_id() {
        let output = UsageOutput {
            provider: "Codex".to_string(),
            account: Some(UsageAccount {
                id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
                label: None,
                is_active: false,
            }),
            plan: None,
            email: None,
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };

        assert_eq!(output.display_name(), "Codex (Account 123e45...4000)");
    }

    #[test]
    fn usage_output_deserializes_legacy_json_without_account() -> Result<()> {
        let output: UsageOutput = serde_json::from_str(
            r#"{
                "provider": "Codex",
                "plan": null,
                "email": null,
                "metrics": []
            }"#,
        )?;

        assert!(output.account.is_none());
        assert_eq!(output.display_name(), "Codex");
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn subscription_cache_rejects_timestamp_far_in_the_future() {
        let temp = tempfile::TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &config_dir) };

        let path = cache_path().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let content = serde_json::json!({
            "timestamp": now + SUBSCRIPTION_CACHE_FUTURE_TOLERANCE_SECS + 60,
            "data": [],
        });
        std::fs::write(path, serde_json::to_vec(&content).unwrap()).unwrap();

        assert!(load_cache_with_observed_at().is_none());

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn subscription_cache_save_returns_persisted_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &config_dir) };

        let saved_observed_at = save_cache_with_observed_at(&[]).unwrap();
        let (loaded, loaded_observed_at) = load_cache_with_observed_at().unwrap();

        assert!(loaded.is_empty());
        assert_eq!(loaded_observed_at, saved_observed_at);

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn subscription_cache_save_reports_failure_without_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let blocked_config = temp.path().join("not-a-directory");
        std::fs::write(&blocked_config, b"blocked").unwrap();
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &blocked_config) };

        assert!(save_cache_with_observed_at(&[]).is_none());

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn expired_subscription_cache_remains_available_as_tui_last_known_good() {
        let temp = tempfile::TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &config_dir) };

        let path = cache_path().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let document = SubscriptionCacheDocument {
            timestamp: now - SUBSCRIPTION_CACHE_TTL_SECS - 1,
            data: vec![cache_output()],
            partial: false,
            stale_identities: Vec::new(),
        };
        std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();

        let loaded = load_cache_for_tui().unwrap();
        assert_eq!(loaded.data.len(), 1);
        assert!(!loaded.is_fresh);
        assert!(load_cache_with_observed_at().is_none());

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn partial_subscription_cache_round_trips_retained_row_provenance() {
        let temp = tempfile::TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &config_dir) };

        let output = cache_output();
        let identity = UsageCacheIdentity::from_output(&output);
        save_cache_with_provenance(
            std::slice::from_ref(&output),
            true,
            std::slice::from_ref(&identity),
        )
        .unwrap();

        let loaded = load_cache_for_tui().unwrap();
        assert!(loaded.is_fresh);
        assert!(loaded.partial);
        assert_eq!(loaded.stale_identities, vec![identity]);
        assert!(load_cache_with_observed_at().is_none());

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn subscription_cache_atomically_replaces_legacy_file_with_private_modes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        let cache_dir = config_dir.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = cache_dir.join("subscription-usage-cache.json");
        std::fs::write(&path, b"legacy cache").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let legacy_inode = std::fs::metadata(&path).unwrap().ino();

        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe { std::env::set_var("TOKSCALE_CONFIG_DIR", &config_dir) };

        assert!(load_cache_with_observed_at().is_none());
        let repaired = std::fs::metadata(&path).unwrap();
        assert_eq!(repaired.ino(), legacy_inode);
        assert_eq!(repaired.permissions().mode() & 0o7777, 0o600);
        assert_eq!(
            std::fs::metadata(&config_dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&cache_dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );

        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let output = UsageOutput {
            provider: "Test".to_string(),
            account: None,
            plan: None,
            email: None,
            metrics: Vec::new(),
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };
        save_cache(&[output]);

        let metadata = std::fs::metadata(&path).unwrap();
        assert_ne!(metadata.ino(), legacy_inode);
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(
            std::fs::metadata(&config_dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&cache_dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        let (loaded, _) = load_cache_with_observed_at().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].provider, "Test");

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }
}
