# Local Web Surface

The local web surface is a companion surface for Tokscale, not a replacement for the TUI and not a hosted web dashboard. Its first job is to render the Overview screen from the same local telemetry and the same TUI rendering path.

## Current Slice

`tokscale serve` starts a localhost-only HTTP server and renders:

- The Overview tab as a styled HTML projection of the Ratatui buffer.
- A small JSON summary at `/data.json`.

The current slice intentionally does not include Today mode, authentication setup, long-form Pulse reports, JavaScript interactions, remote assets, or a bundled frontend toolchain.

The local web surface also intentionally does not embed xterm.js, a PTY, or a real terminal emulator. It renders the TUI output as HTML; it does not run a terminal in the browser.

## Boundary

Good local web responsibilities:

- Render compact reports that benefit from browser inspection, screenshots, or later companion interactions.
- Make generated output easier to inspect, screenshot, and compare.
- Serve read-only local data from `127.0.0.1`.
- Reuse normalized Tokscale data instead of duplicating connector logic.
- Reuse TUI rendering contracts where visual parity matters.

Poor local web responsibilities for the first phase:

- Hosted sync.
- Public sharing.
- Complex connector onboarding.
- Rich application state.
- Remote CSS, fonts, scripts, or analytics.
- Reintroducing a separate npm frontend layer before the renderer contract is stable.
- Maintaining a second hand-built DOM layout for screens that already exist in the TUI.

## Implementation Shape

The local web path should stay adapter-shaped:

```text
local scan / normalized data
  -> TUI app state
  -> Ratatui TestBackend buffer
  -> styled HTML cell spans
  -> localhost server
```

The renderer should consume the same app/report/Pulse state used by other surfaces. HTML code should not perform connector I/O.

For screens whose primary value is visual parity with the TUI, prefer a buffer projection:

```text
App input state
  -> tui::ui::render
  -> ratatui::buffer::Buffer
  -> HTML rows and styled cell runs
```

This is not terminal scraping. It is a direct renderer-level projection from the same in-memory buffer Ratatui would draw to a terminal backend.

Current code placement:

- `commands::serve`: CLI adapter, filter resolution, startup scan.
- `tui::surface`: render helpers that project the TUI into a Ratatui buffer.
- `web::overview`: Overview HTML surface renderer and JSON summary.
- `web::server`: minimal localhost HTTP response layer.

Do not add a separate `web::templates` dashboard layout for Overview unless the product explicitly decides to diverge from TUI parity.

## Interaction Boundary

If interaction is added, the TUI should remain the source of behavior:

```text
Browser key/click/wheel event
  -> local HTTP event endpoint
  -> existing App input handler
  -> rerender Ratatui buffer
  -> replace HTML surface
```

Keyboard events should map to `App::handle_key_event`. Mouse clicks should convert browser pixels to terminal cell coordinates and then use `App::handle_mouse_event`, which already consumes `click_areas` registered during render.

Low-risk interactions are tab switching, sorting, selection movement, drilldown enter/escape, chart granularity changes, and scrolling. Actions with external side effects, such as login flows or connector refreshes, need explicit product review before being exposed through the web surface.

## Security And Privacy

Default behavior must remain local-first:

- Bind to `127.0.0.1`.
- Do not load remote assets.
- Do not add tracking or external analytics.
- Keep output inspectable as plain HTML and JSON.
- Treat future LAN or sharing modes as explicit opt-in features.

## Out Of Scope For Current Slice

Keep these out until the Overview projection and JSON contract are stable:

- hosted sync or sharing
- connector setup and login flows
- remote assets or analytics
- a separate hand-authored Overview DOM layout
- browser-triggered refreshes with external side effects
- a richer local web dashboard that bypasses the normalized data contracts
