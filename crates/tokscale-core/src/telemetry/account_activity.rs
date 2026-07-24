use std::collections::BTreeMap;

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{TelemetryError, TelemetryStore};

const RESET_DEADLINE_ADVANCE_MIN_MS: i64 = 60_000;
const RESET_HORIZON_TOLERANCE_MS: i64 = 60_000;
const RESET_USAGE_DROP_MIN_PERCENT: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDailyUsageInput {
    pub provider: String,
    pub account_id: String,
    pub start_date: String,
    pub tokens: u64,
    pub fetched_at_ms: i64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSummaryInput {
    pub provider: String,
    pub account_id: String,
    pub lifetime_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub longest_running_turn_seconds: Option<u64>,
    pub current_streak_days: Option<u64>,
    pub longest_streak_days: Option<u64>,
    pub fetched_at_ms: i64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaObservationInput {
    pub provider: String,
    pub account_id: String,
    pub limit_id: String,
    pub limit_label: String,
    pub window_seconds: Option<i64>,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaResetEventType {
    ConfirmedManual,
    ObservedRollover,
    ObservedScheduledRollover,
    ObservedEarlyRollover,
}

impl QuotaResetEventType {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedManual => "confirmed_manual",
            Self::ObservedRollover => "observed_rollover",
            Self::ObservedScheduledRollover => "observed_scheduled_rollover",
            Self::ObservedEarlyRollover => "observed_early_rollover",
        }
    }

    fn from_stored(value: &str) -> Result<Self, TelemetryError> {
        match value {
            "confirmed_manual" => Ok(Self::ConfirmedManual),
            "observed_rollover" => Ok(Self::ObservedRollover),
            "observed_scheduled_rollover" => Ok(Self::ObservedScheduledRollover),
            "observed_early_rollover" => Ok(Self::ObservedEarlyRollover),
            _ => Err(TelemetryError::InvalidAccountActivity(
                "stored reset event type",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaResetConfidence {
    Confirmed,
    Inferred,
}

impl QuotaResetConfidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Inferred => "inferred",
        }
    }

    fn from_stored(value: &str) -> Result<Self, TelemetryError> {
        match value {
            "confirmed" => Ok(Self::Confirmed),
            "inferred" => Ok(Self::Inferred),
            _ => Err(TelemetryError::InvalidAccountActivity(
                "stored reset confidence",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaResetEventInput {
    pub event_id: String,
    pub provider: String,
    pub account_id: String,
    pub limit_id: Option<String>,
    pub event_type: QuotaResetEventType,
    pub occurred_at_ms: i64,
    pub observed_at_ms: i64,
    pub source: String,
    pub confidence: QuotaResetConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDailyUsage {
    pub start_date: String,
    pub tokens: u64,
    pub fetched_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSummary {
    pub lifetime_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub longest_running_turn_seconds: Option<u64>,
    pub current_streak_days: Option<u64>,
    pub longest_streak_days: Option<u64>,
    pub fetched_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaActivityPoint {
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowActivity {
    pub limit_id: String,
    pub limit_label: String,
    pub window_seconds: Option<i64>,
    pub observed_consumption_percent: f64,
    pub reset_count: usize,
    pub first_observed_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub max_gap_ms: i64,
    pub points: Vec<QuotaActivityPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaResetEvent {
    pub event_id: String,
    pub limit_id: Option<String>,
    pub event_type: QuotaResetEventType,
    pub occurred_at_ms: i64,
    pub observed_at_ms: i64,
    pub confidence: QuotaResetConfidence,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountActivitySnapshot {
    pub provider: String,
    pub account_id: String,
    pub summary: Option<AccountUsageSummary>,
    pub daily_usage: Vec<AccountDailyUsage>,
    pub quota_windows: Vec<QuotaWindowActivity>,
    pub reset_events: Vec<QuotaResetEvent>,
}

#[derive(Debug, Clone)]
struct StoredObservation {
    limit_id: String,
    limit_label: String,
    window_seconds: Option<i64>,
    used_percent: f64,
    resets_at_ms: Option<i64>,
    observed_at_ms: i64,
}

impl TelemetryStore {
    pub fn upsert_account_usage_summary(
        &self,
        summary: &AccountUsageSummaryInput,
    ) -> Result<bool, TelemetryError> {
        validate_usage_summary(summary)?;
        let lifetime_tokens = optional_u64_to_i64(summary.lifetime_tokens, "lifetime tokens")?;
        let peak_daily_tokens =
            optional_u64_to_i64(summary.peak_daily_tokens, "peak daily tokens")?;
        let longest_running_turn_seconds =
            optional_u64_to_i64(summary.longest_running_turn_seconds, "longest running turn")?;
        let current_streak_days =
            optional_u64_to_i64(summary.current_streak_days, "current streak")?;
        let longest_streak_days =
            optional_u64_to_i64(summary.longest_streak_days, "longest streak")?;
        let connection = self.ready_connection()?;
        let changed = connection.execute(
            "INSERT INTO account_usage_summaries (
               provider, account_id, lifetime_tokens, peak_daily_tokens,
               longest_running_turn_seconds, current_streak_days, longest_streak_days,
               fetched_at_ms, source
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(provider, account_id) DO UPDATE SET
               lifetime_tokens = excluded.lifetime_tokens,
               peak_daily_tokens = excluded.peak_daily_tokens,
               longest_running_turn_seconds = excluded.longest_running_turn_seconds,
               current_streak_days = excluded.current_streak_days,
               longest_streak_days = excluded.longest_streak_days,
               fetched_at_ms = excluded.fetched_at_ms,
               source = excluded.source
             WHERE excluded.fetched_at_ms >= account_usage_summaries.fetched_at_ms",
            params![
                summary.provider.trim(),
                summary.account_id.trim(),
                lifetime_tokens,
                peak_daily_tokens,
                longest_running_turn_seconds,
                current_streak_days,
                longest_streak_days,
                summary.fetched_at_ms,
                summary.source.trim(),
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn upsert_account_daily_usage(
        &self,
        buckets: &[AccountDailyUsageInput],
    ) -> Result<usize, TelemetryError> {
        for bucket in buckets {
            validate_daily_usage(bucket)?;
        }
        if buckets.is_empty() {
            return Ok(0);
        }

        let mut connection = self.ready_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut changed = 0usize;
        for bucket in buckets {
            let tokens = i64::try_from(bucket.tokens)
                .map_err(|_| TelemetryError::InvalidAccountActivity("daily tokens"))?;
            changed = changed.saturating_add(transaction.execute(
                "INSERT INTO account_daily_usage (
                   provider, account_id, start_date, tokens, fetched_at_ms, source
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(provider, account_id, start_date) DO UPDATE SET
                   tokens = excluded.tokens,
                   fetched_at_ms = excluded.fetched_at_ms,
                   source = excluded.source
                 WHERE excluded.fetched_at_ms >= account_daily_usage.fetched_at_ms",
                params![
                    bucket.provider.trim(),
                    bucket.account_id.trim(),
                    bucket.start_date,
                    tokens,
                    bucket.fetched_at_ms,
                    bucket.source.trim(),
                ],
            )?);
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub fn record_quota_observations(
        &self,
        observations: &[QuotaObservationInput],
    ) -> Result<usize, TelemetryError> {
        for observation in observations {
            validate_quota_observation(observation)?;
        }
        if observations.is_empty() {
            return Ok(0);
        }

        let mut connection = self.ready_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = 0usize;
        for observation in observations {
            let previous = transaction
                .query_row(
                    "SELECT used_percent, resets_at_ms, observed_at_ms
                     FROM quota_observations
                     WHERE provider = ?1 AND account_id = ?2 AND limit_id = ?3
                     ORDER BY observed_at_ms DESC LIMIT 1",
                    params![
                        observation.provider.trim(),
                        observation.account_id.trim(),
                        observation.limit_id.trim(),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, f64>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()?;

            let changed = transaction.execute(
                "INSERT OR IGNORE INTO quota_observations (
                   provider, account_id, limit_id, limit_label, window_seconds,
                   used_percent, resets_at_ms, observed_at_ms, source
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    observation.provider.trim(),
                    observation.account_id.trim(),
                    observation.limit_id.trim(),
                    observation.limit_label.trim(),
                    observation.window_seconds,
                    observation.used_percent,
                    observation.resets_at_ms,
                    observation.observed_at_ms,
                    observation.source.trim(),
                ],
            )?;
            inserted = inserted.saturating_add(changed);

            if changed > 0 {
                if let Some((previous_used_percent, previous_reset, previous_observed_at)) =
                    previous
                {
                    if let Some((event_type, occurred_at_ms)) = inferred_rollover(
                        previous_used_percent,
                        previous_reset,
                        previous_observed_at,
                        observation.used_percent,
                        observation.resets_at_ms,
                        observation.observed_at_ms,
                    ) {
                        let confirmed = transaction.query_row(
                            "SELECT EXISTS(
                               SELECT 1 FROM quota_reset_events
                               WHERE provider = ?1 AND account_id = ?2
                                 AND event_type = 'confirmed_manual'
                                 AND occurred_at_ms > ?3 AND occurred_at_ms <= ?4
                             )",
                            params![
                                observation.provider.trim(),
                                observation.account_id.trim(),
                                previous_observed_at,
                                observation.observed_at_ms,
                            ],
                            |row| row.get::<_, bool>(0),
                        )?;
                        if !confirmed {
                            let event_id = format!(
                                "{}:{}:{}:{}:{}",
                                event_type.as_str(),
                                observation.provider.trim(),
                                observation.account_id.trim(),
                                observation.limit_id.trim(),
                                observation.observed_at_ms
                            );
                            transaction.execute(
                                "INSERT OR IGNORE INTO quota_reset_events (
                                   event_id, provider, account_id, limit_id, event_type,
                                   occurred_at_ms, observed_at_ms, source, confidence
                                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'inferred')",
                                params![
                                    event_id,
                                    observation.provider.trim(),
                                    observation.account_id.trim(),
                                    observation.limit_id.trim(),
                                    event_type.as_str(),
                                    occurred_at_ms,
                                    observation.observed_at_ms,
                                    observation.source.trim(),
                                ],
                            )?;
                        }
                    }
                }
            }
        }
        transaction.commit()?;
        Ok(inserted)
    }

    pub fn record_quota_reset_event(
        &self,
        event: &QuotaResetEventInput,
    ) -> Result<bool, TelemetryError> {
        validate_reset_event(event)?;
        let connection = self.ready_connection()?;
        let changed = connection.execute(
            "INSERT OR IGNORE INTO quota_reset_events (
               event_id, provider, account_id, limit_id, event_type,
               occurred_at_ms, observed_at_ms, source, confidence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.event_id.trim(),
                event.provider.trim(),
                event.account_id.trim(),
                event.limit_id.as_deref().map(str::trim),
                event.event_type.as_str(),
                event.occurred_at_ms,
                event.observed_at_ms,
                event.source.trim(),
                event.confidence.as_str(),
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn load_account_activity(
        &self,
        provider: &str,
        account_id: &str,
        daily_since: &str,
        quota_since_ms: i64,
    ) -> Result<AccountActivitySnapshot, TelemetryError> {
        validate_identity(provider, account_id)?;
        NaiveDate::parse_from_str(daily_since, "%Y-%m-%d")
            .map_err(|_| TelemetryError::InvalidAccountActivity("daily query date"))?;
        if quota_since_ms < 0 {
            return Err(TelemetryError::InvalidAccountActivity(
                "quota query timestamp",
            ));
        }

        let connection = self.ready_connection()?;
        let summary = connection
            .query_row(
                "SELECT lifetime_tokens, peak_daily_tokens,
                        longest_running_turn_seconds, current_streak_days,
                        longest_streak_days, fetched_at_ms
                 FROM account_usage_summaries
                 WHERE provider = ?1 AND account_id = ?2",
                params![provider.trim(), account_id.trim()],
                |row| {
                    Ok(AccountUsageSummary {
                        lifetime_tokens: optional_i64_to_u64(row.get(0)?),
                        peak_daily_tokens: optional_i64_to_u64(row.get(1)?),
                        longest_running_turn_seconds: optional_i64_to_u64(row.get(2)?),
                        current_streak_days: optional_i64_to_u64(row.get(3)?),
                        longest_streak_days: optional_i64_to_u64(row.get(4)?),
                        fetched_at_ms: row.get(5)?,
                    })
                },
            )
            .optional()?;
        let mut daily_statement = connection.prepare(
            "SELECT start_date, tokens, fetched_at_ms
             FROM account_daily_usage
             WHERE provider = ?1 AND account_id = ?2 AND start_date >= ?3
             ORDER BY start_date",
        )?;
        let daily_usage = daily_statement
            .query_map(
                params![provider.trim(), account_id.trim(), daily_since],
                |row| {
                    let tokens = row.get::<_, i64>(1)?;
                    Ok(AccountDailyUsage {
                        start_date: row.get(0)?,
                        tokens: u64::try_from(tokens.max(0)).unwrap_or(u64::MAX),
                        fetched_at_ms: row.get(2)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let mut observation_statement = connection.prepare(
            "SELECT limit_id, limit_label, window_seconds, used_percent,
                    resets_at_ms, observed_at_ms
             FROM quota_observations
             WHERE provider = ?1 AND account_id = ?2 AND observed_at_ms >= ?3
             ORDER BY observed_at_ms, observation_id",
        )?;
        let observations = observation_statement
            .query_map(
                params![provider.trim(), account_id.trim(), quota_since_ms],
                |row| {
                    Ok(StoredObservation {
                        limit_id: row.get(0)?,
                        limit_label: row.get(1)?,
                        window_seconds: row.get(2)?,
                        used_percent: row.get(3)?,
                        resets_at_ms: row.get(4)?,
                        observed_at_ms: row.get(5)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let mut reset_statement = connection.prepare(
            "SELECT event_id, limit_id, event_type, occurred_at_ms,
                    observed_at_ms, confidence
             FROM quota_reset_events
             WHERE provider = ?1 AND account_id = ?2 AND observed_at_ms >= ?3
             ORDER BY occurred_at_ms, event_id",
        )?;
        let reset_events = reset_statement
            .query_map(
                params![provider.trim(), account_id.trim(), quota_since_ms],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )?
            .map(|row| {
                let (event_id, limit_id, event_type, occurred_at_ms, observed_at_ms, confidence) =
                    row?;
                Ok(QuotaResetEvent {
                    event_id,
                    limit_id,
                    event_type: QuotaResetEventType::from_stored(&event_type)?,
                    occurred_at_ms,
                    observed_at_ms,
                    confidence: QuotaResetConfidence::from_stored(&confidence)?,
                })
            })
            .collect::<Result<Vec<_>, TelemetryError>>()?;

        let reset_events = reset_events
            .into_iter()
            .filter(|event| reset_event_has_observation_evidence(event, &observations))
            .collect::<Vec<_>>();
        let quota_windows = build_quota_windows(observations, &reset_events);
        Ok(AccountActivitySnapshot {
            provider: provider.trim().to_string(),
            account_id: account_id.trim().to_string(),
            summary,
            daily_usage,
            quota_windows,
            reset_events,
        })
    }
}

fn inferred_rollover(
    previous_used_percent: f64,
    previous_reset_at_ms: Option<i64>,
    previous_observed_at_ms: i64,
    current_used_percent: f64,
    current_reset_at_ms: Option<i64>,
    current_observed_at_ms: i64,
) -> Option<(QuotaResetEventType, i64)> {
    if current_observed_at_ms <= previous_observed_at_ms {
        return None;
    }
    let (previous_reset_at_ms, current_reset_at_ms) = (previous_reset_at_ms?, current_reset_at_ms?);
    if current_reset_at_ms.saturating_sub(previous_reset_at_ms) <= RESET_DEADLINE_ADVANCE_MIN_MS {
        return None;
    }

    let previous_horizon = previous_reset_at_ms.saturating_sub(previous_observed_at_ms);
    let current_horizon = current_reset_at_ms.saturating_sub(current_observed_at_ms);
    if previous_horizon.abs_diff(current_horizon) <= RESET_HORIZON_TOLERANCE_MS as u64 {
        return None;
    }

    if previous_reset_at_ms > previous_observed_at_ms
        && current_observed_at_ms >= previous_reset_at_ms
    {
        return Some((
            QuotaResetEventType::ObservedScheduledRollover,
            previous_reset_at_ms,
        ));
    }
    if previous_used_percent - current_used_percent >= RESET_USAGE_DROP_MIN_PERCENT {
        return Some((
            QuotaResetEventType::ObservedEarlyRollover,
            current_observed_at_ms,
        ));
    }
    None
}

fn reset_event_has_observation_evidence(
    event: &QuotaResetEvent,
    observations: &[StoredObservation],
) -> bool {
    if event.confidence == QuotaResetConfidence::Confirmed {
        return true;
    }
    let Some(limit_id) = event.limit_id.as_deref() else {
        return true;
    };
    let Some(current_index) = observations.iter().position(|observation| {
        observation.limit_id == limit_id && observation.observed_at_ms == event.observed_at_ms
    }) else {
        return true;
    };
    let current = &observations[current_index];
    let Some(previous) = observations[..current_index]
        .iter()
        .rev()
        .find(|observation| observation.limit_id == limit_id)
    else {
        return true;
    };

    inferred_rollover(
        previous.used_percent,
        previous.resets_at_ms,
        previous.observed_at_ms,
        current.used_percent,
        current.resets_at_ms,
        current.observed_at_ms,
    )
    .is_some()
}

fn build_quota_windows(
    observations: Vec<StoredObservation>,
    reset_events: &[QuotaResetEvent],
) -> Vec<QuotaWindowActivity> {
    let mut groups = BTreeMap::<String, Vec<StoredObservation>>::new();
    for observation in observations {
        groups
            .entry(observation.limit_id.clone())
            .or_default()
            .push(observation);
    }

    groups
        .into_iter()
        .filter_map(|(limit_id, observations)| {
            let first_observed_at_ms = observations.first()?.observed_at_ms;
            let last = observations.last()?;
            let last_observed_at_ms = last.observed_at_ms;
            let limit_label = last.limit_label.clone();
            let window_seconds = last.window_seconds;
            let mut observed_consumption_percent = 0.0_f64;
            let mut max_gap_ms = 0_i64;
            for pair in observations.windows(2) {
                let previous = &pair[0];
                let current = &pair[1];
                max_gap_ms = max_gap_ms.max(
                    current
                        .observed_at_ms
                        .saturating_sub(previous.observed_at_ms),
                );
                let recorded_reset = reset_events.iter().any(|event| {
                    event
                        .limit_id
                        .as_deref()
                        .is_none_or(|event_limit| event_limit == limit_id)
                        && event.occurred_at_ms > previous.observed_at_ms
                        && event.occurred_at_ms <= current.observed_at_ms
                });
                if recorded_reset {
                    observed_consumption_percent += current.used_percent;
                } else if current.used_percent >= previous.used_percent {
                    observed_consumption_percent += current.used_percent - previous.used_percent;
                }
            }
            let reset_count = reset_events
                .iter()
                .filter(|event| {
                    event
                        .limit_id
                        .as_deref()
                        .is_none_or(|event_limit| event_limit == limit_id)
                })
                .count();
            let points = observations
                .into_iter()
                .map(|observation| QuotaActivityPoint {
                    used_percent: observation.used_percent,
                    resets_at_ms: observation.resets_at_ms,
                    observed_at_ms: observation.observed_at_ms,
                })
                .collect();
            Some(QuotaWindowActivity {
                limit_id,
                limit_label,
                window_seconds,
                observed_consumption_percent,
                reset_count,
                first_observed_at_ms,
                last_observed_at_ms,
                max_gap_ms,
                points,
            })
        })
        .collect()
}

fn validate_identity(provider: &str, account_id: &str) -> Result<(), TelemetryError> {
    if provider.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("provider"));
    }
    if account_id.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("account id"));
    }
    Ok(())
}

fn validate_daily_usage(bucket: &AccountDailyUsageInput) -> Result<(), TelemetryError> {
    validate_identity(&bucket.provider, &bucket.account_id)?;
    NaiveDate::parse_from_str(&bucket.start_date, "%Y-%m-%d")
        .map_err(|_| TelemetryError::InvalidAccountActivity("daily usage date"))?;
    i64::try_from(bucket.tokens)
        .map_err(|_| TelemetryError::InvalidAccountActivity("daily tokens"))?;
    if bucket.fetched_at_ms < 0 {
        return Err(TelemetryError::InvalidAccountActivity(
            "daily fetch timestamp",
        ));
    }
    if bucket.source.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("daily source"));
    }
    Ok(())
}

fn validate_usage_summary(summary: &AccountUsageSummaryInput) -> Result<(), TelemetryError> {
    validate_identity(&summary.provider, &summary.account_id)?;
    optional_u64_to_i64(summary.lifetime_tokens, "lifetime tokens")?;
    optional_u64_to_i64(summary.peak_daily_tokens, "peak daily tokens")?;
    optional_u64_to_i64(summary.longest_running_turn_seconds, "longest running turn")?;
    optional_u64_to_i64(summary.current_streak_days, "current streak")?;
    optional_u64_to_i64(summary.longest_streak_days, "longest streak")?;
    if summary.fetched_at_ms < 0 {
        return Err(TelemetryError::InvalidAccountActivity(
            "summary fetch timestamp",
        ));
    }
    if summary.source.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("summary source"));
    }
    Ok(())
}

fn optional_u64_to_i64(
    value: Option<u64>,
    field: &'static str,
) -> Result<Option<i64>, TelemetryError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| TelemetryError::InvalidAccountActivity(field))
        })
        .transpose()
}

fn optional_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn validate_quota_observation(observation: &QuotaObservationInput) -> Result<(), TelemetryError> {
    validate_identity(&observation.provider, &observation.account_id)?;
    if observation.limit_id.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("quota limit id"));
    }
    if observation.limit_label.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("quota limit label"));
    }
    if observation
        .window_seconds
        .is_some_and(|seconds| seconds <= 0)
    {
        return Err(TelemetryError::InvalidAccountActivity(
            "quota window duration",
        ));
    }
    if !observation.used_percent.is_finite() || !(0.0..=100.0).contains(&observation.used_percent) {
        return Err(TelemetryError::InvalidAccountActivity("quota used percent"));
    }
    if observation.observed_at_ms < 0 || observation.resets_at_ms.is_some_and(|value| value < 0) {
        return Err(TelemetryError::InvalidAccountActivity(
            "quota observation timestamp",
        ));
    }
    if observation.source.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("quota source"));
    }
    Ok(())
}

fn validate_reset_event(event: &QuotaResetEventInput) -> Result<(), TelemetryError> {
    validate_identity(&event.provider, &event.account_id)?;
    if event.event_id.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("reset event id"));
    }
    if event
        .limit_id
        .as_deref()
        .is_some_and(|limit| limit.trim().is_empty())
    {
        return Err(TelemetryError::InvalidAccountActivity("reset limit id"));
    }
    if event.occurred_at_ms < 0 || event.observed_at_ms < 0 {
        return Err(TelemetryError::InvalidAccountActivity(
            "reset event timestamp",
        ));
    }
    if event.source.trim().is_empty() {
        return Err(TelemetryError::InvalidAccountActivity("reset source"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

    fn store() -> (tempfile::TempDir, TelemetryStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let store = TelemetryStore::open(temp.path().join("telemetry.sqlite")).unwrap();
        (temp, store)
    }

    fn observation(at: i64, used: f64, resets_at: i64) -> QuotaObservationInput {
        QuotaObservationInput {
            provider: "Codex".into(),
            account_id: "acct-work".into(),
            limit_id: "codex:primary".into(),
            limit_label: "5h".into(),
            window_seconds: Some(5 * 60 * 60),
            used_percent: used,
            resets_at_ms: Some(resets_at),
            observed_at_ms: at,
            source: "codex-rate-limits".into(),
        }
    }

    #[test]
    fn daily_usage_upserts_newer_account_bucket() {
        let (_temp, store) = store();
        let mut bucket = AccountDailyUsageInput {
            provider: "Codex".into(),
            account_id: "acct-work".into(),
            start_date: "2026-07-23".into(),
            tokens: 100,
            fetched_at_ms: 10,
            source: "codex-app-server".into(),
        };
        store.upsert_account_daily_usage(&[bucket.clone()]).unwrap();
        bucket.tokens = 140;
        bucket.fetched_at_ms = 20;
        store.upsert_account_daily_usage(&[bucket.clone()]).unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();
        assert_eq!(activity.daily_usage.len(), 1);
        assert_eq!(activity.daily_usage[0].tokens, 140);
    }

    #[test]
    fn account_summary_upserts_newer_official_values() {
        let (_temp, store) = store();
        let mut summary = AccountUsageSummaryInput {
            provider: "Codex".into(),
            account_id: "acct-work".into(),
            lifetime_tokens: Some(1_000),
            peak_daily_tokens: Some(400),
            longest_running_turn_seconds: Some(120),
            current_streak_days: Some(3),
            longest_streak_days: Some(5),
            fetched_at_ms: 10,
            source: "codex-app-server".into(),
        };
        store.upsert_account_usage_summary(&summary).unwrap();
        summary.lifetime_tokens = Some(1_400);
        summary.fetched_at_ms = 20;
        store.upsert_account_usage_summary(&summary).unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();
        let summary = activity.summary.unwrap();
        assert_eq!(summary.lifetime_tokens, Some(1_400));
        assert_eq!(summary.longest_running_turn_seconds, Some(120));
    }

    #[test]
    fn quota_activity_sums_positive_deltas_across_observed_reset() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[
                observation(1_000, 20.0, DAY_MS),
                observation(2_000, 100.0, DAY_MS),
                observation(3_000, 30.0, 2 * DAY_MS),
            ])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();
        assert_eq!(activity.quota_windows.len(), 1);
        assert_eq!(
            activity.quota_windows[0].observed_consumption_percent,
            110.0
        );
        assert_eq!(activity.quota_windows[0].reset_count, 1);
        assert_eq!(activity.reset_events.len(), 1);
        assert_eq!(
            activity.reset_events[0].event_type,
            QuotaResetEventType::ObservedEarlyRollover
        );
    }

    #[test]
    fn scheduled_rollover_uses_provider_deadline_as_occurrence_time() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[
                observation(1_000, 20.0, 120_000),
                observation(121_000, 30.0, DAY_MS),
            ])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();
        assert_eq!(activity.reset_events.len(), 1);
        assert_eq!(
            activity.reset_events[0].event_type,
            QuotaResetEventType::ObservedScheduledRollover
        );
        assert_eq!(activity.reset_events[0].occurred_at_ms, 120_000);
        assert_eq!(activity.reset_events[0].observed_at_ms, 121_000);
    }

    #[test]
    fn quota_activity_detects_rollover_even_when_percent_is_higher_after_reset() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[
                observation(1_000, 20.0, 120_000),
                observation(121_000, 30.0, DAY_MS),
            ])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();

        assert_eq!(activity.quota_windows[0].observed_consumption_percent, 30.0);
        assert_eq!(activity.quota_windows[0].reset_count, 1);
        assert_eq!(activity.reset_events.len(), 1);
    }

    #[test]
    fn quota_activity_ignores_deadline_advance_without_rollover_evidence() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[
                observation(1_000, 20.0, DAY_MS),
                observation(121_000, 30.0, 2 * DAY_MS),
            ])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();

        assert_eq!(activity.quota_windows[0].observed_consumption_percent, 10.0);
        assert_eq!(activity.quota_windows[0].reset_count, 0);
        assert!(activity.reset_events.is_empty());
    }

    #[test]
    fn sliding_deadline_does_not_emit_rollover_events() {
        let (_temp, store) = store();
        const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
        store
            .record_quota_observations(&[
                observation(1_000, 0.0, 1_000 + WEEK_MS),
                observation(121_000, 0.0, 121_000 + WEEK_MS),
            ])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();

        assert_eq!(activity.quota_windows[0].observed_consumption_percent, 0.0);
        assert_eq!(activity.quota_windows[0].reset_count, 0);
        assert!(activity.reset_events.is_empty());
    }

    #[test]
    fn load_filters_unsubstantiated_legacy_rollover_events() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[
                observation(1_000, 20.0, 5_000),
                observation(2_000, 20.0, 5_001),
            ])
            .unwrap();
        store
            .record_quota_reset_event(&QuotaResetEventInput {
                event_id: "legacy-noise".into(),
                provider: "Codex".into(),
                account_id: "acct-work".into(),
                limit_id: Some("codex:primary".into()),
                event_type: QuotaResetEventType::ObservedRollover,
                occurred_at_ms: 2_000,
                observed_at_ms: 2_000,
                source: "codex-rate-limits".into(),
                confidence: QuotaResetConfidence::Inferred,
            })
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();

        assert!(activity.reset_events.is_empty());
        assert_eq!(activity.quota_windows[0].reset_count, 0);
    }

    #[test]
    fn confirmed_reset_suppresses_duplicate_inferred_event() {
        let (_temp, store) = store();
        store
            .record_quota_observations(&[observation(1_000, 100.0, DAY_MS)])
            .unwrap();
        store
            .record_quota_reset_event(&QuotaResetEventInput {
                event_id: "manual-1".into(),
                provider: "Codex".into(),
                account_id: "acct-work".into(),
                limit_id: None,
                event_type: QuotaResetEventType::ConfirmedManual,
                occurred_at_ms: 2_000,
                observed_at_ms: 2_000,
                source: "tokscale-reset".into(),
                confidence: QuotaResetConfidence::Confirmed,
            })
            .unwrap();
        store
            .record_quota_observations(&[observation(3_000, 30.0, 2 * DAY_MS)])
            .unwrap();

        let activity = store
            .load_account_activity("Codex", "acct-work", "2026-07-20", 0)
            .unwrap();
        assert_eq!(activity.reset_events.len(), 1);
        assert_eq!(activity.quota_windows[0].reset_count, 1);
        assert_eq!(activity.quota_windows[0].observed_consumption_percent, 30.0);
    }

    #[test]
    fn rejects_invalid_account_activity_values() {
        let (_temp, store) = store();
        let mut invalid = observation(1_000, f64::NAN, 5_000);
        assert!(matches!(
            store.record_quota_observations(&[invalid.clone()]),
            Err(TelemetryError::InvalidAccountActivity(_))
        ));
        invalid.used_percent = 20.0;
        invalid.account_id.clear();
        assert!(matches!(
            store.record_quota_observations(&[invalid]),
            Err(TelemetryError::InvalidAccountActivity(_))
        ));
    }
}
