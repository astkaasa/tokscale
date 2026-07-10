# Personal Pulse

Personal Pulse is the local signal layer that sits next to AI usage telemetry. Its job is to answer what needs attention now, with evidence, without turning the TUI into a general life dashboard.

Product boundary and surface strategy are defined in [personal-pulse-architecture.md](personal-pulse-architecture.md). This document records the current WeRead-backed implementation contract.

## Current Scope

The current Pulse snapshot combines local AI work with WeRead input. It shows:

- current and previous weekly AI usage, active days, cost, leading model/provider, and known quota pressure
- weekly reading rhythm and period comparison
- focus book, monthly rhythm, category preference, and shelf state
- sampled notebook activity and aggregate note counts
- aggregate source freshness, coverage, auth, partial, error, and upgrade-required health

It does not try to replace WeRead. Long reading, note review, highlight export, and book management should happen outside the TUI through WeRead, Markdown, JSON, Obsidian, local web, or another companion surface.

## Surfaces

Pulse data is available through multiple local surfaces:

- TUI: `Pulse` workspace for compact inspection and manual refresh.
- Markdown: `tokscale pulse --weekly` for weekly review notes.
- JSON: `tokscale pulse --json` for scripts and agent context.
- Local web: `/review`, `/api/v1/pulse`, and `/exports/pulse.md` under `tokscale serve`.

Markdown, JSON, TUI, and web consume the same versioned `PulseSnapshotV1`. Rendering code does not perform network, cache, or settings I/O.

Snapshots are anchored to the machine's current local week when generated. Exports read the latest durable local snapshot by default, so after a week rollover they may return the last older period until a new snapshot is committed. Use `tokscale pulse sync` for an explicit connector refresh, or add `--refresh` to an export.

`tokscale serve` never refreshes connectors. It freezes the Pulse snapshot, Overview data, and Today reference clock at startup; restart it to see later syncs or local usage changes.

## Data Source

WeRead data is fetched through the WeRead Agent Gateway with `WEREAD_API_KEY`.

The normalized state lives in `tokscale_core::pulse::weread`. Current week, previous week, month, shelf, and sampled notebooks are independent connector-internal datasets with an observed time, freshness, coverage, and sanitized issue. `PulseSnapshotV1` folds those states into one aggregate `weread` source-health entry; TUI, Markdown, JSON, and web surfaces do not identify which WeRead dataset failed.

Normalization rules:

- `checked_in` is `read_seconds >= 60`.
- Weekly timestamp buckets are converted to local dates.
- Durations stay as seconds in core and are formatted by presentation helpers.
- Shelf visible count is derived from normalized shelf items, not raw API payload size.
- Note totals combine the upstream note/review/bookmark counts before rendering.
- A failed secondary endpoint retains its last-known-good dataset and produces partial aggregate WeRead source health.
- Any compatibility upgrade response rejects all new values from that sync batch.
- Retained weekly data is used as current/previous reading only when its dates align with the machine's current local week.

## Credentials

Credential lookup order:

1. Process environment variable `WEREAD_API_KEY`.
2. `settings.json` `env.WEREAD_API_KEY`.

The TUI must stay quiet when the key is missing:

- show `auth missing`
- keep the rest of the app usable
- show the setup hint only in Pulse context

The key must not be written to caches, logs, tests, screenshots, panic messages, or normalized state.

## Local Storage

WeRead cache is stored under Tokscale's canonical cache directory:

```text
<config dir>/cache/weread-pulse-cache.json
```

The cache stores normalized state only. It does not store raw API responses.

Durable snapshots are stored separately:

```text
<config dir>/pulse/latest.json
<config dir>/pulse/history/<period-start>.json
```

Writes are user-only where supported. Cache, history, and latest are protected by a process lock and recoverable pending transaction; readers replay an interrupted commit before returning data. Snapshot exports exclude API keys, upstream error messages, raw Gateway responses, local paths, and note content, and expose stable issue codes instead.

Refresh behavior:

- render cached data immediately when present
- mark stale data instead of blocking the TUI
- allow only fresh, complete default-scope AI data to seed the durable Pulse snapshot
- run TUI network refresh in background work
- keep last known data visible when refresh fails
- surface aggregate source or upgrade errors inside Pulse, not globally
- suppress automatic retry after an upgrade-required response until the compiled connector version changes

## TUI Behavior

The Pulse workspace remains a compact cockpit:

- weekly table first
- a short AI/reading summary
- focus book, month rhythm, shelf, and notes as supporting evidence
- sync/auth/error state in header/footer/status lines

Interaction:

- `p`: theme toggle, owned globally by the TUI
- `r`: refresh WeRead when Pulse is active
- mouse click on refresh/status targets triggers the same refresh action

Pulse should not add long scrollable book lists to the top-level view. If richer reading or notes workflows are needed, add an export or companion surface instead.

## Code Boundary

Current ownership:

- `tokscale_core::pulse::weread`: model, fetch, normalize, cache helpers.
- `tokscale_core::pulse::summary`: versioned AI work, reading, knowledge-flow, evidence, and deterministic digest model.
- `tokscale_core::pulse::store`: atomic latest/history snapshot persistence.
- `tokscale-cli::commands::pulse`: CLI adapter for Markdown/JSON output.
- `tokscale-cli::tui::pulse_state`: TUI runtime state and background refresh.
- `tokscale-cli::tui::ui::pulse`: rendering only.
- `tokscale-cli::web::review`: semantic long-form weekly review from the same snapshot.

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
