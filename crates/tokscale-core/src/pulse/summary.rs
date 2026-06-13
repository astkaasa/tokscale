use serde::Serialize;

use super::weread::{format_compare_ratio, format_read_duration, now_millis, WeReadState};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseSummary {
    pub generated_at_ms: u64,
    pub ai: AiPulse,
    pub reading: ReadingPulse,
    pub insights: Vec<PulseInsight>,
    pub recommended_constraints: Vec<String>,
}

impl PulseSummary {
    pub fn collect(ai_sources: Vec<AiQuotaSource>, reading_state: WeReadState) -> Self {
        let ai = AiPulse::from_quota_sources(ai_sources);
        let reading = ReadingPulse::from_weread(reading_state);
        let insights = build_insights(&ai, &reading);
        let recommended_constraints = build_recommended_constraints(&ai, &reading);

        Self {
            generated_at_ms: now_millis(),
            ai,
            reading,
            insights,
            recommended_constraints,
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut lines = vec![
            "# Weekly Pulse".to_string(),
            String::new(),
            "## AI Work".to_string(),
        ];

        lines.extend(self.ai.markdown_lines());
        lines.push(String::new());
        lines.push("## Reading".to_string());
        lines.extend(self.reading.markdown_lines());
        lines.push(String::new());
        lines.push("## Balance".to_string());

        if self.insights.is_empty() {
            lines.push(
                "- Not enough cached signal yet to compare AI output and reading input."
                    .to_string(),
            );
        } else {
            for insight in &self.insights {
                lines.push(format!("- {}: {}", insight.title, insight.summary));
            }
        }

        lines.push(String::new());
        lines.push("## Suggested Actions".to_string());
        if self.recommended_constraints.is_empty() {
            lines.push(
                "- Keep collecting local signals before enabling stronger recommendations."
                    .to_string(),
            );
        } else {
            for action in &self.recommended_constraints {
                lines.push(format!("- {action}"));
            }
        }

        lines.push(String::new());
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQuotaSource {
    pub provider: String,
    pub metrics: Vec<AiQuotaMetric>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQuotaMetric {
    pub label: String,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPulse {
    pub source: &'static str,
    pub quota_risk: SignalLevel,
    pub provider_count: usize,
    pub max_used_percent: Option<f64>,
    pub max_metric_label: Option<String>,
    pub max_provider: Option<String>,
}

impl AiPulse {
    pub fn from_quota_sources(outputs: Vec<AiQuotaSource>) -> Self {
        let mut max: Option<(String, String, f64)> = None;
        let mut provider_count = 0usize;

        for output in outputs {
            provider_count += 1;
            for metric in output.metrics {
                if !metric.used_percent.is_finite() {
                    continue;
                }
                let replace = max
                    .as_ref()
                    .is_none_or(|(_, _, current)| metric.used_percent > *current);
                if replace {
                    max = Some((output.provider.clone(), metric.label, metric.used_percent));
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

        Self {
            source: "subscription-usage-cache",
            quota_risk,
            provider_count,
            max_used_percent,
            max_metric_label: max.as_ref().map(|(_, label, _)| label.clone()),
            max_provider: max.map(|(provider, _, _)| provider),
        }
    }

    fn markdown_lines(&self) -> Vec<String> {
        if self.provider_count == 0 {
            return vec![
                "- No cached subscription usage found.".to_string(),
                "- Open the Usage TUI tab or run `tokscale usage` before generating the next digest.".to_string(),
            ];
        }

        let mut lines = vec![format!(
            "- Quota risk: {} across {} cached provider{}.",
            self.quota_risk.label(),
            self.provider_count,
            if self.provider_count == 1 { "" } else { "s" }
        )];

        if let (Some(provider), Some(label), Some(used)) = (
            &self.max_provider,
            &self.max_metric_label,
            self.max_used_percent,
        ) {
            lines.push(format!(
                "- Highest usage: {provider} {label} at {:.0}% used.",
                used
            ));
        }

        lines
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPulse {
    pub source: &'static str,
    pub status: String,
    pub read_days: Option<u8>,
    pub weekly_total_seconds: Option<u32>,
    pub weekly_total_label: Option<String>,
    pub daily_average_label: Option<String>,
    pub week_over_week: Option<String>,
    pub focus_book: Option<String>,
    pub month_total_label: Option<String>,
    pub month_read_days: Option<u16>,
    pub preferred_category: Option<String>,
    pub notes_total: Option<u32>,
    pub note_books: Option<u32>,
    pub error: Option<String>,
}

impl ReadingPulse {
    pub fn from_weread(state: WeReadState) -> Self {
        let weekly = state.weekly.as_ref();
        let monthly = state.monthly.as_ref();
        let notes = state.notes.as_ref();

        Self {
            source: "weread",
            status: state.status.label().to_string(),
            read_days: weekly.map(|weekly| weekly.read_days),
            weekly_total_seconds: weekly.map(|weekly| weekly.total_seconds),
            weekly_total_label: weekly.map(|weekly| format_read_duration(weekly.total_seconds)),
            daily_average_label: weekly
                .map(|weekly| format_read_duration(weekly.day_average_seconds)),
            week_over_week: weekly
                .and_then(|weekly| weekly.compare_ratio)
                .map(|ratio| format_compare_ratio(Some(ratio))),
            focus_book: weekly
                .and_then(|weekly| weekly.focus.as_ref())
                .map(|focus| focus.title.clone()),
            month_total_label: monthly.map(|monthly| format_read_duration(monthly.total_seconds)),
            month_read_days: monthly.map(|monthly| monthly.read_days),
            preferred_category: monthly.and_then(|monthly| monthly.prefer_category_word.clone()),
            notes_total: notes.map(|notes| notes.total_notes),
            note_books: notes.map(|notes| notes.total_books),
            error: state.error,
        }
    }

    fn markdown_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("- Status: {}.", self.status)];

        if let (Some(days), Some(total), Some(avg)) = (
            self.read_days,
            self.weekly_total_label.as_ref(),
            self.daily_average_label.as_ref(),
        ) {
            let wow = self
                .week_over_week
                .as_deref()
                .map(|value| format!(" ({value} WoW)"))
                .unwrap_or_default();
            lines.push(format!(
                "- This week: {days}/7 days, {total}, avg {avg}/day{wow}."
            ));
        } else {
            lines.push("- No weekly reading data is available yet.".to_string());
        }

        if let Some(book) = &self.focus_book {
            lines.push(format!("- Focus book: {book}."));
        }

        if let (Some(total), Some(books)) = (self.notes_total, self.note_books) {
            lines.push(format!("- Notes: {total} notes across {books} books."));
        }

        if let (Some(total), Some(days)) = (&self.month_total_label, self.month_read_days) {
            let category = self
                .preferred_category
                .as_deref()
                .map(|category| format!(", preference {category}"))
                .unwrap_or_default();
            lines.push(format!(
                "- Month: {total} across {days} read days{category}."
            ));
        }

        if let Some(error) = &self.error {
            lines.push(format!("- Source note: {error}."));
        }

        lines
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseInsight {
    pub title: String,
    pub summary: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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

fn build_insights(ai: &AiPulse, reading: &ReadingPulse) -> Vec<PulseInsight> {
    let mut insights = Vec::new();

    if matches!(ai.quota_risk, SignalLevel::High | SignalLevel::Medium) {
        insights.push(PulseInsight {
            title: "AI quota pressure".to_string(),
            summary: format!("quota risk is {}", ai.quota_risk.label()),
            evidence: ai
                .max_provider
                .as_ref()
                .zip(ai.max_metric_label.as_ref())
                .zip(ai.max_used_percent)
                .map(|((provider, label), used)| {
                    vec![format!("{provider} {label} is at {:.0}% used", used)]
                })
                .unwrap_or_default(),
        });
    }

    if let Some(read_days) = reading.read_days {
        if read_days < 3 {
            insights.push(PulseInsight {
                title: "Reading rhythm weak".to_string(),
                summary: format!("reading happened on {read_days}/7 days"),
                evidence: reading
                    .weekly_total_label
                    .as_ref()
                    .map(|total| vec![format!("weekly reading total is {total}")])
                    .unwrap_or_default(),
            });
        }
    }

    if matches!(ai.quota_risk, SignalLevel::High | SignalLevel::Medium)
        && reading.read_days.is_some_and(|days| days < 4)
    {
        insights.push(PulseInsight {
            title: "Output/input balance".to_string(),
            summary: "AI usage pressure is elevated while reading rhythm is below target"
                .to_string(),
            evidence: vec![
                format!("quota risk is {}", ai.quota_risk.label()),
                format!("reading days: {}/7", reading.read_days.unwrap_or_default()),
            ],
        });
    }

    insights
}

fn build_recommended_constraints(ai: &AiPulse, reading: &ReadingPulse) -> Vec<String> {
    let mut actions = Vec::new();

    if matches!(ai.quota_risk, SignalLevel::High | SignalLevel::Medium) {
        actions.push("Avoid expensive agent runs unless project urgency is explicit.".to_string());
    }

    if reading.read_days.is_some_and(|days| days < 4) {
        actions
            .push("Protect short reading blocks before starting long coding sessions.".to_string());
    }

    if reading.notes_total.is_some_and(|notes| notes > 0) {
        actions.push(
            "Review recent WeRead notes and decide what should become durable knowledge."
                .to_string(),
        );
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_quota_risk_uses_highest_metric() {
        let ai = AiPulse::from_quota_sources(vec![AiQuotaSource {
            provider: "Codex".to_string(),
            metrics: vec![AiQuotaMetric {
                label: "weekly".to_string(),
                used_percent: 75.0,
            }],
        }]);

        assert_eq!(ai.quota_risk, SignalLevel::Medium);
        assert_eq!(ai.max_provider.as_deref(), Some("Codex"));
    }

    #[test]
    fn markdown_contains_core_sections() {
        let summary = PulseSummary {
            generated_at_ms: 1,
            ai: AiPulse::from_quota_sources(Vec::new()),
            reading: ReadingPulse::from_weread(WeReadState::default()),
            insights: Vec::new(),
            recommended_constraints: Vec::new(),
        };

        let markdown = summary.to_markdown();
        assert!(markdown.contains("# Weekly Pulse"));
        assert!(markdown.contains("## AI Work"));
        assert!(markdown.contains("## Reading"));
    }
}
