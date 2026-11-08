# Glass styling spec (from Lunegit)

Source of truth: Lunegit `src/Lunegit.Desktop/` — `MainWindow.axaml`, `Views/TerminalPanelView.axaml`,
`Themes/DarkTheme.axaml`, `Controls/Terminal/TerminalView.cs`, `TerminalChromeGeometry.cs`,
`TerminalChromeAnimation.cs`. The constants live in the `lunegit_dark` module of `crates/lune-ui/src/theme.rs` (`lune_ui::lunegit_dark`): `THEME` for the widget tokens, `WINDOW`, `PANEL` and `CARD` for the glass layers.

## Layer stack (back to front)

1. **OS backdrop.** Transparent window, blur requested with fallback
   `AcrylicBlur → Blur → Transparent`; client area extended into the title bar.
2. **Window acrylic:** tint black, tint opacity 1, material opacity 0.7.
3. **Terminal panel acrylic:** tint 0.8, material 0.7.
4. **Card** (corner radius 8, clipped). Margin `6,0,10,10` with side panel open, else `10,0,10,10`.
   Card acrylic: tint 1, material 0.8.
5. **Header tint** (same material as card), clipped to "card minus tab chrome loop" so the active tab
   reads as punched out.
6. **Tab chrome outline:** 1 px loop, card radius 8, seam radius 8, flare radius 6; colour
   `#1AFFFFFF`, `#30FFFFFF` when the panel has focus.
7. **Terminal body** (padding 10): cells whose background is `#1E1E1E` are not painted, so the glass
   shows through. Palettes therefore never carry a background.

Header strip 32 px. Tabs: radius `8,8,0,0`, width 108–270, spacing 5.

## Inactive window

All acrylic layers hidden; solid `#333333` (tint lightened 20%) instead. In Lunegit this works around
Avalonia#5440 (macOS draws inactive material flat gray). In Luneterm, first try keeping the
NSVisualEffectView state `active` permanently; keep the solid fallback only if that fails.

## Other tokens

| Token | Value |
|---|---|
| Foreground primary | `#CCFFFFFF` |
| Selection overlay | `#33767D85` |
| Accent | `#767D85` |
| Cursor | `#80CCCCCC` block when focused; 1 px hollow outline when not |
| Scroll thumb | accent at 0.55, rounded |

## Running-command animation

Drawn on the tab chrome in accent colour (error/warning colours on failure):
- **Conic sweep** (default): gradient stops 0 / .15 (alpha 64) / .31 (alpha 0) / .69 / .85 / 1,
  period 2400 ms, plus a 3 px bloom at opacity 0.25.
- **Comet:** 14% dash. **Breath:** opacity 0.38–1.0 over 1800 ms.

Tab switch: 0.25 s sine ease-in-out crossfade.

## Open items

- Blur radius and noise texture: Lunegit uses Avalonia's acrylic defaults. Read Avalonia's
  implementation and match it; verify with side-by-side screenshots on all three OSes.
- Linux: no glass (decided 2026-10-29). Opaque dark theme: `Material::Opaque` on the window and
  `Theme::with_acrylic(AcrylicLevel::Opaque)` for the toolkit's layers (plan M10 step 6, P22; the
  toolkit's glass does not depend on the window's active state, D262).
