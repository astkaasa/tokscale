use std::fmt::Write;

use tokscale_core::pulse::{PulseSnapshotV1, SignalLevel};

pub(crate) fn render_weekly_review(snapshot: &PulseSnapshotV1) -> String {
    let mut html = String::new();
    let period_end = snapshot
        .period
        .end_exclusive
        .pred_opt()
        .unwrap_or(snapshot.period.end_exclusive);
    let ai_tokens = snapshot
        .ai
        .total_tokens
        .map(format_tokens)
        .unwrap_or_else(|| "Unavailable".to_string());
    let ai_cost = snapshot
        .ai
        .total_cost
        .map(|cost| format!("${cost:.2}"))
        .unwrap_or_else(|| "Unavailable".to_string());
    let reading_total = snapshot
        .reading
        .weekly_total_label
        .clone()
        .unwrap_or_else(|| "Unavailable".to_string());
    let reading_days = snapshot
        .reading
        .read_days
        .map(|days| format!("{days}/7"))
        .unwrap_or_else(|| "Unavailable".to_string());

    let _ = write!(
        html,
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>Tokscale Weekly Review</title>
<style>
:root {{ color-scheme: light; --bg:#f4f5f6; --surface:#ffffff; --ink:#17191c; --muted:#66707a; --line:#d9dde2; --green:#16784a; --coral:#c1543d; --blue:#2563a8; --amber:#9a6700; }}
@media (prefers-color-scheme:dark) {{ :root {{ color-scheme:dark; --bg:#111315; --surface:#181b1f; --ink:#f1f3f5; --muted:#9da6af; --line:#32373d; --green:#55c98e; --coral:#ee8d76; --blue:#70a7e8; --amber:#d7a83e; }} }}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--ink); font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; letter-spacing:0; }}
a {{ color:var(--blue); text-decoration:none; }} a:hover {{ text-decoration:underline; }}
.shell {{ width:min(1120px,100%); margin:0 auto; padding:0 24px 56px; }}
nav {{ min-height:48px; display:flex; align-items:center; gap:20px; border-bottom:1px solid var(--line); overflow-x:auto; white-space:nowrap; }}
nav strong {{ color:var(--ink); }}
header {{ padding:34px 0 24px; border-bottom:1px solid var(--line); }}
h1 {{ margin:0 0 6px; font:700 30px/1.2 ui-sans-serif,system-ui,sans-serif; letter-spacing:0; }}
.meta {{ color:var(--muted); overflow-wrap:anywhere; }}
.metrics {{ display:grid; grid-template-columns:repeat(4,minmax(0,1fr)); border-bottom:1px solid var(--line); }}
.metric {{ min-width:0; padding:22px 18px 22px 0; }} .metric + .metric {{ border-left:1px solid var(--line); padding-left:18px; }}
.metric span {{ display:block; color:var(--muted); font-size:12px; }} .metric strong {{ display:block; margin-top:5px; font:700 22px/1.2 ui-sans-serif,system-ui,sans-serif; overflow-wrap:anywhere; }}
section {{ padding:28px 0; border-bottom:1px solid var(--line); }}
h2 {{ margin:0 0 16px; font:700 18px/1.3 ui-sans-serif,system-ui,sans-serif; letter-spacing:0; }}
.split {{ display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1fr); gap:36px; }}
.facts {{ margin:0; padding:0; list-style:none; }} .facts li {{ display:flex; justify-content:space-between; gap:18px; padding:8px 0; border-top:1px solid var(--line); }} .facts span:first-child {{ color:var(--muted); }} .facts span:last-child {{ text-align:right; overflow-wrap:anywhere; }}
.rhythm {{ display:grid; grid-template-columns:repeat(7,minmax(0,1fr)); gap:8px; min-width:0; min-height:148px; align-items:end; }}
.day {{ min-width:0; text-align:center; color:var(--muted); font-size:11px; overflow-wrap:anywhere; }} .track {{ height:104px; display:flex; align-items:end; background:color-mix(in srgb,var(--line) 55%,transparent); }} .fill {{ width:100%; min-height:2px; background:var(--green); }} .day strong {{ display:block; margin-top:7px; color:var(--ink); font-weight:600; }}
.attention {{ margin:0; padding:0; list-style:none; }} .attention li {{ padding:14px 0; border-top:1px solid var(--line); }} .attention strong {{ display:block; margin-bottom:3px; }} .attention p {{ margin:0; color:var(--muted); }}
.actions {{ margin:0; padding:0; list-style:none; }} .actions li {{ padding:14px 0; border-top:1px solid var(--line); }} .actions strong {{ display:block; margin-bottom:3px; }} .actions p {{ margin:0; color:var(--muted); overflow-wrap:anywhere; }}
.level-high {{ color:var(--coral); }} .level-medium {{ color:var(--amber); }} .level-low {{ color:var(--green); }} .level-unknown {{ color:var(--muted); }}
.table-wrap {{ overflow-x:auto; }} table {{ width:100%; border-collapse:collapse; }} th,td {{ padding:10px 12px 10px 0; border-top:1px solid var(--line); text-align:left; vertical-align:top; overflow-wrap:anywhere; }} th {{ color:var(--muted); font-weight:500; font-size:12px; }}
code {{ color:var(--blue); font:inherit; }}
footer {{ padding-top:20px; color:var(--muted); font-size:12px; overflow-wrap:anywhere; }}
@media (max-width:720px) {{ .shell {{ padding:0 16px 40px; }} header {{ padding-top:24px; }} h1 {{ font-size:25px; }} .metrics {{ grid-template-columns:repeat(2,minmax(0,1fr)); }} .metric:nth-child(3) {{ border-left:0; border-top:1px solid var(--line); padding-left:0; }} .metric:nth-child(4) {{ border-top:1px solid var(--line); }} .split {{ grid-template-columns:1fr; gap:26px; }} .rhythm {{ gap:4px; }} .facts li {{ align-items:flex-start; }} }}
@media (max-width:480px) {{ nav {{ min-height:0; padding:10px 0; flex-wrap:wrap; gap:8px 16px; overflow-x:visible; white-space:normal; }} nav strong {{ flex-basis:100%; }} .facts li {{ display:grid; grid-template-columns:minmax(0,1fr); gap:2px; }} .facts span:last-child {{ text-align:left; }} }}
@media (max-width:300px) {{ .shell {{ padding:0 8px 32px; }} .metrics {{ grid-template-columns:minmax(0,1fr); }} .metric,.metric + .metric {{ border-left:0; border-top:1px solid var(--line); padding:16px 0; }} .metric:first-child {{ border-top:0; }} .rhythm {{ gap:2px; }} }}
</style>
</head>
<body>
<div class="shell">
<nav><strong>Tokscale</strong><a href="/overview">Overview</a><a href="/review" aria-current="page">Weekly Review</a><a href="/api/v1/pulse">JSON</a><a href="/exports/pulse.md">Markdown</a></nav>
<header><h1>Weekly Review</h1><div class="meta">{} to {} · snapshot <code>{}</code></div></header>
<main>
<div class="metrics">
<div class="metric"><span>AI tokens</span><strong>{}</strong></div>
<div class="metric"><span>AI cost</span><strong>{}</strong></div>
<div class="metric"><span>Reading</span><strong>{}</strong></div>
<div class="metric"><span>Read days</span><strong>{}</strong></div>
</div>"#,
        snapshot.period.start,
        period_end,
        escape_html(&snapshot.snapshot_id),
        escape_html(&ai_tokens),
        escape_html(&ai_cost),
        escape_html(&reading_total),
        escape_html(&reading_days),
    );

    render_work_and_reading(&mut html, snapshot);
    render_attention(&mut html, snapshot);
    render_recommendations(&mut html, snapshot);
    render_evidence(&mut html, snapshot);
    render_source_health(&mut html, snapshot);

    let _ = write!(
        html,
        "</main><footer>Generated {} · local read-only report</footer></div></body></html>",
        escape_html(&snapshot.generated_at.to_rfc3339())
    );
    html
}

fn render_work_and_reading(html: &mut String, snapshot: &PulseSnapshotV1) {
    let model = snapshot
        .ai
        .leading_model
        .as_deref()
        .unwrap_or("Unavailable");
    let provider = snapshot
        .ai
        .leading_provider
        .as_deref()
        .unwrap_or("Unavailable");
    let quota = snapshot
        .ai
        .max_used_percent
        .map(|value| format!("{value:.0}% used"))
        .unwrap_or_else(|| "Unavailable".to_string());
    let focus = snapshot
        .reading
        .focus_book
        .as_deref()
        .unwrap_or("Unavailable");
    let notes = snapshot
        .knowledge_flow
        .total_notes
        .map(|total| total.to_string())
        .unwrap_or_else(|| "Unavailable".to_string());

    let _ = write!(
        html,
        r#"<section><div class="split"><div><h2>AI Work</h2><ul class="facts">
<li><span>Leading model</span><span>{}</span></li>
<li><span>Provider</span><span>{}</span></li>
<li><span>Active days</span><span>{}</span></li>
<li><span>Quota pressure</span><span>{}</span></li>
</ul></div><div><h2>Reading Input</h2><ul class="facts">
<li><span>Focus book</span><span>{}</span></li>
<li><span>Month</span><span>{}</span></li>
<li><span>Notes</span><span>{}</span></li>
<li><span>Source</span><span>{}</span></li>
</ul></div></div></section>"#,
        escape_html(model),
        escape_html(provider),
        snapshot
            .ai
            .active_days
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unavailable".to_string()),
        escape_html(&quota),
        escape_html(focus),
        escape_html(
            snapshot
                .reading
                .month_total_label
                .as_deref()
                .unwrap_or("Unavailable")
        ),
        escape_html(&notes),
        escape_html(&snapshot.reading.status),
    );

    if snapshot.reading.days.is_empty() {
        return;
    }
    let max_seconds = snapshot
        .reading
        .days
        .iter()
        .map(|day| day.read_seconds)
        .max()
        .unwrap_or(1)
        .max(1);
    html.push_str("<section><h2>Reading Rhythm</h2><div class=\"rhythm\">");
    for day in &snapshot.reading.days {
        let height = ((day.read_seconds as f64 / max_seconds as f64) * 100.0).round() as u32;
        let label = day.date.format("%a").to_string();
        let minutes = day.read_seconds / 60;
        let _ = write!(
            html,
            "<div class=\"day\"><div class=\"track\"><div class=\"fill\" style=\"height:{}%\"></div></div><strong>{}</strong>{}m</div>",
            height.max(u32::from(day.read_seconds > 0) * 2),
            escape_html(&label),
            minutes
        );
    }
    html.push_str("</div></section>");
}

fn render_attention(html: &mut String, snapshot: &PulseSnapshotV1) {
    html.push_str("<section><h2>Attention</h2><ul class=\"attention\">");
    if snapshot.insights.is_empty() {
        html.push_str("<li><strong>No evidence-backed attention item</strong><p>Comparable local periods will make change detection more useful.</p></li>");
    } else {
        for insight in &snapshot.insights {
            let _ = write!(
                html,
                "<li><strong class=\"{}\">{}</strong><p>{} · evidence {}</p></li>",
                level_class(insight.level),
                escape_html(&insight.title),
                escape_html(&insight.summary),
                escape_html(&insight.evidence_refs.join(", "))
            );
        }
    }
    html.push_str("</ul></section>");
}

fn render_recommendations(html: &mut String, snapshot: &PulseSnapshotV1) {
    html.push_str("<section><h2>Suggested Actions</h2><ul class=\"actions\">");
    if snapshot.recommendations.is_empty() {
        html.push_str("<li><strong>Keep collecting comparable local periods.</strong></li>");
    } else {
        for recommendation in &snapshot.recommendations {
            let evidence = recommendation.evidence_refs.join(", ");
            let _ = write!(
                html,
                "<li><strong>{}</strong><p>{}",
                escape_html(&recommendation.title),
                escape_html(&recommendation.rationale)
            );
            if !evidence.is_empty() {
                let _ = write!(html, " · evidence {}", escape_html(&evidence));
            }
            html.push_str("</p></li>");
        }
    }
    html.push_str("</ul></section>");
}

fn render_evidence(html: &mut String, snapshot: &PulseSnapshotV1) {
    html.push_str("<section><h2>Evidence</h2><div class=\"table-wrap\"><table><thead><tr><th>ID</th><th>Value</th><th>Source</th></tr></thead><tbody>");
    for evidence in &snapshot.evidence {
        let value = match &evidence.value {
            serde_json::Value::String(value) => value.clone(),
            value => value.to_string(),
        };
        let unit = evidence
            .unit
            .as_deref()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td>{}{}</td><td>{}</td></tr>",
            escape_html(&evidence.id),
            escape_html(&value),
            escape_html(&unit),
            escape_html(&evidence.source_id)
        );
    }
    html.push_str("</tbody></table></div></section>");
}

fn render_source_health(html: &mut String, snapshot: &PulseSnapshotV1) {
    html.push_str("<section><h2>Source Health</h2><div class=\"table-wrap\"><table><thead><tr><th>Source</th><th>Status</th><th>Freshness</th><th>Coverage</th></tr></thead><tbody>");
    for source in &snapshot.sources {
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape_html(&source.id),
            escape_html(&source.status),
            source.freshness.label(),
            source.coverage.label()
        );
    }
    html.push_str("</tbody></table></div></section>");
}

fn level_class(level: SignalLevel) -> &'static str {
    match level {
        SignalLevel::High => "level-high",
        SignalLevel::Medium => "level-medium",
        SignalLevel::Low => "level-low",
        SignalLevel::Unknown => "level-unknown",
    }
}

fn format_tokens(tokens: u64) -> String {
    match tokens {
        value if value >= 1_000_000_000 => format!("{:.1}B", value as f64 / 1_000_000_000.0),
        value if value >= 1_000_000 => format!("{:.1}M", value as f64 / 1_000_000.0),
        value if value >= 1_000 => format!("{:.1}K", value as f64 / 1_000.0),
        value => value.to_string(),
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use tokscale_core::pulse::weread::WeReadSyncState;
    use tokscale_core::pulse::{
        AiWorkInput, AiWorkPeriodInput, PulseRecommendation, PulseSnapshotV1,
    };

    use super::*;

    fn snapshot() -> PulseSnapshotV1 {
        PulseSnapshotV1::from_inputs(
            AiWorkInput {
                current: Some(AiWorkPeriodInput {
                    total_tokens: 1_500_000,
                    total_cost: 9.5,
                    active_days: 4,
                    peak_day: None,
                    peak_day_tokens: 700_000,
                    leading_model: Some("<model>".to_string()),
                    leading_provider: Some("openai".to_string()),
                }),
                ..AiWorkInput::default()
            },
            WeReadSyncState::default(),
        )
    }

    #[test]
    fn review_uses_snapshot_and_relative_assets_only() {
        let snapshot = snapshot();
        let html = render_weekly_review(&snapshot);

        assert!(html.contains(&snapshot.snapshot_id));
        assert!(html.contains("/api/v1/pulse"));
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
    }

    #[test]
    fn review_escapes_signal_text() {
        let html = render_weekly_review(&snapshot());

        assert!(html.contains("&lt;model&gt;"));
        assert!(!html.contains("<model>"));
    }

    #[test]
    fn review_renders_and_escapes_suggested_actions() {
        let mut snapshot = snapshot();
        snapshot.recommendations = vec![PulseRecommendation {
            id: "recommendation.escape".to_string(),
            title: "Review <priority> & act".to_string(),
            rationale: "Quota is > 90% \"used\".".to_string(),
            evidence_refs: vec!["quota.<script>".to_string()],
        }];

        let html = render_weekly_review(&snapshot);

        assert!(html.contains("<h2>Suggested Actions</h2>"));
        assert!(html.contains("Review &lt;priority&gt; &amp; act"));
        assert!(html.contains("Quota is &gt; 90% &quot;used&quot;."));
        assert!(html.contains("quota.&lt;script&gt;"));
        assert!(!html.contains("<priority>"));
        assert!(!html.contains("quota.<script>"));
    }

    #[test]
    fn review_css_keeps_reading_rhythm_within_240px_viewport() {
        let html = render_weekly_review(&snapshot());

        assert!(html.contains("grid-template-columns:repeat(7,minmax(0,1fr))"));
        assert!(html.contains("@media (max-width:300px)"));
        assert!(html.contains(".shell { padding:0 8px 32px; }"));
        assert!(html.contains(".rhythm { gap:2px; }"));
        assert!(!html.contains("minmax(30px,1fr)"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("src=\"http"));
        assert!(!html.contains("href=\"http"));
    }
}
