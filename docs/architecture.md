# Architecture

```
crates/
  lune-vt         VT engine (xterm.js port). No deps, no unsafe. Pure: bytes in, state + events out.
  lune-pty        Pty / PtyBackend traits; platform backends behind them.
  lune-ui         App-agnostic glass UI toolkit: painter, widgets, menus and dialogs, blur, text,
                  OS materials, window helpers, Lunegit dark theme. See "lune-ui modules".
  lune-termview   Engine-agnostic cell grid renderer: glyph atlas, instanced quads, GPU-drawn inside
                  a lune-ui frame. Depends on lune-ui. See "lune-termview".
  luneterm-ffi    C ABI over lune-vt for Lunegit.
  luneterm        The app: window, event loop, sessions, settings.
spikes/glass-ui   Reference consumer of lune-ui (mock terminal, settings, menus) and the pixel gate.
tools/xterm-oracle  Real xterm.js 6.0.0, used to generate differential-test expectations.
scripts/          Dev scripts (xterm.js reference, Linux check, library boundary check).
docs/             Decisions, porting guide, styling spec, bug ledger.
```

## Dependency direction

`luneterm` → `lune-termview` → `lune-ui`; `luneterm` → `lune-vt`, `lune-pty`;
`luneterm-ffi` → `lune-vt`; `glass-ui` → `lune-ui`. The engine depends on nothing, so it can be
tested, fuzzed and embedded on its own. No `lune-*` crate depends on an app crate (`luneterm*`,
`glass-ui`).

## lune-ui modules

