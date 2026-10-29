# Architecture

```
crates/
  lune-vt         VT engine (xterm.js port). No deps, no unsafe. Pure: bytes in, state + events out.
  lune-pty        Pty / PtyBackend traits; platform backends behind them.
  lune-ui         App-agnostic glass UI toolkit: painter, widgets, menus and dialogs, blur, text,
                  OS materials, window helpers, Lunegit dark theme. See "lune-ui modules".
  lune-termview   Terminal paint passes and cell tokens. Depends on lune-ui.
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
| `acrylic` | Avalonia acrylic colour math (tested) and the noise texture |
| `paint` | `Painter<H>`: immediate-mode layers, hit regions (`H` is the app's hit type), clip, transform |
| `widgets` | Controls generic over `H`; `widgets::menu`, `dropdown` and `dialog` for the overlays |
| `text`, `icon` | Shaped-text cache over glyphon (the app registers its fonts), Codicon glyphs |
| `gpu`, `blur`, `frame` | wgpu setup and pipelines, backdrop blur, and the scene → blur → target pass sequence (`FrameRenderer`) |
| `dump` | Offscreen targets and GPU readback to PNG (no screen capture) |
| `chrome` | Tab chrome loop geometry |
| `material` | `Material` and `apply(window, Material)`: OS backdrop per platform in `#[cfg]` submodules (the only `unsafe` is `material/macos.rs`) |
| `window` | Glass window attributes, `edge_at`, `DoubleClick`, `caption_buttons` |

The app owns the winit event loop. lune-ui never starts a loop or reads the clock or the
environment: the app passes time, sizes, fonts, theme and hit values in, which keeps frames
deterministic.

## Extension points

| Point | Mechanism | Example |
|---|---|---|
| Escape-sequence behaviour | `lune_vt::Extension` registers handlers in `HandlerRegistry` (newest-first, like xterm.js `register*Handler`) | kitty keyboard, XTVERSION, OSC 7 cwd, OSC 9;4 progress |
| PTY strategy | `lune_pty::PtyBackend` | portable-pty backend; later a session-daemon backend |
| OS glass | `lune_ui::material::Material` applied by `lune_ui::material::apply` (the `lune_ui::Backdrop` trait documents the capability levels) | macOS vibrancy / Liquid Glass, Windows acrylic/Mica, KDE blur |
| Theme | `lune_ui::theme::Theme` value handed to the `Painter` | `lunegit_dark::THEME` |
| Paint | `lune_termview::Pass` ordered passes | new overlay = new pass at its z-position |
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
`scripts/lib-boundaries.allow`); no clock or environment reads in `lune-ui/src`; every manifest
inherits the licence. It runs locally and on the Linux CI job.

## Threading

One engine per session, owned by that session (not `Sync`). PTY reading happens on a dedicated thread;
bytes are handed to the session, which writes them into its engine and marks dirty rows. The renderer
paints on vsync from the dirty state and honours synchronized output (mode 2026), which Lunegit parses
but does not use for painting.
