use ratatui::prelude::*;
use ratatui::widgets::ScrollbarState;
use tokscale_core::ClientId;

use crate::tui::client_ui;
use crate::tui::config::TokscaleConfig;

pub fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        format_tokens_with_commas(tokens)
    }
}

pub fn format_tokens(tokens: u64) -> String {
    format_tokens_compact(tokens)
}

pub fn format_tokens_with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

pub fn format_cost(cost: f64) -> String {
    if !cost.is_finite() || cost < 0.0 {
        return "$0.00".to_string();
    }
    if cost >= 1000.0 {
        format!("${:.1}K", cost / 1000.0)
    } else {
        format!("${:.2}", cost)
    }
}

/// Cost per million tokens: useful for comparing model efficiency across sessions.
/// Returns "—" when there are no tokens to avoid division by zero.
pub fn format_cost_per_million(cost: f64, total_tokens: u64) -> String {
    if total_tokens == 0 || !cost.is_finite() || cost < 0.0 {
        return "\u{2014}".to_string(); // —
    }
    let per_m = cost / (total_tokens as f64) * 1_000_000.0;
    format!("${:.2}", per_m)
}

/// Cache reuse multiplier: cached reads per full-price input token.
/// `cache_read / (input + cache_write)` — how many low-cost reads you
/// got for every token you paid full price (fresh input or cache write).
pub fn format_cache_hit_rate(cache_read: u64, input: u64, cache_write: u64) -> String {
    let paid = input.saturating_add(cache_write);
    if paid == 0 {
        return if cache_read > 0 {
            "∞".to_string()
        } else {
            "—".to_string()
        };
    }
    let ratio = cache_read as f64 / paid as f64;
    format!("{:.1}x", ratio)
}

pub fn format_ms_per_1k(ms_per_1k_tokens: Option<f64>) -> String {
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

pub fn scrollbar_state(
    content_len: usize,
    scroll_offset: usize,
    viewport_len: usize,
) -> ScrollbarState {
    let viewport_len = viewport_len.max(1);
    ScrollbarState::new(content_len)
        .position(scrollbar_position(scroll_offset, content_len, viewport_len))
        .viewport_content_length(viewport_len)
}

fn scrollbar_position(scroll_offset: usize, content_len: usize, viewport_len: usize) -> usize {
    let max_scroll = content_len.saturating_sub(viewport_len);
    if max_scroll == 0 {
        0
    } else {
        ((scroll_offset.min(max_scroll) as u128) * (content_len.saturating_sub(1) as u128)
            / (max_scroll as u128)) as usize
    }
}

pub(crate) fn light_ratio_bar_spans(
    ratio: f64,
    width: usize,
    fill_style: Style,
    empty_style: Style,
) -> Vec<Span<'static>> {
    ratio_bar_spans(
        ratio,
        width,
        fill_style,
        RatioBarTrack::Visible {
            symbol: "·",
            style: empty_style,
        },
    )
}

enum RatioBarTrack {
    Visible { symbol: &'static str, style: Style },
}

fn ratio_bar_spans(
    ratio: f64,
    width: usize,
    fill_style: Style,
    track: RatioBarTrack,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let cells = ratio_bar_cells(ratio, width);

    let mut spans = Vec::with_capacity(3);
    if cells.filled > 0 {
        spans.push(Span::styled("█".repeat(cells.filled), fill_style));
    }
    if cells.trace {
        spans.push(Span::styled("▏", fill_style));
    }
    if cells.empty > 0 {
        match track {
            RatioBarTrack::Visible { symbol, style } => {
                spans.push(Span::styled(symbol.repeat(cells.empty), style));
            }
        }
    }
    spans
}

struct RatioBarCells {
    filled: usize,
    trace: bool,
    empty: usize,
}

fn ratio_bar_cells(ratio: f64, width: usize) -> RatioBarCells {
    let ratio = ratio.clamp(0.0, 1.0);
    let scaled = ratio * width as f64;
    let trace = ratio > 0.0 && ratio < 0.01 && scaled < 1.0;
    let filled = if ratio > 0.0 && !trace {
        (scaled.round() as usize).clamp(1, width)
    } else {
        0
    };
    let empty = width.saturating_sub(filled + usize::from(trace));

    RatioBarCells {
        filled,
        trace,
        empty,
    }
}

pub(crate) fn truncate_ascii(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 3 {
        s.chars().take(max_chars).collect()
    } else {
        let head: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", head)
    }
}

pub(crate) fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars == 1 {
        "…".to_string()
    } else {
        let head: String = s.chars().take(max_chars - 1).collect();
        format!("{head}…")
    }
}

