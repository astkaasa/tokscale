use ratatui::style::{Color, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalColorMode {
    FullColor,
    Compatible,
}

impl TerminalColorMode {
    pub(crate) fn from_env<I, K, V>(env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut term = String::new();
        let mut term_program = String::new();
        let mut colorterm = String::new();
        let mut no_color = false;

        for (key, value) in env {
            let key = key.as_ref();
            let value = value.as_ref();
            match key {
                "TERM" => term = value.to_ascii_lowercase(),
                "TERM_PROGRAM" => term_program = value.to_ascii_lowercase(),
                "COLORTERM" => colorterm = value.to_ascii_lowercase(),
                "NO_COLOR" => no_color = true,
                _ => {}
            }
        }

        if no_color || term == "dumb" || term_program == "apple_terminal" {
            return Self::Compatible;
        }

        if matches!(colorterm.as_str(), "truecolor" | "24bit")
            || term.contains("truecolor")
            || term.contains("24bit")
        {
            return Self::FullColor;
        }

        Self::FullColor
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub border: Color,
    pub highlight: Color,
    pub muted: Color,
    pub accent: Color,
    pub selection: Color,
    color_mode: TerminalColorMode,
}

impl Theme {
    pub fn for_current_terminal() -> Self {
        Self::with_color_mode(TerminalColorMode::from_env(std::env::vars()))
    }

    pub(crate) fn with_color_mode(color_mode: TerminalColorMode) -> Self {
        let mut theme = Self {
            background: Color::Rgb(13, 17, 23),
            foreground: Color::Rgb(201, 209, 217),
            border: Color::Rgb(48, 54, 61),
            highlight: Color::Rgb(13, 65, 157),
            muted: Color::Rgb(139, 148, 158),
            accent: Color::Cyan,
            selection: Color::Rgb(48, 54, 61),
            color_mode,
        };

        if color_mode == TerminalColorMode::Compatible {
            theme.background = Color::Black;
            theme.foreground = Color::White;
            theme.border = Color::DarkGray;
            theme.highlight = Color::Cyan;
            theme.muted = Color::DarkGray;
            theme.accent = Color::Cyan;
            theme.selection = Color::DarkGray;
        }

        theme
    }

    pub(crate) fn color(&self, color: Color) -> Color {
        match (self.color_mode, color) {
            (TerminalColorMode::Compatible, Color::Rgb(r, g, b)) => compatible_rgb(r, g, b),
            _ => color,
        }
    }

    pub(crate) fn metric_input_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(100, 200, 100)))
    }

    pub(crate) fn metric_output_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(200, 100, 100)))
    }

    pub(crate) fn metric_cache_read_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(100, 150, 200)))
    }

    pub(crate) fn metric_cache_write_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(200, 150, 100)))
    }

    pub(crate) fn secondary_text_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(170, 170, 170)))
    }

    pub(crate) fn subtle_text_style(&self) -> Style {
        Style::default().fg(self.color(Color::Rgb(102, 102, 102)))
    }

    pub(crate) fn striped_row_style(&self) -> Style {
        if self.color_mode == TerminalColorMode::Compatible {
            Style::default()
        } else {
            Style::default().bg(Color::Rgb(20, 24, 30))
        }
    }

    pub(crate) fn current_row_style(&self) -> Style {
        if self.color_mode == TerminalColorMode::Compatible {
            Style::default().bg(self.selection)
        } else {
            Style::default().bg(Color::Rgb(28, 42, 34))
        }
    }
}

fn compatible_rgb(r: u8, g: u8, b: u8) -> Color {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);

    if max < 64 {
        return Color::Black;
    }

    if max.saturating_sub(min) < 40 {
        return if max < 160 {
            Color::DarkGray
        } else {
            Color::Gray
        };
    }

    if r >= g && r >= b {
        if g >= 150 {
            Color::Yellow
        } else if b >= 150 {
            Color::Magenta
        } else {
            Color::Red
        }
    } else if g >= r && g >= b {
        if b >= 150 {
            Color::Cyan
        } else {
            Color::Green
        }
    } else if r >= 150 {
        Color::Magenta
    } else if g >= 150 {
        Color::Cyan
    } else {
        Color::Blue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn apple_terminal_uses_compatible_color_mode() {
        let mode = TerminalColorMode::from_env(env(&[
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("TERM", "xterm-256color"),
        ]));

        assert_eq!(mode, TerminalColorMode::Compatible);
    }

    #[test]
    fn vscode_truecolor_keeps_full_color_mode() {
        let mode = TerminalColorMode::from_env(env(&[
            ("TERM_PROGRAM", "vscode"),
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
        ]));

        assert_eq!(mode, TerminalColorMode::FullColor);
    }

    #[test]
    fn no_color_forces_compatible_color_mode() {
        let mode =
            TerminalColorMode::from_env(env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]));

        assert_eq!(mode, TerminalColorMode::Compatible);
    }

    #[test]
    fn compatible_theme_avoids_rgb_palette() {
        let theme = Theme::with_color_mode(TerminalColorMode::Compatible);

        assert!(!matches!(theme.background, Color::Rgb(..)));
        assert_ne!(theme.background, Color::Reset);
        assert!(!matches!(theme.foreground, Color::Rgb(..)));
        assert!(!matches!(theme.selection, Color::Rgb(..)));
    }

    #[test]
    fn full_color_theme_preserves_rgb_accent_styles() {
        let theme = Theme::with_color_mode(TerminalColorMode::FullColor);

        assert_eq!(
            theme.metric_input_style().fg,
            Some(Color::Rgb(100, 200, 100))
        );
        assert_eq!(theme.highlight, Color::Rgb(13, 65, 157));
        assert_eq!(theme.striped_row_style().bg, Some(Color::Rgb(20, 24, 30)));
    }

    #[test]
    fn compatible_theme_downgrades_rgb_accent_styles() {
        let theme = Theme::with_color_mode(TerminalColorMode::Compatible);

        let styles = [
            theme.metric_input_style(),
            theme.metric_output_style(),
            theme.metric_cache_read_style(),
            theme.metric_cache_write_style(),
            theme.secondary_text_style(),
            theme.subtle_text_style(),
            theme.striped_row_style(),
            theme.current_row_style(),
        ];

        for style in styles {
            assert!(
                !matches!(style.fg, Some(Color::Rgb(..))),
                "compatible foreground should not use RGB: {:?}",
                style.fg
            );
            assert!(
                !matches!(style.bg, Some(Color::Rgb(..))),
                "compatible background should not use RGB: {:?}",
                style.bg
            );
        }
    }
}
