# Local Telemetry Ledger

Status: baseline implemented; source-aware adapters continue incrementally

Tokscale preserves local AI usage independently from disposable render and
parser caches. The personal fork is Codex-first for account actions, but it
remains multi-agent for local historical telemetry.

## User Promise

- Existing local AI usage remains queryable after a source application is
  removed, a parser is retired, or its local files disappear.
- Local parsers do not trigger network requests.
- Remote quota and connector requests are explicit and independently
  configurable.
- Stored events retain enough provenance to explain client, model, provider,
  token totals, and cost treatment.
- TUI, reports, Pulse refresh, and local web consume the same normalized
  history.

## Source Classes

Sources declare one of four behaviors:

1. `local_parser`: reads durable files or databases owned by a local AI agent.
2. `archive_import`: imports a user-owned snapshot such as a Cursor usage CSV.
3. `remote_connector`: performs an explicit sync and writes normalized local
   state.
4. `quota_provider`: reads current account, balance, or reset state for the
   operational Usage workspace.

The first two participate in historical ingestion. The latter two are never
invoked merely because a parser is enabled or a credential was discovered.
Quota observations are written only by an explicit Usage refresh or while the
existing Usage auto-refresh setting is enabled.

## Storage Boundary

Durable normalized history lives at:

```text
<config>/data/telemetry.sqlite
```

Explicit source archives live outside cache storage:

```text
<config>/archive/<source>/...
```

The TUI and source-message caches remain disposable acceleration:

```text
<config>/cache/tui-data-cache.json
<config>/cache/source-message-cache.bin
```

Deleting `<config>/cache` must not delete historical telemetry. Tokscale does
not copy every local transcript into its own archive. Raw archival is reserved
for explicit imports and sources whose upstream data is otherwise ephemeral.

## Event Contract

The schema separates canonical events from observations so several sources can
point at one event without duplicating usage:

```text
telemetry_ingest_runs
  ordered run id, start/finish time, parser-set version

telemetry_sources
  source id/kind/ref, parser version, authoritative flag, health

telemetry_events
  deterministic event id and identity components
  client/provider/model/session/workspace/time/tokens/duration/turn metadata
  cost kind: reported | estimated | unknown, currency, pricing provenance
  first/last observed run and normalization version

telemetry_event_sources
  event-to-source mapping, source record ref, present | missing state

account_daily_usage
  official account-level token buckets keyed by provider, account, and date

account_usage_summaries
  latest official lifetime, peak-day, longest-turn, and streak values

quota_observations
  local timestamped samples of provider quota windows and reset deadlines

quota_reset_events
  confirmed local resets, legacy inferred rollovers, and inferred scheduled or
  early provider-window rollovers
```

Account activity is deliberately separate from canonical local AI events. The
official daily token buckets are account-level and do not expose input, output,
or cache-token breakdowns. Quota consumption between samples is shown as an
observed lower bound, and coverage gaps remain visible instead of being
interpolated. These rows support compact operational inspection in Usage; they
do not contribute to local totals, cost reports, or historical model analysis.

Identity is explicit rather than guessed by the store:

- `native`: immutable upstream event id within a namespace and scope.
- `raw_record`: content digest plus occurrence within an account/source scope.
- `snapshot_bucket`: stable bucket for mutable cumulative observations.

The bridge from existing parsers only persists clients with a proven native or
raw-record identity. Mutable sources such as Trae, Warp, Mux, Goose, Hermes,
Droid, Zed, and Crush remain live-only until they have source-specific
`snapshot_bucket` adapters. This prevents refreshes from double-counting a
cumulative snapshot.

Tokens and reported cost are observations. Estimated cost includes pricing
provenance. A later estimate cannot replace a source-reported charge, and an
unknown cost cannot erase an existing estimate.

## Ingestion And Reconciliation

- Ingestion is idempotent and upserts by deterministic event id.
- Newer runs may correct normalized fields; an older concurrent run cannot
  overwrite them.
