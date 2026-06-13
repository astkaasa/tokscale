# TUI Overview Design

This branch treats the Overview tab as an operational dashboard, not a shortened Models table.

Directional mockups are saved next to this document. The relevant Overview targets are embedded below.

## Mockups

### Overview

![Overview all-time dashboard](assets/overview-all.png)

### Today Mode

![Overview today mode dashboard](assets/overview-today.png)

## Goals

- Keep the first screen scannable. A user should quickly see trend, current spend, token volume, active days, and the dominant providers/models.
- Preserve terminal density. The layout should use compact text, box borders, and stable row heights instead of decorative panels.
- Keep the chart as the primary visual. The stacked token chart remains the largest element because it answers "when did usage happen?" faster than a table.
- Show provider mix without requiring a separate tab. Provider aggregation is useful when model names come from gateways or when unknown-provider fallback colors are being evaluated.
- Degrade cleanly on narrow terminals. Wide terminals use a two-column top section; narrow terminals fall back to a vertical chart and compact summary.

## Layout

Wide terminals:

1. Top band
   - Left: stacked token trend chart.
   - Right: compact summary metrics followed by provider mix.
2. Middle strip
   - Legend for the top colored models.
3. Bottom band
   - Scrollable model ranking with selection support.

Narrow terminals:

1. Stacked token trend chart.
2. One-line compact summary.
3. Legend.
4. Scrollable model ranking.

## Interaction Constraints

- Overview continues to use the same selection and scroll model as Models.
- The number of navigable rows must match the number of model rows actually visible in the bottom list.
- Colors are identity cues. Known provider colors should keep recognizable hue families, and uncategorized provider colors should remain stable.