pub fn get_model_color(model: &str) -> Color {
    get_provider_shade(get_provider_from_model(model), 0)
}

/// Returns the shade for a given `(provider, rank)` pair.
/// Honors `[colors.providers]` config overrides at every rank by deriving
/// a 7-step lighten-to-white palette from the override base color.
pub fn get_provider_shade(provider: &str, rank: usize) -> Color {
    let config = TokscaleConfig::load();
    if let Some(base) = config.get_provider_color(provider) {
        return shade_from_base(base, rank);
    }

    let key = canonical_provider_color_key(provider);
    if key != provider {
        if let Some(base) = config.get_provider_color(&key) {
            return shade_from_base(base, rank);
        }
    }

    if let Some(palette) = branded_provider_palette(&key) {
        let idx = rank.min(palette.len() - 1);
        let (r, g, b) = palette[idx];
        return Color::Rgb(r, g, b);
    }

    if let Some(base) = branded_provider_base(&key) {
        return shade_from_base(base, rank);
    }

    shade_from_base(uncategorized_provider_base(&key), rank)
}

/// Generates a 7-step monochromatic palette from `base` by interpolating
/// toward white. Factors roughly match the end-of-ramp lightness of the
/// hardcoded palettes so overrides feel visually consistent.
fn shade_from_base(base: Color, rank: usize) -> Color {
    const FACTORS: [f32; 7] = [0.00, 0.11, 0.22, 0.33, 0.44, 0.56, 0.67];
    let Color::Rgb(r, g, b) = base else {
        return base;
    };
    let idx = rank.min(FACTORS.len() - 1);
    let f = FACTORS[idx];
    let lerp = |c: u8| -> u8 {
        let c = c as f32;
        (c + (255.0 - c) * f).round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(r), lerp(g), lerp(b))
}

const ANTHROPIC_SHADES: [(u8, u8, u8); 7] = [
    (218, 119, 86),  // #DA7756
    (223, 136, 107), // #DF886B
    (227, 153, 128), // #E39980
    (232, 170, 149), // #E8AA95
    (236, 184, 166), // #ECB8A6
    (239, 197, 183), // #EFC5B7
    (243, 210, 199), // #F3D2C7
];

const OPENAI_SHADES: [(u8, u8, u8); 7] = [
    (16, 185, 129),  // #10B981
    (18, 208, 145),  // #12D091
    (20, 232, 162),  // #14E8A2
    (41, 236, 172),  // #29ECAC
    (61, 238, 179),  // #3DEEB3
    (97, 241, 193),  // #61F1C1
    (133, 244, 208), // #85F4D0
];

const GOOGLE_SHADES: [(u8, u8, u8); 7] = [
    (59, 130, 246),  // #3B82F6
    (83, 146, 247),  // #5392F7
    (108, 161, 248), // #6CA1F8
    (132, 177, 249), // #84B1F9
    (153, 190, 250), // #99BEFA
    (172, 202, 251), // #ACCAFB
    (190, 214, 252), // #BED6FC
];

const DEEPSEEK_SHADES: [(u8, u8, u8); 7] = [
    (6, 182, 212),   // #06B6D4
    (7, 203, 237),   // #07CBED
    (21, 215, 248),  // #15D7F8
    (45, 219, 249),  // #2DDBF9
    (66, 223, 250),  // #42DFFA
    (85, 226, 250),  // #55E2FA
    (105, 229, 251), // #69E5FB
];

const XAI_SHADES: [(u8, u8, u8); 7] = [
    (234, 179, 8),   // #EAB308
    (247, 192, 21),  // #F7C015
    (248, 199, 45),  // #F8C72D
    (249, 205, 70),  // #F9CD46
    (249, 211, 91),  // #F9D35B
    (250, 216, 110), // #FAD86E
    (251, 221, 129), // #FBDD81
];

