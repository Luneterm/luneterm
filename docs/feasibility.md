# Feasibility (2026-10-29)

**Verdict: feasible.** Every part has a known path; the hard parts are text rendering quality and
matching Lunegit's acrylic exactly, not the engine.

## Starting point: Lunegit's terminal

- Engine: real `@xterm/headless` 6.0.0 + `@xterm/addon-unicode11` 0.9.0, bundled with esbuild and
  run in ClearScript V8, one V8 engine per session. Lunegit adds `driver.js` (707 lines): kitty
  keyboard, modifyOtherKeys, XTVERSION, the VS16 width provider, OSC 8/52 hooks.
- PTY: Porta.Pty 2.2.3 (ConPTY / forkpty). Master fd obtained by reflection.
- Rendering: custom Avalonia `TerminalView` (2505 lines), `FormattedText` runs, 16 ms timer, no shaders.
- Glass: Avalonia `TransparencyLevelHint="AcrylicBlur, Blur, Transparent"` plus stacked
  `ExperimentalAcrylicBorder` layers (see `glass-styling.md`).
- Size of the C# + JS terminal code: about 16k lines; terminal tests about 20.6k lines.
- Earlier plan (Lunegit `todo.md`, 2026-10-13) kept xterm.js on V8. Superseded for Luneterm by the
  decisions below: Rust, small binary, no V8.

## Decisions

1. Rust, one codebase for Windows, macOS and Linux.
2. VT engine: our own port of xterm.js 6.0.0 `src/common`, test-first. No third-party VT engine.
3. Third-party crates are fine for platform layers: windowing, GPU, PTY, OS glass.

## Engine scope (measured on the pinned xterm.js checkout)

| Part | Lines (TS) |
|---|---|
| `src/common` (non-test) | 13,081 |
| `src/common` tests | 10,878 |
| `src/common/input` (incl. `Keyboard.ts`) | 2,210 incl. tests |
| `InputHandler.ts` (largest file) | 3,495 |
| headless wrapper | 340 |

Plus Lunegit-side code to port: `driver.js`, `XtermVtInputEncoder`, the OSC 7/9;9/9;4/133 scanners
and the unicode11 width tables.

Translation hazards: JS strings are UTF-16 (surrogates, width), the event/disposable pattern, and
xterm's DI container. Typed-array cell storage (`BufferLine`) maps well to Rust.

## Platform layers

| Concern | Plan | Note |
|---|---|---|
| Window/events | `winit` | |
| Drawing | `wgpu` (pick backends per OS) or native GPU | Choose by measured binary size and frame time |
| Glyphs | `swash` rasterization + own atlas | COLR/CBDT emoji support to verify; Lunegit falls back to bundled sbix Twemoji |
| PTY | `portable-pty` | Exposes the master fd directly; replaces the reflection hack |
| OS glass | `window-vibrancy` on macOS/Windows | Linux: no glass, opaque dark theme (decided) |

## Risks

1. **Acrylic fidelity.** Avalonia's acrylic is OS blur plus its own tint/material/noise blend; we
   must reproduce that blend (Avalonia is MIT, read its implementation) and compare screenshots.
2. **Text rendering.** Font fallback chains, emoji, wide-glyph containment: historically where
   Lunegit spent most terminal bug-fix effort.
3. **Binary size.** `wgpu` size is unmeasured; measure in the first window spike.
4. **Engine drift.** Upstream xterm.js fixes must be carried over by hand; mirror its layout to keep
   that cheap.

## Lunegit migration path

`luneterm-ffi` exposes the engine through a C ABI shaped like Lunegit's `IVtEngine`. Lunegit swaps
`XtermJsVtEngine` (V8) for it behind the same interface first; replacing the Avalonia renderer is a
later, separate decision.
