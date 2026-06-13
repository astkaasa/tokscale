# TUI Provider Colors

Tokscale colors model rows, charts, and legends by a provider color key. The color key is a stable identity used only for visual grouping; it is separate from display names, pricing providers, and client names.

## Goals

- Give known model providers recognizable color families. Exact RGB values may evolve with the TUI design, but Anthropic should read warm/coral, OpenAI green, Google blue, DeepSeek cyan, xAI amber, Meta indigo, Cursor violet, and so on.
- Avoid gray as the default unknown-provider bucket. Gray is reserved for muted, disabled, or low-emphasis UI; an unrecognized provider should still be visually distinguishable.
- Keep unknown provider colors stable. A provider key should map to the same base color across refreshes, machines, and Rust versions.
- Make future provider support predictable. Adding display names or aliases should not change colors when the canonical color key stays the same. Adding a branded color family for a provider is allowed when it improves recognition.
- Keep terminal compatibility in the theme layer. Provider color resolution returns semantic RGB colors; terminal color-mode downgrades happen through `Theme::color`.

## Color Family Principle

Provider colors are identity cues, not immutable brand assets. The implementation should preserve recognizable hue families while allowing the exact palette to move with the surrounding TUI design. Tests should guard against regressions such as gray fallback, alias drift, unstable hashes, or multiple providers collapsing into indistinguishable colors; they should not freeze every RGB value forever.

## Resolution Order

Provider shades are resolved in this order:

1. User override from `[colors.providers]`.
2. Built-in branded provider palette or provider base for known provider families.
3. Stable uncategorized provider base selected from a fixed curated palette by hashing the canonical color key.
4. Rank shade derived from the selected base color.

The same rank-shading rule applies to both branded and uncategorized providers: the highest-cost model for a provider uses the base color, and lower-ranked models use progressively lighter shades.

## Canonical Color Keys

Color keys should be canonicalized before hashing:

- Normalize case and simple separators.
- Map aliases to one canonical key where the provider identity is the same.
- Keep display names out of color keys.
- Do not hash merged provider labels such as `a, b`; derive the model owner first, then fall back to the first usable provider key only when needed.
- Treat routing or host layers such as OpenRouter, Fireworks, and Cursor as carriers when the model name reveals a clearer model owner. This keeps model lists from collapsing into one carrier color.

This keeps visual identity stable when UI labels improve later.

## Extension Policy

New providers can start in the stable uncategorized palette, but common model vendors should get a branded color family once the provider identity is clear. Users can always pin a provider color in config when they want local colors without waiting for upstream or branch defaults.