const META_SHADES: [(u8, u8, u8); 7] = [
    (99, 102, 241),  // #6366F1
    (122, 125, 243), // #7A7DF3
    (146, 148, 245), // #9294F5
    (169, 171, 247), // #A9ABF7
    (189, 190, 249), // #BDBEF9
    (207, 208, 251), // #CFD0FB
    (225, 226, 252), // #E1E2FC
];

const CURSOR_SHADES: [(u8, u8, u8); 7] = [
    (139, 92, 246),  // #8B5CF6
    (154, 114, 247), // #9A72F7
    (169, 135, 248), // #A987F8
    (184, 156, 250), // #B89CFA
    (199, 177, 251), // #C7B1FB
    (215, 199, 252), // #D7C7FC
    (230, 220, 253), // #E6DCFD
];

const UNCATEGORIZED_PROVIDER_BASES: [(u8, u8, u8); 16] = [
    (244, 114, 182), // #F472B6 pink
    (45, 212, 191),  // #2DD4BF teal
    (250, 204, 21),  // #FACC15 yellow
    (251, 146, 60),  // #FB923C orange
    (34, 211, 238),  // #22D3EE cyan
    (248, 113, 113), // #F87171 red
    (132, 204, 22),  // #84CC16 lime
    (14, 165, 233),  // #0EA5E9 sky
    (52, 211, 153),  // #34D399 emerald
    (251, 113, 133), // #FB7185 rose
    (96, 165, 250),  // #60A5FA blue
    (245, 158, 11),  // #F59E0B amber
    (74, 222, 128),  // #4ADE80 green
    (251, 113, 59),  // #FB713B coral
    (6, 182, 212),   // #06B6D4 cyan
    (234, 179, 8),   // #EAB308 gold
];

fn branded_provider_palette(provider_key: &str) -> Option<&'static [(u8, u8, u8)]> {
    match provider_key {
        s if s.contains("anthropic") => Some(&ANTHROPIC_SHADES),
        s if s.contains("openai") => Some(&OPENAI_SHADES),
        s if s.contains("google") || s.contains("gemini") => Some(&GOOGLE_SHADES),
        s if s.contains("deepseek") => Some(&DEEPSEEK_SHADES),
        s if s.contains("xai") || s.contains("grok") => Some(&XAI_SHADES),
        s if s.contains("meta") || s.contains("llama") => Some(&META_SHADES),
        s if s.contains("cursor") => Some(&CURSOR_SHADES),
        _ => None,
    }
}

fn branded_provider_base(provider_key: &str) -> Option<Color> {
    match provider_key {
        "mistral" => Some(Color::Rgb(249, 115, 22)),   // orange
        "qwen" => Some(Color::Rgb(20, 184, 166)),      // teal
        "moonshotai" => Some(Color::Rgb(244, 63, 94)), // rose
        "zai" => Some(Color::Rgb(132, 204, 22)),       // lime
        "minimax" => Some(Color::Rgb(244, 114, 182)),  // pink
        "nvidia" => Some(Color::Rgb(118, 185, 0)),     // green
        "cohere" => Some(Color::Rgb(245, 158, 11)),    // amber
        _ => None,
    }
}

fn canonical_provider_color_key(provider: &str) -> String {
    let first = provider
        .split(',')
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or("unknown");
    let normalized = first
        .trim_end_matches('/')
        .to_lowercase()
        .replace(['-', ' ', '.'], "_");

    match normalized.as_str() {
        "" | "unknown" => "unknown".to_string(),
        "x_ai" | "xai" => "xai".to_string(),
        "openai_codex" => "openai".to_string(),
        "google" | "gemini" => "google".to_string(),
        "meta_llama" | "llama" => "meta".to_string(),
        "mistral" | "mistralai" => "mistral".to_string(),
        "moonshot" | "moonshotai" | "kimi" | "kimi_for_coding" => "moonshotai".to_string(),
        "z_ai" | "zai" | "zhipu" | "zhipuai" | "glm" => "zai".to_string(),
        "minimax" | "minimaxai" | "minimax_ai" => "minimax".to_string(),
        "nvidia" | "nemotron" => "nvidia".to_string(),
        "fireworks_ai" => "fireworks".to_string(),
        "together_ai" => "together".to_string(),
        other => other.to_string(),
    }
}

