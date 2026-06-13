# Personal Pulse Product Architecture

Personal Pulse should not be defined as "more cards inside the TUI." The stronger product definition is:

```text
Tokscale Personal Pulse is a local-first personal telemetry layer for AI builders.
The TUI is its cockpit and power surface, not the product boundary.
```

This document defines the boundary so future modules do not turn the TUI into a crowded personal dashboard.

## Product Position

Tokscale already has a strong wedge: local AI usage, token, cost, model, and quota telemetry. Personal Pulse should extend that wedge into personal operating context: reading, notes, work queues, agents, attention, and eventually other local signals.

The durable value should live below any single interface:

```text
connectors
  -> local telemetry store
  -> normalized signals
  -> insights and actions
  -> TUI / Markdown / JSON / MCP / local web
```

The TUI remains important because Tokscale's first users are terminal-native AI builders. It should be the place to inspect the current state, understand anomalies, and trigger actions quickly.

## Layer Model

### Local Telemetry Core

This is the product core. It should be independent from Ratatui layout decisions.

Responsibilities:

- Connector configuration and health.
- Local cache or event-store ownership.
- Normalized signal models.
- Privacy and redaction rules.
- Refresh policies.
- Insight generation.
- Action registry.
- Agent-readable exports.

Examples:

- `AI usage` becomes cost pace, quota risk, model mix, and project or source drivers.
- `WeRead` becomes reading rhythm, focus book continuity, note volume, topic preference, and stale/auth state.
- `GitHub` should become review pressure, blocked PRs, CI health, and stale work signals rather than a generic GitHub mirror.

### Power TUI

The TUI is the cockpit. It should answer four questions:

- What is happening now?
- What needs attention?
- Why did it happen?
- What can I do next?

Good TUI surfaces:

- Dense status overviews.
- Module detail pages.
- Inspectors for source health and evidence.
- Lists, logs, queues, and short trends.
- Keyboard-first actions.
- Connector health and refresh state.

Poor TUI-only surfaces:

- Complex onboarding and auth flows.
- Long-form reading.
- Large note or highlight review.
- Rich graph exploration.
- Drag-and-drop configuration.
- Long weekly reports.
- Cross-device reminders.
- Sharing and collaboration.

The TUI should use a stable interaction shape:

```text
Summary -> Detail -> Inspector -> Action
```

Each module should provide compact summary signals, a detail view with evidence, an inspector for source/cache/auth state, and a small set of safe actions.

### Companion Surfaces

Companion surfaces let the same local telemetry escape the terminal without turning the TUI into a catch-all interface.

Near-term surfaces:

- Markdown weekly digest.
- JSON summary for scripts and agents.

Later surfaces:

- MCP resources/tools.
- Local web report.
- Raycast or launcher entry.
- Menubar summary.
- Obsidian export.
- GitHub issue or task export.

These surfaces should be generated from the same normalized signals as the TUI. They should not reimplement connector logic.

## TUI Scope Rules

A feature belongs in the TUI when the primary workflow is:

- Scan a state.
- Notice an anomaly.
- Triage a short queue.
- Inspect a compact explanation.
- Trigger an action.

A feature should not be TUI-only when the primary workflow is:

- Read long content.
- Edit long content.
- Configure credentials or privacy in detail.
- Explore rich visualizations.
- Review a long historical report.
- Receive a reminder while away from the terminal.
- Share or publish an artifact.

Use this rule to keep future modules small. For example, WeRead belongs in the TUI as a pulse view, not as a full reading or note-review product.

## WeRead Boundary

WeRead is the first non-AI module because it represents input quality. It should create a contrast with Tokscale's AI output telemetry:

```text
Did this week only burn tokens and produce code,
or did it also replenish high-quality input and durable notes?
```

The TUI should show:

- Weekly reading grid.
- Weekly total and read-day count.
- Week-over-week comparison.
- Focus book.
- Month rhythm summary.
- Topic preference.
- Shelf and note signals.
- Sync/auth/stale state.

The TUI should not become:

- A full note reader.
- A book manager.
- A long-form monthly report.
- A knowledge-card editor.

Those workflows should go to Markdown, Obsidian, JSON, local web, or explicit export actions.

## First Non-TUI Exits

Personal Pulse should get lightweight non-TUI exits before adding many more modules.

### Markdown Weekly Digest

Target command shape:

```bash
tokscale pulse --weekly > weekly-pulse.md
```

Target output:

```markdown
# Weekly Pulse

## AI Work
Cost and quota movement, model mix, and notable spikes.

## Reading
Read days, total duration, focus book, and note volume.

## Balance
Whether output pressure and input quality were balanced.

## Suggested Actions
- Review unprocessed notes
- Cap expensive agent runs
- Protect reading blocks
```

The digest is for Obsidian, weekly review, and human reflection. It can include more text than the TUI.

### JSON / Agent-Readable Summary

Target command shape:

```bash
tokscale pulse --json
```

Target output shape:

```json
{
  "aiCostStatus": "elevated",
  "quotaRisk": "medium",
  "readingRhythm": "stable",
  "focusBook": "Example",
  "knowledgeBacklog": 8,
  "recommendedConstraints": [
    "avoid expensive agent runs unless urgency is explicit",
    "suggest note review before large architecture work"
  ]
}
```

The JSON output is for automation, future MCP support, and agents. It should be compact, stable, and evidence-backed.

## Module Admission Criteria

Do not add a new Personal Pulse module just because data is available. A module should have:

- A clear user question.
- A local-first data source or explicit privacy model.
- A normalized signal model.
- A compact TUI summary.
- A detail/inspector story.
- At least one useful action or export.
- A plan for non-TUI output if the content is long or report-like.

This keeps the product from becoming a lifestyle card wall.

## Implementation Direction

Near-term sequence:

1. Keep WeRead as the first proof of a non-AI signal in the cockpit.
2. Factor shared Pulse signal types only after the second module creates real duplication.
3. Add Markdown and JSON Pulse exports before expanding into many connectors.
4. Keep connector credentials in user configuration or OS credential stores, never in caches, test fixtures, screenshots, or logs.
5. Use the TUI to show evidence and actions; use companion surfaces for long-form consumption and agent context.

The main product risk is overfitting to TUI aesthetics. The main product opportunity is owning the local personal telemetry layer that both humans and agents can read.
