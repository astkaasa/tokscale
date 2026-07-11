# Tokscale

Tokscale is becoming a local-first personal telemetry console for AI builders.

It started as a token usage and cost dashboard for AI coding tools. This fork keeps that core, but expands the product direction: Tokscale should help a local developer understand AI output, subscription pressure, reading input, knowledge flow, and work rhythm from one private control surface.

The TUI is the first power surface. It is not the product boundary.

## Direction

Tokscale should be a local telemetry layer with multiple surfaces:

```text
local connectors
  -> normalized local signals
  -> TUI cockpit
  -> Markdown / JSON digests
  -> local web / future MCP / companion surfaces
```

Current priorities:

- Keep AI usage, cost, model mix, source mix, and quota status strong.
- Make Personal Pulse useful without turning the TUI into a crowded card wall.
- Use WeRead as the first non-AI signal: reading rhythm, focus book continuity, notes, library signals, and stale/auth health.
- Prefer local files, local cache, explicit configuration, and inspectable output.
- Keep connector logic reusable from the core crate instead of coupling it to terminal rendering.

## What Works Now

- Interactive TUI cockpit with Overview, Models, Timeline, Usage, and Pulse workspaces.
- Local AI usage parsing across supported coding clients. Run `tokscale clients` to see detected local sources and paths on your machine.
- Subscription quota/status view via `tokscale usage` and the Usage TUI workspace.
- WeRead Pulse in the TUI, backed by a WeRead API key stored in settings or provided through the environment.
- Local read-only Overview and Pulse Weekly Review surfaces via `tokscale serve`.
- Personal Pulse digest export:

```bash
tokscale pulse --weekly
tokscale pulse --json
```

`--weekly` emits Markdown for review notes or Obsidian. `--json` emits agent-readable local context. Both read the latest durable local snapshot by default.

## Install And Run

For this fork and branch, build from source:

```bash
cargo build -p tokscale-cli
./target/debug/tokscale
```

Useful commands:

```bash
./target/debug/tokscale tui
./target/debug/tokscale tui --today
./target/debug/tokscale --light --no-spinner
./target/debug/tokscale models --json --no-spinner
./target/debug/tokscale --no-spinner usage
./target/debug/tokscale pulse sync --no-spinner
./target/debug/tokscale pulse --weekly --no-spinner
./target/debug/tokscale pulse --json --no-spinner
./target/debug/tokscale serve
./target/debug/tokscale clients
```

When running Tokscale from automation or an agent, pass `--no-spinner` unless spinner behavior is being tested.

## Configuration

Tokscale stores user configuration at:

```text
~/.config/tokscale/settings.json
```

Use `TOKSCALE_CONFIG_DIR` to override the config root for tests or isolated runs.

Minimal example:

```json
{
  "uiTheme": "dark",
  "defaultClients": ["codex", "claude"],
  "usage": {
    "enabledProviders": ["Codex"]
  },
  "env": {
    "WEREAD_API_KEY": "<your-weread-api-key>"
  },
  "scanner": {
    "extraScanPaths": {
      "codex": [
        "/Users/me/project/.codex/sessions"
      ]
    }
  }
}
```

Important settings:

| Setting | Purpose |
| --- | --- |
| `uiTheme` | TUI theme: `dark`, `light`, or `auto`. |
| `defaultClients` | Default client filter when no `--client` flag is passed. |
| `usage.enabledProviders` | Remote quota providers allowed to run. Missing defaults to Codex; `[]` disables all remote quota requests. |
| `usage.excludedProviders` | Legacy denylist applied after `enabledProviders`. |
| `env` | Persistent per-user integration secrets. Process environment variables still take precedence. |
| `env.WEREAD_API_KEY` | Enables WeRead Pulse and `tokscale pulse` refreshes. |
| `scanner.extraScanPaths` | Additional per-client local scan roots. |
| `light.writeCache` | Lets `--light` refresh the TUI startup cache. |

Secrets in `settings.json` are local to your machine. Tokscale should not require cloud upload for Personal Pulse features.

## WeRead Pulse

WeRead is the first Personal Pulse connector. It represents input quality rather than AI output volume.

Configure:

```json
{
  "env": {
    "WEREAD_API_KEY": "<your-weread-api-key>"
  }
}
```

Then run the TUI and open the Pulse workspace:

```bash
./target/debug/tokscale tui
```

Digest exports:

```bash
./target/debug/tokscale pulse sync --no-spinner
./target/debug/tokscale pulse --weekly --no-spinner > weekly-pulse.md
./target/debug/tokscale pulse --json --no-spinner
```

`pulse sync` refreshes connectors and writes the durable snapshot. Exports are local-only unless `--refresh` is passed. `tokscale serve` exposes the same snapshot at `/review`, `/api/v1/pulse`, and `/exports/pulse.md` without refreshing connectors.

`tokscale serve` has no user authentication and freezes its data at startup. Other users or sandboxes on the same machine may be able to read served titles and IDs; do not run it under that threat model, and restart it after later syncs.

If `WEREAD_API_KEY` is set in the real process environment, it overrides `settings.json` for that run.

## Repository Map

Use commands rather than maintained exhaustive lists:

```bash
ls crates
ls crates/tokscale-cli/src
ls crates/tokscale-core/src
```

Current boundaries:

- `crates/tokscale-core`: parsers, aggregation, pricing, scanner, normalized pulse signals, WeRead fetch/cache/normalization, digest generation.
- `crates/tokscale-cli`: command adapter, TUI runtime, settings, auth/sync commands, rendering.
- `crates/tokscale-cli/docs/tui-design`: product and TUI design notes for the current fork.

For Personal Pulse modules, prefer this shape:

```text
core connector + normalized signal
  -> CLI command adapter
  -> TUI runtime state
  -> TUI renderer
  -> Markdown / JSON / local web export
```

## Development

Prerequisites:

- Rust toolchain

Common checks:

```bash
cargo fmt --check
cargo clippy -p tokscale-cli --all-targets
cargo test -p tokscale-core
cargo test -p tokscale-cli
cargo build -p tokscale-cli
```

Some tests open local listeners. If they fail under a restricted sandbox with permission errors, rerun them in a normal shell before treating the failure as a code regression.

## Design Notes

The active product notes live under:

```text
crates/tokscale-cli/docs/tui-design/
```

Start with:

- `README.md`
- `personal-pulse.md`
- `personal-pulse-architecture.md`
- `navigation.md`
- `overview.md`
- `provider-colors.md`
- `mouse-selection.md`

These documents describe direction and boundaries. They are not pixel-perfect contracts.

## License

MIT. See `LICENSE`.
