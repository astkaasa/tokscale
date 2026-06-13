use anyhow::Result;
use serde::Serialize;

use crate::commands::usage;
use crate::tui::integrations::weread::{
    self, format_compare_ratio, format_read_duration, WeReadState,
};
use crate::tui::settings::Settings;

pub fn run(json: bool) -> Result<()> {
    let settings = Settings::load();
    let summary = PulseSummary::collect(&settings);

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("{}", summary.to_markdown());
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PulseSummary {
    generated_at_ms: u64,
    ai: AiPulse,
    reading: ReadingPulse,
    insights: Vec<PulseInsight>,
    recommended_constraints: Vec<String>,
}

impl PulseSummary {
    fn collect(settings: &Settings) -> Self {
        let ai = AiPulse::from_usage_cache(usage::load_cache().unwrap_or_default());
        let reading = ReadingPulse::from_weread(load_weread(settings));
        let insights = build_insights(&ai, &reading);
        let recommended_constraints = build_recommended_constraints(&ai, &reading);

        Self {
            generated_at_ms: weread::now_millis(),
            ai,
            reading,
            insights,
            recommended_constraints,
        }
    }

    fn to_markdown(&self) -> String {
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
struct AiPulse {
    source: &'static str,
    quota_risk: SignalLevel,
    provider_count: usize,
    max_used_percent: Option<f64>,
    max_metric_label: Option<String>,
    max_provider: Option<String>,
}

impl AiPulse {
    fn from_usage_cache(outputs: Vec<usage::UsageOutput>) -> Self {
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
struct ReadingPulse {
    source: &'static str,
    status: String,
    read_days: Option<u8>,
    weekly_total_seconds: Option<u32>,
    weekly_total_label: Option<String>,
    daily_average_label: Option<String>,
    week_over_week: Option<String>,
    focus_book: Option<String>,
    month_total_label: Option<String>,
    month_read_days: Option<u16>,
    preferred_category: Option<String>,
    notes_total: Option<u32>,
    note_books: Option<u32>,
    error: Option<String>,
}

impl ReadingPulse {
    fn from_weread(state: WeReadState) -> Self {
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
struct PulseInsight {
    title: String,
    summary: String,
    evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SignalLevel {
    Low,
    Medium,
    High,
    Unknown,
}

impl SignalLevel {
    fn label(self) -> &'static str {
        match self {
            SignalLevel::Low => "low",
            SignalLevel::Medium => "medium",
            SignalLevel::High => "high",
            SignalLevel::Unknown => "unknown",
        }
    }
}

fn load_weread(settings: &Settings) -> WeReadState {
    let cached = crate::tui::integrations::weread::cache::load();
    let should_refresh = cached
        .as_ref()
        .is_none_or(|state| !state.has_data() || state.is_stale_at(weread::now_millis()));

    if !should_refresh {
        return cached.unwrap_or_default();
    }

    let Some(api_key) = settings.env_value("WEREAD_API_KEY") else {
        let mut state = cached.unwrap_or_default();
        state.mark_auth_missing();
        return state;
    };

    match weread::fetch_current(&api_key) {
        Ok(state) => state,
        Err(error) => {
            let mut state = cached.unwrap_or_default();
            state.mark_error(sanitize_error(error, &api_key));
            state
        }
    }
}

fn sanitize_error(error: anyhow::Error, secret: &str) -> String {
    let mut message = error.to_string();
    let secret = secret.trim();
    if !secret.is_empty() {
        message = message.replace(secret, "[redacted]");
    }
    message
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
        let ai = AiPulse::from_usage_cache(vec![usage::UsageOutput {
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
        }]);

        assert_eq!(ai.quota_risk, SignalLevel::Medium);
        assert_eq!(ai.max_provider.as_deref(), Some("Codex"));
    }

    #[test]
    fn markdown_contains_core_sections() {
        let summary = PulseSummary {
            generated_at_ms: 1,
            ai: AiPulse::from_usage_cache(Vec::new()),
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
