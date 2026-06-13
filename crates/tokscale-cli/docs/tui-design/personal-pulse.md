# Personal Pulse Dashboard Design

Personal Pulse is a low-noise dashboard for personal operating context. It should answer "what needs my attention now?" instead of mirroring every connected app.

The first implementation target is WeRead. Other modules are intentionally framed as future integrations so the initial surface stays small and shippable.

## Goals

- Add a personal dashboard workspace that can host compact external-data modules.
- Keep the top-level view dense, calm, and glanceable.
- Let every module expand into an in-place detail view under the normal TUI header/footer.
- Implement WeRead first with real data, background refresh, cache, and a compact Overview-ready widget.
- Keep credentials out of project files, cache files, screenshots, and logs.

## Non-Goals

- Do not build a runtime dynamic plugin system yet.
- Do not add long lists to the main dashboard.
- Do not block TUI rendering on network requests.
- Do not store WeRead API keys in the repository, cache payloads, or test fixtures.
- Do not make Personal Pulse the default startup tab until at least one non-WeRead module is production-ready.

## Information Architecture

Personal Pulse has three levels:

1. `Pulse summary`: one row per module, with two or three signals each.
2. `Module detail`: a full-page in-place view for the selected module.
3. `Source inspector`: a diagnostic/detail panel for auth state, refresh state, raw-source health, and privacy-sensitive errors.

The main page should look like this conceptually:

```text
Personal Pulse
+-------------------------------------------------------+
| Today     focus 2h10m   next 14:30   attention 2      |
+-------------------------------------------------------+
| Time      next meeting 14:30   free block 2h          |
| Work      PR 2   CI 1 red   tasks 4 due               |
| Mind      notes 3   anki 24   reading 4/7             |
| Life      sleep 6h42m   steps 4.2k   rain 18:00       |
| Money     spend $38   renewals 2 this week            |
| System    battery 84%   backup ok   disk 71%          |
+-------------------------------------------------------+
```

The WeRead module belongs under `Mind`, but the first implementation can render it directly in Overview or in a dedicated `Pulse` workspace before the broader dashboard exists.

## Navigation

- `Up` / `Down`: move between modules or rows.
- `Enter`: open the selected module detail.
- `Esc` / `Backspace`: return to the parent page and restore selection.
- `Tab`: switch module subviews, such as week, month, shelf, and notes.
- `r`: refresh the selected module; dashboard-level refresh refreshes stale modules only.
- Mouse click: open a module or day cell when the target is unambiguous.

The interaction should match the existing drilldown model in `drilldown.md`: detail pages are in-place pages, not modal dialogs.

## Internal Module Interface

Tokscale currently has static TUI tabs and render dispatch. For this work, implement an internal module boundary rather than a runtime plugin ABI.

```rust
trait PulseModule {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn status(&self) -> PulseStatus;
    fn summary(&self) -> PulseSummary;
    fn request_refresh(&mut self);
    fn handle_event(&mut self, event: PulseModuleEvent);
    fn render_summary(&self, frame: &mut Frame, area: Rect, theme: &Theme);
    fn render_detail(&self, frame: &mut Frame, area: Rect, theme: &Theme);
}
```

This trait is a design boundary, not necessarily the exact first patch. The first patch can use concrete structs and functions if that fits the existing TUI code better.

Each module should expose:

- `summary`: the main dashboard row or compact widget.
- `detail`: the expanded full-page view.
- `refresh policy`: stale threshold and manual refresh behavior.
- `health`: `ok`, `stale`, `auth_missing`, `loading`, or `error`.
- `privacy rules`: which fields may be cached, rendered, or logged.

## Shared State Model

Module state should be owned by `App`, similar to `subscription_usage` and background usage fetch state. Rendering reads cached state only.

Suggested shape:

```rust
pub struct PulseState {
    pub selected_module: usize,
    pub detail: Option<PulseDetail>,
    pub modules: PulseModules,
}

pub struct PulseModuleState<T> {
    pub data: Option<T>,
    pub status: PulseStatus,
    pub last_refresh: Option<Instant>,
    pub error: Option<String>,
}
```

