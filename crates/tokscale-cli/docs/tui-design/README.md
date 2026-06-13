# TUI Design Mockups

These notes and images describe the local TUI redesign branch. They are product and implementation references, not pixel-perfect contracts.

Status labels:

- `Implemented`: represented in the current branch.
- `Design target`: directionally designed, partially implemented or still evolving.
- `Future`: not intended for the current implementation slice.

## Cross-Cutting Notes

- Navigation and workspace strategy: [navigation.md](navigation.md) (`Design target`)
- Overview dashboard strategy: [overview.md](overview.md) (`Implemented / Design target`)
- Provider color identity rules: [provider-colors.md](provider-colors.md) (`Implemented`)

## Personal Pulse

Status: `Implemented / Design target`

Product architecture: [personal-pulse-architecture.md](personal-pulse-architecture.md)

Detailed WeRead-first plan: [personal-pulse.md](personal-pulse.md)

## Overview

Status: `Implemented / Design target`

All-time/range dashboard target for the default Overview mode.

![Overview all-time dashboard](assets/overview-all.png)

## Today Mode

Status: `Implemented`

Live-focused Overview mode entered with `t` or `--today`.

![Today live dashboard](assets/today-live.svg)

Earlier Overview-style direction:

![Overview today mode dashboard](assets/overview-today.png)

## Models

Status: `Future`

Future model-analysis workspace target: dense table plus selected-row inspector.

![Models table with selected model inspector](assets/models.png)

## Timeline

Status: `Future`

Future replacement for separate Daily, Hourly, and Minutely top-level tabs.

![Timeline workspace with hourly granularity and detail panel](assets/timeline.png)

## Usage

Status: `Implemented / Design target`

Operational account/quota/sync status workspace with readiness, fallback, reset, and Codex multi-account controls.

![Usage status and quota workspace](assets/usage.png)

Source SVG: [assets/usage.svg](assets/usage.svg)

## Drilldown

Status: `Implemented / Design target`

Full-page subviews for explaining selected models and selected time periods without adding more top-level tabs.

![Model detail](assets/drilldown-model.svg)

![Period detail](assets/drilldown-period.svg)

![Narrow drilldown layout](assets/drilldown-narrow.svg)

Design notes: [drilldown.md](drilldown.md)
