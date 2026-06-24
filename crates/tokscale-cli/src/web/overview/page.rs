use super::html::HtmlColorPalette;

pub(super) fn render_buffer_page(
    surface: &str,
    width: u16,
    height: u16,
    palette: &HtmlColorPalette,
) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Tokscale Overview</title>
  <style>
{css}
  </style>
</head>
<body>
  <main class="viewport" aria-label="Tokscale terminal overview">
    <span id="cell-probe" aria-hidden="true">00000000000000000000</span>
    <div id="terminal-container">{surface}</div>
  </main>
  <script>
{script}
  </script>
</body>
</html>
"#,
        css = terminal_css(palette),
        surface = surface,
        script = resize_script(width, height),
    )
}

pub(super) fn terminal_style(width: u16, height: u16, palette: &HtmlColorPalette) -> String {
    format!(
        "--terminal-cols:{width};--terminal-rows:{height};--bg:{bg};--fg:{fg};--muted:{muted};",
        bg = palette.bg,
        fg = palette.fg,
        muted = palette.muted,
    )
}

fn resize_script(initial_width: u16, initial_height: u16) -> String {
    format!(
        r#"    (() => {{
      const minCols = 40;
      const maxCols = 320;
      const minRows = 16;
      const maxRows = 120;
      const pad = 8;
      const probe = document.getElementById('cell-probe');
      const container = document.getElementById('terminal-container');
      let currentKey = '{initial_width}x{initial_height}';
      let resizeTimer = 0;
      let controller = null;

      const clamp = (value, min, max) => Math.max(min, Math.min(max, value));

      const measuredCell = () => {{
        const rect = probe.getBoundingClientRect();
        const width = Math.max(1, rect.width / Math.max(1, probe.textContent.length));
        const height = Math.max(1, rect.height);
        return {{ width, height }};
      }};

      const terminalSize = () => {{
        const cell = measuredCell();
        const cols = clamp(Math.floor((window.innerWidth - pad * 2) / cell.width), minCols, maxCols);
        const rows = clamp(Math.floor((window.innerHeight - pad * 2) / cell.height), minRows, maxRows);
        return {{ cols, rows, key: `${{cols}}x${{rows}}` }};
      }};

      const refresh = async () => {{
        const size = terminalSize();
        if (size.key === currentKey) {{
          return;
        }}
        if (controller) {{
          controller.abort();
        }}
        controller = new AbortController();
        try {{
          const response = await fetch(`/surface?cols=${{size.cols}}&rows=${{size.rows}}`, {{
            signal: controller.signal,
            cache: 'no-store',
          }});
          if (!response.ok) {{
            return;
          }}
          container.innerHTML = await response.text();
          currentKey = size.key;
        }} catch (error) {{
          if (error.name !== 'AbortError') {{
            console.warn('Tokscale surface refresh failed', error);
          }}
        }}
      }};

      const schedule = () => {{
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(refresh, 80);
      }};

      window.addEventListener('resize', schedule, {{ passive: true }});
      refresh();
    }})();"#,
        initial_width = initial_width,
        initial_height = initial_height,
    )
}

fn terminal_css(palette: &HtmlColorPalette) -> String {
    const TERMINAL_LINE_HEIGHT: f64 = 1.0;
    format!(
        r#"    :root {{
      color-scheme: {color_scheme};
      --bg: {bg};
      --fg: {fg};
      --muted: {muted};
      --pad: 8px;
      --terminal-font-size: 12px;
      --terminal-line-height: {TERMINAL_LINE_HEIGHT:.2}em;
    }}

    * {{
      box-sizing: border-box;
    }}

    html,
    body {{
      width: 100%;
      height: 100%;
      background: var(--bg);
    }}

    body {{
      margin: 0;
      overflow: hidden;
      background: var(--bg);
      color: var(--fg);
      font-family: Menlo, Monaco, "SF Mono", SFMono-Regular, Consolas, "Liberation Mono", monospace;
      font-variant-ligatures: none;
      font-variant-numeric: tabular-nums;
      font-feature-settings: "liga" 0, "calt" 0, "kern" 0;
      letter-spacing: 0;
      -webkit-font-smoothing: antialiased;
      text-rendering: geometricPrecision;
    }}

    .viewport {{
      position: fixed;
      inset: 0;
      width: 100vw;
      height: 100vh;
      margin: 0;
      overflow: auto;
      background: var(--bg);
    }}

    .terminal-screen {{
      display: block;
      position: relative;
      width: max-content;
      min-width: 100vw;
      min-height: 100vh;
      margin: 0;
      padding: var(--pad);
      background: var(--bg);
      color: var(--fg);
      font: inherit;
      font-size: var(--terminal-font-size);
      line-height: var(--terminal-line-height);
      white-space: pre;
      tab-size: 1;
    }}

    .terminal-row {{
      display: block;
      height: var(--terminal-line-height);
      white-space: pre;
    }}

    .terminal-row > span {{
      white-space: pre;
    }}

    .terminal-overlay {{
      position: absolute;
      left: var(--pad);
      top: var(--pad);
      z-index: 2;
      pointer-events: none;
    }}

    .terminal-block-rect {{
      position: absolute;
      width: 1ch;
    }}

    .terminal-mix-bar-segment {{
      position: absolute;
      z-index: 3;
      height: calc(var(--terminal-line-height) * .72);
      pointer-events: none;
    }}

    .terminal-heatmap-cell {{
      position: absolute;
      z-index: 3;
      pointer-events: none;
      border-radius: 2px;
    }}

    .terminal-heatmap-cell.empty {{
      box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.055);
    }}

    .terminal-heatmap-cell.active {{
      box-shadow:
        inset 0 0 0 1px rgba(255, 255, 255, 0.10),
        0 0 0 1px rgba(0, 0, 0, 0.10);
    }}

    .terminal-heatmap-axis-label {{
      position: absolute;
      z-index: 4;
      color: var(--muted);
      line-height: var(--terminal-line-height);
      pointer-events: none;
    }}

    .terminal-heatmap-legend {{
      position: absolute;
      z-index: 4;
      display: inline-flex;
      align-items: center;
      gap: 4px;
      color: var(--muted);
      line-height: var(--terminal-line-height);
      pointer-events: none;
      white-space: nowrap;
    }}

    .terminal-heatmap-legend-cell {{
      display: inline-block;
      width: 10px;
      height: 10px;
      border-radius: 2px;
      box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    }}

    #cell-probe {{
      position: fixed;
      left: -9999px;
      top: -9999px;
      visibility: hidden;
      white-space: pre;
      font: inherit;
      font-size: var(--terminal-font-size);
      line-height: var(--terminal-line-height);
    }}
"#,
        bg = palette.bg,
        fg = palette.fg,
        muted = palette.muted,
        color_scheme = palette.color_scheme,
    )
}
