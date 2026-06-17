# AI Agent Guidelines

This repository is a fork of Tokscale. Treat the current direction as:

> Local-first personal telemetry for AI builders, with the TUI as the first power surface.

Do not optimize the project as a TUI-only dashboard or as a public social leaderboard. Keep AI usage/cost strong, but build Personal Pulse features around local signals, explainable summaries, and reusable core data.

## Product Direction

- Local-first by default. Personal Pulse data should be stored, processed, and exported locally unless a feature explicitly says otherwise.
- The TUI is a cockpit: current state, anomalies, evidence, quick actions, compact inspection.
- Long-form reading, reports, note review, onboarding, and agent context should leave the TUI through Markdown, JSON, MCP, local web, or another companion surface.
- WeRead is the first non-AI signal and should represent reading rhythm, focus book continuity, notes, library state, and sync health.
- Future modules should follow the same pattern before adding UI: connector and normalized signal in core, CLI adapter, TUI runtime state, renderer, then export surface if needed.

## Code Boundaries

- `tokscale-core` owns parsing, aggregation, pricing, scanner behavior, normalized pulse signals, connector normalization, and digest generation.
- `tokscale-cli` owns command wiring, settings, auth/sync commands, TUI runtime state, and rendering.
- TUI render modules should consume state. They should not perform network, cache, or settings I/O.
- `app.rs` should coordinate state and input, not accumulate connector-specific logic. Put module runtime state behind focused structs such as `PulseState`.
- Reuse local helpers and existing UI primitives before adding new abstractions.

## Documentation Rules

- Delete outdated content instead of preserving it for completeness.
- Do not maintain exhaustive client/module lists in docs. Prefer commands such as `tokscale clients`, `ls crates`, or links to focused design docs.
- Do not hardcode counts such as "10 crates" or "8 modules".
- Document constraints, boundaries, and gotchas rather than obvious descriptions of files.
- Put detailed product notes under `crates/tokscale-cli/docs/tui-design/` instead of inflating the root README.
- Root `README.md` should stay short and aligned with the local-first Personal Pulse direction.

## Git And Commits

Before committing, inspect:

```bash
git config user.name
git config user.email
git remote -v
git status --short --branch
```

Never commit as worker/agent identities such as `worker1`, `worker2`, `worker3`, or `*@example.invalid`.

Use conventional commit messages:

```text
<type>(<scope>): <what changed>
```

Common types:

- `feat`
- `fix`
- `refactor`
- `docs`
- `test`
- `chore`
- `perf`

Good examples:

```text
feat(pulse): add weekly reading digest
fix(tui): keep pulse footer aligned on narrow terminals
refactor(pulse): move connector normalization into core
docs: rewrite project README for local telemetry direction
```

Avoid vague or agent-internal wording in commits and PR titles. Do not mention review labels, internal audit language, or implementation phases.

## Command Execution

- Prefer `rg` and `rg --files` for searching.
- Use `cargo fmt --check`, `cargo clippy`, `cargo test`, and `cargo build` for Rust validation.
- When running Tokscale commands from automation, pass `--no-spinner` unless spinner behavior is the thing being tested.
- Some tests open local listeners. If they fail in a restricted sandbox with permission errors, rerun outside the sandbox before treating the failure as a regression.

## Editing Rules

- Use `apply_patch` for manual file edits.
- Do not revert user changes unless explicitly asked.
- Keep edits scoped to the requested work.
- Do not introduce broad refactors while fixing a narrow bug unless the current structure makes the fix unsafe.
- Avoid comments that restate the code. Add comments only for non-obvious constraints.

## PR And GitHub Text

When using `gh` to create or edit PR bodies, issue bodies, or comments, prefer `--body-file` over inline heredocs.

Write prose paragraphs as continuous lines in GitHub markdown bodies. Do not hard-wrap prose at 80 columns. Hard wraps are fine inside fenced code blocks.

## Validation Expectations

For ordinary Rust/TUI changes, start with:

```bash
cargo fmt --check
cargo clippy -p tokscale-cli --all-targets
cargo test -p tokscale-core
cargo test -p tokscale-cli
```

For narrow UI helper changes, focused TUI tests plus clippy may be enough, but say what was and was not run.

For Personal Pulse or WeRead work, also verify:

```bash
./target/debug/tokscale pulse --weekly
./target/debug/tokscale pulse --json
```

Use actual TUI rendering checks when layout changes affect the user-visible terminal screen.
