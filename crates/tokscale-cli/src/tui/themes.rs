use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ThemePreference {
    #[default]
    Dark,
    Light,
    Auto,
}

impl FromStr for ThemePreference {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "invalid theme `{other}`; expected dark, light, or auto"
            )),
        }
    }
}

impl ThemePreference {
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark | Self::Auto => Self::Light,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeKind {
    Dark,
    Light,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalBackground {
    Dark,
    Light,
    Unknown,
}

impl TerminalBackground {
    pub(crate) fn from_env<I, K, V>(env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut colorfgbg = None;

        for (key, value) in env {
            let key = key.as_ref();
            let value = value.as_ref();
            match key {
                "TOKSCALE_TERMINAL_BG" | "TERMINAL_BACKGROUND" => {
                    match value.trim().to_ascii_lowercase().as_str() {
                        "dark" | "black" => return Self::Dark,
                        "light" | "white" => return Self::Light,
                        _ => {}
                    }
                }
                "COLORFGBG" => colorfgbg = Some(value.to_string()),
                _ => {}
            }
        }

        colorfgbg
            .as_deref()
            .and_then(background_from_colorfgbg)
            .unwrap_or(Self::Unknown)
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
    secondary: Color,
    subtle: Color,
    striped_row: Color,
    current_row: Color,
    metric_input: Color,
    metric_output: Color,
    metric_cache_read: Color,
    metric_cache_write: Color,
    success: Color,
    warning: Color,
    danger: Color,
    info: Color,
    color_mode: TerminalColorMode,
}

impl Theme {
    #[cfg(test)]
    pub(crate) fn for_current_terminal() -> Self {
        Self::for_current_terminal_with_preference(ThemePreference::Dark)
    }

    pub(crate) fn for_current_terminal_with_preference(preference: ThemePreference) -> Self {
        Self::from_env(preference, std::env::vars())
    }

    pub(crate) fn from_env<I, K, V>(preference: ThemePreference, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let pairs = env
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
            .collect::<Vec<_>>();
        let color_mode = TerminalColorMode::from_env(pairs.iter().map(|(k, v)| (k, v)));
        let terminal_background = TerminalBackground::from_env(pairs.iter().map(|(k, v)| (k, v)));
        Self::with_terminal(preference, color_mode, terminal_background)
    }

    #[cfg(test)]
    fn with_color_mode(color_mode: TerminalColorMode) -> Self {
        Self::with_preference_and_color_mode(ThemePreference::Dark, color_mode)
    }

    #[cfg(test)]
    fn with_preference_and_color_mode(
        preference: ThemePreference,
        color_mode: TerminalColorMode,
    ) -> Self {
        Self::with_terminal(preference, color_mode, TerminalBackground::Unknown)
    }

    pub(crate) fn with_terminal(
        preference: ThemePreference,
        color_mode: TerminalColorMode,
        terminal_background: TerminalBackground,
    ) -> Self {
        let kind = resolve_theme_kind(preference, terminal_background);
        let mut theme = match kind {
            ThemeKind::Dark => dark_theme(color_mode),
            ThemeKind::Light => light_theme(color_mode),
        };
        theme.color_mode = color_mode;
        theme
    }

    pub(crate) fn color(&self, color: Color) -> Color {
        match (self.color_mode, color) {
            (TerminalColorMode::Compatible, Color::Rgb(r, g, b)) => compatible_rgb(r, g, b),
            _ => color,
        }
    }

    pub(crate) fn metric_input_style(&self) -> Style {
        Style::default().fg(self.metric_input)
    }

    pub(crate) fn metric_output_style(&self) -> Style {
        Style::default().fg(self.metric_output)
    }

    pub(crate) fn metric_cache_read_style(&self) -> Style {
        Style::default().fg(self.metric_cache_read)
    }

    pub(crate) fn metric_cache_write_style(&self) -> Style {
        Style::default().fg(self.metric_cache_write)
    }

    pub(crate) fn success_style(&self) -> Style {
        Style::default().fg(self.success_color())
    }

    pub(crate) fn warning_style(&self) -> Style {
        Style::default().fg(self.warning_color())
    }

    pub(crate) fn danger_style(&self) -> Style {
        Style::default().fg(self.danger_color())
    }

    pub(crate) fn info_style(&self) -> Style {
        Style::default().fg(self.info_color())
    }

    pub(crate) fn success_color(&self) -> Color {
        self.success
    }

    pub(crate) fn warning_color(&self) -> Color {
        self.warning
    }

    pub(crate) fn danger_color(&self) -> Color {
        self.danger
    }

    pub(crate) fn info_color(&self) -> Color {
        self.info
    }

    pub(crate) fn secondary_text_style(&self) -> Style {
        Style::default().fg(self.secondary)
    }

    pub(crate) fn subtle_text_style(&self) -> Style {
        Style::default().fg(self.subtle)
    }

    pub(crate) fn active_control_style(&self) -> Style {
        let background = if self.color_mode == TerminalColorMode::Compatible {
            Color::Blue
        } else {
            self.highlight
        };

        Style::default()
            .fg(Color::White)
            .bg(background)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn striped_row_style(&self) -> Style {
        if self.color_mode == TerminalColorMode::Compatible {
            Style::default()
        } else {
            Style::default().bg(self.striped_row)
        }
    }

    pub(crate) fn current_row_style(&self) -> Style {
        if self.color_mode == TerminalColorMode::Compatible {
            Style::default().bg(self.selection)
        } else {
            Style::default().bg(self.current_row)
        }
    }
}

fn resolve_theme_kind(
    preference: ThemePreference,
    terminal_background: TerminalBackground,
) -> ThemeKind {
    match preference {
        ThemePreference::Dark => ThemeKind::Dark,
        ThemePreference::Light => ThemeKind::Light,
        ThemePreference::Auto => match terminal_background {
            TerminalBackground::Light => ThemeKind::Light,
            TerminalBackground::Dark | TerminalBackground::Unknown => ThemeKind::Dark,
        },
    }
}

fn background_from_colorfgbg(value: &str) -> Option<TerminalBackground> {
    let bg = value
        .split(';')
        .next_back()
        .and_then(|part| part.trim().parse::<u16>().ok())?;

    match bg {
        0..=6 | 8 => Some(TerminalBackground::Dark),
        7 | 9..=15 => Some(TerminalBackground::Light),
        _ => None,
    }
}

fn dark_theme(color_mode: TerminalColorMode) -> Theme {
    if color_mode == TerminalColorMode::Compatible {
        return Theme {
            background: Color::Black,
            foreground: Color::White,
            border: Color::DarkGray,
            highlight: Color::Cyan,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            selection: Color::DarkGray,
            secondary: Color::Gray,
            subtle: Color::DarkGray,
            striped_row: Color::Black,
            current_row: Color::DarkGray,
            metric_input: Color::Green,
            metric_output: Color::Red,
            metric_cache_read: Color::Cyan,
            metric_cache_write: Color::Yellow,
            success: Color::Green,
            warning: Color::Yellow,
            danger: Color::Red,
            info: Color::Cyan,
            color_mode,
        };
    }

    Theme {
        background: Color::Rgb(13, 17, 23),
        foreground: Color::Rgb(201, 209, 217),
        border: Color::Rgb(48, 54, 61),
        highlight: Color::Rgb(13, 65, 157),
        muted: Color::Rgb(139, 148, 158),
        accent: Color::Cyan,
        selection: Color::Rgb(48, 54, 61),
        secondary: Color::Rgb(170, 170, 170),
        subtle: Color::Rgb(102, 102, 102),
        striped_row: Color::Rgb(20, 24, 30),
        current_row: Color::Rgb(28, 42, 34),
        metric_input: Color::Rgb(100, 200, 100),
        metric_output: Color::Rgb(200, 100, 100),
        metric_cache_read: Color::Rgb(100, 150, 200),
        metric_cache_write: Color::Rgb(200, 150, 100),
        success: Color::Green,
        warning: Color::Yellow,
        danger: Color::Red,
        info: Color::Cyan,
        color_mode,
    }
}

fn light_theme(color_mode: TerminalColorMode) -> Theme {
    if color_mode == TerminalColorMode::Compatible {
        return Theme {
            background: Color::White,
            foreground: Color::Black,
            border: Color::Gray,
            highlight: Color::Blue,
            muted: Color::DarkGray,
            accent: Color::Blue,
            selection: Color::Gray,
            secondary: Color::DarkGray,
            subtle: Color::DarkGray,
            striped_row: Color::White,
            current_row: Color::Gray,
            metric_input: Color::DarkGray,
            metric_output: Color::Red,
            metric_cache_read: Color::Blue,
            metric_cache_write: Color::DarkGray,
            success: Color::DarkGray,
            warning: Color::DarkGray,
            danger: Color::Red,
            info: Color::Blue,
            color_mode,
        };
    }

    Theme {
        background: Color::Rgb(255, 255, 255),
        foreground: Color::Rgb(22, 22, 22),
        border: Color::Rgb(212, 212, 212),
        highlight: Color::Rgb(59, 92, 246),
        muted: Color::Rgb(92, 92, 92),
        accent: Color::Rgb(59, 92, 246),
        selection: Color::Rgb(238, 238, 238),
        secondary: Color::Rgb(58, 58, 58),
        subtle: Color::Rgb(128, 128, 128),
        striped_row: Color::Rgb(250, 250, 250),
        current_row: Color::Rgb(232, 240, 255),
        metric_input: Color::Rgb(25, 139, 67),
        metric_output: Color::Rgb(184, 45, 53),
        metric_cache_read: Color::Rgb(59, 92, 246),
        metric_cache_write: Color::Rgb(154, 91, 0),
        success: Color::Rgb(25, 139, 67),
        warning: Color::Rgb(154, 91, 0),
        danger: Color::Rgb(184, 45, 53),
        info: Color::Rgb(59, 92, 246),
        color_mode,
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
            theme.active_control_style(),
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

    #[test]
    fn light_theme_uses_opencode_stats_palette() {
        let theme = Theme::with_preference_and_color_mode(
            ThemePreference::Light,
            TerminalColorMode::FullColor,
        );

        assert_eq!(theme.background, Color::Rgb(255, 255, 255));
        assert_eq!(theme.foreground, Color::Rgb(22, 22, 22));
        assert_eq!(theme.accent, Color::Rgb(59, 92, 246));
        assert_eq!(theme.selection, Color::Rgb(238, 238, 238));
        assert_eq!(theme.active_control_style().fg, Some(Color::White));
        assert_eq!(
            theme.active_control_style().bg,
            Some(Color::Rgb(59, 92, 246))
        );
        assert_eq!(
            theme.striped_row_style().bg,
            Some(Color::Rgb(250, 250, 250))
        );
    }

    #[test]
    fn auto_theme_uses_terminal_background_when_available() {
        let light = Theme::from_env(
            ThemePreference::Auto,
            env(&[("COLORTERM", "truecolor"), ("COLORFGBG", "0;15")]),
        );
        let dark = Theme::from_env(
            ThemePreference::Auto,
            env(&[("COLORTERM", "truecolor"), ("COLORFGBG", "15;0")]),
        );

        assert_eq!(light.background, Color::Rgb(255, 255, 255));
        assert_eq!(dark.background, Color::Rgb(13, 17, 23));
    }

    #[test]
    fn auto_theme_falls_back_to_dark_without_background_signal() {
        let theme = Theme::from_env(ThemePreference::Auto, env(&[("COLORTERM", "truecolor")]));

        assert_eq!(theme.background, Color::Rgb(13, 17, 23));
    }

    #[test]
    fn compatible_light_theme_avoids_rgb_palette() {
        let theme = Theme::with_preference_and_color_mode(
            ThemePreference::Light,
            TerminalColorMode::Compatible,
        );

        assert_eq!(theme.background, Color::White);
        assert_eq!(theme.foreground, Color::Black);
        assert!(!matches!(theme.selection, Color::Rgb(..)));
    }

    #[test]
    fn theme_preference_parses_supported_values_only() {
        assert_eq!("dark".parse::<ThemePreference>(), Ok(ThemePreference::Dark));
        assert_eq!(
            "light".parse::<ThemePreference>(),
            Ok(ThemePreference::Light)
        );
        assert_eq!("auto".parse::<ThemePreference>(), Ok(ThemePreference::Auto));
        assert!("blue".parse::<ThemePreference>().is_err());
    }
}
