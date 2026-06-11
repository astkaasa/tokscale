# TUI Drilldown Design

Drilldowns are in-place subpages, not new top-level tabs. They answer detail questions from the selected object while preserving the parent workspace context.

## Interaction

- `Enter` opens the selected row or chart bar detail.
- Mouse click opens details for chart bars and table rows where the target is unambiguous.
- `Esc` or `Backspace` returns to the parent page and restores selection/scroll where practical.
- Sort keys keep their existing meanings inside detail tables: `d` date/name, `c` cost, `t` tokens.
- Detail pages are full-page views under the normal header/footer, not modal dialogs.

## Model Detail

Used from Overview top models and Models table rows.

![Model detail](assets/drilldown-model.svg)

The page explains where one model's cost/tokens came from. It should show summary, trend/mix, and a breakdown by period/source/workspace. Selecting a breakdown row can open the matching period detail.

## Period Detail

Used from Overview chart bars and Timeline rows. The same shape covers day, week, and month periods.

![Period detail](assets/drilldown-period.svg)

The page explains why a period was high or low. It should show period totals, provider mix, token mix, top models, and a model breakdown. Selecting a model row can open model detail.

## Narrow Layout

Narrow screens keep the same page model but stack sections vertically.

![Narrow drilldown layout](assets/drilldown-narrow.svg)

Dense table rows become two-line rows. The top summary remains visible above the breakdown so the page does not collapse into a raw table.

## Scope

Implement first:

- Model detail
- Period detail

Defer:

- Provider detail
- Segment-level chart drilldown
- Usage account drilldown, because wide Usage already has a selected-account inspector and narrow Usage should be handled during the narrow-screen pass.
