# TUI Design Notes

These notes document the current local-first TUI direction. They are product and implementation references, not pixel-perfect contracts. Verify visible layout with actual TUI renders.

## Core Documents

- [navigation.md](navigation.md): top-level workspaces, Today mode, and drilldown navigation.
- [overview.md](overview.md): Overview dashboard layout and interaction constraints.
- [drilldown.md](drilldown.md): model and period detail pages.
- [local-telemetry-ledger.md](local-telemetry-ledger.md): durable multi-agent history, source classes, migration, and remote-provider boundaries.
- [provider-colors.md](provider-colors.md): provider identity colors and terminal compatibility.
- [mouse-selection.md](mouse-selection.md): mouse capture and native terminal text selection.

## Personal Pulse

- [personal-pulse-architecture.md](personal-pulse-architecture.md): product boundary and module admission rules.
- [personal-pulse.md](personal-pulse.md): current WeRead Pulse contract.
- [pulse-snapshot-v1.md](pulse-snapshot-v1.md): implemented versioned snapshot, sync, export, and local review contract.

## Companion Surfaces

- [local-web-surface.md](local-web-surface.md): localhost Overview projection, Pulse review, and read-only API boundary.

## Assets

SVG files under `assets/` are historical design references for major TUI surfaces. Keep an asset only while it still communicates a current layout decision better than prose or render tests.
