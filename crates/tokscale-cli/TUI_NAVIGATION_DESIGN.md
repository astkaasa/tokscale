# TUI Navigation Design

Tokscale's TUI should keep a small set of stable workspaces at the top level, then use drilldown pages for details that naturally come from the selected context.

## Mockups

Directional mockups for the target TUI are saved in [`docs/tui-design/`](docs/tui-design/README.md).

### Overview

![Overview all-time dashboard](docs/tui-design/assets/overview-all.png)

### Today Mode

![Overview today mode dashboard](docs/tui-design/assets/overview-today.png)

### Models

![Models table with selected model inspector](docs/tui-design/assets/models.png)

### Timeline

![Timeline workspace with hourly granularity and detail panel](docs/tui-design/assets/timeline.png)

### Usage

![Usage status and quota workspace](docs/tui-design/assets/usage.png)

Usage is the operational account/quota workspace. The target interaction includes refreshing subscription status, adding Codex accounts through browser login, switching the active Codex account, and two-step account removal. The main page should answer whether the current account is ready, which fallback is safest, when quota resets, and which saved accounts need attention.

## Top-Level Workspaces

Target top-level tabs:

1. `Overview`
2. `Models`
3. `Timeline`
4. `Usage`

`Overview` is the default landing workspace. It shows the broad picture and can focus on today without becoming a separate tab.

`Models` is the durable model-analysis workspace. It should stay table-first, with an inspector for the selected model once the Overview polish is stable.

`Timeline` replaces separate `Daily`, `Hourly`, and `Minutely` tabs. The time granularity belongs inside one workspace because the user is still asking the same question: when did usage happen?

`Usage` is not historical analytics. It should be the operational status surface for subscriptions, quotas, credentials, cache/sync state, and provider account health.

## Deferred Or Hidden Top-Level Tabs

`Daily`, `Hourly`, and `Minutely` should remain as implementation modules until `Timeline` reaches parity, but they should not stay prominent top-level concepts.

`Stats` overlaps with `Overview` and should be absorbed by Overview or Timeline.

`Agents` should not be a top-level tab for now. Agent metadata is useful when present, but it is not a universal dimension across all clients. Show agent information as a low-priority optional section in Overview, Today, and detail pages only when data exists.

## Overview Modes

Overview has two modes:

- `All`: the default all-time/range dashboard.
- `Today`: a live-focused dashboard for usage since local midnight.

Today mode is a time focus, not a visible tab and not a segmented control. It is entered with `t`, by starting the TUI from `--today`, or eventually by selecting today's cell/bar from Overview. It exits with `t` or with normal navigation to another workspace.

The page title carries the mode:

- `Overview`
- `Today · Jun 9`

The footer may mention `t today`, but the main content should not show an explicit Today/All toggle.

## Drilldown Model

Top-level tabs switch analysis workspaces. Drilldown pages answer detail questions from the currently selected object.

Primary drilldowns:

- Overview trend bar or calendar day -> Day Detail
- Today hour -> Hour Detail
- Top model row -> Model Detail
- Provider mix row -> Provider Detail
- Timeline row -> Date/Hour/Minute Detail

Drilldowns should be full-screen pages with breadcrumb-style titles and `Esc` to return. They are not modal picker dialogs.

## CLI Relationship

The TUI should not be designed around command-line options. CLI filters can remain for scripting and compatibility, but the interactive product should let users change source, date focus, grouping, and granularity inside the TUI.

`--today` is the only CLI flag worth treating as a first-class TUI entry shortcut. It should open the TUI directly in Overview Today mode.

## Phased Implementation

1. Polish Overview and add Today mode.
2. Simplify top-level navigation.
3. Redesign Models around a table plus selected-row inspector.
4. Merge Daily, Hourly, and Minutely into Timeline.
5. Redesign Usage as account/quota/sync status.
6. Add full-screen drilldown pages.
