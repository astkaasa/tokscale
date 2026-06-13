use unicode_truncate::UnicodeTruncateStr;
use unicode_width::UnicodeWidthStr;

pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) fn truncate_display(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let (head, _) = text.unicode_truncate(max_width - 1);
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_display_respects_cjk_cell_width() {
        let truncated = truncate_display("Focus  万物发明指南  5h9m this week", 18);

        assert!(display_width(&truncated) <= 18, "{truncated}");
        assert!(truncated.ends_with('…'));
    }
}
