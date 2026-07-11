use std::collections::HashMap;

use anyhow::{bail, Result};
use chrono::{Duration as ChronoDuration, Local, NaiveDate, Utc};
use tokscale_core::pulse::weread::{self, WeReadSyncState};
use tokscale_core::pulse::{
    store, AiQuotaMetric, AiQuotaSource, AiWorkInput, AiWorkPeriodInput, PulseSnapshotV1,
};
use tokscale_core::{ClientId, GroupBy};

use crate::commands::usage::{self, UsageOutput};
use crate::report_support::PricingCacheOnlyGuard;
use crate::spinner::LightSpinner;
use crate::tui::data::UsageData;
use crate::tui::settings::Settings;
use crate::tui::{
    load_cache_with_observed_at, CacheReportScope, CacheResult, DataLoader, TUI_DEFAULT_GROUP_BY,
};
use crate::ClientFilter;

pub(crate) struct PulseRunArgs {
    pub(crate) json: bool,
    pub(crate) weekly: bool,
    pub(crate) refresh: bool,
    pub(crate) sync_only: bool,
    pub(crate) no_spinner: bool,
}

impl PulseRunArgs {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.sync_only && (self.json || self.weekly) {
            bail!("`pulse sync` cannot be combined with --json or --weekly");
        }
        Ok(())
    }
}

pub fn run(args: PulseRunArgs) -> Result<()> {
    args.validate()?;
    let PulseRunArgs {
        json,
        weekly: _,
        refresh,
        sync_only,
        no_spinner,
    } = args;

    if !refresh && !sync_only {
        if let Some(snapshot) = store::load_latest() {
            return write_snapshot(&snapshot, json);
        }
    }

    let spinner = if no_spinner {
        None
    } else {
        Some(LightSpinner::start("Building local Pulse snapshot..."))
    };

    let settings = Settings::load();
    let (usage_data, local_observed_at) = load_ai_usage(refresh || sync_only)?;
    let (quota_outputs, quota_observed_at) = match usage::load_cache_with_observed_at() {
        Some((outputs, observed_at)) => (outputs, Some(observed_at)),
        None => (Vec::new(), None),
    };
    let reading = if refresh || sync_only {
        sync_weread(&settings)?
    } else {
        load_weread_local(&settings)
    };
    let snapshot = build_snapshot(
        &usage_data,
        &quota_outputs,
        local_observed_at,
        quota_observed_at,
        reading.clone(),
    );
    let (snapshot, committed) = persist_snapshot(&snapshot, &reading)?;

    if let Some(spinner) = spinner {
        spinner.stop();
    }

    if sync_only {
        if committed {
            println!("Pulse synced: {}", snapshot.snapshot_id);
        } else {
            println!(
                "Pulse sync superseded; using newer snapshot: {}",
                snapshot.snapshot_id
            );
        }
        for source in &snapshot.sources {
            println!("  {}: {}", source.id, source.status);
        }
        return Ok(());
    }

    write_snapshot(&snapshot, json)
}

fn persist_snapshot(
    snapshot: &PulseSnapshotV1,
    reading: &WeReadSyncState,
) -> Result<(PulseSnapshotV1, bool)> {
    match store::save(snapshot, reading)? {
        store::SaveOutcome::Committed(snapshot) => Ok((snapshot, true)),
        store::SaveOutcome::Superseded(Some(durable)) => {
            let reconciled =
                PulseSnapshotV1::with_refreshed_reading(Some(&durable), reading.clone());
            match store::save(&reconciled, reading)? {
                store::SaveOutcome::Committed(snapshot) => Ok((snapshot, true)),
                store::SaveOutcome::Superseded(Some(snapshot)) => Ok((snapshot, false)),
                store::SaveOutcome::Superseded(None) => {
                    bail!("Pulse sync was superseded without a durable replacement")
                }
            }
        }
        store::SaveOutcome::Superseded(None) => {
            bail!("Pulse sync was superseded by newer local state; retry the command")
        }
    }
}

pub(crate) fn build_snapshot(
    usage_data: &UsageData,
    quota_outputs: &[UsageOutput],
    local_observed_at: Option<chrono::DateTime<Utc>>,
    quota_observed_at: Option<chrono::DateTime<Utc>>,
    reading: WeReadSyncState,
) -> PulseSnapshotV1 {
    let input = build_ai_work_input(usage_data, quota_outputs);
    let local_observed_at = (input.current.is_some() || input.previous.is_some())
        .then_some(local_observed_at)
        .flatten();
    let quota_observed_at = (!input.quota_sources.is_empty())
        .then_some(quota_observed_at)
        .flatten();
    PulseSnapshotV1::from_inputs_with_source_observed_at(
        input,
        local_observed_at,
        quota_observed_at,
        reading,
    )
}

