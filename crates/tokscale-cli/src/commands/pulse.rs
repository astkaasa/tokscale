use anyhow::Result;
use tokscale_core::pulse::weread::{self, WeReadState};
use tokscale_core::pulse::{AiQuotaMetric, AiQuotaSource, PulseSummary};

use crate::commands::usage;
use crate::tui::settings::Settings;

pub fn run(json: bool) -> Result<()> {
    let settings = Settings::load();
    let summary = PulseSummary::collect(load_ai_quota_sources(), load_weread(&settings));

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("{}", summary.to_markdown());
    }

    Ok(())
}

fn load_ai_quota_sources() -> Vec<AiQuotaSource> {
    usage::load_cache()
        .unwrap_or_default()
        .into_iter()
        .map(|output| AiQuotaSource {
            provider: output.provider,
            metrics: output
                .metrics
                .into_iter()
                .map(|metric| AiQuotaMetric {
                    label: metric.label,
                    used_percent: metric.used_percent,
                })
                .collect(),
        })
        .collect()
}

fn load_weread(settings: &Settings) -> WeReadState {
    let cached = weread::cache::load();
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokscale_core::pulse::SignalLevel;

    #[test]
    fn adapts_usage_cache_to_ai_quota_sources() {
        let source = AiQuotaSource {
            provider: "Codex".to_string(),
            metrics: vec![AiQuotaMetric {
                label: "weekly".to_string(),
                used_percent: 75.0,
            }],
        };
        let summary = PulseSummary::collect(vec![source], WeReadState::default());

        assert_eq!(summary.ai.quota_risk, SignalLevel::Medium);
        assert_eq!(summary.ai.max_provider.as_deref(), Some("Codex"));
    }

    #[test]
    fn markdown_contains_core_sections() {
        let summary = PulseSummary::collect(Vec::new(), WeReadState::default());

        let markdown = summary.to_markdown();
        assert!(markdown.contains("# Weekly Pulse"));
        assert!(markdown.contains("## AI Work"));
        assert!(markdown.contains("## Reading"));
    }
}
