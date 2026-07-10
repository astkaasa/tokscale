# Pulse Snapshot V1

Status: implemented contract

This document defines the next Personal Pulse vertical slice. It turns the
current AI quota and WeRead summary into one versioned, evidence-backed local
snapshot consumed by the TUI, Markdown, JSON, and local web.

## Decision

The first slice delivers:

- WeRead Agent Gateway contract version 1.0.4.
- Connector-internal dataset health and partial-success sync, aggregated as one
  WeRead source in the snapshot.
- A serialized `PulseSnapshotV1` owned by `tokscale-core`.
- Weekly AI work and reading comparison with structured evidence.
- A read-only local Weekly Review backed by the same snapshot.

The data flow is:

```text
local AI usage + explicit WeRead sync
  -> normalized local datasets
  -> PulseSnapshotV1
  -> TUI / Markdown / JSON / local web
```

The snapshot describes observations, not a score of the user. Comparisons
prefer the user's previous comparable period. Any fixed threshold must appear
in the evidence that triggered the insight.

## Scope Boundary

V1 does not include a full WeRead client, annual reports, full notebook
pagination, whole-shelf progress, note content, browser writes, hosted sync,
LAN serving, remote assets, analytics, or a generic connector framework.

The first notebook page is sampled activity, not a global ranking. Total notes
use `reviewCount + noteCount + bookmarkCount`, but they are not an unreviewed
backlog. A backlog requires a later local review checkpoint.

The `weread-skills` package is an API contract reference and agent-side client.
Tokscale must not require it as a runtime dependency.

## Ownership

`tokscale-core` owns connector normalization, dataset health, cache and
snapshot serialization, evidence, deterministic insights, and Markdown/JSON
generation.

`tokscale-cli` owns credential resolution, adapting local usage and quota
caches, TUI runtime state, immutable web input assembly, routing, and
presentation.

TUI and web renderers consume injected state. They do not perform connector,
settings, cache, or scan I/O while rendering.

## Serialized Contract

```text
PulseSnapshotV1
  schemaVersion: 1
  snapshotId: string
  generatedAt: RFC3339 timestamp
  period: PulsePeriod
  sources: SourceHealth[]
  aiWork: AiWorkSignal
  readingInput: ReadingInputSignal
  knowledgeFlow: KnowledgeFlowSignal
  insights: PulseInsight[]
  evidence: PulseEvidence[]
  recommendations: PulseRecommendation[]
```

`snapshotId` is an opaque persisted identifier. Renderers preserve it so CLI
and web output can be shown to come from the same snapshot.

`PulsePeriod` contains `kind`, inclusive local `start`, exclusive local
`endExclusive`, and a `timezone` string. V1 serializes the machine's fixed UTC
offset at generation time, for example `+08:00`; it does not serialize or
promise an IANA timezone identifier. Raw values and units are authoritative;
formatted labels are presentation data.

Each newly generated snapshot is anchored to the machine's current local week
at generation time. A retained WeRead weekly dataset is projected into
current/previous reading fields only when its period matches that week. Stale
data from an older week remains source-health evidence and must not re-anchor
AI aggregation or be labelled as current reading. After a week rollover, a
previously persisted latest snapshot remains anchored to its older period
until another durable snapshot is committed.

### Connector Dataset Health And Snapshot Source Health

Inside the WeRead connector, every independently refreshed dataset uses:

```text
Dataset<T>
  value: T | null
  observedAt: RFC3339 timestamp | null
  freshness: fresh | stale | missing
  coverage: complete | partial | unknown
  issue: SourceIssue | null
```

`SourceIssue` has a stable code, sanitized message, and retry classification.
Initial codes are `auth_missing`, `upgrade_required`, `transport_error`,
`gateway_error`, `invalid_response`, and `normalization_error`.