fn write_snapshot(snapshot: &PulseSnapshotV1, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(snapshot)?);
    } else {
        print!("{}", snapshot.to_markdown());
    }
    Ok(())
}

fn sync_weread(settings: &Settings) -> Result<WeReadSyncState> {
    let Some(api_key) = settings.env_value("WEREAD_API_KEY") else {
        let mut state = weread::cache::load_sync().unwrap_or_default();
        state.mark_auth_missing(Utc::now());
        return Ok(state);
    };
    weread::sync_current_unpersisted(&api_key)
}

fn load_weread_local(settings: &Settings) -> WeReadSyncState {
    let mut state = weread::cache::load_sync().unwrap_or_default();
    if settings.env_value("WEREAD_API_KEY").is_none() {
        state.mark_auth_missing(Utc::now());
    }
    state
}

fn load_ai_usage(refresh: bool) -> Result<(UsageData, Option<chrono::DateTime<Utc>>)> {
    if !refresh {
        return Ok(match load_cached_ai_usage() {
            Some((data, observed_at)) => (data, Some(observed_at)),
            None => (UsageData::default(), None),
        });
    }

    let today = Local::now().date_naive();
    let current_start = weread::week_start_for(today);
    let previous_start = current_start
        .checked_sub_signed(ChronoDuration::days(7))
        .unwrap_or(current_start);
    let filters = ClientFilter::default_set();
    let clients = filters
        .iter()
        .copied()
        .filter_map(ClientFilter::to_client_id)
        .collect::<Vec<ClientId>>();
    let _pricing_cache_only = PricingCacheOnlyGuard::enable();
    let data = DataLoader::with_filters(
        Some(previous_start.to_string()),
        Some(today.to_string()),
        None,
    )
    .load(&clients, &GroupBy::ClientProviderModel, false)?;
    Ok((data, Some(Utc::now())))
}

fn load_cached_ai_usage() -> Option<(UsageData, chrono::DateTime<Utc>)> {
    let filters = ClientFilter::default_set();
    let (result, observed_at) = load_cache_with_observed_at(
        &filters,
        &TUI_DEFAULT_GROUP_BY,
        &CacheReportScope::default(),
    );
    match (result, observed_at) {
        (CacheResult::Fresh(data), Some(observed_at)) => Some((data, observed_at)),
        (CacheResult::Stale(_), _) | (CacheResult::StaleSubset(_), _) | (CacheResult::Miss, _) => {
            None
        }
        _ => None,
    }
}

fn build_ai_work_input(data: &UsageData, quota_outputs: &[UsageOutput]) -> AiWorkInput {
    let today = Local::now().date_naive();
    let current_start = weread::week_start_for(today);
    let current_end = current_start
        .checked_add_signed(ChronoDuration::days(7))
        .unwrap_or(current_start);
    let previous_start = current_start
        .checked_sub_signed(ChronoDuration::days(7))
        .unwrap_or(current_start);

    let current = aggregate_ai_period(data, current_start, current_end);
    let previous = aggregate_ai_period(data, previous_start, current_start);
    let quota_sources: Vec<AiQuotaSource> = quota_outputs
        .iter()
        .map(|output| AiQuotaSource {
            provider: output.provider.clone(),
            metrics: output
                .metrics
                .iter()
                .map(|metric| AiQuotaMetric {
                    label: metric.label.clone(),
                    used_percent: metric.used_percent,
                })
                .collect(),
        })
        .collect();

    AiWorkInput {
        observed_at: None,
        current,
        previous,
        quota_sources,
    }
}

#[derive(Default)]
struct ModelAggregate {
    tokens: u64,
    cost: f64,
}