| Module | Holds |
|---|---|
| `color` | `Rgba = [u8; 4]` (straight alpha), premultiply and blend helpers |
| `theme` | `Theme` (tokens widgets read) and the `lunegit_dark` preset: `THEME` plus the glass layers `WINDOW`, `PANEL`, `CARD` |
| `acrylic` | Avalonia acrylic colour math (tested) and the noise opacity |
| `paint` | `Painter<H>`: immediate-mode layers, hit regions (`H` is the app's hit type), clip, transform |
| `widgets` | Controls generic over `H`; `widgets::menu`, `dropdown` and `dialog` for the overlays |
| `text`, `icon` | Shaped-text cache over glyphon (the app registers its fonts), Codicon glyphs |
| `gpu`, `blur`, `frame` | wgpu setup and pipelines, the Avalonia noise texture (`assets/acrylic-noise.png`), backdrop blur, and the scene → blur → target pass sequence (`FrameRenderer`) |
| `dump` | Offscreen targets and GPU readback to PNG (no screen capture) |
| `chrome` | Tab chrome loop geometry |
| `material` | `Material` and `apply(window, Material)`: OS backdrop per platform in `#[cfg]` submodules (the only `unsafe` is `material/macos.rs`) |
| `window` | Glass window attributes, `edge_at`, `DoubleClick`, `caption_buttons` |

The app owns the winit event loop. lune-ui never starts a loop or reads the clock or the
environment: the app passes time, sizes, fonts, theme and hit values in, which keeps frames
deterministic.

## lune-termview

Draws a cell grid inside a lune-ui frame. It knows nothing about the VT engine: the producer (the
engine from M5, the mock adapter in `spikes/glass-ui/src/term_grid.rs` until then) fills the grid.

- **Model** (`model`): `Grid` of `Cell` (text, `Color` = default / palette / RGB, `Attrs`,
  underline style and colour, link id) with per-row dirty flags. A wide character is a width-2
  leading cell followed by a width-0 spacer (xterm.js convention). Cluster segmentation and widths
  are the producer's job; the renderer never computes Unicode widths. Per-frame `ViewState`
  carries cursor, selection ranges, focus, blink phase and the hovered link id.
- **API** (`TermView`): `new(device, format, TermFonts, TermTheme)`, `metrics(fs, scale)` (integer
  device-pixel cell size, so columns never drift), `update(fs, &mut Grid, &ViewState, origin,
  scale)` (rebuilds dirty rows on the CPU), and `impl CustomDraw` (`prepare` uploads, `render`
  draws). `GridLayout` maps pixels to cells and back.
- **Inside lune-ui:** the app records `Painter::custom(slot, rect)` in the Base layer and calls
  `FrameRenderer::draw_with(.., &mut [&mut term])`. The item draws after the layer's under-text
  shapes and before its text, so a popup's backdrop blur sees the grid.
- **Passes** (`Pass`, back to front): cell backgrounds (default and theme-background cells are
  skipped so glass shows through), glyphs (mask, then colour), decorations (five underline styles,
  strikethrough, overline), cursor, selection, link hover. Glass, scrollbar and chrome are drawn by
  the app around the grid.
- **Text:** one-line cosmic-text shaping per cluster (`Shaping::Advanced`, with fallback),
  cached by (text, bold, italic). Clusters with U+FE0F try `TermFonts.emoji_family` first. Glyphs
  sit in an etagere atlas (a mask page and a colour page) keyed by cosmic `CacheKey` (font id,
  glyph id, size, subpixel bin, weight, flags) or, for procedural glyphs, `AtlasKey::Custom`.
  Ligatures (ledger 15) are not shaped yet; whole-row shaping can reuse the same key later.
- **Containment** (`contain`): Lunegit's rule with two deviations. A width-1 non-colour glyph
  followed by a blank cell may spill one cell. Anything wider is fitted to its slot by
  re-rasterizing at a smaller size, and an oversized colour glyph is scaled, not clipped. A
  multi-glyph cluster is fitted on its base glyph's advance, and later glyphs sit at the base's
  pen plus their own offset (some fonts give every combining mark a full cell of advance). Every
  glyph instance also carries a clip rect (its slot plus the allowance horizontally, its row
  vertically) that the fragment shader enforces, so nothing paints outside its row.
- **Procedural glyphs:** `custom_glyphs.rs` ports xterm.js `CustomGlyphs.ts` (box drawing, blocks
  and shades, the legacy-computing set, powerline; the whole upstream table). It draws through
  `glyph_canvas.rs` (an A8 canvas on zeno) into the mask page at exactly the cell size, so they
  join across cells whatever the font. One `// LUNETERM:` deviation changes behaviour: the stroke
  snap puts both edges of even-width lines on pixel boundaries (upstream's formula is only crisp at
  odd widths, i.e. 1x).
- **GPU buffers:** one persistent instance buffer per kind (backgrounds, mask glyphs, colour
  glyphs, decorations) with a fixed slot per row; only dirty rows are rewritten. Overlays are
  rebuilt each frame. An atlas reset or growth rebuilds every row (the UVs moved).
- **No clock, no environment:** blink phase and time come from the app; rule 4 of
  `scripts/check-lib-boundaries.sh` covers `crates/lune-termview/src`.
- **Tests:** CPU unit tests, offscreen GPU golden PNGs (`crates/lune-termview/tests/golden.rs`,
  fonts from committed subsets in `tests/fonts`), and a font-independent "no ink outside its
  cell" invariant. Without an adapter the GPU tests print `SKIPPED`; `LUNE_REQUIRE_GPU=1` turns a
  skip into a failure. `LUNE_TERMVIEW_BLESS=1` rewrites the goldens.
- **Frame time** (`cargo run --release -p lune-termview --example frame_time`; Apple M1 Pro,
  Metal, 3840x2160 at scale 2, 240 x 63 cells of dense random text; median / p95 ms):

  | Case | `update` (CPU) | prepare + render + submit + poll |
  |---|---|---|
  | all rows dirty | 2.49 / 2.59 | 3.87 / 7.25 |
  | one dirty row | 0.064 / 0.085 | 2.06 / 4.58 |
  | cursor blink only | 0.013 / 0.019 | 2.25 / 5.05 |

  The GPU column includes waiting for the GPU to finish, so it is an upper bound on the cost of
  a frame; a blink-only frame costs about the same as an idle redraw of the whole grid.

## Extension points

| Point | Mechanism | Example |
|---|---|---|
| Escape-sequence behaviour | `lune_vt::Extension` registers handlers in `HandlerRegistry` (newest-first, like xterm.js `register*Handler`) | kitty keyboard, XTVERSION, OSC 7 cwd, OSC 9;4 progress |
| PTY strategy | `lune_pty::PtyBackend` | portable-pty backend; later a session-daemon backend |
| OS glass | `lune_ui::material::Material` applied by `lune_ui::material::apply` (the `lune_ui::Backdrop` trait documents the capability levels) | macOS vibrancy / Liquid Glass, Windows acrylic/Mica, KDE blur |
| Theme | `lune_ui::theme::Theme` value handed to the `Painter` | `lunegit_dark::THEME` |
| Paint | `lune_termview::Pass` ordered passes: cell backgrounds, text (mask then colour), decorations, cursor, selection, link hover | new overlay = new pass at its z-position |
| Host embedding | `luneterm-ffi` C ABI, versioned by `luneterm_abi_version()` | Lunegit P/Invoke |

## Split points

Where the repo can be cut later (nothing is split yet):

- `lune-vt` can move to a library repo once the FFI, its second consumer, has settled its API. It
  takes `tools/xterm-oracle`, the goldens and the conformance harness with it.
- `lune-ui` moves when a second app starts. `lune-termview` goes with it, and `lune-pty` with
  whichever moves first.

Invariants that keep a split cheap:

1. No `lune-*` crate depends on a `luneterm*` crate.
2. No `lune-*` crate holds Luneterm identity (names, env values, copy, paths).
3. Each library's tests and test data live in its crate directory, or in a tool named for it.
4. Every crate sets `license` (inherited from the workspace: MIT OR Apache-2.0).

`scripts/check-lib-boundaries.sh` enforces them with grep rules: no `luneterm` in `lune-*`
manifests or files; no app or terminal words in `lune-ui` (the allow list is
`scripts/lib-boundaries.allow`); no clock or environment reads in `lune-ui/src` or `lune-termview/src` (rule 4; tests and
examples may); every manifest inherits the licence. It runs locally and on the Linux CI job.

## Threading

One engine per session, owned by that session (not `Sync`). PTY reading happens on a dedicated thread;
bytes are handed to the session, which writes them into its engine and marks dirty rows. The renderer
paints on vsync from the dirty state and honours synchronized output (mode 2026), which Lunegit parses
but does not use for painting.
