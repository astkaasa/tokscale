pub(crate) fn format_currency(n: f64) -> String {
    format!("${:.2}", n)
}

pub(crate) fn format_cost_per_million(cost: f64, total_tokens: i64) -> String {
    if total_tokens <= 0 || !cost.is_finite() {
        return "—".to_string();
    }
    let cost_per_m = cost * 1_000_000.0 / total_tokens as f64;
    if !cost_per_m.is_finite() {
        "—".to_string()
    } else {
        format!("${:.2}/M", cost_per_m)
    }
}

pub(crate) fn format_ms_per_1k(ms_per_1k_tokens: Option<f64>) -> String {
    let Some(value) = ms_per_1k_tokens else {
        return "—".to_string();
    };
    if !value.is_finite() || value <= 0.0 {
        "—".to_string()
    } else if value >= 1000.0 {
        format!("{:.1}s", value / 1000.0)
    } else {
        format!("{:.0}ms", value)
    }
}

pub(crate) fn model_entry_total_tokens(entry: &tokscale_core::ModelUsage) -> i64 {
    entry.input.max(0)
        + entry.output.max(0)
        + entry.cache_read.max(0)
        + entry.cache_write.max(0)
        + entry.reasoning.max(0)
}

pub(crate) fn aggregate_model_report_performance(
    entries: &[tokscale_core::ModelUsage],
) -> tokscale_core::ModelPerformance {
    let mut performance = tokscale_core::ModelPerformance::default();
    for entry in entries {
        performance.total_duration_ms = performance
            .total_duration_ms
            .saturating_add(entry.performance.total_duration_ms);
        performance.timed_tokens = performance
            .timed_tokens
            .saturating_add(entry.performance.timed_tokens);
        performance.sample_count = performance
            .sample_count
            .saturating_add(entry.performance.sample_count);
    }
    let total_tokens = entries.iter().map(model_entry_total_tokens).sum();
    performance.finalize(total_tokens);
    performance
}

pub(crate) fn dim_borders(table_str: &str) -> String {
    let border_chars: &[char] = &['┌', '─', '┬', '┐', '│', '├', '┼', '┤', '└', '┴', '┘'];
    let mut result = String::with_capacity(table_str.len() * 2);

    for ch in table_str.chars() {
        if border_chars.contains(&ch) {
            result.push_str("\x1b[90m");
            result.push(ch);
            result.push_str("\x1b[0m");
        } else {
            result.push(ch);
        }
    }

    result
}

pub(crate) fn format_model_name(model: &str) -> String {
    let name = model.strip_prefix("claude-").unwrap_or(model);
    if name.len() > 9 {
        let potential_date = &name[name.len() - 8..];
        if potential_date.chars().all(|c| c.is_ascii_digit())
            && name.as_bytes()[name.len() - 9] == b'-'
        {
            return name[..name.len() - 9].to_string();
        }
    }
    name.to_string()
}

pub(crate) fn capitalize_client(client: &str) -> String {
    match client {
        "opencode" => "OpenCode".to_string(),
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "cursor" => "Cursor".to_string(),
        "gemini" => "Gemini".to_string(),
        "amp" => "Amp".to_string(),
        "codebuff" => "Codebuff".to_string(),
        "droid" => "Droid".to_string(),
        "crush" => "Crush".to_string(),
        "openclaw" => "openclaw".to_string(),
        "hermes" => "Hermes Agent".to_string(),
        "goose" => "Goose".to_string(),
        "warp" => "Warp".to_string(),
        "pi" => "Pi".to_string(),
        "gjc" => "Gajae-Code".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn format_duration_ms(ms: i64) -> String {
    if ms <= 0 {
        return "0s".to_string();
    }
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

pub(crate) fn format_tokens_with_commas(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_with_commas_small() {
        assert_eq!(format_tokens_with_commas(123), "123");
    }

    #[test]
    fn format_tokens_with_commas_thousands() {
        assert_eq!(format_tokens_with_commas(1234), "1,234");
    }

    #[test]
    fn format_tokens_with_commas_millions() {
        assert_eq!(format_tokens_with_commas(1234567), "1,234,567");
    }

    #[test]
    fn format_tokens_with_commas_billions() {
        assert_eq!(format_tokens_with_commas(1234567890), "1,234,567,890");
    }

    #[test]
    fn format_tokens_with_commas_zero() {
        assert_eq!(format_tokens_with_commas(0), "0");
    }

    #[test]
    fn format_tokens_with_commas_negative() {
        assert_eq!(format_tokens_with_commas(-1234567), "-1,234,567");
    }

    #[test]
    fn format_currency_zero() {
        assert_eq!(format_currency(0.0), "$0.00");
    }

    #[test]
    fn format_currency_small() {
        assert_eq!(format_currency(12.34), "$12.34");
    }

    #[test]
    fn format_currency_large() {
        assert_eq!(format_currency(1234.56), "$1234.56");
    }

    #[test]
    fn format_currency_rounds() {
        assert_eq!(format_currency(12.345), "$12.35");
        assert_eq!(format_currency(12.344), "$12.34");
    }

    #[test]
    fn capitalize_client_opencode() {
        assert_eq!(capitalize_client("opencode"), "OpenCode");
    }

    #[test]
    fn capitalize_client_claude() {
        assert_eq!(capitalize_client("claude"), "Claude");
    }

    #[test]
    fn capitalize_client_codex() {
        assert_eq!(capitalize_client("codex"), "Codex");
    }

    #[test]
    fn capitalize_client_cursor() {
        assert_eq!(capitalize_client("cursor"), "Cursor");
    }

    #[test]
    fn capitalize_client_gemini() {
        assert_eq!(capitalize_client("gemini"), "Gemini");
    }

    #[test]
    fn capitalize_client_amp() {
        assert_eq!(capitalize_client("amp"), "Amp");
    }

    #[test]
    fn capitalize_client_droid() {
        assert_eq!(capitalize_client("droid"), "Droid");
    }

    #[test]
    fn capitalize_client_crush() {
        assert_eq!(capitalize_client("crush"), "Crush");
    }

    #[test]
    fn capitalize_client_openclaw() {
        assert_eq!(capitalize_client("openclaw"), "openclaw");
    }

    #[test]
    fn capitalize_client_hermes() {
        assert_eq!(capitalize_client("hermes"), "Hermes Agent");
    }

    #[test]
    fn capitalize_client_codebuff() {
        assert_eq!(capitalize_client("codebuff"), "Codebuff");
    }

    #[test]
    fn capitalize_client_pi() {
        assert_eq!(capitalize_client("pi"), "Pi");
    }

    #[test]
    fn capitalize_client_unknown() {
        assert_eq!(capitalize_client("unknown"), "unknown");
    }

    #[test]
    fn format_duration_handles_hours() {
        assert_eq!(format_duration_ms(3_723_000), "1h 2m 3s");
    }
}
