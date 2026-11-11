# Glass styling spec (from Lunegit)

Source of truth: Lunegit `src/Lunegit.Desktop/` — `MainWindow.axaml`, `Views/TerminalPanelView.axaml`,
`Themes/DarkTheme.axaml`, `Controls/Terminal/TerminalView.cs`, `TerminalChromeGeometry.cs`,
`TerminalChromeAnimation.cs`. The constants live in the `lunegit_dark` module of luneui-rs's `lune-ui-core` theme (`lune_ui::lunegit_dark`; the pinned rev's `src/theme.rs`): `THEME` for the widget tokens, `WINDOW`, `PANEL` and `CARD` for the glass layers.

## The default look is near-black (since M10 step 13, 2026-11-09)

The layer stack below is **Lunegit's**. Which layers the window uses is the **Theme** setting
(Settings > Appearance, key `theme`, `luneterm` by default or `lunegit`, applied live like Glass; a
missing or unknown value reads as `luneterm`), under every glass style: Luneterm is the near-black
look, Lunegit is the layer stack below. The glass style picks only the OS material, and its
`default` follows the theme: `near_black::MATERIAL` for Luneterm, `Material::Vibrancy` (light
vibrancy, acrylic on Windows) for Lunegit; the liquid styles and Off are the same under both. The
near-black look is luneui-rs's `near_black` (landed at luneui-rs `5be5dbe`; the pin is now v0.5.1,
`crates/lune-ui-core/src/theme/near_black.rs`, the user-approved test build), as one value:

| What | Value |
|---|---|
| Window material | `near_black::MATERIAL`: macOS `UltraDark` vibrancy, state `Active`; Windows acrylic, tint `#A0000000`, dark (the tint is honoured on builds 17763 to 22522 only) |
| Theme | the whole `near_black::THEME`, never `theme.glass = GLASS` alone: menus, dropdowns, popovers, tooltips, toasts and dialogs fill from `menu_glass` (`near_black::MENU_GLASS`, `#0A0A0A9D`) |
| Layers | tint black, tint opacity 1, material opacity 0.1; luminosity `black_max` 0.60 window, 0.55 panel, 0.65 card |
| Header tint | the app's own `shell::NEAR_BLACK_HEADER_TINT`, a 6% white wash (`#FFFFFF10`), not luneui-rs's 4% `near_black::HEADER_TINT`: over black glass a black wash shows nothing, and at 4% the header was within 6 levels of the title band over a dark backdrop (3 at `#30`, crossing over a mid one); at 6% band against header is 12 / 8 / 3 levels over `#10` / `#30` / `#60` and header against card about 14. The header is a lighter band and the active tab is punched out of it |

The app chooses all three by the pair (glass behind the window, theme): `render::theme(glass, look)`
and `render::header_tint(look)`; the glass style is no input. No glass (Linux, `Off`, a refused
material) is the look's own theme with `with_acrylic(Opaque)`, the near-black one included: the
step-7 dump of `opaque@2x.png` measures its body at `#101010` and Lunegit's (`lunegit-opaque@2x.png`)
at `#161616`, not the light grey the earlier note feared (that note read the layer's material opacity
without the opaque backdrop under it). The glass does not depend on the window's active state, so the look is the same
active and inactive. `shell::DIALOG_FILL` (`[16,16,16,235]`) is the dialog card's own fill and is judged
on the near-black dumps.

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

## Inactive window (Lunegit)

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
Implemented in `crates/luneterm/src/fade.rs` (pure rules, unit-tested) and drawn by `lune-termview`'s
`TermView::set_opacity`: the outgoing tab's grid draws in a second view under the incoming one, each
at its opacity (the glass and the chrome do not fade). As in Lunegit's `TerminalPanelViewModel`:
every layer animates from the opacity it has now, so a quick second switch continues from where the
first was (the generation rule); `Full` is 250 ms, `Reduced` 180 ms, `None` a hard cut; the first tab
and a restored workspace do not fade; redraws are requested only while it runs.

## Text rendering

Glyphs are smoothed (luneui-rs D296, variant A, `TextRendering::SMOOTH`: no hinting, stem
darkening by size) in the UI text and in the terminal grid alike. Lunegit has no setting for
it; Luneterm adds one (Settings > Terminal > Appearance, "Font smoothing", on by default, key
`font_smoothing`, applied live): off gives `TextRendering::OFF`, the hinted look of before. The
unsmoothed look is also reachable for dumps (`LUNETERM_DUMP_TEXT=off`) and in the termview goldens
(`attributes-text-off@2x`). Box, block
and powerline glyphs are drawn procedurally and look the same either way.

## Open items

- Blur radius and noise texture: Lunegit uses Avalonia's acrylic defaults. Read Avalonia's
  implementation and match it; verify with side-by-side screenshots on all three OSes.
- Linux: no glass (decided 2026-10-29). Opaque dark theme: `Material::Opaque` on the window and
  `Theme::with_acrylic(AcrylicLevel::Opaque)` for the toolkit's layers (plan M10 step 6, P22; the
  toolkit's glass does not depend on the window's active state, D262).