These dataset records are connector and cache state, not per-dataset entries in
`PulseSnapshotV1.sources`. The snapshot derives one aggregate `weread`
`SourceHealth` with status `fresh`, `partial`, `stale`, `auth_missing`, `error`,
or `upgrade_required`. It is partial when usable data and a failed or partially
covered dataset coexist. Surfaces expose the aggregate status, coverage, and a
sanitized issue code; they do not identify which WeRead dataset failed.
Different connector datasets may have different stale windows.

### Domain Signals

`AiWorkSignal` includes weekly tokens, cost, active days, peak day, leading
model/provider, highest known quota pressure, and an optional previous-period
comparison. Missing usage or quota input is missing coverage, not zero.

`ReadingInputSignal` includes current and previous weekly periods, daily
buckets, read days, total seconds, natural-day average, week-over-week change,
current focus book, current month rhythm, category preference, and shelf
counts. Shelf visible count is:

```text
books + albums + (mp present ? 1 : 0)
```

V1 does not attach a media-type discriminator to book or album references.
Tokscale may retain an upstream `deepLink` locally but never synthesizes one.
Focus continuity and progress stay optional until bounded progress requests
and enough local history exist.

`KnowledgeFlowSignal` includes total note books, total notes, and the sampled
notebook page. It records `coverage: partial` while `hasMore` is true and names
the collection `recentNotebooks` or `sampledNotebooks`, never `topBooks`.
`noteDelta` is only a count change between comparable local snapshots. Note
text belongs in a later explicit `NoteExportBundle`, not this snapshot.

### Evidence

```text
PulseEvidence
  id, sourceId, signal, value, unit, period, observedAt, freshness
  comparator: optional previous value or threshold

PulseInsight
  id, level, title, summary, evidenceRefs[]

PulseRecommendation
  id, title, rationale, evidenceRefs[]
```

Every insight and recommendation references existing evidence IDs. Generation
is deterministic core logic and does not call an LLM. Wording describes an
observed deviation instead of making a moral judgment.

## WeRead Sync Contract

Every request sends `skill_version: "1.0.4"`, keeps business parameters at the
body top level, and sanitizes errors before they enter state or output.

Sync runs as follows:

1. Load last-known-good normalized datasets.
2. Resolve `WEREAD_API_KEY` from process environment, then settings.
3. Fetch current weekly data as the compatibility gate.
4. On top-level or nested `upgrade_info`, preserve old data, persist a
   structured blocking issue, stop the batch, and suppress automatic retry
   while the compiled connector version is unchanged.
5. After the gate succeeds, fetch previous week, current month, shelf, and the
   first notebook page concurrently.
6. Normalize results independently and merge successes with last-known-good
   data. A normal endpoint failure must not discard another successful result.
7. Commit connector cache and the resulting Pulse snapshot atomically.

If any concurrent response reports `upgrade_info`, no new connector data is
committed because the response contract may have changed. Other endpoint
failures produce partial health and retain usable data.

## Local Storage And Privacy

Normalized connector cache remains disposable:

```text
<config>/cache/weread-pulse-cache.json
```

Pulse snapshots are durable local telemetry:

```text
<config>/pulse/latest.json
<config>/pulse/history/<period-start>.json
```

Each durable snapshot commit atomically replaces the latest file and the
current period file. Existing WeRead cache schema v1 is accepted as
last-known-good migration input with its old timestamp and `coverage: unknown`;
it is rewritten in v2 form on the next durable snapshot commit, not only after
a successful connector sync.

Cache, history, and latest projections are committed under one process lock.
A user-only pending transaction journal contains the normalized WeRead state
and snapshot until all projections succeed. Readers replay that journal before
loading either cache or snapshot, so a crash cannot expose a half-committed
generation to Tokscale surfaces.

Concurrent writers compare connector and AI observation generations. An older
writer returns a superseded result instead of claiming success or replacing a
newer snapshot; callers use the durable winner or retry once with its AI state
and the incoming reading state. Legacy cache-only WeRead writers invalidate
`latest.json` before replacing the cache, so they cannot leave a mixed pair
visible after interruption.

