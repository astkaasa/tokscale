use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::weread::{
    format_compare_ratio, format_read_duration, week_start_for, DatasetCoverage, DatasetFreshness,
    SourceIssueCode, WeReadNotebookSummary, WeReadState, WeReadStatus, WeReadSyncState,
    SKILL_VERSION,
};

pub const PULSE_SCHEMA_VERSION: u32 = 1;
const AI_COST_INCREASE_THRESHOLD_RATIO: f64 = 0.25;
const QUOTA_CACHE_FRESHNESS_SECS: i64 = 300;
const SOURCE_FUTURE_TOLERANCE_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseSnapshotV1 {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub generated_at: DateTime<Utc>,
    pub period: PulsePeriod,
    pub sources: Vec<SourceHealth>,
    #[serde(rename = "aiWork")]
    pub ai: AiPulse,
    #[serde(rename = "readingInput")]
    pub reading: ReadingPulse,
    pub knowledge_flow: KnowledgeFlowSignal,
    pub insights: Vec<PulseInsight>,
    pub evidence: Vec<PulseEvidence>,
    pub recommendations: Vec<PulseRecommendation>,
}

pub type PulseSummary = PulseSnapshotV1;

impl PulseSnapshotV1 {
    /// Compatibility entry point for existing quota-cache and legacy WeRead callers.
    pub fn collect(ai_sources: Vec<AiQuotaSource>, reading_state: WeReadState) -> Self {
        let reading_state =
            WeReadSyncState::from_legacy(reading_state, DatasetCoverage::Unknown, Utc::now());
        Self::from_inputs(
            AiWorkInput {
                quota_sources: ai_sources,
                ..AiWorkInput::default()
            },
            reading_state,
        )
    }

    pub fn from_inputs(ai_input: AiWorkInput, reading_state: WeReadSyncState) -> Self {
        let observed_at = ai_input.observed_at;
        Self::from_inputs_with_source_observed_at(ai_input, observed_at, observed_at, reading_state)
    }

    /// Builds a snapshot with independent local-usage and quota generations.
    ///
    /// `from_inputs` retains the legacy shared `AiWorkInput::observed_at` contract.
    pub fn from_inputs_with_source_observed_at(
        ai_input: AiWorkInput,
        local_observed_at: Option<DateTime<Utc>>,
        quota_observed_at: Option<DateTime<Utc>>,
        reading_state: WeReadSyncState,
    ) -> Self {
        let generated_at = Utc::now();
        let period = snapshot_period();
        let ai = AiPulse::from_input(&ai_input);
        let reading =
            ReadingPulse::from_weread_sync_for_period(&reading_state, &period, generated_at);
        let knowledge_flow = KnowledgeFlowSignal::from_weread_sync(&reading_state);
        let sources = build_source_health(
            &ai_input,
            local_observed_at,
            quota_observed_at,
            &reading_state,
            generated_at,
        );
        Self::from_normalized(
            generated_at,
            period,
            sources,
            ai,
            reading,
            knowledge_flow,
            &reading_state,
        )
    }

    pub fn with_refreshed_reading(existing: Option<&Self>, reading_state: WeReadSyncState) -> Self {
        let generated_at = Utc::now();
        let period = snapshot_period();
        let fallback_input = AiWorkInput::default();
        let fallback_ai = AiPulse::from_input(&fallback_input);
        let fallback_sources =
            build_source_health(&fallback_input, None, None, &reading_state, generated_at);
        let existing = existing
            .filter(|snapshot| {
                snapshot.period.start == period.start
                    && snapshot.period.end_exclusive == period.end_exclusive
            })
            .cloned()
            .map(|mut snapshot| {
                snapshot.refresh_time_sensitive_source_health(generated_at);
                snapshot
            });
        let ai = existing
            .as_ref()
            .map(|snapshot| snapshot.ai.clone())
            .unwrap_or(fallback_ai);
        let mut sources = Vec::with_capacity(3);
        for id in ["local-ai-usage", "subscription-usage-cache"] {
            if let Some(source) = existing
                .as_ref()
                .and_then(|snapshot| snapshot.sources.iter().find(|source| source.id == id))
                .or_else(|| fallback_sources.iter().find(|source| source.id == id))
            {
                sources.push(source.clone());
            }
        }
        if let Some(source) = fallback_sources.iter().find(|source| source.id == "weread") {
            sources.push(source.clone());
        }
        let reading =
            ReadingPulse::from_weread_sync_for_period(&reading_state, &period, generated_at);
        let knowledge_flow = KnowledgeFlowSignal::from_weread_sync(&reading_state);
        Self::from_normalized(
            generated_at,
            period,
            sources,
            ai,
            reading,
            knowledge_flow,
            &reading_state,
        )
    }

    /// Returns a presentation-only copy with time-sensitive source health evaluated at `now`.
    ///
    /// The returned copy preserves the canonical snapshot identity and generation time. Callers
    /// must not write it back to durable storage or use it for canonical JSON/Markdown exports.
    pub fn for_presentation_at(&self, now: DateTime<Utc>) -> Self {
        let mut snapshot = self.clone();
        snapshot.refresh_time_sensitive_source_health(now);
        snapshot
    }

    fn refresh_time_sensitive_source_health(&mut self, now: DateTime<Utc>) {
        let Some((current_freshness, observed_at)) = self
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .map(|source| (source.freshness, source.observed_at))
        else {
            return;
        };
        if current_freshness == PulseFreshness::Missing {
            return;
        }

        let freshness = quota_source_freshness(true, observed_at, now);
        if freshness == PulseFreshness::Stale && current_freshness == PulseFreshness::Fresh {
            self.mark_source_degraded("subscription-usage-cache", "stale_cache");
        } else {
            self.sync_evidence_freshness("subscription-usage-cache", current_freshness);
        }
    }