Network refresh should use a background thread or async task and send results back through a channel. The TUI render path must stay pure and non-blocking.

## Product Research Notes

The research direction is not "add lifestyle cards to Tokscale." The stronger product shape is a local-first personal telemetry console for AI-native builders: token usage, cost, reading, knowledge, time, code, agents, and personal rhythm should be unified into a local layer that can explain what needs attention now.

The TUI should be treated as Tokscale's power surface, not the product boundary. It is ideal for dense monitoring, keyboard workflow, and immediate operational actions. The durable asset should be the normalized local data layer: the same state should support TUI rendering, JSON export, Markdown digests, future web views, Raycast-style launchers, and agent-readable context.

WeRead is the right first non-AI module because it represents input quality and learning rhythm. It creates a useful contrast with Tokscale's existing AI output and cost data: did this week only burn tokens and produce code, or did it also replenish high-quality input and durable notes?

Tokscale's differentiation should not be "more reading statistics." It should be cross-domain interpretation: explain whether reading dropped because late-night AI coding expanded, whether token spend spiked because of an agent retry loop, or whether captured highlights are failing to become notes, issues, or reusable context.

### Product Thesis

- A TUI-only personal dashboard is cool but has a constrained ceiling. It will appeal to CLI/TUI enthusiasts, heavy developers, and independent hackers, but it is unlikely to become a broad daily consumer entry point.
- A local-first telemetry layer has much larger product value. The TUI can remain the first surface while the underlying store, connector model, normalized signals, and action layer become reusable.
- The terminal is likely to become stronger in AI developer workflows. Coding agents already operate in local terminals and repos, so Tokscale can become the telemetry and control console around those agents.
- The tone should be observational, not judgmental: observe without guilt, explain with evidence, act with consent.

### Personal Telemetry Layer

Long-term architecture should look like this:

```text
local connectors
  -> local event store
  -> normalized personal signals
  -> TUI / Markdown / JSON / Raycast / MCP / Web
  -> human insight + agent context + action console
```

This avoids binding the product to a single terminal layout. It also lets Personal Pulse produce structured context for agents, not just charts for humans.

### Reference Product Map

