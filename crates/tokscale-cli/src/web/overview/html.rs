use ratatui::{
    buffer::Cell,
    style::{Color, Modifier},
};

use crate::tui::Theme;

pub(super) const DEFAULT_FG: &str = "#c9d1d9";
pub(super) const DEFAULT_BG: &str = "#0d1117";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HtmlColorPalette {
    pub(super) fg: String,
    pub(super) bg: String,
    pub(super) muted: String,
    pub(super) color_scheme: &'static str,
    ansi: Vec<String>,
}

impl HtmlColorPalette {
    pub(super) fn from_theme(theme: &Theme) -> Self {
        let fg = raw_color_hex(theme.foreground, DEFAULT_FG);
        let bg = raw_color_hex(theme.background, DEFAULT_BG);
        let muted = raw_color_hex(theme.muted, "#8b949e");
        let color_scheme = if is_light_hex_color(&bg) {
            "light"
        } else {
            "dark"
        };
        let ansi = terminal_ansi_palette();

        Self {
            fg,
            bg,
            muted,
            color_scheme,
            ansi,
        }
    }

    #[cfg(test)]
    pub(super) fn dark() -> Self {
        Self::from_theme(&Theme::for_web_with_preference(
            crate::tui::ThemePreference::Dark,
        ))
    }

    pub(super) fn color_hex(&self, color: Color, fallback: &str) -> String {
        match color {
            Color::Reset => fallback.to_string(),
            Color::Black => self.ansi[0].clone(),
            Color::Red => self.ansi[1].clone(),
            Color::Green => self.ansi[2].clone(),
            Color::Yellow => self.ansi[3].clone(),
            Color::Blue => self.ansi[4].clone(),
            Color::Magenta => self.ansi[5].clone(),
            Color::Cyan => self.ansi[6].clone(),
            Color::Gray => self.ansi[7].clone(),
            Color::DarkGray => self.ansi[8].clone(),
            Color::LightRed => self.ansi[9].clone(),
            Color::LightGreen => self.ansi[10].clone(),
            Color::LightYellow => self.ansi[11].clone(),
            Color::LightBlue => self.ansi[12].clone(),
            Color::LightMagenta => self.ansi[13].clone(),
            Color::LightCyan => self.ansi[14].clone(),
            Color::White => self.ansi[15].clone(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Indexed(index) => self.indexed_color_hex(index),
        }
    }

    fn indexed_color_hex(&self, index: u8) -> String {
        self.ansi
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| self.fg.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HtmlCellStyle {
    pub(super) fg: String,
    pub(super) bg: String,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    crossed: bool,
    hidden: bool,
}

impl HtmlCellStyle {
    pub(super) fn from_cell(cell: &Cell, palette: &HtmlColorPalette) -> Self {
        let mut fg = palette.color_hex(cell.fg, &palette.fg);
        let mut bg = palette.color_hex(cell.bg, &palette.bg);
        if cell.modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }

        Self {
            fg,
            bg,
            bold: cell.modifier.contains(Modifier::BOLD),
            dim: cell.modifier.contains(Modifier::DIM),
            italic: cell.modifier.contains(Modifier::ITALIC),
            underline: cell.modifier.contains(Modifier::UNDERLINED),
            crossed: cell.modifier.contains(Modifier::CROSSED_OUT),
            hidden: cell.modifier.contains(Modifier::HIDDEN),
        }
    }

    pub(super) fn open_span(&self) -> String {
        let mut style = format!("color:{};background-color:{};", self.fg, self.bg);
        if self.bold {
            style.push_str("font-weight:700;");
        }
        if self.dim {
            style.push_str("opacity:.58;");
        }
        if self.italic {
            style.push_str("font-style:italic;");
        }
        if self.underline || self.crossed {
            let mut decorations = Vec::new();
            if self.underline {
                decorations.push("underline");
            }
            if self.crossed {
                decorations.push("line-through");
            }
            style.push_str("text-decoration:");
            style.push_str(&decorations.join(" "));
            style.push(';');
        }
        if self.hidden {
            style.push_str("visibility:hidden;");
        }
        format!(r#"<span style="{style}">"#)
    }
}

pub(super) fn cell_symbol(cell: &Cell) -> &str {
    cell.symbol()
}

pub(super) fn cell_text(cell: &Cell) -> String {
    let symbol = cell_symbol(cell);
    if symbol.is_empty() {
        " ".to_string()
    } else {
        symbol.to_string()
    }
}

pub(super) fn block_level(symbol: &str) -> Option<u8> {
    match symbol {
        "▁" => Some(1),
        "▂" => Some(2),
        "▃" => Some(3),
        "▄" => Some(4),
        "▅" => Some(5),
        "▆" => Some(6),
        "▇" => Some(7),
        "█" => Some(8),
        _ => None,
    }
}

pub(super) fn block_fill_fraction(level: u8) -> f64 {
    match level {
        1 => 0.125,
        2 => 0.25,
        3 => 0.375,
        4 => 0.5,
        5 => 0.625,
        6 => 0.75,
        7 => 0.875,
        _ => 1.0,
    }
}

fn raw_color_hex(color: Color, fallback: &str) -> String {
    match color {
        Color::Reset => fallback.to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#181818".to_string(),
        Color::Red => "#ac4242".to_string(),
        Color::Green => "#90a959".to_string(),
        Color::Yellow => "#f4bf75".to_string(),
        Color::Blue => "#6a9fb5".to_string(),
        Color::Magenta => "#aa759f".to_string(),
        Color::Cyan => "#75b5aa".to_string(),
        Color::Gray => "#d8d8d8".to_string(),
        Color::DarkGray => "#6b6b6b".to_string(),
        Color::LightRed => "#c55555".to_string(),
        Color::LightGreen => "#aac474".to_string(),
        Color::LightYellow => "#feca88".to_string(),
        Color::LightBlue => "#82b8c8".to_string(),
        Color::LightMagenta => "#c28cb8".to_string(),
        Color::LightCyan => "#93d3c3".to_string(),
        Color::White => "#f8f8f8".to_string(),
        Color::Indexed(index) => terminal_ansi_palette()
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| fallback.to_string()),
    }
}

fn terminal_ansi_palette() -> Vec<String> {
    [
        "#181818", "#ac4242", "#90a959", "#f4bf75", "#6a9fb5", "#aa759f", "#75b5aa", "#d8d8d8",
        "#6b6b6b", "#c55555", "#aac474", "#feca88", "#82b8c8", "#c28cb8", "#93d3c3", "#f8f8f8",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(super) fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

pub(super) fn is_light_hex_color(value: &str) -> bool {
    let Some((r, g, b)) = parse_hex_color(value) else {
        return false;
    };
    u16::from(r) + u16::from(g) + u16::from(b) > 540
}

pub(super) fn blend_hex_color(from: &str, to: &str, factor: f64) -> Option<String> {
    let (from_r, from_g, from_b) = parse_hex_color(from)?;
    let (to_r, to_g, to_b) = parse_hex_color(to)?;
    let blend = |from: u8, to: u8| -> u8 {
        let from = from as f64;
        let to = to as f64;
        (from + (to - from) * factor.clamp(0.0, 1.0))
            .round()
            .clamp(0.0, 255.0) as u8
    };

    Some(format!(
        "#{:02x}{:02x}{:02x}",
        blend(from_r, to_r),
        blend(from_g, to_g),
        blend(from_b, to_b)
    ))
}

pub(super) fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}
