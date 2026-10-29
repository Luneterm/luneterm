# Architecture

```
crates/
  lune-vt       VT engine (xterm.js port). No deps, no unsafe. Pure: bytes in, state + events out.
  lune-pty      Pty / PtyBackend traits; platform backends behind them.
  lune-ui    OS backdrop (Backdrop trait) + Lunegit glass tokens.
  lune-termview   Paint passes, glyph atlas, glass compositing. Depends on lune-ui.
  luneterm-ffi      C ABI over lune-vt for Lunegit.
  luneterm          The app: window, event loop, sessions, settings.
tools/xterm-oracle  Real xterm.js 6.0.0, used to generate differential-test expectations.
scripts/            Dev scripts (fetch pinned xterm.js reference).
docs/               Decisions, porting guide, styling spec, bug ledger.
```

## Dependency direction

`luneterm` → `lune-termview` → `lune-ui`; `luneterm` → `lune-vt`, `lune-pty`;
`luneterm-ffi` → `lune-vt`. The engine depends on nothing, so it can be tested, fuzzed and
embedded on its own. No crate depends on the app.

## Extension points

| Point | Mechanism | Example |
|---|---|---|
| Escape-sequence behaviour | `lune_vt::Extension` registers handlers in `HandlerRegistry` (newest-first, like xterm.js `register*Handler`) | kitty keyboard, XTVERSION, OSC 7 cwd, OSC 9;4 progress |
| PTY strategy | `lune_pty::PtyBackend` | portable-pty backend; later a session-daemon backend |
| OS glass | `lune_ui::Backdrop` per platform | macOS vibrancy / Liquid Glass, Windows acrylic/Mica, KDE blur |
| Paint | `lune_termview::Pass` ordered passes | new overlay = new pass at its z-position |
| Host embedding | `luneterm-ffi` C ABI, versioned by `luneterm_abi_version()` | Lunegit P/Invoke |

## Threading

One engine per session, owned by that session (not `Sync`). PTY reading happens on a dedicated thread;
bytes are handed to the session, which writes them into its engine and marks dirty rows. The renderer
paints on vsync from the dirty state and honours synchronized output (mode 2026), which Lunegit parses
but does not use for painting.
