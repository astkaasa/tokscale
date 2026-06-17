# Mouse Selection

Tokscale's TUI uses mouse capture for tabs, filters, rows, charts, and other interactive regions. Native terminal text selection still works through the terminal's modifier-key bypass.

## Selection

Use your terminal's native selection gesture with a modifier key:

| Terminal | Selection gesture | Notes |
| --- | --- | --- |
| Ghostty | Shift + drag | If Shift-click extends selection unexpectedly, set `mouse-shift-capture = never`. |
| iTerm2 | Shift + drag or Option + drag | Both modes work. |
| WezTerm | Shift + drag |  |
| Alacritty | Shift + drag |  |
| Kitty | Shift + drag |  |
| Windows Terminal | Shift + drag |  |
| GNOME Terminal | Shift + drag |  |
| Konsole | Shift + drag |  |

Normal click and drag events go to the TUI. Modifier-assisted drag bypasses application mouse capture and lets the terminal select text.

## Ghostty

For more predictable selection in Ghostty:

```ini
mouse-shift-capture = never
```

## Implementation Note

The TUI enables standard crossterm mouse capture modes including basic mouse tracking, button event tracking, any-event tracking, and SGR mouse encoding. Modern terminals generally keep a modifier-key escape hatch for native text selection even when these modes are active.