fn uncategorized_provider_base(provider_key: &str) -> Color {
    let hash = stable_color_hash(provider_key.as_bytes());
    let idx = (hash as usize) % UNCATEGORIZED_PROVIDER_BASES.len();
    let (r, g, b) = UNCATEGORIZED_PROVIDER_BASES[idx];
    Color::Rgb(r, g, b)
}

fn stable_color_hash(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub fn get_provider_from_model(model: &str) -> &'static str {
    let model_lower = model.to_lowercase();

    if model_lower.contains("claude")
        || model_lower.contains("sonnet")
        || model_lower.contains("opus")
        || model_lower.contains("haiku")
    {
        "anthropic"
    } else if model_lower.contains("gpt")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.contains("codex")
        || model_lower.contains("text-embedding")
        || model_lower.contains("dall-e")
        || model_lower.contains("whisper")
        || model_lower.contains("tts")
    {
        "openai"
    } else if model_lower.contains("gemini") {
        "google"
    } else if model_lower.contains("deepseek") {
        "deepseek"
    } else if model_lower.contains("grok") {
        "xai"
    } else if model_lower.contains("llama") {
        "meta"
    } else if model_lower.contains("mistral") || model_lower.contains("mixtral") {
        "mistral"
    } else if model_lower.contains("kimi") {
        "moonshotai"
    } else if model_lower.contains("qwen") {
        "qwen"
    } else if model_lower.contains("minimax") {
        "minimax"
    } else if model_lower.contains("glm")
        || model_lower.contains("zai")
        || model_lower.contains("z-ai")
        || model_lower.contains("zhipu")
    {
        "zai"
    } else if model_lower.contains("nemotron") {
        "nvidia"
    } else if model_lower == "auto"
        || model_lower.contains("cursor")
        || model_lower.contains("composer")
    {
        "cursor"
    } else {
        "unknown"
    }
}

pub fn get_client_color(client: &str) -> Color {
    let config = TokscaleConfig::load();
    if let Some(color) = config.get_client_color(client) {
        return color;
    }
    match client.to_lowercase().as_str() {
        "opencode" => Color::Rgb(34, 197, 94),     // #22c55e
        "claude" => Color::Rgb(218, 119, 86),      // #DA7756 Claude brand coral
        "codex" => Color::Rgb(59, 130, 246),       // #3b82f6
        "cursor" => Color::Rgb(168, 85, 247),      // #a855f7
        "gemini" => Color::Rgb(6, 182, 212),       // #06b6d4
        "amp" => Color::Rgb(236, 72, 153),         // #EC4899
        "droid" => Color::Rgb(16, 185, 129),       // #10b981
        "openclaw" => Color::Rgb(239, 68, 68),     // #ef4444
        "hermes" => Color::Rgb(255, 215, 0),       // #ffd700
        "goose" => Color::Rgb(100, 180, 220),      // #64b4dc
        "codebuff" => Color::Rgb(124, 58, 237),    // #7C3AED Codebuff brand purple
        "antigravity" => Color::Rgb(99, 102, 241), // #6366F1 Antigravity indigo
        "zed" => Color::Rgb(8, 76, 207),           // #084CCF Zed blue
        "warp" => Color::Rgb(1, 155, 150),         // #019B96 Warp teal
        "gjc" => Color::Rgb(220, 38, 38),          // #DC2626 gajae-code red-claw
        _ => Color::Rgb(136, 136, 136),            // #888888
    }
}

pub fn get_client_display_name(client: &str) -> String {
    let config = TokscaleConfig::load();
    if let Some(name) = config.get_client_display_name(client) {
        return name.to_string();
    }
    let client_lower = client.to_lowercase();
    if client_lower == ClientId::OpenClaw.as_str() {
        return "🦞 OpenClaw".to_string();
    }
    if let Some(client_id) = ClientId::from_str(&client_lower) {
        return client_ui::display_name(client_id).to_string();
    }
    client.to_string()
}