    pub fn mark_source_degraded(&mut self, source_id: &str, issue_code: &str) -> bool {
        let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)
        else {
            return false;
        };
        source.status = PulseFreshness::Stale.label().to_string();
        source.freshness = PulseFreshness::Stale;
        source.coverage = PulseCoverage::Partial;
        source.issue_code = Some(issue_code.to_string());
        self.sync_evidence_freshness(source_id, PulseFreshness::Stale);
        true
    }

    fn sync_evidence_freshness(&mut self, source_id: &str, freshness: PulseFreshness) {
        for evidence in &mut self.evidence {
            if evidence.source_id == source_id {
                evidence.freshness = freshness;
            }
        }
    }

    fn from_normalized(
        generated_at: DateTime<Utc>,
        period: PulsePeriod,
        sources: Vec<SourceHealth>,
        ai: AiPulse,
        reading: ReadingPulse,
        knowledge_flow: KnowledgeFlowSignal,
        reading_state: &WeReadSyncState,
    ) -> Self {
        let evidence = build_evidence(
            &period,
            &sources,
            &ai,
            &reading,
            &knowledge_flow,
            reading_state,
            generated_at,
        );
        let insights = build_insights(&ai, &reading, &evidence);
        let recommendations = build_recommendations(&ai, &reading, &insights);
        let snapshot_id = content_snapshot_id(&period, &ai, &reading, &knowledge_flow, &evidence);

        Self {
            schema_version: PULSE_SCHEMA_VERSION,
            snapshot_id,
            generated_at,
            period,
            sources,
            ai,
            reading,
            knowledge_flow,
            insights,
            evidence,
            recommendations,
        }
    }

    pub fn validate_evidence_refs(&self) -> bool {
        let ids = self
            .evidence
            .iter()
            .map(|evidence| evidence.id.as_str())
            .collect::<HashSet<_>>();
        self.insights
            .iter()
            .flat_map(|insight| insight.evidence_refs.iter())
            .chain(
                self.recommendations
                    .iter()
                    .flat_map(|recommendation| recommendation.evidence_refs.iter()),
            )
            .all(|reference| ids.contains(reference.as_str()))
    }

    pub fn to_markdown(&self) -> String {
        let mut lines = vec![
            "# Weekly Pulse".to_string(),
            String::new(),
            format!(
                "Period: {} to {} (snapshot `{}`)",
                self.period.start,
                self.period
                    .end_exclusive
                    .pred_opt()
                    .unwrap_or(self.period.end_exclusive),
                self.snapshot_id
            ),
            String::new(),
            "## AI Work".to_string(),
        ];

        lines.extend(self.ai.markdown_lines());
        lines.push(String::new());
        lines.push("## Reading Input".to_string());
        lines.extend(self.reading.markdown_lines());
        lines.push(String::new());
        lines.push("## Knowledge Flow".to_string());
        lines.extend(self.knowledge_flow.markdown_lines());
        lines.push(String::new());
        lines.push("## Balance".to_string());

        if self.insights.is_empty() {
            lines.push("- No evidence-backed attention item for this period.".to_string());
        } else {
            lines.extend(
                self.insights
                    .iter()
                    .map(|insight| format!("- {}: {}", insight.title, insight.summary)),
            );
        }

        lines.push(String::new());
        lines.push("## Evidence".to_string());
        if self.evidence.is_empty() {
            lines.push("- No evidence is available yet.".to_string());
        } else {
            lines.extend(self.evidence.iter().map(PulseEvidence::markdown_line));
        }

        lines.push(String::new());
        lines.push("## Suggested Actions".to_string());
        if self.recommendations.is_empty() {
            lines.push("- Keep collecting comparable local periods.".to_string());
        } else {
            lines.extend(
                self.recommendations
                    .iter()
                    .map(|recommendation| format!("- {}", recommendation.title)),
            );
        }

        lines.push(String::new());
        lines.push("## Source Health".to_string());
        lines.extend(self.sources.iter().map(|source| {
            let issue = source
                .issue_code
                .as_deref()
                .map(|code| format!(", issue {code}"))
                .unwrap_or_default();
            format!(
                "- {}: {} ({}, {}{}).",
                source.id,
                source.status,
                source.freshness.label(),
                source.coverage.label(),
                issue
            )
        }));
        lines.push(String::new());
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PulsePeriod {
    pub kind: PulsePeriodKind,
    pub start: NaiveDate,
    pub end_exclusive: NaiveDate,
    pub timezone: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PulsePeriodKind {
    Week,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PulseFreshness {
    Fresh,
    Stale,
    Missing,
}

impl PulseFreshness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PulseCoverage {
    Complete,
    Partial,
    Unknown,
}

impl PulseCoverage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealth {
    pub id: String,
    pub status: String,
    pub transport: String,
    pub storage: String,
    pub connector_version: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub freshness: PulseFreshness,
    pub coverage: PulseCoverage,
    pub issue_code: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiWorkInput {
    pub current: Option<AiWorkPeriodInput>,
    pub previous: Option<AiWorkPeriodInput>,
    pub quota_sources: Vec<AiQuotaSource>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiWorkPeriodInput {
    pub total_tokens: u64,
    pub total_cost: f64,
    pub active_days: u32,
    pub peak_day: Option<NaiveDate>,
    pub peak_day_tokens: u64,
    pub leading_model: Option<String>,
    pub leading_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQuotaSource {
    pub provider: String,
    pub metrics: Vec<AiQuotaMetric>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQuotaMetric {
    pub label: String,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPulse {
    pub source: String,
    pub status: String,
    pub total_tokens: Option<u64>,
    pub total_cost: Option<f64>,
    pub active_days: Option<u32>,
    pub peak_day: Option<NaiveDate>,
    pub peak_day_tokens: Option<u64>,
    pub leading_model: Option<String>,
    pub leading_provider: Option<String>,
    pub previous_total_tokens: Option<u64>,
    pub previous_total_cost: Option<f64>,
    pub token_change_ratio: Option<f64>,
    pub cost_change_ratio: Option<f64>,
    pub quota_risk: SignalLevel,
    pub provider_count: usize,
    pub max_used_percent: Option<f64>,
    pub max_metric_label: Option<String>,
    pub max_provider: Option<String>,
}

impl AiPulse {
    pub fn from_quota_sources(outputs: Vec<AiQuotaSource>) -> Self {
        Self::from_input(&AiWorkInput {
            quota_sources: outputs,
            ..AiWorkInput::default()
        })
    }

    pub fn from_input(input: &AiWorkInput) -> Self {
        let mut max: Option<(String, String, f64)> = None;
        for output in &input.quota_sources {
            for metric in &output.metrics {
                if !metric.used_percent.is_finite() {
                    continue;
                }
                let replace = max
                    .as_ref()
                    .is_none_or(|(_, _, current)| metric.used_percent > *current);
                if replace {
                    max = Some((
                        output.provider.clone(),
                        metric.label.clone(),
                        metric.used_percent,
                    ));
                }
            }
        }

        let max_used_percent = max.as_ref().map(|(_, _, used)| *used);
        let quota_risk = match max_used_percent {
            Some(used) if used >= 90.0 => SignalLevel::High,
            Some(used) if used >= 70.0 => SignalLevel::Medium,
            Some(_) => SignalLevel::Low,
            None => SignalLevel::Unknown,
        };
        let current = input.current.as_ref();
        let previous = input.previous.as_ref();

        Self {
            source: "local-ai-usage".to_string(),
            status: if current.is_some() {
                "fresh"
            } else {
                "missing"
            }
            .to_string(),
            total_tokens: current.map(|period| period.total_tokens),
            total_cost: current.map(|period| finite_or_zero(period.total_cost)),
            active_days: current.map(|period| period.active_days),
            peak_day: current.and_then(|period| period.peak_day),
            peak_day_tokens: current.map(|period| period.peak_day_tokens),
            leading_model: current.and_then(|period| period.leading_model.clone()),
            leading_provider: current.and_then(|period| period.leading_provider.clone()),
            previous_total_tokens: previous.map(|period| period.total_tokens),
            previous_total_cost: previous.map(|period| finite_or_zero(period.total_cost)),
            token_change_ratio: current.zip(previous).and_then(|(current, previous)| {
                ratio_change(current.total_tokens as f64, previous.total_tokens as f64)
            }),
            cost_change_ratio: current.zip(previous).and_then(|(current, previous)| {
                ratio_change(current.total_cost, previous.total_cost)
            }),
            quota_risk,
            provider_count: input.quota_sources.len(),
            max_used_percent,
            max_metric_label: max.as_ref().map(|(_, label, _)| label.clone()),
            max_provider: max.map(|(provider, _, _)| provider),
        }
    }

    fn markdown_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let (Some(tokens), Some(cost), Some(active_days)) =
            (self.total_tokens, self.total_cost, self.active_days)
        {
            lines.push(format!(
                "- This week: {tokens} tokens, ${cost:.2}, {active_days} active days."
            ));
            if let Some(change) = self.cost_change_ratio {
                lines.push(format!(
                    "- Cost change from previous week: {}.",
                    format_compare_ratio(Some(change))
                ));
            }
            if let Some(model) = &self.leading_model {
                lines.push(format!("- Leading model: {model}."));
            }
        } else {
            lines.push("- Weekly local AI usage is not available in the snapshot.".to_string());
        }

        if let (Some(provider), Some(label), Some(used)) = (
            &self.max_provider,
            &self.max_metric_label,
            self.max_used_percent,
        ) {
            lines.push(format!(
                "- Quota: {provider} {label} is at {:.0}% used ({} risk).",
                used,
                self.quota_risk.label()
            ));
        } else {
            lines.push("- No cached subscription quota is available.".to_string());
        }
        lines
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingDaySignal {
    pub date: NaiveDate,
    pub read_seconds: u32,
    pub checked_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPulse {
    pub source: String,
    pub status: String,
    pub read_days: Option<u8>,
    pub weekly_total_seconds: Option<u32>,
    pub weekly_total_label: Option<String>,
    pub daily_average_seconds: Option<u32>,
    pub daily_average_label: Option<String>,
    pub week_over_week_ratio: Option<f64>,
    pub week_over_week: Option<String>,
    pub previous_read_days: Option<u8>,
    pub previous_weekly_total_seconds: Option<u32>,
    pub days: Vec<ReadingDaySignal>,
    pub focus_book: Option<String>,
    pub focus_book_id: Option<String>,
    pub focus_read_seconds: Option<u32>,
    pub month_total_label: Option<String>,
    pub month_read_days: Option<u16>,
    pub preferred_category: Option<String>,
    pub shelf_visible_items: Option<u32>,
    pub issue_code: Option<String>,
}

impl ReadingPulse {
    pub fn from_weread(state: WeReadState) -> Self {
        let sync = WeReadSyncState::from_legacy(state, DatasetCoverage::Unknown, Utc::now());
        Self::from_weread_sync(&sync)
    }

    pub fn from_weread_sync(state: &WeReadSyncState) -> Self {
        Self::from_weread_sync_for_period(state, &snapshot_period(), Utc::now())
    }

    fn from_weread_sync_for_period(
        state: &WeReadSyncState,
        period: &PulsePeriod,
        generated_at: DateTime<Utc>,
    ) -> Self {
        let previous_start = period
            .start
            .checked_sub_signed(ChronoDuration::days(7))
            .unwrap_or(period.start);
        let weekly = state
            .datasets
            .current_week
            .value
            .as_ref()
            .filter(|weekly| weekly.period_start == period.start);
        let previous = state
            .datasets
            .previous_week
            .value
            .as_ref()
            .filter(|weekly| weekly.period_start == previous_start);
        let generated_local = generated_at.with_timezone(&Local);
        let monthly_is_current = state
            .datasets
            .current_month
            .observed_at
            .is_some_and(|observed| {
                let observed_local = observed.with_timezone(&Local);
                observed_local.year() == generated_local.year()
                    && observed_local.month() == generated_local.month()
            });
        let monthly = if monthly_is_current {
            state.datasets.current_month.value.as_ref()
        } else {
            None
        };
        let shelf = state.datasets.shelf.value.as_ref();
        let issue = first_weread_issue(state);
        let total_seconds_ratio = weekly.zip(previous).and_then(|(current, previous)| {
            ratio_change(current.total_seconds as f64, previous.total_seconds as f64)
        });
        let week_over_week_ratio =
            total_seconds_ratio.or_else(|| weekly.and_then(|weekly| weekly.compare_ratio));

        Self {
            source: "weread".to_string(),
            status: state.status.label().to_string(),
            read_days: weekly.map(|weekly| weekly.read_days),
            weekly_total_seconds: weekly.map(|weekly| weekly.total_seconds),
            weekly_total_label: weekly.map(|weekly| format_read_duration(weekly.total_seconds)),
            daily_average_seconds: weekly.map(|weekly| weekly.day_average_seconds),
            daily_average_label: weekly
                .map(|weekly| format_read_duration(weekly.day_average_seconds)),
            week_over_week_ratio,
            week_over_week: week_over_week_ratio.map(|ratio| format_compare_ratio(Some(ratio))),
            previous_read_days: previous.map(|weekly| weekly.read_days),
            previous_weekly_total_seconds: previous.map(|weekly| weekly.total_seconds),
            days: weekly
                .map(|weekly| {
                    weekly
                        .days
                        .iter()
                        .map(|day| ReadingDaySignal {
                            date: day.date,
                            read_seconds: day.read_seconds,
                            checked_in: day.checked_in,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            focus_book: weekly
                .and_then(|weekly| weekly.focus.as_ref())
                .map(|focus| focus.title.clone()),
            focus_book_id: weekly
                .and_then(|weekly| weekly.focus.as_ref())
                .map(|focus| focus.id.clone()),
            focus_read_seconds: weekly
                .and_then(|weekly| weekly.focus.as_ref())
                .map(|focus| focus.read_seconds),
            month_total_label: monthly.map(|monthly| format_read_duration(monthly.total_seconds)),
            month_read_days: monthly.map(|monthly| monthly.read_days),
            preferred_category: monthly.and_then(|monthly| monthly.prefer_category_word.clone()),
            shelf_visible_items: shelf.map(|shelf| shelf.visible_items),
            issue_code: issue.map(|issue| issue_code_label(issue.code).to_string()),
        }
    }

    fn markdown_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("- Status: {}.", self.status)];
        if let (Some(days), Some(total), Some(avg)) = (
            self.read_days,
            self.weekly_total_label.as_ref(),
            self.daily_average_label.as_ref(),
        ) {
            let comparison = self
                .week_over_week
                .as_deref()
                .map(|value| format!(" ({value} from previous week)"))
                .unwrap_or_default();
            lines.push(format!(
                "- This week: {days}/7 days, {total}, natural-day average {avg}{comparison}."
            ));
        } else {
            lines.push("- No weekly reading data is available yet.".to_string());
        }
        if let Some(book) = &self.focus_book {
            let duration = self
                .focus_read_seconds
                .map(format_read_duration)
                .map(|value| format!(", {value} this week"))
                .unwrap_or_default();
            lines.push(format!("- Focus book: {book}{duration}."));
        }
        if let (Some(total), Some(days)) = (&self.month_total_label, self.month_read_days) {
            lines.push(format!("- Month: {total} across {days} read days."));
        }
        lines
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFlowSignal {
    pub total_books: Option<u32>,
    pub total_notes: Option<u32>,
    pub coverage: PulseCoverage,
    pub sampled_notebooks: Vec<WeReadNotebookSummary>,
    pub note_delta: Option<i64>,
}

impl KnowledgeFlowSignal {
    fn from_weread_sync(state: &WeReadSyncState) -> Self {
        let dataset = &state.datasets.notebooks;
        let notes = dataset.value.as_ref();
        Self {
            total_books: notes.map(|notes| notes.total_books),
            total_notes: notes.map(|notes| notes.total_notes),
            coverage: map_coverage(dataset.coverage),
            sampled_notebooks: notes
                .map(|notes| notes.top_books.clone())
                .unwrap_or_default(),
            note_delta: None,
        }
    }

    fn markdown_lines(&self) -> Vec<String> {
        match (self.total_notes, self.total_books) {
            (Some(total), Some(books)) => vec![format!(
                "- {total} notes across {books} books; sampled notebook coverage is {}.",
                self.coverage.label()
            )],
            _ => vec!["- Notebook totals are not available yet.".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseEvidence {
    pub id: String,
    pub source_id: String,
    pub signal: String,
    pub value: Value,
    pub unit: Option<String>,
    pub period: PulsePeriod,
    pub observed_at: DateTime<Utc>,
    pub freshness: PulseFreshness,
    pub comparator: Option<Value>,
}

impl PulseEvidence {
    fn markdown_line(&self) -> String {
        let unit = self
            .unit
            .as_deref()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        let value = match &self.value {
            Value::String(value) => value.clone(),
            value => value.to_string(),
        };
        format!("- `{}`: {value}{unit}.", self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseInsight {
    pub id: String,
    pub level: SignalLevel,
    pub title: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseRecommendation {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalLevel {
    Low,
    Medium,
    High,
    Unknown,
}

impl SignalLevel {
    pub fn label(self) -> &'static str {
        match self {
            SignalLevel::Low => "low",
            SignalLevel::Medium => "medium",
            SignalLevel::High => "high",
            SignalLevel::Unknown => "unknown",
        }
    }
}

fn snapshot_period() -> PulsePeriod {
    let today = Local::now().date_naive();
    let start = week_start_for(today);
    let end_exclusive = start
        .checked_add_signed(ChronoDuration::days(7))
        .unwrap_or(start);

    PulsePeriod {
        kind: PulsePeriodKind::Week,
        start,
        end_exclusive,
        timezone: Local::now().offset().to_string(),
    }
}

fn build_source_health(
    ai: &AiWorkInput,
    local_observed_at: Option<DateTime<Utc>>,
    quota_observed_at: Option<DateTime<Utc>>,
    reading: &WeReadSyncState,
    generated_at: DateTime<Utc>,
) -> Vec<SourceHealth> {
    let ai_freshness = if ai.current.is_some() {
        PulseFreshness::Fresh
    } else {
        PulseFreshness::Missing
    };
    let quota_freshness = quota_source_freshness(
        !ai.quota_sources.is_empty(),
        quota_observed_at,
        generated_at,
    );
    let observed_at = reading.datasets.current_week.observed_at;
    let issue = first_weread_issue(reading);
    let freshness = map_freshness(reading.datasets.current_week.freshness);
    let coverage = match reading.status {
        WeReadStatus::Partial
        | WeReadStatus::AuthMissing
        | WeReadStatus::Error
        | WeReadStatus::UpgradeRequired => {
            if reading.datasets.current_week.value.is_some() {
                PulseCoverage::Partial
            } else {
                PulseCoverage::Unknown
            }
        }
        WeReadStatus::Loading | WeReadStatus::Fresh | WeReadStatus::Stale => {
            map_coverage(reading.datasets.current_week.coverage)
        }
    };

    vec![
        SourceHealth {
            id: "local-ai-usage".to_string(),
            status: ai_freshness.label().to_string(),
            transport: "local_files".to_string(),
            storage: "local".to_string(),
            connector_version: None,
            observed_at: local_observed_at,
            freshness: ai_freshness,
            coverage: if ai.current.is_some() {
                PulseCoverage::Complete
            } else {
                PulseCoverage::Unknown
            },
            issue_code: None,
        },
        SourceHealth {
            id: "subscription-usage-cache".to_string(),
            status: quota_freshness.label().to_string(),
            transport: "local_cache".to_string(),
            storage: "local".to_string(),
            connector_version: None,
            observed_at: quota_observed_at,
            freshness: quota_freshness,
            coverage: match quota_freshness {
                PulseFreshness::Fresh => PulseCoverage::Complete,
                PulseFreshness::Stale => PulseCoverage::Partial,
                PulseFreshness::Missing => PulseCoverage::Unknown,
            },
            issue_code: (quota_freshness == PulseFreshness::Stale)
                .then(|| "stale_cache".to_string()),
        },
        SourceHealth {
            id: "weread".to_string(),
            status: reading.status.label().to_string(),
            transport: "remote_gateway".to_string(),
            storage: "local_normalized".to_string(),
            connector_version: Some(SKILL_VERSION.to_string()),
            observed_at,
            freshness,
            coverage,
            issue_code: issue.map(|issue| issue_code_label(issue.code).to_string()),
        },
    ]
}

fn quota_source_freshness(
    has_data: bool,
    observed_at: Option<DateTime<Utc>>,
    generated_at: DateTime<Utc>,
) -> PulseFreshness {
    if !has_data {
        return PulseFreshness::Missing;
    }
    let Some(observed_at) = observed_at else {
        return PulseFreshness::Stale;
    };
    let age = generated_at
        .signed_duration_since(observed_at)
        .num_seconds();
    if (-SOURCE_FUTURE_TOLERANCE_SECS..=QUOTA_CACHE_FRESHNESS_SECS).contains(&age) {
        PulseFreshness::Fresh
    } else {
        PulseFreshness::Stale
    }
}

fn build_evidence(
    period: &PulsePeriod,
    sources: &[SourceHealth],
    ai: &AiPulse,
    reading: &ReadingPulse,
    knowledge: &KnowledgeFlowSignal,
    reading_state: &WeReadSyncState,
    observed_at: DateTime<Utc>,
) -> Vec<PulseEvidence> {
    let mut evidence = Vec::new();
    let mut push = |id: &str,
                    source_id: &str,
                    signal: &str,
                    value: Value,
                    unit: Option<&str>,
                    comparator: Option<Value>| {
        let source = sources.iter().find(|source| source.id == source_id);
        evidence.push(PulseEvidence {
            id: id.to_string(),
            source_id: source_id.to_string(),
            signal: signal.to_string(),
            value,
            unit: unit.map(str::to_string),
            period: period.clone(),
            observed_at: source
                .and_then(|source| source.observed_at)
                .unwrap_or(observed_at),
            freshness: source
                .map(|source| source.freshness)
                .unwrap_or(PulseFreshness::Missing),
            comparator,
        });
    };

    if let Some(tokens) = ai.total_tokens {
        push(
            "ai.work.total_tokens",
            "local-ai-usage",
            "weekly_total_tokens",
            json!(tokens),
            Some("tokens"),
            ai.previous_total_tokens.map(|value| json!(value)),
        );
    }
    if let Some(cost) = ai.total_cost {
        push(
            "ai.work.total_cost",
            "local-ai-usage",
            "weekly_total_cost",
            json!(cost),
            Some("USD"),
            ai.previous_total_cost.map(|value| json!(value)),
        );
    }
    if let Some(ratio) = ai.cost_change_ratio {
        push(
            "ai.work.cost_change_ratio",
            "local-ai-usage",
            "weekly_cost_change_ratio",
            json!(ratio),
            Some("ratio"),
            Some(json!(AI_COST_INCREASE_THRESHOLD_RATIO)),
        );
    }
    if let Some(used) = ai.max_used_percent {
        push(
            "ai.quota.max_used_percent",
            "subscription-usage-cache",
            "max_used_percent",
            json!(used),
            Some("percent"),
            Some(json!({"medium": 70.0, "high": 90.0})),
        );
    }
    if let Some(days) = reading.read_days {
        push(
            "reading.week.read_days",
            "weread",
            "weekly_read_days",
            json!(days),
            Some("days"),
            reading.previous_read_days.map(|value| json!(value)),
        );
    }
    if let Some(seconds) = reading.weekly_total_seconds {
        push(
            "reading.week.total_seconds",
            "weread",
            "weekly_total_seconds",
            json!(seconds),
            Some("seconds"),
            reading
                .previous_weekly_total_seconds
                .map(|value| json!(value)),
        );
    }
    if let (Some(total), Some(notebooks_observed_at)) = (
        knowledge.total_notes,
        reading_state.datasets.notebooks.observed_at,
    ) {
        evidence.push(PulseEvidence {
            id: "knowledge.notes.total".to_string(),
            source_id: "weread".to_string(),
            signal: "total_notes".to_string(),
            value: json!(total),
            unit: Some("notes".to_string()),
            period: period.clone(),
            observed_at: notebooks_observed_at,
            freshness: map_freshness(reading_state.datasets.notebooks.freshness),
            comparator: None,
        });
    }
    evidence
}

fn build_insights(
    ai: &AiPulse,
    reading: &ReadingPulse,
    evidence: &[PulseEvidence],
) -> Vec<PulseInsight> {
    let ids = evidence
        .iter()
        .map(|evidence| evidence.id.as_str())
        .collect::<HashSet<_>>();
    let mut insights = Vec::new();

    if matches!(ai.quota_risk, SignalLevel::High | SignalLevel::Medium)
        && ids.contains("ai.quota.max_used_percent")
    {
        insights.push(PulseInsight {
            id: "ai.quota.pressure".to_string(),
            level: ai.quota_risk,
            title: "AI quota pressure".to_string(),
            summary: format!("highest cached quota risk is {}", ai.quota_risk.label()),
            evidence_refs: vec!["ai.quota.max_used_percent".to_string()],
        });
    }

    if let (Some(current), Some(previous)) = (reading.read_days, reading.previous_read_days) {
        if current < previous && ids.contains("reading.week.read_days") {
            insights.push(PulseInsight {
                id: "reading.rhythm.change".to_string(),
                level: SignalLevel::Medium,
                title: "Reading rhythm changed".to_string(),
                summary: format!(
                    "reading occurred on {current} days, down from {previous} in the previous week"
                ),
                evidence_refs: vec!["reading.week.read_days".to_string()],
            });
        }
    }

    if ai
        .cost_change_ratio
        .is_some_and(|ratio| ratio >= AI_COST_INCREASE_THRESHOLD_RATIO)
        && ids.contains("ai.work.total_cost")
        && ids.contains("ai.work.cost_change_ratio")
    {
        insights.push(PulseInsight {
            id: "ai.cost.change".to_string(),
            level: SignalLevel::Medium,
            title: "AI cost increased".to_string(),
            summary: format!(
                "weekly cost is {} above the previous week",
                format_compare_ratio(ai.cost_change_ratio)
            ),
            evidence_refs: vec![
                "ai.work.total_cost".to_string(),
                "ai.work.cost_change_ratio".to_string(),
            ],
        });
    }

    insights
}

fn build_recommendations(
    ai: &AiPulse,
    reading: &ReadingPulse,
    insights: &[PulseInsight],
) -> Vec<PulseRecommendation> {
    let mut recommendations = Vec::new();
    if matches!(ai.quota_risk, SignalLevel::High | SignalLevel::Medium) {
        recommendations.push(PulseRecommendation {
            id: "constrain.expensive-runs".to_string(),
            title: "Reserve expensive agent runs for explicit priorities.".to_string(),
            rationale: "Cached quota pressure is elevated.".to_string(),
            evidence_refs: vec!["ai.quota.max_used_percent".to_string()],
        });
    }
    if reading
        .read_days
        .zip(reading.previous_read_days)
        .is_some_and(|(current, previous)| current < previous)
    {
        recommendations.push(PulseRecommendation {
            id: "review.reading-change".to_string(),
            title: "Review what displaced reading input this week.".to_string(),
            rationale: "Reading occurred on fewer days than the previous week.".to_string(),
            evidence_refs: vec!["reading.week.read_days".to_string()],
        });
    }
    if insights.is_empty() {
        recommendations.clear();
    }
    recommendations
}

fn first_weread_issue(state: &WeReadSyncState) -> Option<&super::weread::SourceIssue> {
    state.primary_issue()
}

fn map_freshness(freshness: DatasetFreshness) -> PulseFreshness {
    match freshness {
        DatasetFreshness::Fresh => PulseFreshness::Fresh,
        DatasetFreshness::Stale => PulseFreshness::Stale,
        DatasetFreshness::Missing => PulseFreshness::Missing,
    }
}

fn map_coverage(coverage: DatasetCoverage) -> PulseCoverage {
    match coverage {
        DatasetCoverage::Complete => PulseCoverage::Complete,
        DatasetCoverage::Partial => PulseCoverage::Partial,
        DatasetCoverage::Unknown => PulseCoverage::Unknown,
    }
}

fn issue_code_label(code: SourceIssueCode) -> &'static str {
    match code {
        SourceIssueCode::AuthMissing => "auth_missing",
        SourceIssueCode::UpgradeRequired => "upgrade_required",
        SourceIssueCode::TransportError => "transport_error",
        SourceIssueCode::GatewayError => "gateway_error",
        SourceIssueCode::InvalidResponse => "invalid_response",
        SourceIssueCode::NormalizationError => "normalization_error",
    }
}

fn ratio_change(current: f64, previous: f64) -> Option<f64> {
    if !current.is_finite() || !previous.is_finite() || previous <= 0.0 {
        return None;
    }
    Some((current - previous) / previous)
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn content_snapshot_id(
    period: &PulsePeriod,
    ai: &AiPulse,
    reading: &ReadingPulse,
    knowledge: &KnowledgeFlowSignal,
    evidence: &[PulseEvidence],
) -> String {
    let evidence_content = evidence
        .iter()
        .map(|item| {
            (
                &item.id,
                &item.source_id,
                &item.signal,
                &item.value,
                &item.unit,
                &item.comparator,
            )
        })
        .collect::<Vec<_>>();
    let bytes =
        serde_json::to_vec(&(period, ai, reading, knowledge, evidence_content)).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    let suffix = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("pulse-{}-{suffix}", period.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::weread::{
        Dataset, WeReadDatasets, WeReadDay, WeReadFocusBook, WeReadMonthly, WeReadNotesSummary,
        WeReadWeekly,
    };

    fn reading_fixture() -> WeReadSyncState {
        let start = week_start_for(Local::now().date_naive());
        let previous_start = start.checked_sub_signed(ChronoDuration::days(7)).unwrap();
        let weekly = |period_start: NaiveDate, read_days: u8, total_seconds: u32| WeReadWeekly {
            period_start,
            period_end: period_start
                .checked_add_signed(ChronoDuration::days(6))
                .unwrap(),
            read_days,
            total_seconds,
            day_average_seconds: total_seconds / 7,
            compare_ratio: None,
            days: std::array::from_fn(|index| {
                WeReadDay::new(
                    period_start
                        .checked_add_signed(ChronoDuration::days(index as i64))
                        .unwrap(),
                    if index < read_days as usize { 600 } else { 0 },
                )
            }),
            focus: Some(WeReadFocusBook {
                id: "book-1".to_string(),
                title: "Systems".to_string(),
                author: None,
                read_seconds: 1200,
            }),
        };
        let observed_at = Utc::now();
        let mut state = WeReadSyncState {
            datasets: WeReadDatasets {
                current_week: Dataset::success(
                    weekly(start, 3, 3600),
                    observed_at,
                    DatasetCoverage::Complete,
                ),
                previous_week: Dataset::success(
                    weekly(previous_start, 5, 5400),
                    observed_at,
                    DatasetCoverage::Complete,
                ),
                ..WeReadDatasets::default()
            },
            status: WeReadStatus::Partial,
            last_attempt_at: Some(observed_at),
        };
        state.status = state.derive_status();
        state
    }

    fn ai_fixture() -> AiWorkInput {
        AiWorkInput {
            current: Some(AiWorkPeriodInput {
                total_tokens: 1_000,
                total_cost: 12.0,
                active_days: 4,
                peak_day: NaiveDate::from_ymd_opt(2026, 7, 8),
                peak_day_tokens: 500,
                leading_model: Some("gpt-5".to_string()),
                leading_provider: Some("openai".to_string()),
            }),
            previous: Some(AiWorkPeriodInput {
                total_tokens: 800,
                total_cost: 8.0,
                active_days: 3,
                peak_day: None,
                peak_day_tokens: 0,
                leading_model: None,
                leading_provider: None,
            }),
            quota_sources: vec![AiQuotaSource {
                provider: "Codex".to_string(),
                metrics: vec![AiQuotaMetric {
                    label: "weekly".to_string(),
                    used_percent: 75.0,
                }],
            }],
            observed_at: Some(Utc::now()),
        }
    }

    #[test]
    fn ai_quota_risk_uses_highest_metric() {
        let ai = AiPulse::from_input(&ai_fixture());

        assert_eq!(ai.quota_risk, SignalLevel::Medium);
        assert_eq!(ai.max_provider.as_deref(), Some("Codex"));
        assert_eq!(ai.total_tokens, Some(1_000));
    }

    #[test]
    fn snapshot_references_existing_evidence() {
        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading_fixture());

        assert_eq!(snapshot.schema_version, PULSE_SCHEMA_VERSION);
        assert!(snapshot.validate_evidence_refs());
        assert!(snapshot
            .evidence
            .iter()
            .any(|evidence| evidence.id == "reading.week.read_days"));
    }

    #[test]
    fn ai_sources_preserve_distinct_observation_times() {
        let local_observed_at = Utc::now() - ChronoDuration::minutes(2);
        let quota_observed_at = Utc::now() - ChronoDuration::minutes(1);
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(local_observed_at),
            Some(quota_observed_at),
            reading_fixture(),
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
    fn expired_quota_cache_is_stale_with_partial_coverage() {
        let observed_at = Utc::now() - ChronoDuration::minutes(10);
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(Utc::now()),
            Some(observed_at),
            reading_fixture(),
        );
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();

        assert_eq!(source.observed_at, Some(observed_at));
        assert_eq!(source.freshness, PulseFreshness::Stale);
        assert_eq!(source.coverage, PulseCoverage::Partial);
        assert_eq!(source.issue_code.as_deref(), Some("stale_cache"));
    }

    #[test]
    fn presentation_copy_expires_quota_source_without_mutating_snapshot() {
        let observed_at = Utc::now();
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(observed_at),
            Some(observed_at),
            reading_fixture(),
        );
        let now = observed_at + ChronoDuration::minutes(10);
        let serialized_before = serde_json::to_vec(&snapshot).unwrap();
        let snapshot_id = snapshot.snapshot_id.clone();
        let generated_at = snapshot.generated_at;

        let presentation = snapshot.for_presentation_at(now);

        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), serialized_before);
        assert_eq!(snapshot.snapshot_id, snapshot_id);
        assert_eq!(snapshot.generated_at, generated_at);
        assert_eq!(presentation.snapshot_id, snapshot_id);
        assert_eq!(presentation.generated_at, generated_at);

        let source = presentation
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();
        assert_eq!(source.freshness, PulseFreshness::Stale);
        assert_eq!(source.coverage, PulseCoverage::Partial);
        assert_eq!(source.issue_code.as_deref(), Some("stale_cache"));
        let quota_evidence = presentation
            .evidence
            .iter()
            .filter(|evidence| evidence.source_id == "subscription-usage-cache")
            .collect::<Vec<_>>();
        assert!(!quota_evidence.is_empty());
        assert!(quota_evidence
            .iter()
            .all(|evidence| evidence.freshness == PulseFreshness::Stale));
    }

    #[test]
    fn presentation_copy_is_deterministic_for_the_same_time() {
        let observed_at = Utc::now();
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(observed_at),
            Some(observed_at),
            reading_fixture(),
        );
        let now = observed_at + ChronoDuration::minutes(10);

        let first = serde_json::to_vec(&snapshot.for_presentation_at(now)).unwrap();
        let second = serde_json::to_vec(&snapshot.for_presentation_at(now)).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn presentation_copy_honors_quota_freshness_boundaries() {
        let observed_at = Utc::now();
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(observed_at),
            Some(observed_at),
            reading_fixture(),
        );

        for (offset_seconds, expected) in [
            (-61, PulseFreshness::Stale),
            (-60, PulseFreshness::Fresh),
            (300, PulseFreshness::Fresh),
            (301, PulseFreshness::Stale),
        ] {
            let presentation =
                snapshot.for_presentation_at(observed_at + ChronoDuration::seconds(offset_seconds));
            let source = presentation
                .sources
                .iter()
                .find(|source| source.id == "subscription-usage-cache")
                .unwrap();

            assert_eq!(source.freshness, expected, "offset: {offset_seconds}");
        }
    }

    #[test]
    fn presentation_copy_does_not_fabricate_missing_quota_health() {
        let observed_at = Utc::now();
        let mut ai = ai_fixture();
        ai.quota_sources.clear();
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai,
            Some(observed_at),
            Some(observed_at),
            reading_fixture(),
        );

        let presentation = snapshot.for_presentation_at(observed_at + ChronoDuration::minutes(10));
        let source = presentation
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();

        assert_eq!(source.status, "missing");
        assert_eq!(source.freshness, PulseFreshness::Missing);
        assert_eq!(source.coverage, PulseCoverage::Unknown);
        assert_eq!(source.issue_code, None);
        assert!(!presentation
            .evidence
            .iter()
            .any(|evidence| evidence.source_id == "subscription-usage-cache"));
    }

    #[test]
    fn source_degradation_updates_matching_evidence() {
        let now = Utc::now();
        let mut snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(now),
            Some(now),
            reading_fixture(),
        );

        assert!(snapshot.mark_source_degraded("subscription-usage-cache", "cache_write_failed"));

        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();
        assert_eq!(source.freshness, PulseFreshness::Stale);
        assert_eq!(source.issue_code.as_deref(), Some("cache_write_failed"));
        let quota_evidence = snapshot
            .evidence
            .iter()
            .filter(|evidence| evidence.source_id == "subscription-usage-cache")
            .collect::<Vec<_>>();
        assert!(!quota_evidence.is_empty());
        assert!(quota_evidence
            .iter()
            .all(|evidence| evidence.freshness == PulseFreshness::Stale));
    }

    #[test]
    fn quota_without_trustworthy_generation_is_not_reported_as_fresh() {
        let snapshot = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(Utc::now()),
            None,
            reading_fixture(),
        );
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();

        assert_eq!(source.freshness, PulseFreshness::Stale);
        assert_eq!(source.coverage, PulseCoverage::Partial);
    }

    #[test]
    fn legacy_shared_ai_observation_time_applies_to_both_sources() {
        let observed_at = Utc::now() - ChronoDuration::minutes(1);
        let input: AiWorkInput = serde_json::from_value(json!({
            "current": ai_fixture().current,
            "previous": null,
            "quotaSources": ai_fixture().quota_sources,
            "observedAt": observed_at,
        }))
        .unwrap();

        let snapshot = PulseSnapshotV1::from_inputs(input, reading_fixture());

        for source_id in ["local-ai-usage", "subscription-usage-cache"] {
            assert_eq!(
                snapshot
                    .sources
                    .iter()
                    .find(|source| source.id == source_id)
                    .and_then(|source| source.observed_at),
                Some(observed_at)
            );
        }
    }

    #[test]
    fn markdown_contains_review_sections_and_snapshot_id() {
        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading_fixture());
        let markdown = snapshot.to_markdown();

        assert!(markdown.contains("# Weekly Pulse"));
        assert!(markdown.contains("## AI Work"));
        assert!(markdown.contains("## Reading Input"));
        assert!(markdown.contains("## Evidence"));
        assert!(markdown.contains(&snapshot.snapshot_id));
    }

    #[test]
    fn content_snapshot_id_is_stable_for_same_inputs() {
        let ai = ai_fixture();
        let reading = reading_fixture();
        let first = PulseSnapshotV1::from_inputs(ai.clone(), reading.clone());
        let second = PulseSnapshotV1::from_inputs(ai, reading);

        assert_eq!(first.snapshot_id, second.snapshot_id);
    }

    #[test]
    fn old_weread_week_does_not_reanchor_current_snapshot() {
        let current_start = week_start_for(Local::now().date_naive());
        let old_start = current_start
            .checked_sub_signed(ChronoDuration::days(7))
            .unwrap();
        let old_week = WeReadWeekly {
            period_start: old_start,
            period_end: old_start
                .checked_add_signed(ChronoDuration::days(6))
                .unwrap(),
            read_days: 3,
            total_seconds: 1_800,
            day_average_seconds: 257,
            compare_ratio: None,
            days: std::array::from_fn(|index| {
                WeReadDay::new(
                    old_start
                        .checked_add_signed(ChronoDuration::days(index as i64))
                        .unwrap(),
                    300,
                )
            }),
            focus: None,
        };
        let mut reading = WeReadSyncState::default();
        reading.datasets.current_week =
            Dataset::success(old_week, Utc::now(), DatasetCoverage::Complete);
        reading.refresh_freshness_at(Utc::now());

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);

        assert_eq!(snapshot.period.start, current_start);
        assert_eq!(snapshot.reading.weekly_total_seconds, None);
        assert!(snapshot.reading.days.is_empty());
    }

    #[test]
    fn serialized_snapshot_exposes_issue_code_not_gateway_message() {
        let mut reading = reading_fixture();
        reading.datasets.current_week.issue = Some(crate::pulse::weread::SourceIssue::new(
            SourceIssueCode::GatewayError,
            "account user@example.com failed at https://private.example",
            crate::pulse::weread::RetryClassification::Retryable,
        ));
        reading.status = reading.derive_status();

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);
        let json = serde_json::to_string(&snapshot).unwrap();

        assert!(json.contains("\"issueCode\":\"gateway_error\""));
        assert!(!json.contains("user@example.com"));
        assert!(!json.contains("private.example"));
    }

    #[test]
    fn reading_refresh_preserves_current_durable_ai_signal() {
        let local_observed_at = Utc::now() - ChronoDuration::minutes(2);
        let quota_observed_at = Utc::now() - ChronoDuration::minutes(1);
        let existing = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(local_observed_at),
            Some(quota_observed_at),
            WeReadSyncState::default(),
        );
        let refreshed = PulseSnapshotV1::with_refreshed_reading(Some(&existing), reading_fixture());

        assert_eq!(refreshed.ai.total_tokens, existing.ai.total_tokens);
        assert_eq!(refreshed.ai.total_cost, existing.ai.total_cost);
        assert_eq!(refreshed.ai.leading_model, existing.ai.leading_model);
        assert_eq!(refreshed.reading.weekly_total_seconds, Some(3_600));
        for source_id in ["local-ai-usage", "subscription-usage-cache"] {
            let observed_at = |snapshot: &PulseSnapshotV1| {
                snapshot
                    .sources
                    .iter()
                    .find(|source| source.id == source_id)
                    .and_then(|source| source.observed_at)
            };
            assert_eq!(observed_at(&refreshed), observed_at(&existing));
        }
    }

    #[test]
    fn reading_refresh_expires_copied_quota_health() {
        let now = Utc::now();
        let mut existing = PulseSnapshotV1::from_inputs_with_source_observed_at(
            ai_fixture(),
            Some(now),
            Some(now),
            WeReadSyncState::default(),
        );
        existing
            .sources
            .iter_mut()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap()
            .observed_at = Some(now - ChronoDuration::minutes(10));

        let refreshed = PulseSnapshotV1::with_refreshed_reading(Some(&existing), reading_fixture());
        let quota = refreshed
            .sources
            .iter()
            .find(|source| source.id == "subscription-usage-cache")
            .unwrap();

        assert_eq!(quota.freshness, PulseFreshness::Stale);
        assert_eq!(quota.issue_code.as_deref(), Some("stale_cache"));
        assert!(refreshed
            .evidence
            .iter()
            .filter(|evidence| evidence.source_id == "subscription-usage-cache")
            .all(|evidence| evidence.freshness == PulseFreshness::Stale));
    }

    #[test]
    fn week_over_week_uses_total_seconds_when_previous_week_exists() {
        let mut reading = reading_fixture();
        reading
            .datasets
            .current_week
            .value
            .as_mut()
            .unwrap()
            .compare_ratio = Some(9.0);

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);
        let ratio = snapshot.reading.week_over_week_ratio.unwrap();
        let total_evidence = snapshot
            .evidence
            .iter()
            .find(|evidence| evidence.id == "reading.week.total_seconds")
            .unwrap();

        assert!((ratio - (-1.0 / 3.0)).abs() < f64::EPSILON);
        assert_eq!(snapshot.reading.week_over_week.as_deref(), Some("-33%"));
        assert_eq!(total_evidence.value, json!(3_600));
        assert_eq!(total_evidence.comparator, Some(json!(5_400)));
    }

    #[test]
    fn prior_month_cache_is_not_projected_as_current_month() {
        let mut reading = reading_fixture();
        let monthly = WeReadMonthly {
            read_days: 4,
            total_seconds: 1_200,
            day_average_seconds: 300,
            prefer_category_word: Some("technology".to_string()),
            categories: Vec::new(),
        };
        reading.datasets.current_month =
            Dataset::success(monthly.clone(), Utc::now(), DatasetCoverage::Complete);

        let current = PulseSnapshotV1::from_inputs(ai_fixture(), reading.clone());
        assert_eq!(current.reading.month_read_days, Some(4));

        reading.datasets.current_month = Dataset::success(
            monthly,
            Utc::now() - ChronoDuration::days(40),
            DatasetCoverage::Complete,
        );
        let retained = PulseSnapshotV1::from_inputs(ai_fixture(), reading);

        assert_eq!(retained.reading.month_read_days, None);
        assert_eq!(retained.reading.month_total_label, None);
        assert_eq!(retained.reading.preferred_category, None);
    }

    #[test]
    fn notebook_evidence_uses_notebook_dataset_provenance() {
        let mut reading = reading_fixture();
        let notebooks_observed_at = Utc::now() - ChronoDuration::hours(2);
        reading.datasets.notebooks = Dataset::success(
            WeReadNotesSummary {
                total_books: 2,
                total_notes: 8,
                top_books: Vec::new(),
            },
            notebooks_observed_at,
            DatasetCoverage::Complete,
        );
        reading.refresh_freshness_at(Utc::now());

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);
        let evidence = snapshot
            .evidence
            .iter()
            .find(|evidence| evidence.id == "knowledge.notes.total")
            .unwrap();

        assert_eq!(evidence.observed_at, notebooks_observed_at);
        assert_eq!(evidence.freshness, PulseFreshness::Stale);
    }

    #[test]
    fn ai_cost_insight_references_threshold_evidence() {
        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading_fixture());
        let evidence = snapshot
            .evidence
            .iter()
            .find(|evidence| evidence.id == "ai.work.cost_change_ratio")
            .unwrap();
        let insight = snapshot
            .insights
            .iter()
            .find(|insight| insight.id == "ai.cost.change")
            .unwrap();

        assert_eq!(evidence.value, json!(0.5));
        assert_eq!(
            evidence.comparator,
            Some(json!(AI_COST_INCREASE_THRESHOLD_RATIO))
        );
        assert!(insight
            .evidence_refs
            .contains(&"ai.work.cost_change_ratio".to_string()));
    }

    #[test]
    fn source_issue_code_matches_upgrade_blocking_status() {
        let mut reading = reading_fixture();
        reading.datasets.current_week.issue = Some(crate::pulse::weread::SourceIssue::new(
            SourceIssueCode::GatewayError,
            "gateway unavailable",
            crate::pulse::weread::RetryClassification::Retryable,
        ));
        reading.datasets.current_month.issue = Some(
            crate::pulse::weread::SourceIssue::upgrade_required("upgrade connector"),
        );
        reading.status = reading.derive_status();

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "weread")
            .unwrap();

        assert_eq!(source.status, "upgrade required");
        assert_eq!(source.coverage, PulseCoverage::Partial);
        assert_eq!(source.issue_code.as_deref(), Some("upgrade_required"));
    }

    #[test]
    fn blocked_weread_without_cached_data_has_unknown_coverage() {
        let mut reading = WeReadSyncState::default();
        reading.mark_auth_missing(Utc::now());

        let snapshot = PulseSnapshotV1::from_inputs(ai_fixture(), reading);
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == "weread")
            .unwrap();

        assert_eq!(source.status, "auth missing");
        assert_eq!(source.coverage, PulseCoverage::Unknown);
        assert_eq!(source.issue_code.as_deref(), Some("auth_missing"));
    }
}