- A successful authoritative scan may mark unseen source mappings missing. It
  does not delete canonical history.
- Failed and non-authoritative scans preserve prior source presence.
- Hard deletion requires an explicit future action and is never part of
  refresh.
- SQLite writes are transactional, use foreign keys and WAL, and reject files
  with an unrelated application id or newer schema.

The compatibility bridge is non-authoritative because the flattened legacy
message vector no longer carries physical parser failures. It upserts safe
events, performs a structural parity check, reads durable history, and merges
live-only messages. A ledger failure falls back to current parser output. The
public parse-only APIs stay unchanged; standard CLI reports, TUI, Pulse refresh,
and local web opt into the ledger explicitly, while `--home` profiles remain
isolated.

Parity diagnostics contain only fixed status and issue codes. They do not emit
personal paths, source ids, dates, models, counts, totals, or raw errors.

## Cursor History

Cursor cleanup retains its data plane and removes its control plane:

- Preserve `ClientId::Cursor`, provider normalization, colors, and a read-only
  CSV parser because historical rows depend on them.
- `tokscale cursor import` copies current CSV files into a content-addressed
  object store with account-scoped manifests and checksums. It never deletes or
  rewrites the source CSV.
- Import verifies row count, time range, token totals, cost, SQLite integrity,
  and source health before reporting success.
- Ambiguous sanitized account ids or unresolved duplicate files are archived
  but not imported until reviewed.
- Cursor login, logout, account switching, API sync, automatic refresh, network
  endpoints, and sync warnings are not part of the current fork.
- Historical rows remain `client = cursor`, including records imported under a
  stable `legacy-active` account scope when only one old CSV exists.

## Source And Provider Policy

Local parsers remain available and auto-detected. Provider identity, pricing,
and colors remain generic, so Cursor or DeepSeek may appear in locally parsed
OpenCode data without a corresponding remote integration.

Operational network providers use `usage.enabledProviders`. A missing field
defaults to Codex; an explicit empty list disables all remote quota requests.
Legacy `usage.excludedProviders` remains readable and is applied after the
allowlist. Credential discovery is only called for the resulting effective
set, so an unrelated credential cannot trigger network activity.

## Deferred Operational Providers

These are later Usage workspace tasks, not ledger blockers:

- **DeepSeek API credit balance**: use the documented read-only
  `GET /user/balance` endpoint. Show currency, total, granted, topped-up, and
  availability without inventing a percentage bar. Implement and propose as a
  focused upstream PR from `origin/main`.
- **OpenCode Go usage**: retain local `provider = opencode-go` history now. A
  Usage row may expose a clearly labelled local estimate only after its window
  semantics are verified. Do not scrape private Console endpoints. Prefer an
  official quota endpoint when one becomes available, then propose a separate
  upstream PR.

## Implementation Sequence

1. Completed: SQLite schema, deterministic identity, run ordering, source
   health, cost provenance, and idempotency tests in `tokscale-core`.
2. Completed: compatibility ingestion and privacy-safe structural parity.
3. Completed: safe Cursor archive/import and removal of its remote control
   plane.
4. Completed: ledger-backed standard reports and the shared DataLoader used by
   TUI, Pulse refresh, and local web.
5. Completed: explicit remote Usage provider allowlist with legacy denylist
   compatibility.
6. Completed: account-level summaries and daily buckets, quota observations,
   and reset events with an inline Usage account inspector.
7. Next: replace the compatibility bridge client by client with physical
   source observations and snapshot adapters.
8. Later: add DeepSeek balance and evaluate OpenCode Go quota support as
   separate operational features.

## Remaining Engineering Questions

- Define `snapshot_bucket` identities and authoritative scan outcomes for each
  mutable source before persisting it.
- Decide whether explicit prune belongs in a CLI command or a future source
  inspector. Refresh never hard-deletes history.
- Move source-reported versus estimated cost classification out of the legacy
  bridge as each source adapter becomes provenance-aware.