pub fn get_provider_display_name(provider: &str) -> String {
    let config = TokscaleConfig::load();
    if let Some(name) = config.get_provider_display_name(provider) {
        return name.to_string();
    }
    match provider.to_lowercase().as_str() {
        "anthropic" => "Anthropic".to_string(),
        "openai" => "OpenAI".to_string(),
        "google" => "Google".to_string(),
        "cursor" => "Cursor".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "xai" => "xAI".to_string(),
        "meta" => "Meta".to_string(),
        "mistral" => "Mistral".to_string(),
        "qwen" => "Qwen".to_string(),
        "moonshotai" => "Moonshot AI".to_string(),
        "zai" => "Z.ai".to_string(),
        "minimax" => "MiniMax".to_string(),
        "nvidia" => "NVIDIA".to_string(),
        "cohere" => "Cohere".to_string(),
        "fireworks" => "Fireworks".to_string(),
        "together" => "Together".to_string(),
        "opencode" => "OpenCode".to_string(),
        s if s.starts_with("github-cop") || s.contains("copilot") => "GitHub Copilot".to_string(),
        _ => capitalize_first(provider),
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_state_maps_bottom_offset_to_last_position() {
        assert_eq!(scrollbar_position(15, 20, 5), 19);
    }

    #[test]
    fn scrollbar_state_keeps_top_at_zero() {
        assert_eq!(scrollbar_position(0, 20, 5), 0);
    }

    #[test]
    fn scrollbar_state_clamps_overscroll_to_bottom() {
        assert_eq!(scrollbar_position(999, 20, 5), 19);
    }

    #[test]
    fn scrollbar_state_single_page_stays_at_zero() {
        assert_eq!(scrollbar_position(0, 5, 10), 0);
    }

    #[test]
    fn scrollbar_state_uses_wide_math_for_large_lengths() {
        let content_len = usize::MAX;
        let viewport_len = 2;
        let scroll_offset = content_len / 2;
        let max_scroll = content_len.saturating_sub(viewport_len);
        let expected = ((scroll_offset.min(max_scroll) as u128)
            * (content_len.saturating_sub(1) as u128)
            / (max_scroll as u128)) as usize;

        assert_eq!(
            scrollbar_position(scroll_offset, content_len, viewport_len),
            expected
        );
    }

    #[test]
    fn light_ratio_bar_uses_trace_for_sub_cell_values() {
        let spans = light_ratio_bar_spans(
            0.001,
            12,
            Style::default().fg(Color::Green),
            Style::default().fg(Color::DarkGray),
        );
        let text = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text.chars().count(), 12);
        assert!(text.contains("▏"), "{text}");
        assert!(!text.contains("█"), "{text}");
        assert!(text.contains("·"), "{text}");
    }

    #[test]
    fn light_ratio_bar_fills_proportional_cells() {
        let spans = light_ratio_bar_spans(
            0.5,
            12,
            Style::default().fg(Color::Green),
            Style::default().fg(Color::DarkGray),
        );
        let text = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text.chars().count(), 12);
        assert!(text.contains("█"), "{text}");
        assert!(!text.contains("▏"), "{text}");
        assert_eq!(text.chars().filter(|ch| *ch == '█').count(), 6);
    }

    #[test]
    fn light_ratio_bar_keeps_one_cell_for_visible_percentages() {
        let spans = light_ratio_bar_spans(
            0.05,
            10,
            Style::default().fg(Color::Green),
            Style::default().fg(Color::DarkGray),
        );
        let text = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text.chars().count(), 10);
        assert!(text.starts_with('█'), "{text}");
        assert!(!text.contains("▏"), "{text}");
    }

    #[test]
    fn truncate_ascii_uses_three_dot_suffix_when_space_allows() {
        assert_eq!(truncate_ascii("abcdef", 0), "");
        assert_eq!(truncate_ascii("abcdef", 2), "ab");
        assert_eq!(truncate_ascii("abcdef", 5), "ab...");
        assert_eq!(truncate_ascii("abc", 5), "abc");
    }

    #[test]
    fn truncate_ellipsis_uses_single_cell_suffix() {
        assert_eq!(truncate_ellipsis("abcdef", 0), "");
        assert_eq!(truncate_ellipsis("abcdef", 1), "…");
        assert_eq!(truncate_ellipsis("abcdef", 2), "a…");
        assert_eq!(truncate_ellipsis("abcdef", 5), "abcd…");
        assert_eq!(truncate_ellipsis("abc", 5), "abc");
    }

    #[test]
    fn shade_from_base_rank_0_equals_base() {
        let base = Color::Rgb(255, 0, 0);
        assert_eq!(shade_from_base(base, 0), base);
    }

    #[test]
    fn shade_from_base_lightens_monotonically_toward_white() {
        let base = Color::Rgb(0, 0, 0);
        let mut prev_r: u8 = 0;
        for rank in 0..7 {
            let Color::Rgb(r, _, _) = shade_from_base(base, rank) else {
                panic!("expected Rgb")
            };
            assert!(
                r >= prev_r,
                "shade at rank {} should not be darker than rank {}",
                rank,
                rank - 1
            );
            prev_r = r;
        }
    }

    #[test]
    fn shade_from_base_clamps_beyond_palette_length() {
        let base = Color::Rgb(100, 100, 100);
        // Rank beyond FACTORS.len() saturates to the lightest shade.
        assert_eq!(shade_from_base(base, 100), shade_from_base(base, 6));
    }

    #[test]
    fn shade_from_base_passes_through_non_rgb() {
        // Indexed terminal colors can't be lightened channel-wise — return as-is.
        assert_eq!(shade_from_base(Color::Indexed(42), 5), Color::Indexed(42));
    }

    #[test]
    fn known_provider_palettes_keep_color_families() {
        let providers = [
            "anthropic",
            "openai",
            "google",
            "deepseek",
            "xai",
            "meta",
            "cursor",
            "mistral",
            "qwen",
            "moonshotai",
            "zai",
            "minimax",
            "nvidia",
            "cohere",
        ];

        for provider in providers {
            let rank_0 = get_provider_shade(provider, 0);
            let rank_3 = get_provider_shade(provider, 3);
            assert_colorful(rank_0, provider);
            assert_ne!(rank_0, rank_3, "{provider} should keep rank shades");
        }
    }

    #[test]
    fn uncategorized_provider_uses_stable_non_gray_base() {
        let rank_0 = get_provider_shade("some-new-provider", 0);
        let rank_3 = get_provider_shade("some-new-provider", 3);

        assert_eq!(rank_0, get_provider_shade("some-new-provider", 0));
        assert_ne!(rank_0, rank_3);
        assert_ne!(rank_0, Color::Rgb(255, 255, 255));
        assert_ne!(rank_0, Color::Rgb(136, 136, 136));
    }

    #[test]
    fn uncategorized_provider_aliases_share_color_key() {
        assert_eq!(get_provider_shade("z-ai", 0), get_provider_shade("zai", 0));
        assert_eq!(get_provider_shade("zhipu", 0), get_provider_shade("zai", 0));
        assert_eq!(
            get_provider_shade("kimi", 0),
            get_provider_shade("moonshotai", 0)
        );
    }

    #[test]
    fn cursor_provider_has_distinct_shades_per_rank() {
        // Regression: CURSOR_SHADES used to be a single-entry palette so all
        // Cursor models collapsed to one color.
        let rank_0 = get_provider_shade("cursor", 0);
        let rank_6 = get_provider_shade("cursor", 6);
        assert_ne!(rank_0, rank_6);
    }

    #[test]
    fn get_provider_shade_saturates_at_palette_end() {
        let last = get_provider_shade("anthropic", 6);
        let past_end = get_provider_shade("anthropic", 99);
        assert_eq!(last, past_end);
    }

    #[test]
    fn get_provider_shade_fuzzy_matching() {
        assert_eq!(
            get_provider_shade("test-anthropic", 0),
            get_provider_shade("anthropic", 0)
        );
        assert_eq!(
            get_provider_shade("company-google", 0),
            get_provider_shade("google", 0)
        );
        assert_eq!(
            get_provider_shade("openrouter-gemini-prod", 0),
            get_provider_shade("google", 0)
        );
        assert_eq!(
            get_provider_shade("deepseek-api", 0),
            get_provider_shade("deepseek", 0)
        );
        assert_eq!(
            get_provider_shade("meta-llama-endpoint", 0),
            get_provider_shade("meta", 0)
        );
    }

    fn assert_colorful(color: Color, label: &str) {
        let Color::Rgb(r, g, b) = color else {
            panic!("{label} should resolve to an RGB provider color");
        };
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        assert!(
            max.saturating_sub(min) >= 40,
            "{label} should not resolve to a neutral gray color: {color:?}"
        );
    }
}