API keys and raw Gateway responses are never stored. Files use user-only
permissions where supported. `TOKSCALE_CONFIG_DIR` continues to isolate cache
and durable Pulse state for tests.

Normal JSON and web output exclude note content, upstream error messages,
local paths, and connector deep links by default. Source failures are exported
as stable issue codes and retry/health state. A later agent-context projection
may redact book titles without changing this snapshot schema.

## Command And Surface Contract

Target commands:

```text
tokscale pulse sync
tokscale pulse --weekly [--refresh]
tokscale pulse --json [--refresh]
tokscale serve
```

`pulse sync` is the explicit connector network operation. Exports read the
latest durable snapshot verbatim by default; after a week rollover, that may
still be the last snapshot for an older period. `--refresh` runs the same sync
service first and generates a snapshot anchored to the current local week. TUI
entry may use configured background refresh and keeps `r` as manual refresh.
`serve` never triggers a remote connector request.

The TUI preserves the weekly rhythm as its primary reading visual, adds only a
compact AI/reading summary, source health, and a short attention queue, and
does not add report prose or long note lists.

Markdown sections are `AI Work`, `Reading Input`, `Knowledge Flow`, `Balance`,
`Evidence`, `Suggested Actions`, and `Source Health`. JSON serializes the same
snapshot and evidence IDs.

Local web keeps two distinct responsibilities:

- `/` and `/overview` retain Ratatui buffer projection for TUI parity.
- `/review` renders a semantic Weekly Review from `PulseSnapshotV1`.
- `/api/v1/pulse` returns the same versioned snapshot.
- `/exports/pulse.md` returns the same Markdown report as the CLI.

The web command builds immutable site inputs before serving. It freezes
Overview data, the render reference clock, and the durable Pulse snapshot at
startup, so the process must be restarted to see later syncs or local usage
changes. Request handling performs
no settings, cache, scan, or connector I/O. The Overview page uses a small
inline resize script to request a viewport-sized `/surface` projection and
loads no remote assets. Security headers cover framing, referrers, browser
permissions, content sniffing, and content security policy.

The HTTP server stays on `127.0.0.1`, accepts only `GET` and `HEAD`, and
validates local Host values, but it has no user authentication. Other users or
sandboxes on the same machine may be able to read served titles and IDs. Users
with that threat model should not run `tokscale serve`.

## Implementation Order

1. Add core contracts, 1.0.4 fixtures, dataset health, and cache migration.
2. Implement partial-success sync and current/previous weekly inputs.
3. Build the snapshot, evidence, insights, and Markdown/JSON renderers.
4. Adapt the compact TUI state and renderer.
5. Add immutable `WebSnapshot`, `/review`, JSON, and Markdown routes.
6. Update current-behavior docs after verification.

Bounded focus progress and Knowledge Inbox are later vertical slices. They
extend this contract rather than bypass it.

## Acceptance Criteria

- All WeRead requests use 1.0.4 and flat parameters.
- Upgrade responses stop the batch, preserve old data, and cannot auto-loop.
- A month, shelf, or notebook failure does not hide fresh weekly data.
- Every connector dataset tracks observed time, freshness, coverage, and a
  sanitized issue internally.
- Snapshot surfaces expose one aggregate WeRead source-health entry and do not
  name a failed dataset.
- Legacy migration never claims complete notebook coverage.
- AI work contains weekly local usage, not quota alone.
- Current and previous weekly data produce evidence-backed comparison.
- Every insight and recommendation references valid evidence IDs.
- Markdown, CLI JSON, and web API share one snapshot ID and raw metrics.
- `tokscale serve` performs no connector network request and requires a restart
  to show data committed after startup.
- Web remains loopback-only, read-only, free of remote assets and analytics,
  and rejects unsafe Host values.
- No key or raw Gateway response reaches cache, export, log, fixture,
  screenshot, or HTML.
- Focused normalization, sync, migration, schema, route, and renderer tests
  pass, followed by full fmt, clippy, core tests, and CLI tests.
- TUI and web changes receive actual wide and narrow render verification.
