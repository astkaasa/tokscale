# Local Web Surface

The local web surface is a companion surface for Tokscale, not a replacement for the TUI and not a hosted web dashboard. It serves both TUI-parity operational views and longer semantic Pulse review from immutable local data.

## Current Slice

`tokscale serve` starts a localhost-only HTTP server and renders:

- `/` and `/overview`: the Overview tab as a styled HTML projection of the Ratatui buffer.
- `/surface`: a dynamically sized projection using the same TUI renderer.
- `/data.json`: the compact Overview JSON summary.
- `/review`: a responsive Weekly Review generated from `PulseSnapshotV1`.
- `/api/v1/pulse`: the versioned Pulse snapshot as JSON.
- `/exports/pulse.md`: the same Markdown report exposed by the CLI.

Pulse routes are present only when a durable local snapshot exists. The command reads that snapshot and never performs a connector refresh. Overview data, its Today reference clock, and the Pulse snapshot are frozen at startup; restart `tokscale serve` to see later syncs or local usage changes.

The current slice intentionally does not include user authentication, browser writes, interactive mode switching, application interactions beyond resize projection, remote assets, or a bundled frontend toolchain. A frozen Today projection is available when `tokscale serve` starts with the Today date scope.

The Overview page contains a small inline script that measures the viewport, requests a matching `/surface` projection, and repeats that request after resize. It loads no remote scripts, styles, fonts, or other assets.

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

Overview stays buffer-projection-shaped:

```text
local scan / normalized data
  -> TUI app state
  -> Ratatui TestBackend buffer
  -> styled HTML cell spans
  -> localhost server
```

Weekly Review stays snapshot-projection-shaped:

```text
PulseSnapshotV1
  -> semantic review / JSON / Markdown
  -> localhost server
```

`commands::serve` assembles frozen site inputs before accepting requests. Request handling and rendering perform no connector, settings, cache, or scan I/O, so later local commits are not visible until the server restarts.

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
- `web::review`: semantic Pulse Weekly Review renderer.
- `web::server`: minimal localhost HTTP response layer.

Do not add a separate `web::templates` dashboard layout for Overview unless the product explicitly decides to diverge from TUI parity.

## Interaction Boundary

If stateful application interaction is added, the TUI should remain the source of behavior:

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

`tokscale serve` has no user authentication. Loopback binding and Host validation limit network exposure, but they are not an access boundary between users, processes, or sandboxes on the same machine. Those peers may be able to request served pages and read book or model titles and snapshot or source IDs. Users with that threat model should not run `tokscale serve`.

- Bind to `127.0.0.1`.
- Accept only `GET` and `HEAD`, and reject missing, duplicate, or non-local Host values.
- Do not load remote assets.
- Do not add tracking or external analytics.
- Keep output inspectable as plain HTML and JSON.
- Send no-store and browser hardening headers.
- Treat future LAN or sharing modes as explicit opt-in features.

## Out Of Scope For Current Slice

Keep these out until a later explicit product decision:

- hosted sync or sharing
- connector setup and login flows
- remote assets or analytics
- a separate hand-authored Overview DOM layout
- browser-triggered refreshes with external side effects
- a richer local web dashboard that bypasses the normalized data contracts