fn aggregate_ai_period(
    data: &UsageData,
    start: NaiveDate,
    end_exclusive: NaiveDate,
) -> Option<AiWorkPeriodInput> {
    let days = data
        .daily
        .iter()
        .filter(|day| day.date >= start && day.date < end_exclusive)
        .filter(|day| day.tokens.total() > 0 || day.cost > 0.0)
        .collect::<Vec<_>>();
    if days.is_empty() {
        return None;
    }

    let mut total_tokens = 0u64;
    let mut total_cost = 0.0;
    let mut peak_day = None;
    let mut peak_day_tokens = 0u64;
    let mut models = HashMap::<(String, String), ModelAggregate>::new();

    for day in &days {
        let tokens = day.tokens.total();
        total_tokens = total_tokens.saturating_add(tokens);
        if day.cost.is_finite() {
            total_cost += day.cost;
        }
        if tokens > peak_day_tokens {
            peak_day_tokens = tokens;
            peak_day = Some(day.date);
        }

        for source in day.source_breakdown.values() {
            for (model_key, model) in &source.models {
                let model_name = model_key
                    .strip_prefix(&model.provider)
                    .and_then(|suffix| suffix.strip_prefix(':'))
                    .unwrap_or(model_key);
                let entry = models
                    .entry((model.provider.clone(), model_name.to_string()))
                    .or_default();
                entry.tokens = entry.tokens.saturating_add(model.tokens.total());
                if model.cost.is_finite() {
                    entry.cost += model.cost;
                }
            }
        }
    }

    let leading = models.into_iter().max_by(|left, right| {
        left.1
            .cost
            .total_cmp(&right.1.cost)
            .then_with(|| left.1.tokens.cmp(&right.1.tokens))
    });

    Some(AiWorkPeriodInput {
        total_tokens,
        total_cost,
        active_days: days.len() as u32,
        peak_day,
        peak_day_tokens,
        leading_model: leading.as_ref().map(|((_, model), _)| model.clone()),
        leading_provider: leading.map(|((provider, _), _)| provider),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::env;
    use std::path::Path;

    use super::*;
    use crate::tui::data::{DailyModelInfo, DailySourceInfo, DailyUsage, TokenBreakdown};

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

    fn token_breakdown(total: u64) -> TokenBreakdown {
        TokenBreakdown {
            input: total,
            ..TokenBreakdown::default()
        }
    }

    fn day(date: NaiveDate, tokens: u64, cost: f64, model: &str) -> DailyUsage {
        let model_info = DailyModelInfo {
            provider: "openai".to_string(),
            display_name: model.to_string(),
            color_key: "openai".to_string(),
            tokens: token_breakdown(tokens),
            cost,
            messages: 1,
        };
        DailyUsage {
            date,
            tokens: token_breakdown(tokens),
            cost,
            source_breakdown: BTreeMap::from([(
                "codex".to_string(),
                DailySourceInfo {
                    tokens: token_breakdown(tokens),
                    cost,
                    models: BTreeMap::from([(model.to_string(), model_info)]),
                },
            )]),
            message_count: 1,
            turn_count: 1,
        }
    }

    #[test]
    fn aggregates_weekly_ai_work_from_daily_usage() {
        let start = weread::week_start_for(Local::now().date_naive());
        let data = UsageData {
            daily: vec![
                day(start, 100, 1.0, "gpt-5"),
                day(
                    start.checked_add_signed(ChronoDuration::days(1)).unwrap(),
                    300,
                    3.0,
                    "gpt-5",
                ),
            ],
            ..UsageData::default()
        };

        let period = aggregate_ai_period(
            &data,
            start,
            start.checked_add_signed(ChronoDuration::days(7)).unwrap(),
        )
        .unwrap();

        assert_eq!(period.total_tokens, 400);
        assert_eq!(period.total_cost, 4.0);
        assert_eq!(period.active_days, 2);
        assert_eq!(period.leading_model.as_deref(), Some("gpt-5"));
        assert_eq!(period.leading_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn ai_work_preserves_the_data_load_observation_time() {
        let observed_at = Utc::now() - ChronoDuration::minutes(2);
        let start = weread::week_start_for(Local::now().date_naive());
        let data = UsageData {
            daily: vec![day(start, 100, 1.0, "gpt-5")],
            ..UsageData::default()
        };

        let snapshot = build_snapshot(
            &data,
            &[],
            Some(observed_at),
            None,
            WeReadSyncState::default(),
        );
        let local_source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "local-ai-usage")
            .unwrap();

        assert_eq!(local_source.observed_at, Some(observed_at));
    }

    #[test]
    fn ai_work_keeps_local_and_quota_generations_separate() {
        let local_observed_at = Utc::now() - ChronoDuration::minutes(2);
        let quota_observed_at = Utc::now() - ChronoDuration::minutes(1);
        let start = weread::week_start_for(Local::now().date_naive());
        let data = UsageData {
            daily: vec![day(start, 100, 1.0, "gpt-5")],
            ..UsageData::default()
        };
        let quota = UsageOutput {
            provider: "Codex".to_string(),
            account: None,
            plan: None,
            email: None,
            metrics: vec![usage::UsageMetric {
                label: "weekly".to_string(),
                used_percent: 75.0,
                remaining_percent: 25.0,
                remaining_label: None,
                resets_at: None,
            }],
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };

        let snapshot = build_snapshot(
            &data,
            &[quota],
            Some(local_observed_at),
            Some(quota_observed_at),
            WeReadSyncState::default(),
        );
        let source_observed_at = |id: &str| {
            snapshot
                .sources
                .iter()
                .find(|source| source.id == id)
                .and_then(|source| source.observed_at)
        };

        assert_eq!(
            source_observed_at("local-ai-usage"),
            Some(local_observed_at)
        );
        assert_eq!(
            source_observed_at("subscription-usage-cache"),
            Some(quota_observed_at)
        );
    }

    #[test]
    #[serial_test::serial]
    fn refresh_with_missing_quota_keeps_durable_quota_and_fresh_local_usage() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::set(dir.path());
        let newer = Utc::now();
        let older = newer - ChronoDuration::minutes(10);
        let start = weread::week_start_for(Local::now().date_naive());
        let quota = UsageOutput {
            provider: "Codex".to_string(),
            account: None,
            plan: None,
            email: None,
            metrics: vec![usage::UsageMetric {
                label: "weekly".to_string(),
                used_percent: 75.0,
                remaining_percent: 25.0,
                remaining_label: None,
                resets_at: None,
            }],
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        };
        let durable = build_snapshot(
            &UsageData {
                daily: vec![day(start, 100, 1.0, "gpt-5")],
                ..UsageData::default()
            },
            &[quota],
            Some(older),
            Some(older),
            WeReadSyncState::default(),
        );
        store::save(&durable, &WeReadSyncState::default()).unwrap();
        let incoming = build_snapshot(
            &UsageData {
                daily: vec![day(start, 250, 2.5, "gpt-5")],
                ..UsageData::default()
            },
            &[],
            Some(newer),
            None,
            WeReadSyncState::default(),
        );

        let (merged, committed) = persist_snapshot(&incoming, &WeReadSyncState::default()).unwrap();

        assert!(committed);
        assert_eq!(merged.ai.total_tokens, Some(250));
        assert_eq!(merged.ai.max_used_percent, Some(75.0));
        let source_observed_at = |id: &str| {
            merged
                .sources
                .iter()
                .find(|source| source.id == id)
                .and_then(|source| source.observed_at)
        };
        assert_eq!(source_observed_at("local-ai-usage"), Some(newer));
        assert_eq!(source_observed_at("subscription-usage-cache"), Some(older));
    }

    #[test]
    fn leading_model_excludes_provider_presentation_prefix() {
        let start = weread::week_start_for(Local::now().date_naive());
        let mut usage_day = day(start, 400, 4.0, "gpt-5");
        let models = &mut usage_day.source_breakdown.get_mut("codex").unwrap().models;
        let mut model = models.remove("gpt-5").unwrap();
        model.display_name = "openai / gpt-5".to_string();
        models.insert("openai:gpt-5".to_string(), model);
        let data = UsageData {
            daily: vec![usage_day],
            ..UsageData::default()
        };

        let period = aggregate_ai_period(
            &data,
            start,
            start.checked_add_signed(ChronoDuration::days(7)).unwrap(),
        )
        .unwrap();

        assert_eq!(period.leading_model.as_deref(), Some("gpt-5"));
        assert_eq!(period.leading_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn compatibility_snapshot_keeps_quota_signal() {
        let snapshot = PulseSnapshotV1::from_inputs(
            AiWorkInput {
                quota_sources: vec![AiQuotaSource {
                    provider: "Codex".to_string(),
                    metrics: vec![AiQuotaMetric {
                        label: "weekly".to_string(),
                        used_percent: 75.0,
                    }],
                }],
                ..AiWorkInput::default()
            },
            WeReadSyncState::default(),
        );

        assert_eq!(snapshot.ai.max_provider.as_deref(), Some("Codex"));
        assert!(snapshot.to_markdown().contains("## Reading Input"));
    }
}
