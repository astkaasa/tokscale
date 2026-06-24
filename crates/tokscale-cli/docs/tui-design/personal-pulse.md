# Personal Pulse

Personal Pulse is the local signal layer that sits next to AI usage telemetry. Its job is to answer what needs attention now, with evidence, without turning the TUI into a general life dashboard.

Product boundary and surface strategy are defined in [personal-pulse-architecture.md](personal-pulse-architecture.md). This document records the current WeRead-backed implementation contract.

## Current Scope

The current Pulse workspace is WeRead-first.

It shows:

- weekly reading rhythm
- weekly total, day average, and week-over-week comparison
- focus book continuity
- monthly rhythm and category preference
- shelf and notes signals
- sync, stale, auth, error, and upgrade-required states

It does not try to replace WeRead. Long reading, note review, highlight export, and book management should happen outside the TUI through WeRead, Markdown, JSON, Obsidian, local web, or another companion surface.

## Surfaces

Pulse data is available through multiple local surfaces:

- TUI: `Pulse` workspace for compact inspection and manual refresh.
- Markdown: `tokscale pulse --weekly` for weekly review notes.
- JSON: `tokscale pulse --json` for scripts and agent context.

All surfaces should consume normalized core data. Rendering code should not perform network, cache, or settings I/O.

## Data Source

WeRead data is fetched through the WeRead Agent Gateway with `WEREAD_API_KEY`.

The normalized state lives in `tokscale_core::pulse::weread` and currently includes:

- `WeReadWeekly`
- `WeReadMonthly`
- `WeReadShelfSummary`
- `WeReadNotesSummary`
- `WeReadStatus`

Normalization rules:

- `checked_in` is `read_seconds >= 60`.
- Weekly timestamp buckets are converted to local dates.
- Durations stay as seconds in core and are formatted by presentation helpers.
- Shelf visible count is derived from normalized shelf items, not raw API payload size.
- Note totals combine the upstream note/review/bookmark counts before rendering.

## Credentials

Credential lookup order:

1. Process environment variable `WEREAD_API_KEY`.
2. `settings.json` `env.WEREAD_API_KEY`.

The TUI must stay quiet when the key is missing:

- show `auth missing`
- keep the rest of the app usable
- show the setup hint only in Pulse context

The key must not be written to caches, logs, tests, screenshots, panic messages, or normalized state.

## Cache

WeRead cache is stored under Tokscale's canonical cache directory:

```text
<config dir>/cache/weread-pulse-cache.json
```

The cache stores normalized state only. It must not store raw API responses.

Refresh behavior:

- render cached data immediately when present
- mark stale data instead of blocking the TUI
- run network refresh in background work
- keep last known data visible when refresh fails
- surface endpoint or upgrade errors inside Pulse, not globally

## TUI Behavior

The Pulse workspace uses a compact summary/detail shape:

- weekly table first
- focus book and month rhythm second
- shelf, notes, and recent books as supporting evidence
- sync/auth/error state in header/footer/status lines

Interaction:

- `p`: theme toggle, owned globally by the TUI
- `r`: refresh WeRead when Pulse is active
- mouse click on refresh/status targets triggers the same refresh action

Pulse should not add long scrollable book lists to the top-level view. If richer reading or notes workflows are needed, add an export or companion surface instead.

## Code Boundary

Current ownership:

- `tokscale_core::pulse::weread`: model, fetch, normalize, cache helpers.
- `tokscale_core::pulse::summary`: AI quota + reading digest model and Markdown/JSON summary.
- `tokscale-cli::commands::pulse`: CLI adapter for Markdown/JSON output.
- `tokscale-cli::tui::pulse_state`: TUI runtime state and background refresh.
- `tokscale-cli::tui::ui::pulse`: rendering only.

Future Pulse modules should follow the same order:

```text
core connector + normalized signal
  -> CLI adapter
  -> TUI runtime state
  -> renderer
  -> Markdown / JSON / local web export when useful
```

## Admission Criteria

Do not add a new Pulse module just because data is available. A module needs:

- a clear user question
- local-first storage or an explicit privacy model
- normalized signal types in core
- compact summary and evidence
- source health and stale/auth/error behavior
- one useful action or export
- tests for normalization and narrow TUI rendering

Reading is the first non-AI signal. Good next candidates are local notes, local work queues, PR/CI pressure, and agent/background task health. Health, finance, and broad lifestyle telemetry should stay out until the privacy and product boundaries are explicit.
