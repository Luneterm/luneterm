# Glass styling spec (from Lunegit)

Source of truth: Lunegit `src/Lunegit.Desktop/` — `MainWindow.axaml`, `Views/TerminalPanelView.axaml`,
`Themes/DarkTheme.axaml`, `Controls/Terminal/TerminalView.cs`, `TerminalChromeGeometry.cs`,
`TerminalChromeAnimation.cs`. The constants live in the `lunegit_dark` module of luneui-rs's `lune-ui-core` theme (`lune_ui::lunegit_dark`; the pinned rev's `src/theme.rs`): `THEME` for the widget tokens, `WINDOW`, `PANEL` and `CARD` for the glass layers.

## The default look is near-black (since M10 step 13, 2026-11-09)

The layer stack below is **Lunegit's**; Luneterm shows it only under the liquid styles. The default
style (`GlassStyle::Default`, persisted as `default`) is luneui-rs's `near_black` look (pin `5be5dbe`,
`crates/lune-ui-core/src/theme/near_black.rs`, the user-approved test build), as one value:

| What | Value |
|---|---|
| Window material | `near_black::MATERIAL`: macOS `UltraDark` vibrancy, state `Active`; Windows acrylic, tint `#A0000000`, dark (the tint is honoured on builds 17763 to 22522 only) |
| Theme | the whole `near_black::THEME` (as `DEFAULT_GLASS_THEME`), never `theme.glass = GLASS` alone: menus, dropdowns, popovers, tooltips, toasts and dialogs fill from `menu_glass` (`near_black::MENU_GLASS`, `#0A0A0A9D`) |
| Layers | tint black, tint opacity 1, material opacity 0.1; luminosity `black_max` **0.42** window, **0.38** panel, **0.48** card: the app's `render::DEFAULT_GLASS_THEME`, `near_black::THEME` with only these three lowered from the toolkit's 0.60 / 0.55 / 0.65 (the user's eye-check of 2026-11-09: "more transparent"). Raise them for blacker glass with less blur showing through, lower them for more transparency |
| Header tint | the app's own `shell::NEAR_BLACK_HEADER_TINT`, a 6% white wash (`#FFFFFF10`), not luneui-rs's 4% `near_black::HEADER_TINT`: over black glass a black wash shows nothing, and at 4% the header was within 6 levels of the title band over a dark backdrop (3 at `#30`, crossing over a mid one); at 6% band against header is 12 / 8 / 3 levels over `#10` / `#30` / `#60` and header against card about 14. The header is a lighter band and the active tab is punched out of it |

**macOS experiment (2026-11-09, user eye-check against iTerm):** the `UltraDark` vibrancy blurs and
desaturates the desktop and the 0.1 grey material layer adds grey, so the window read as a flat
brown-grey where iTerm is plain black at partial alpha with the desktop's hue kept. On macOS the
default style therefore applies no OS material (`app::apply_material` clears any effect view; the
window is created transparent) and draws `render::FLAT_GLASS_THEME`: all three layers black with
material opacity 0 and `render::FLAT_BLACK` = 0.65 opacity (iTerm profile: black, transparency 0.347) (layers replace what is under them, so one
value keeps the seams invisible). Menus and dialogs (`MENU_GLASS`) and the header tint are as above.
Windows keeps `near_black::MATERIAL` and `render::DEFAULT_GLASS_THEME` (0.42 / 0.38 / 0.48). Not seen
on a real window yet.

The app chooses all three by the pair (glass behind the window, style): `render::theme` and
`render::header_tint`. No glass (Linux, `Off`, a refused material) is Lunegit's theme with
`with_acrylic(Opaque)` for every style; the near-black theme is never made opaque with
`with_acrylic` (its material opacity of 1 would give a light grey layer). Liquid styles keep Lunegit's
theme and header tint. The glass does not depend on the window's active state, so the look is the same
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

## Open items

- Blur radius and noise texture: Lunegit uses Avalonia's acrylic defaults. Read Avalonia's
  implementation and match it; verify with side-by-side screenshots on all three OSes.
- Linux: no glass (decided 2026-10-29). Opaque dark theme: `Material::Opaque` on the window and
  `Theme::with_acrylic(AcrylicLevel::Opaque)` for the toolkit's layers (plan M10 step 6, P22; the
  toolkit's glass does not depend on the window's active state, D262).