| Product shape | References | Useful pattern | What to borrow | What to avoid |
| --- | --- | --- | --- | --- |
| TUI/CLI cockpit | [btop](https://github.com/aristocratos/btop), [bottom](https://github.com/ClementTsang/bottom), [Glances](https://github.com/nicolargo/glances), [k9s](https://k9scli.io/), [lazygit](https://github.com/jesseduffield/lazygit) | High-density state, fast navigation, selected-object detail, immediate actions | Stable summary/detail/inspector/action flow; keyboard-first interaction; anomaly-first scanning | Raw metric walls that require the user to infer meaning |
| TUI framework ecosystem | Ratatui, Textual, Bubble Tea | Widget primitives such as tables, gauges, sparklines, tabs, and Elm-style state loops | A small Pulse widget schema: metric, sparkline, calendar grid, timeline, table, inspector | Becoming a generic component showcase instead of a product with opinions |
| Developer and AI analytics | WakaTime, GitHub Insights, OpenAI usage, LangSmith, Helicone | Time, project, model, agent, cost, and trace attribution | Cost drivers, model mix, quota burn-down, project/agent attribution, anomaly explanation | Team surveillance tone or cloud-first telemetry assumptions |
| Programmable dashboards | [Grafana dashboards](https://grafana.com/docs/grafana/latest/visualizations/dashboards/), [Datadog dashboards](https://docs.datadoghq.com/dashboards/) | Data source, query/transform, panel, row/tab | Normalize upstream data into stable view models; keep panels composable | Full query builders or arbitrary dashboard editing in the first version |
| Home and local automation | [Home Assistant dashboards](https://www.home-assistant.io/dashboards/) | At-a-glance state plus one-step controls | Local control, privacy-first integrations, card actions, conditional visibility | Smart-home metaphors and complex user-authored automation too early |
| Quantified self and attention analytics | [ActivityWatch](https://activitywatch.net/), [RescueTime](https://www.rescuetime.com/), ManicTime, Exist.io, Screen Time | Automatic tracking, weekly reports, category trends, correlations | Local/privacy-first telemetry, baseline comparison, weekly deltas, correlations | Guilt-inducing productivity scores or invasive data capture |
| Knowledge/work dashboards | Obsidian dashboards, Notion dashboards, Raycast extensions, GitHub personal inboxes | Personal inbox and quick-action surfaces | Compact review queues, stale work signals, export actions | Manual-maintenance dashboards that become another inbox |

### Trend Thesis

- TUI dashboards are power-user tools, but local-first personal telemetry is broader than the terminal. Keep the data layer portable.
- Dashboards are moving from passive charts toward action consoles: detect a state, explain it, and offer a safe next action.
- Agent-readable dashboards matter. A future agent should be able to ask for the same Pulse summary the user sees and use it as context through Markdown, JSON, or MCP resources/tools.
- Privacy is a product feature. Personal Pulse should normalize and cache only the fields it renders, never raw upstream payloads when the data is sensitive.
- Text matters as much as charts. Dense metrics need short interpretation labels such as `above usual pace`, `stale`, `reading streak healthy`, or `notes output low`.
- The product should move from metrics toward narrative: "what happened this week?" is more useful than a wall of disconnected panels.
- Every insight should be auditable: show evidence first, then interpretation, then optional recommendation.

### Horizontal Module Map

| Domain | Value | Compact signals | Detail view | Actions | Risk |
| --- | --- | --- | --- | --- | --- |
| AI usage | Understand daily AI burn, model mix, and quota pressure | Today cost, projected cost, top model, quota reset | Hourly pace, model/source drivers, provider mix, quota health | Refresh, open model detail, export JSON | Cost/account data can be sensitive |
| Reading and learning | Track input rhythm and knowledge intake | WeRead 4/7, total time, focus book, notes count | Week grid, month rhythm, shelf, notebooks | Open book, export highlights, generate digest | Reading history and private shelf data |
| Notes and knowledge | Keep captured ideas moving toward useful output | Inbox count, notes today, stale notes | Obsidian inbox, recent links, orphan notes, review queue | Open note, create task, link reading highlight | Local vault privacy |
| Time and attention | Show schedule pressure and focus availability | Next event, free block, app/site focus split | Calendar blocks, ActivityWatch categories, focus debt | Start focus session, mute module, open calendar | Behavioral telemetry can feel invasive |
| Work | Surface blocked or stale engineering work | PRs, CI red, reviews owed, dirty worktrees | GitHub queues, branch state, local repo health | Open PR, rerun check, create handoff note | Work metadata and repo names |
| Agents and automation | Make background agents observable | Running agents, blocked panes, last update | Herdr panes, task states, failures, handoff notes | Focus pane, restart task, summarize state | Cross-pane private content |
| Inbox and reminders | Keep only actionable changes in one queue | Actionable count, overdue count, source mix | Insight inbox, reason, linked source, snoozed items | Done, snooze, open, create task, mute rule | Notifications, messages, and work context |
| System | Avoid local environment surprises | Disk, battery, backup, dev server status | Resource and service checks | Open logs, restart service, clean cache | Local machine paths and process names |
| Money and subscriptions | Track recurring cost and usage limits | Renewals, monthly spend, quota risk | Subscription list, AI provider spend, anomalies | Mark reviewed, export report | Financial data |
| Life signals | Add only attention-relevant context | Weather, sleep, commute, reminders | Minimal daily context | Open source app, dismiss reminder | Scope creep and sensitive health data |

Suggested integration priority:

```text
AI usage/cost/quota
  -> WeRead reading/learning
  -> Obsidian notes
  -> GitHub/PR/CI
  -> ActivityWatch/time
  -> calendar
  -> agent/background tasks
  -> subscriptions
  -> health/weather/life signals
```

Do not start with health or finance. They are valuable, but the privacy and product-responsibility cost is higher than reading, notes, work, or local agent telemetry.

### Vertical Capability Model

Every module should mature along the same path:

```text
raw metric -> normalized signal -> explanation -> recommendation -> action -> automation
```

The UI layers should stay consistent:

1. `Glance`: one line or one compact card with two to four signals.
2. `Detail`: why this signal matters, with trend and breakdown.
3. `Inspector`: auth, refresh, cache, source health, and sanitized errors.
4. `Action`: open the source, refresh, export, generate a digest, or create a task.

Cross-module insights are the long-term differentiator. Examples:

- Reading dropped while AI token usage spiked and calendar load increased.
- Token cost spiked because one model/source dominated a morning work block.
- WeRead highlights increased but Obsidian inbox also grew, indicating knowledge capture without synthesis.
- ActivityWatch shows long browser time after failed CI, suggesting a blocked-debugging pattern.

### Signal Vocabulary

Raw metrics are too fragmented to become product language on their own. Personal Pulse should derive a small vocabulary of normalized signals:

| Signal | Inputs | Product meaning |
| --- | --- | --- |
| Input quality | Reading depth, focus book continuity, note density, topic match | Whether the user is replenishing useful context |
| Output pressure | AI tokens, coding time, PR/issue throughput, agent tasks | Whether execution load is unusually high |
| Attention fragmentation | Context switches, notification spikes, meeting fragmentation, short sessions | Whether work is being broken into shallow pieces |
| Knowledge sedimentation | Highlights to notes to linked notes to tasks/cards | Whether captured ideas become durable artifacts |
| Cost discipline | Cost per project, quota burn rate, model efficiency, budget anomaly | Whether AI usage is financially controlled |
| Recovery capacity | Sleep, breaks, low-meeting windows, system stability | Whether the user has enough slack for high-cognition work |

The weekly digest can then say:

```text
Output pressure was high: AI coding cost +68%, four long agent runs.
Input quality weakened: reading -42%, notes mostly unprocessed.
Likely driver: late-night coding replaced the usual reading window.
Suggested action: protect three reading blocks and cap the noisy agent.
```

The agent context can be shorter and more operational:

```text
User is cost-sensitive this week. Reading backlog is growing.
Avoid suggesting high-token refactors unless project urgency is explicit.
```

### Cross-Module Insight Patterns

- `Reading <-> AI coding`: Reading time drops while late-night agent sessions increase. Interpret as the output loop expanding into the usual input window.
- `Token cost <-> agent loop`: Token cost growth is concentrated in one agent, project, or model. Interpret as a control problem, not general usage growth.
- `Notes <-> work output`: Reading notes accumulate but none are exported to Obsidian, linked to projects, or turned into tasks. Interpret as knowledge capture without synthesis.
- `Meetings <-> attention fragmentation`: Meeting-heavy days correlate with shorter reading sessions, shorter prompts, and more corrective AI work. Interpret as a planning problem, not a motivation problem.

### Product Concepts To Preserve

- `Personal Balance Sheet`: cognitive assets such as reading, notes, knowledge cards, and deep focus versus debts such as token overspend, unprocessed highlights, stale reviews, agent failures, and fragmented attention.
- `Input/Output Ratio`: compare reading, notes, and durable knowledge creation against AI coding time, tokens, PRs, and agent tasks.
- `Focus Book`: treat the current book as an intellectual thread, not just a title. Track continuity, notes, project relevance, and whether to continue, pause, or switch.
- `Agent Readiness`: decide whether conditions are good for a large agent run: quota, local system health, recent fail rate, CI state, and user availability.
- `Weekly Diff`: render the week like a developer diff: `+ AI cost`, `+ PR merged`, `- reading time`, `! retry loop detected`, `! focus book stale`.
- `Insight Inbox`: route insights to a triage queue where the user can mark useful, dismiss, snooze, turn into a rule, or turn into a task.

### WeRead Product Implications

WeRead should be framed as the `Mind/Input` module, not as a mini WeRead clone. It should answer whether high-quality input is happening and whether that input is turning into notes or work.

The first version should answer four questions:

1. Did I keep a reading rhythm this week?
2. What is my current intellectual thread?
3. Did reading produce notes or reusable knowledge?
4. Is my input rhythm balanced with AI output and cost?

Recommended compact shape:

```text
WeRead Pulse
Mon Tue Wed Thu Fri Sat Sun
 x   .   x   x   .   x   .

Week  2h48m  4 days  avg 42m/day  -18%
Focus The Current Book  last read Sat  63m this week
Notes 12 notes  3 books with notes
Sync  12m ago
```

Information priority:

- `P0`: seven-day reading grid, weekly total, read days, daily average, week-over-week comparison.
- `P1`: focus book, last-read recency, note count, books with notes, stale/cache state.
- `P2`: shelf scale, topic preference, next-pick candidate, export/digest action.

First-version implications:

- Prioritize weekly rhythm, focus book, month trend, shelf scale, and notes count.
- Keep summary compact; do not show long book lists on the top-level dashboard.
- Treat `read_seconds >= 60` as check-in, but label it as an effective day rather than a moral streak.
- Use `focus book` and `topic preference` to make the view feel personal without exposing too much private history.
- Render auth, stale, loading, and error states quietly so the whole TUI never feels broken because WeRead is unavailable.
- Keep the main view small and refined. The detail view can be richer, but the top-level widget should not become a large lifestyle card.

Post-MVP directions:

- `Input/output loop`: show whether reading produced notes, highlights, or exported knowledge cards.
- `Focus drift`: compare current reading topics with recent coding, notes, or work themes.
- `Reading debt`: flag when the current week falls below the user's recent baseline.
- `Best reading window`: use available WeRead time-of-day signals when present.
- `Current book lane`: track one to three active books instead of rendering the whole shelf.
- `Digest action`: generate a weekly reading summary or export recent highlights to a notes system.
- `Next pick`: recommend a next book based on recent reading plus work/AI context, not only upstream recommendations.

## WeRead MVP

WeRead is the detailed implementation target for the first pass.

### User Value

The WeRead module should help answer:

- Did I read this week?
- What is my current intellectual thread or focus book?
- How does this month look compared to the week?
- What kind of topics am I spending time on?
- Did reading produce notes or reusable knowledge?
- Is my input rhythm balanced with recent AI output and cost?

It should not try to replace the WeRead app. Tokscale should show a compact personal rhythm view and open richer details only on demand.

### Data Source

Use the WeRead Agent Gateway:

```text
POST https://i.weread.qq.com/api/agent/gateway
Authorization: Bearer $WEREAD_API_KEY
Content-Type: application/json
```

Every request must include `skill_version`.

Initial endpoints:

| Endpoint | Purpose | Required For MVP |
| --- | --- | --- |
| `/readdata/detail` with `mode=weekly` | Weekly check-in, daily buckets, weekly total, read days, comparison | Yes |
| `/readdata/detail` with `mode=monthly` | Month rhythm, active days, topic preference, monthly longest books | Yes |
| `/shelf/sync` | Shelf count, private/public split, recent books | Yes |
| `/user/notebooks` | Note counts and books with notes | Yes |
| `/book/recommend` | One optional next-pick candidate | Nice-to-have |
| `/book/getprogress` | Current book progress after selecting a book | Defer unless detail page needs it |
| `/book/bookmarklist` and `/review/list/mine` | Export/highlight detail for a selected book | Defer |

### Credential Policy

Phase one should read the WeRead key from Tokscale's user settings env map:

```json
{
  "env": {
    "WEREAD_API_KEY": "..."
  }
}
```

The canonical settings path is `~/.config/tokscale/settings.json` on Unix-like systems. `TOKSCALE_CONFIG_DIR` may override the config root for tests or isolated profiles. A real process environment variable named `WEREAD_API_KEY` takes precedence over the settings value for one-off overrides.

If the key is missing:

- Do not show an error state in the whole TUI.
- Render the module as `auth missing`.
- Show a concise hint in detail view: `Set env.WEREAD_API_KEY in settings.json to enable WeRead`.

Except for the user-authored `settings.json` value, do not write the key to:

- cache files
- logs
- test snapshots
- screenshots
- panic/error strings

Keychain or OS credential-store support can be a later enhancement.

### Data Model

Keep WeRead-specific data separate from token usage data.

```rust
pub struct WeReadState {
    pub weekly: Option<WeReadWeekly>,
    pub monthly: Option<WeReadMonthly>,
    pub shelf: Option<WeReadShelfSummary>,
    pub notes: Option<WeReadNotesSummary>,
    pub recommendation: Option<WeReadRecommendation>,
    pub status: PulseStatus,
    pub last_refresh: Option<SystemTime>,
    pub error: Option<String>,
}

pub struct WeReadWeekly {
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub read_days: u8,
    pub total_seconds: u32,
    pub day_average_seconds: u32,
    pub compare_ratio: Option<f64>,
    pub days: [WeReadDay; 7],
    pub focus: Option<WeReadFocusBook>,
}

pub struct WeReadDay {
    pub date: NaiveDate,
    pub read_seconds: u32,
    pub checked_in: bool,
}

pub struct WeReadMonthly {
    pub read_days: u16,
    pub total_seconds: u32,
    pub day_average_seconds: u32,
    pub prefer_category_word: Option<String>,
    pub categories: Vec<WeReadCategory>,
}

pub struct WeReadShelfSummary {
    pub books: u32,
    pub albums: u32,
    pub has_mp: bool,
    pub visible_items: u32,
    pub private_items: u32,
    pub recent: Vec<WeReadBookRef>,
}

pub struct WeReadNotesSummary {
    pub total_books: u32,
    pub total_notes: u32,
    pub top_books: Vec<WeReadNotebookSummary>,
}
```

Rules:

- `checked_in` is `read_seconds >= 60`.
- Weekly `readTimes` keys are Unix timestamps and must be converted to local dates.
- Reading durations are seconds in API responses and must be formatted before rendering.
- Shelf total must be `books.length + albums.length + (mp exists ? 1 : 0)`.
- Notes total per book must be `reviewCount + noteCount + bookmarkCount`.

### Cache

Use a separate cache file under Tokscale's existing cache directory:

```text
<tokscale cache dir>/weread-pulse-cache.json
```

Suggested cache schema:

```json
{
  "schemaVersion": 1,
  "timestamp": 1781107200000,
  "data": {
    "weekly": {},
    "monthly": {},
    "shelf": {},
    "notes": {},
    "recommendation": {}
  }
}
```

Do not cache raw API responses. Cache only normalized fields needed by the UI. This reduces privacy exposure and makes rendering code independent from upstream response shape.

Staleness:

- Weekly/monthly reading: stale after 15 minutes.
- Shelf and notebooks: stale after 60 minutes.
- Recommendation: stale after 24 hours.
- Manual refresh always bypasses staleness if `env.WEREAD_API_KEY` or the process `WEREAD_API_KEY` is present.

If a refresh fails:

- Keep rendering the last cached data.
- Mark the module as stale/error.
- Show the error only in the detail/inspector view, not in the top-level row.

### Summary Widget

Use the compact design from `assets/weread-pulse.svg`.

Compact dashboard variant:

```text
+-- WeRead -----------------------------+
| 4/7  4h56                             |
| Mon Tue Wed Thu Fri Sat Sun           |
| x   x   x   x   .   .   .             |
| avg 59m   effective >=1m   +35%       |
| Focus  Wan Wu Fa Ming Zhi Nan         |
| Notes  77 total   sync 12m ago        |
+---------------------------------------+
```

Summary signals:

- Weekly check-in count.
- Weekly total reading time.
- Seven day markers.
- Natural-day average.
- Week-over-week comparison if present.
- Focus book and last-read recency when available.
- Notes count and books-with-notes count when available.
- Sync state: `fresh`, `stale`, `auth missing`, `loading`, or `error`.

The widget must fit in a dashboard card and in a narrow terminal. It should avoid large cells. Use stable widths so the layout does not shift when a day changes from `0m` to `1h20m`.

### Detail View

Use the wide detail design from `assets/weread-pulse.svg`.

Detail layout:

```text
WeRead detail
+-- WeRead Pulse ----------------------+ +-- Month rhythm ---------------+
| week markers, durations, focus book  | | total, active days, category  |
| avg, compare, signal tag             | +-------------------------------+
+--------------------------------------+ +-- Library signals ------------+
                                        | shelf, notes, next pick       |
                                        +-------------------------------+
```

Sections:

- `WeRead Pulse`: week period, daily check-ins, per-day duration, weekly total, focus book, week-over-week compare.
- `Month rhythm`: monthly total, active days, average, top category bars.
- `Library signals`: visible shelf items, private/public hint, notes count, books-with-notes count, optional next pick.
- `Recent focus`: when space allows, show the latest read books from shelf data.

Narrow layout:

- Stack sections vertically.
- Keep the weekly check-in above the fold.
- Collapse category bars to two rows.
- Hide recommendation first when height is constrained.

### Interaction

In summary view:

- `Enter`: open WeRead detail.
- Click on the widget: open WeRead detail.

In detail view:

- `Tab`: cycle `Week`, `Month`, `Shelf`, and `Notes`.
- `Left` / `Right`: move selected day in Week mode.
- `Enter` on a day: show day details if available.
- `Enter` on a book: open book progress/detail if that endpoint has been loaded.
- `r`: refresh WeRead.
- `Esc`: return to parent view.

### Refresh Flow

Startup:

1. Load `weread-pulse-cache.json`.
2. If cache exists, render it immediately with `stale` marker when needed.
3. If `env.WEREAD_API_KEY` or process `WEREAD_API_KEY` exists and data is stale, start background refresh.
4. If no key exists, render `auth missing`.

Manual refresh:

1. Set module status to `loading`.
2. Fetch endpoints in priority order: weekly, monthly, shelf, notebooks, recommendation.
3. Normalize each response independently.
4. If a lower-priority endpoint fails, keep successful higher-priority data.
5. Persist normalized cache after successful normalization.
6. Send the new `WeReadState` back to `App`.

Rendering must never call the network client.

### Error Handling

Top-level summary:

- `auth missing`: subtle muted state.
- `stale`: render data with a small stale marker.
- `error`: render last known data and a muted `!` marker.
- `loading`: render cached data and spinner in the module title when possible.

Detail/inspector:

- Show endpoint-level status.
- Show last successful refresh time.
- Show sanitized error messages.
- Never print request headers or the API key.

If the API returns `upgrade_info`, pause WeRead refresh and surface the upgrade message in the inspector. Do not ignore it.

### Files

Suggested first implementation files:

```text
crates/tokscale-cli/src/tui/integrations/mod.rs
crates/tokscale-cli/src/tui/integrations/weread/mod.rs
crates/tokscale-cli/src/tui/integrations/weread/client.rs
crates/tokscale-cli/src/tui/integrations/weread/cache.rs
crates/tokscale-cli/src/tui/integrations/weread/model.rs
crates/tokscale-cli/src/tui/ui/weread.rs
```

App integration points:

- Add `weread_state` and `weread_rx` to `App`.
- Add `maybe_fetch_weread_on_entry()`.
- Add WeRead rendering inside Overview or Pulse.
- Add a detail state variant for WeRead if the existing drilldown model is reused.
- Add footer hints only when the WeRead widget/detail is active.

### Tests

Unit tests:

- Normalize weekly `readTimes` into seven local-date buckets.
- Treat `read_seconds >= 60` as checked in.
- Format durations from seconds.
- Calculate shelf visible item count with `books + albums + mp`.
- Calculate note totals with `reviewCount + noteCount + bookmarkCount`.
- Redact API key from errors.
- Load stale cache and preserve data after refresh failure.

Render tests:

- Compact widget fits narrow width.
- Detail view does not stretch the weekly row on wide width.
- Missing auth state is quiet and does not crash.
- Error state keeps cached data visible.

Manual verification:

- Launch with no `env.WEREAD_API_KEY` and no process `WEREAD_API_KEY`.
- Launch with a valid key and no cache.
- Launch with a valid key and stale cache.
- Trigger manual refresh.
- Resize wide to narrow and confirm the weekly check-in stays readable.

## Future Modules

The future modules should use the same three-level model: summary, detail, inspector. They should not be implemented until WeRead proves the module boundary works.

### Time

Purpose:

- Next meeting.
- Free focus blocks.
- Daily and weekly commitment density.

Possible sources:

- Calendar provider.
- Local `.ics`.
- Manual agenda file.

Summary example:

```text
Time   next 14:30   free 2h10m   heavy afternoon
```

### Work

Purpose:

- PRs needing attention.
- CI failures.
- Stale or blocked tasks.

Possible sources:

- GitHub.
- Linear/Jira.
- Local git worktree state.

Summary example:

```text
Work   PR 2   CI 1 red   tasks 4 due
```

### Mind

Purpose:

- Notes captured today.
- Learning backlog.
- Reading and review rhythm.

Possible sources:

- WeRead.
- Obsidian.
- Anki.

Summary example:

```text
Mind   notes 3   anki 24 due   reading 4/7
```

### Life

Purpose:

- Sleep, activity, weather, and commute signals.

Possible sources:

- Health export.
- Weather API.
- Manual status file.

Summary example:

```text
Life   sleep 6h42m   steps 4.2k   rain 18:00
```

### Money

Purpose:

- Spending pace.
- Upcoming renewals.
- Anomaly detection.

Possible sources:

- CSV export.
- Budget app export.
- Manual subscriptions file.

Summary example:

```text
Money  spend $38   renewals 2   budget ok
```

### System

Purpose:

- Device and development environment health.

Possible sources:

- Local OS commands.
- Backup status files.
- Dev server probes.

Summary example:

```text
System battery 84%   backup ok   disk 71%
```

## Rollout Plan

Phase 1: WeRead data layer

- Add WeRead client, normalized model, cache, and tests.
- Read `WEREAD_API_KEY` from settings `env` with process env as an override.
- Add manual CLI or test harness path if needed for local verification.

Phase 2: WeRead compact widget

- Render compact widget in Overview or a hidden Pulse workspace.
- Support auth missing, loading, stale, and error states.
- Add render tests for narrow and wide widths.

Phase 3: WeRead detail view

- Add in-place detail view.
- Add week/month/shelf/notes subview navigation.
- Add footer hints and click targets.

Phase 4: Personal Pulse shell

- Add `Pulse` workspace if the Overview placement starts crowding the existing dashboard.
- Move WeRead into the module shell.
- Add module health and source inspector.

Phase 5: Future integrations

- Add one future module at a time.
- Prefer sources with stable auth and low privacy risk first.
- Promote runtime plugin architecture only after two or more non-WeRead modules share the same internal boundary.

## Open Questions

- Should WeRead live in Overview first, or should `Pulse` be introduced immediately?
- Should cached WeRead data be disabled by default for privacy-sensitive users?
- Should the first version show private/public shelf counts, or just total visible items?
- Should recommendations appear in summary, or only in detail view?
- Should the detail view use existing drilldown state or a separate Pulse detail state?
