use ratatui::buffer::Buffer;

use super::{
    html::{cell_text, escape_html, HtmlCellStyle, HtmlColorPalette},
    overlay::{
        chart_overlay_region, overlay_cell_replacement, provider_mix_bar_overlay,
        render_block_overlay, BlockOverlayRegion, ProviderMixBarOverlay,
    },
    page::terminal_style,
};

pub(super) fn render_buffer_surface(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    format!(
        r#"<pre class="terminal-screen" role="img" aria-label="Tokscale Overview rendered from the TUI buffer" data-cols="{width}" data-rows="{height}" style="{terminal_style}">{surface}<span class="terminal-overlay" aria-hidden="true">{overlay}</span></pre>"#,
        width = width,
        height = height,
        terminal_style = terminal_style(width, height, palette),
        surface = render_buffer_html(buffer, width, height, palette),
        overlay = render_block_overlay(buffer, width, height, palette),
    )
}

pub(super) fn render_buffer_html(
    buffer: &Buffer,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    let overlay_region = chart_overlay_region(buffer, width, height);
    let provider_mix_bar = provider_mix_bar_overlay(buffer, width, height, palette);
    render_buffer_html_with_region(
        buffer,
        width,
        height,
        overlay_region,
        provider_mix_bar.as_ref(),
        palette,
    )
}

pub(super) fn render_buffer_html_with_region(
    buffer: &Buffer,
    width: u16,
    height: u16,
    overlay_region: Option<BlockOverlayRegion>,
    provider_mix_bar: Option<&ProviderMixBarOverlay>,
    palette: &HtmlColorPalette,
) -> String {
    let mut html = String::new();
    for y in 0..height {
        html.push_str(r#"<span class="terminal-row">"#);
        let mut x = 0;
        while x < width {
            let cell = &buffer[(x, y)];
            let style = HtmlCellStyle::from_cell(cell, palette);
            if let Some(replacement_style) =
                overlay_cell_replacement(buffer, overlay_region, provider_mix_bar, x, y, palette)
            {
                html.push_str(&replacement_style.open_span());
                html.push(' ');
                html.push_str("</span>");
                x += 1;
                continue;
            }

            let mut text = cell_text(cell);
            x += 1;

            while x < width {
                let next = &buffer[(x, y)];
                let next_style = HtmlCellStyle::from_cell(next, palette);
                if next_style != style
                    || overlay_cell_replacement(
                        buffer,
                        overlay_region,
                        provider_mix_bar,
                        x,
                        y,
                        palette,
                    )
                    .is_some()
                {
                    break;
                }
                text.push_str(&cell_text(next));
                x += 1;
            }

            html.push_str(&style.open_span());
            html.push_str(&escape_html(&text));
            html.push_str("</span>");
        }
        html.push_str("</span>");
    }
    html
}
