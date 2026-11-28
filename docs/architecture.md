# Architecture

```
crates/
  lune-vt         VT engine (xterm.js port). No deps, no unsafe. Pure: bytes in, state + events out.
                  Parser, decoders, charsets, buffer, every core service, the InputHandler,
                  CoreTerminal and the headless Terminal (`VtEngine`) are ported; see "lune-vt
                  parser", "lune-vt buffer" and "lune-vt engine".
  lune-vt-conformance  Unpublished test-data crate: corpus, xterm.js goldens, golden reader,
                  parser-trace and screen runners for lune-vt. See "lune-vt parser".
  lune-pty        Pty / PtyBackend traits; PortablePtyBackend (portable-pty 0.9.0: ConPTY on Windows,
                  openpty on Unix) with raw-mode and foreground-process queries. See "lune-pty".
  lune-termview   Engine-agnostic cell grid renderer: glyph atlas, instanced quads, GPU-drawn inside
                  a lune-ui frame. Depends on lune-ui (luneui-rs, below). See "lune-termview".
  luneterm-ffi    C ABI over lune-vt for Lunegit (header `include/luneterm.h`). See "luneterm-ffi (C ABI)".
  luneterm        The app (lib + bin): window, frame loop, tabs (one session per tab), selection,
                  links, input routing, spawn, settings and their files, shell integration.
                  See "luneterm app".
(no lune-ui crate: the toolkit is luneui-rs, a git dependency pinned at one rev in the root
 `Cargo.toml` and in `scripts/check-deps.sh`; see "lune-ui (toolkit)")
tools/xterm-oracle  Real xterm.js 6.0.0, used to generate differential-test expectations.
scripts/          Dev scripts (xterm.js reference, Linux check, library boundary check, dependency
                  source check).
docs/             Decisions, porting guide, styling spec, bug ledger.
```

## Dependency direction

`luneterm` → `lune-termview` → `lune-ui` (luneui-rs, git rev); `luneterm` → `lune-ui` (luneui-rs),
`lune-vt`, `lune-pty`, `lunepack` (the update client, git rev); `luneterm-ffi` → `lune-vt`. The engine depends on nothing, so it can be
tested, fuzzed and embedded on its own. No `lune-*` crate depends on an app crate (`luneterm*`).
`scripts/check-deps.sh` reads `cargo metadata --locked` and fails on any package outside the
workspace, crates.io and the two pinned git `(repo, rev)` pairs, luneui-rs and lunepack (`[patch]`,
source replacement and branch or tag pins included); CI runs it and builds with `--locked`. The
lunepack runtime and update window that ship beside the app are built from lunepack's own lock and
are checked by `scripts/build-lunepack.{sh,ps1}`, not by this script ("Updates" below).

## lune-ui (toolkit)

The app and `lune-termview` use **luneui** (`https://github.com/lune-rs/luneui`, formerly luneui-rs; `lune-ui` 0.8.0, tag `v0.8.0`: `core`,
`wgpu`, `winit`, `platform` crates behind one facade): its module map and component catalogue are in
that repo's `docs/architecture.md` and `docs/components/INDEX.md`. What Luneterm found while being
its first real consumer, and the two components it owes evidence for, is in
`docs/luneui-rs-handoff.md`. Luneterm never patches the toolkit (roadmap D23): a gap is recorded
there and worked around app-side.

The in-tree toolkit (the first `lune-ui` package, 0.1.0) and its reference consumer, the glass UI spike, were
retired in M10 step 12: nothing imported them but each other (`cargo tree -i lune-ui@0.1.0` listed
only the spike), and the pieces the app still needed (the palettes, the settings model, the
menus) had moved into `crates/luneterm` in step 6. The app owns the winit event loop; the toolkit
never starts a loop or reads the clock or the environment: the app passes time, sizes, fonts,
theme and hit values in, which keeps frames deterministic.

## lune-termview

Draws a cell grid inside a lune-ui frame. It knows nothing about the VT engine: the producer fills
the grid (in the app, `session::apply_update` from the engine thread's Updates; see "luneterm app").

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
  cached by (text, bold, italic). Clusters with U+FE0F try `TermFonts.emoji_family` first; a
  private-use code point the terminal's own family lacks comes from `TermFonts.icon_family`, ahead
  of the platform's fallback list. Glyphs
  sit in an etagere atlas (a mask page and a colour page) keyed by cosmic `CacheKey` (font id,
  glyph id, size, subpixel bin, weight, flags) **and** the toolkit's `GlyphParams` (hinting and
  stem darkening, from `TextRendering::glyph_params` at the glyph's own size) or, for procedural
  glyphs, `AtlasKey::Custom`. Glyphs are rasterized by the toolkit's `GlyphRasterizer`.
- **Text rendering:** one `TextRendering` (`SMOOTH`, the toolkit's look since luneui-rs D296, or
  `OFF` when the `font_smoothing` setting is off; `Effect::TextRendering` applies a change live
  through `Renderer::set_text_rendering`) is held by the app's `Renderer` and handed both to the frame (`FrameParams.text`) and to
  every `TermView::update` (the active grid and the fading ones), so the UI text and the grid
  draw alike. It is part of the builder's `BuildKey`: a change rebuilds every row, and the atlas
  keeps the entries of the setting not in use, so a toggle never serves a glyph rasterized under
  the other one. Procedural glyphs are not font outlines and ignore it. Frame dumps take
  `LUNETERM_DUMP_TEXT=on|off` (default `on`).
- **Ligatures** (ledger 15; the `ligatures` setting, `TermFonts.ligatures`, off by default since v0.4.1). A per-cell
  glyph substitution, in `build.rs` (`ligature_subs`): in each row a run is a maximal stretch of two or
  more shown cells of one ASCII-punctuation character each (one cell wide) with the same bold, italic
  and resolved foreground. The run is shaped as one string with harfrust directly (cosmic-text's
  `Buffer` splits `||=`), and only when every cell on its own shapes to one real glyph of one face
  and the run gives one glyph per cell; the cells whose glyph differs from their own draw the run's
  glyph at their own placement, the ligature's glyph clipped from its first cell (the clip widens left
  over the span of substituted cells), the others draw nothing. Anything else draws per cell (fail
  closed): a wide or non-ASCII cell, a colour or style change, a blank cell or a run whose shaped glyph
  count is not the cell count. Limits: no join across wrapped rows (a row is the unit), none across a
  style or colour change, none through a blank cell, and the cursor does not split a run (it is drawn
  over the substituted cells). The shaped runs are cached in the `Shaper` (bounded, cleared by a font
  change). The face decides which sequences join: RznMono 1.003 has 128 `calt` sequences; a system font
  chosen as the terminal font ligates only what it carries (unverified beyond RznMono). Cost: all rows
  dirty at 4K, 3.08 ms with ligatures off and 3.30 ms on (`frame_time`).
  With Font smoothing on, three ligatures (`|]`, `|}`, `<||`) drew at about half their ink until the
  luneui-rs v0.6.1 pin (`docs/luneui-rs-handoff.md` gap 9, closed).
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
- **Line height:** `natural_line_height(fs, family, size)` is the one place the app asks for the
  font's natural line height (the same "W" shaping read as `cell_metrics`), so cell height comes from
  the font, not from a constant (M9 D16).
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

## lune-pty

`Pty` / `PtyBackend` (`crates/lune-pty/src/lib.rs`) and the one backend, `PortablePtyBackend`
(`portable.rs`; `unix.rs` and `windows.rs` hold the platform parts). `Pty` has `read`, `write_all`,
`resize`, `try_exit_code` and `kill`, plus `take_reader()`, `writer()` (a cloneable `PtyWriter`),
`process_id()`, `raw_mode()` and `foreground_process()`. `PtyWriter::new` is public (M9 D20: a
backend outside this crate, and the session's fake PTY, build one), as are `restrict_dll_search()`
and `conpty_build()` (D17, below).

- **Validation fails closed:** `columns` and `rows` must be 1..=32767, `cwd` an existing directory,
  and `args` need a `program`.
- **Environment:** the child gets the inherited environment plus `SpawnOptions.env`; the crate
  adds no terminal identity (`TERM`, `COLORTERM` and the like are the app's job). One exception
  from portable-pty: on Unix `SHELL` is set when it is missing. On Windows portable-pty would also
  merge the registry environment and let the user `PATH` replace the inherited one; the backend
  clears that and uses the inherited environment (assumed decision Q7, reversible in four lines of
  `portable.rs`).
- **Raw mode** (ledger 9): `tcgetattr` on the master, else the slave opened transiently by
  `tty_name()`. No `unsafe`, no termios offsets. Windows: always `Unknown`;
  `foreground_process()` is `None` there.
- **Windows ConPTY input** was measured on CI (ledger 10): `docs/conpty-input.md`.

Obligations on the host:

- **DSR.** ConPTY asks for the cursor position (`ESC[6n`) at startup and may stall until the
  answer is written back. The VT engine answers every `ESC[6n` (a `HostEvent::Response`, which the
  session writes to the PTY: done, `crates/luneterm/src/session/engine.rs` pushes every `Response` to the
  writer thread); `lune-pty` does not, since a second answer would be injected input. Tests and the
  ConPTY harness answer it in their reader.
- **Unix EOF** arrives only after every holder of the slave closes it, so a background job can
  delay it past the shell's exit.

**DLL search hardening (decision D4, Windows).** portable-pty loads `conpty.dll` by bare name. The
first spawn therefore calls `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` once
(`windows.rs`), and spawn fails if the call fails. The effect is process-wide and permanent: later
bare-name `LoadLibrary` calls skip the current directory and `PATH`.
`crates/lune-pty/tests/dll_search.rs` proves the plant is not loaded after a spawn.

**Verified in M9 (wgpu 30, Windows): the GPU stack needed the same hardening, earlier.** wgpu's
`Dx12Compiler` defaults to `Auto` (wgpu-types 30.0.1 `backend.rs`), which tries `dxcompiler.dll` by
bare name through `libloading::Library::new` (wgpu-hal 30.0.1 `dx12/instance.rs`,
`shader_compilation.rs`) and falls back to FXC `d3dcompiler_47.dll`; `wgpu::Instance::new`
initializes each backend eagerly (wgpu-core `instance.rs`), so the load happens inside
`gpu::instance(..)`, which runs before the first PTY spawn. With the default search order a
`dxcompiler.dll` in the current directory or on `PATH` would be loaded into the process. luneui-rs
sets no compiler (`lune-ui-wgpu/src/gpu.rs:201-204`, `platform/windows.rs` sets only the swapchain
kind), so the app cannot choose FXC either (see `docs/luneui-rs-handoff.md`). **Fix (D17):**
`lune_pty::restrict_dll_search()` is public and `luneterm`'s `main` calls it first, before the event
loop or any GPU object, and exits 1 if it fails. Proof: `crates/luneterm/tests/dll_search_gpu.rs`
(Windows, `harness = false`) re-runs itself as three children with `version.dll` planted as
`dxcompiler.dll`: the control (no hardening, cwd = the plant's directory) loads the plant, so the
instrument sees a plant at all; the hardened children, in the plant's directory and with it on
`PATH`, get NULL for the bare-name load and for `GetModuleHandleW` after `gpu::instance(None)` and
`Gpu::new`. CI run 42474728954 (windows-latest, the step 7 commit): control loaded the plant, both
hardened children loaded NULL, with the GPU initialized.

**Executable resolution (finding S1, Windows).** portable-pty passes the program to
`CreateProcessW` as `lpApplicationName`, which completes a relative name with the host's current
directory, so `spawn` first resolves it to an absolute existing file (`windows.rs`
`resolve_program`): a bare name only from absolute, non-empty `PATH` entries, `ComSpec` only if
absolute (else System32 `cmd.exe`), anything else `NotFound`. `tests/exe_search.rs` plants
executables in the current directory and checks both the hole (portable-pty alone) and the fix.
`SpawnOptions::cwd: None` starts the child in the home directory (`$HOME` / `%USERPROFILE%`).

## lune-vt parser

`crates/lune-vt/src/parser/`, `input/text_decoder.rs`, `input/x_parse_color.rs`, `data/` are ports
of the xterm.js 6.0.0 files of the same names (`docs/porting-xterm.md` has the status of every
`src/common` file). The buffer slice is described under "lune-vt buffer"; `InputHandler`,
`CoreTerminal` and the `VtEngine` implementation under "lune-vt engine".

**Handler API.** `EscapeSequenceParser<C: ParserHost>` is generic over the host state `C` and
never owns or references it. `parse(&mut self, host: &mut C, data: &[u32])` (and `reset`) borrow
the host for the duration of the call and pass it to every handler as `&mut C`. Handlers are
`'static` boxed closures (`FnMut(&mut C, ..)`) or, for OSC and DCS, `OscHandler<C>` /
`DcsHandler<C>` trait objects with `start/put/end` and `hook/put/unhook`. They are stored per
identifier and tried newest-first, as xterm.js's `register*Handler` does; `true` consumes the
sequence, and the fallback runs when none did (OSC/DCS: when the identifier has no handler).
Registration returns a `HandlerId`; `dispose_handler(id)` replaces `IDisposable`.

**No back-references.** A handler holds no reference to the parser or the host: the host arrives
per call, so there is no `Rc<RefCell<..>>` cycle and no re-entrancy panic. A consequence is that a
handler cannot register or dispose handlers while it runs (it has no parser). Nothing under
`parser/` uses `Rc`, `RefCell` or raw pointers.

**`ParserHost`.** xterm.js keeps `precedingJoinState` on the parser; `print` and REP read and
write it while the parser is dispatching, so it lives on the host behind
`ParserHost::preceding_join_state(&mut self) -> &mut u32`, and the parser zeroes it where
upstream does (after EXECUTE, CSI and ESC dispatch, OSC end, DCS unhook, and in `reset`).

**Async.** Handlers are synchronous and return `bool`; upstream's resumable/`Promise` path is not
ported (the engine's `write` is synchronous).

**Lunegit's additions (M7).** Source files and deviations: `docs/porting-xterm.md`, "Ported from
Lunegit". Nothing below is active until the host calls
`Terminal::install_standard_extensions(&StandardExtensions) -> Result<(), ExtensionError>` once,
right after `Terminal::new`; a terminal that never calls it behaves as xterm.js, so every
upstream-equal test and golden is unaffected. `StandardExtensions { xtversion: Option<String>,
vs16: bool, clipboard_write: bool }`; a second call is `ExtensionError::AlreadyInstalled`, a bad
XTVERSION name `ExtensionError::InvalidXtversion` (nothing is registered then).

- **Extensions** (`ext/`, one file each): kitty keyboard flags (1 and 4; stack of 8 per screen) and
  modifyOtherKeys; XTVERSION (`DCS > | name ST`, name from the host); the unknown-OSC report;
  `"11-vs16"` Unicode version; OSC 52 clipboard write; OSC 7 and 9;9 cwd; OSC 9;4 progress;
  OSC 133 marks with the synchronized-update latch; and, only with
  `StandardExtensions::notifications`, the notification extension (`ext/osc_notify.rs`: a plain
  OSC 9 message and `OSC 777;notify;<title>;<body>` become `Notification`; an OSC 9 whose first
  field is digits is a sub-command and never a message; the payload is capped at 4096 bytes and
  dropped whole over it; the text is never logged; with the flag off, OSC 777 stays an
  `UnknownOsc` and a plain OSC 9 stays silent). They are handlers on the parser's own registry
  (no second dispatch path); an extension that registers CSI `t` bypasses the window-option guard
  (`extension.rs`).
- **Events** the host receives from `drain_events`: `CwdChanged(String)`, `Progress(Progress {
  state, percent })`, `CommandMark(CommandMark { kind, exit_code })`, `SyncUpdateInCommand`,
  `ClipboardWrite(String)`, `UnknownOsc { ident, len }` and `Notification(Notification { title,
  body })` (raw program text, flag-gated, at most 8 per drain). `Modes` gains `kitty_keyboard_flags`
  (active screen) and `modify_other_keys`.
- **Encoders** on `Terminal`, all returning the bytes to write to the PTY (`Option<Vec<u8>>`, `None`
  when nothing is sent: `disable_stdin`, mouse not reported, focus reporting off, empty paste),
  all through `triggerDataEvent` so `scroll_on_user_input` applies as upstream: `key_event(&TerminalKeyEvent)`
  (kitty, then modifyOtherKeys, then legacy, from the live modes and DECCKM), `key_input`,
  `char_input`, `mouse_event(&TerminalMouseEvent)` (the engine's own DECSET protocol and
  encoding, zero-based cells in), `focus_event(bool)` (`ESC [ I` / `ESC [ O` only after
  `CSI ? 1004 h`, no scroll) and `paste(&str)`. Pure forms: `encode_key(&TerminalKeyEvent,
  KeyboardModes) -> Vec<u8>` and `paste_bytes(&str, bracketed: bool) -> Vec<u8>` (the sanitizer,
  ledger row 19). The bytes are returned, not queued as `HostEvent::Response`.
- **Reset hook:** `Terminal::reset_keyboard_protocol()` (ledger row 11).
- **Runtime gate:** `Terminal::set_clipboard_write(bool)` / `clipboard_write()` (OSC 52).

**Host duties M9 inherits from M7** (in addition to the list above; paths under `crates/luneterm/src`):
- Write every encoder result to the PTY (`PtyWriter`), after mapping the toolkit's events to
  `TerminalKeyEvent`, `TerminalModifiers` and `TerminalMouseEvent`. Mouse positions are zero-based
  cells; pixel `x`, `y` matter only under `CSI ? 1016 h`. **Done:** the app maps winit's `KeyEvent`
  itself (`input/keys.rs`, not the toolkit's coarser `Key`, D4/D5), mouse and wheel in
  `input/pointer.rs`; the engine thread encodes (`session/engine.rs`) and the writer thread writes
  (`session/threads.rs`); `UpdateModes.mouse_encoding` carries `?1016` to the app.
- Send paste text only through `Terminal::paste` / `paste_bytes`; never write clipboard text to the
  PTY directly. **Done:** `Command::Paste` is the only path from the clipboard to the PTY
  (`session/engine.rs` runs `Terminal::paste`; e2e E3 checks the bytes).
- Selection (M10 step 3): the model and the word, line and wrapped-line rules are a port of
  xterm.js's `SelectionModel` and `SelectionService` in `crates/luneterm/src/session/selection.rs`,
  on the engine thread (the UI never holds a `Terminal`). The UI sends `Command::Select` with
  viewport cells and `Command::CopySelection`; the mouse-up that ends a selection is `SelectCmd::End { copy }`, and with the Copy on select setting on (Terminal > Behavior, `copy_on_select`, default on; `copy` true) the engine copies in that same command when something is selected (`has_selection`: a plain click selects nothing and never writes the clipboard); the focus-loss and overlay releases, Select All and the menu and keyboard copies are not mouse-ups and send `copy: false` or `CopySelection` as before; an Update carries `selection` (buffer coordinates,
  only when it changed) and `selection_text` (at most 32 MiB, else `CopyResult::TooLarge` and a
  toast). A marker on the anchor line follows trim (`Terminal::dispose_marker` frees it); a
  vertical resize, a buffer switch and typing clear the selection.
- Links (M10 step 4): `crates/luneterm/src/links/`. `scan.rs` ports Lunegit's link scanner (OSC 8
  runs by the engine's link id plus bare `http(s)://` text) and runs on demand over the row under
  the pointer; `open.rs` is the only code that opens anything: a closed URL grammar (`check_url`)
  applied at open, then one argv element to `/usr/bin/open`, `xdg-open` or `ShellExecuteW`
  (`unsafe` lives in that Windows module). Hovering a link `check_url` accepts underlines it and,
  after the toolkit's tooltip delay (`Ui::tooltip` over a hover-only region on the link's cells),
  shows `tip_url` (the checked URI, shortened for display only: the host is never cut at its start)
  and the hint "Cmd+click to open" (macOS) or "Shift+click to open"; a refused link has neither.
  The pointer is a hand only while the modifier is held. A Cmd-click (macOS) or Shift-click opens
  (Shift also on macOS), a drag selects. The row under the pointer is read on pointer moves and
  once after output that hid a shown hover, never on output alone.
- Own the clipboard setting: `StandardExtensions::clipboard_write` and `set_clipboard_write`
  (default `false`). `ClipboardWrite` arrives only while it is on; the host decides what to do
  with the text (no query is ever answered). **Done:** `Settings.osc_clipboard` (default on, as
  Lunegit) feeds the engine gate at start and `Command::SetClipboardWrite`, and the app writes the
  clipboard only while the setting is on (D8; e2e E4).
- Pass `StandardExtensions::xtversion` (the terminal's name; `lune-vt` has none) and choose `vs16`.
  **Done:** `Luneterm(<version>)` (`spawn::xtversion`), `vs16: true` (`session/engine.rs`
  `build_terminal`).
- Route `CwdChanged` (new-tab directory), `Progress` and `CommandMark` / `SyncUpdateInCommand` to
  the UI; validate a cwd before using it (the engine only guarantees no control characters, at most
  4096 bytes and a non-empty text). **Done (M10):** the session puts all four in the `Update`
  (`session/update.rs`, marks capped at 64 per Update). A reported directory is checked by
  `cwd::syntax` (no filesystem call; a path that begins with two separators is refused on every
  OS, because a `stat` of `\\host\share` on Windows would start an SMB handshake with a host the
  session chose) and `cwd::validated` (stats, so only off the UI thread) before a tab starts in
  it; it feeds the new-tab directory, `workspace.json` and "Copy Working Directory". Progress
  and the marks feed the running-outline cue (`outline.rs`, "Tabs" below).
- Call `reset_keyboard_protocol()` when the foreground process changes. **Done (M10 step 1,
  ledger 11):** the engine thread probes the PTY (`session/engine.rs`, `ForegroundWatch`) and
  calls it on the transition from "something else is foreground" to "the shell is" (on Windows,
  where the foreground is unknown, on the shell's next prompt mark); flags set while a program
  runs are never touched.
- Not done in `lune-vt`: win32-input-mode, kitty flags 2, 8 and 16, mouse wheel to arrow keys in the
  alternate screen (the browser layer's job in xterm.js; **done by the app**, D7: one arrow per
  wheel event by DECCKM, `Command::WheelArrows`), OSC 133 options beyond the mark letter and `D`'s
  exit code.

**Errors and limits.** Upstream `throw`s are `Result::Err` (`IdentifierError`, `ParamsError`).
The parser never panics on any input: clippy denies indexing, `unwrap`, `expect` and `panic` in
its non-test code, and `crates/lune-vt/tests/no_panic.rs` feeds it seeded random bytes (debug
build, so integer overflow panics too). Param counts (32), sub-params (32) and OSC/DCS payloads
(`PAYLOAD_LIMIT`, 10,000,000 UTF-16 units) are bounded; `tests/payload_limits.rs` pins the edges.

**`lune-vt-conformance`.** An unpublished crate that holds what pins `lune-vt` to xterm.js: the
corpus (hand-written inputs), the goldens (line format, generated by `tools/xterm-oracle` through
`scripts/regen-vt-goldens.sh`, committed so Rust tests need no Node), Lunegit's 27 tmux vectors,
the golden reader and the parser-trace runner. `lune-vt` itself keeps no `[dependencies]` or
`[dev-dependencies]`; its fuzz test includes the runner's `trace.rs` and `escape.rs` by path.
Format and runners: `crates/lune-vt-conformance/FORMAT.md`. Since M6 the screen goldens (hand and
random cases) and Lunegit's vectors run through `lune_vt::Terminal` (`src/screen.rs`,
`tests/screen_goldens.rs`, `tests/lunegit_vectors.rs`, `tests/handler_coverage.rs`); the engine
fuzz includes `screen.rs` by path.

## lune-vt buffer

`crates/lune-vt/src/buffer/`, `circular_list.rs`, `color.rs`, `input/unicode_v6.rs`,
`addons/unicode_v11.rs` and `services/{buffer,charset,options,osc_link,unicode}_service.rs` are
ports of the xterm.js 6.0.0 files of the same names (status of every file:
`docs/porting-xterm.md`, which also lists the conventions this section summarises).

**Storage.** A `BufferLine` is `Vec<u32>` of `cols x CELL_SIZE (3)` words (content, fg, bg, with
the `buffer/Constants.ts` bit layout) plus two ordered side maps, `combined` (cell index to a
multi-code-point string) and `extended_attrs` (cell index to `ExtendedAttrs`, a `Copy` value).
JS typed-array behaviour is reproduced where code relies on it: an out-of-range read is 0 and a
write is a no-op, while the side maps are as unchecked as JS object writes. A `Buffer` holds
`lines: CircularList<BufferLine, MarkerSet>`, the cursor (`x`, `y`), `ybase`/`ydisp`, the scroll
region, `tabs: BTreeSet<usize>` and the saved-cursor fields. `BufferSet` owns the normal buffer
(with scrollback) and the alt buffer (none); `BufferService` owns `cols`, `rows` and the
`BufferSet`. `CircularList` stores `VecDeque<Option<T>>` and grows on demand: a terminal with
`scrollback = 4_294_967_295` allocates nothing in proportion to it.

**Observers and markers.** `CircularList<T, O: ListObserver>` calls `on_trim`, `on_insert` and
`on_delete` synchronously where xterm.js fires `onTrim`, `onInsert` and `onDelete`. `MarkerSet`
is the observer of `Buffer.lines`: it moves, shifts and disposes markers (`MarkerId`, ids from one
process-wide counter) exactly as the three listeners of `Buffer.addMarker` do. A disposed marker
is appended to a log that the host drains (`Buffer::take_disposed_markers()`); nothing in
`lune-vt` holds a back-reference or a callback.

**Events are return values.** `BufferSet::{reset, activate_normal_buffer, activate_alt_buffer}`
return the `onBufferActivate` payload exactly when upstream fires it; `BufferService::resize`,
`scroll` and `scroll_lines` return the `onResize` / `onScroll` payload; `OptionsService::set`
returns `Some(key)` iff the value changed; `UnicodeService::set_active_version` returns the
`onChange` payload.

**Options by reference.** `TerminalOptions` is a typed struct of the 21 options the core reads
(renderer and browser options belong to `lune-termview` and the app settings). Methods that read
`rawOptions` upstream take `opts: &TerminalOptions`, so a read is as live as upstream's.

**Unicode.** `UnicodeService` holds `UnicodeVersionProvider`s (`Send + Sync`); V6 is registered at
construction, V11 (`addons/unicode_v11.rs`) by `register` plus `set_active_version("11")`.

**Errors and limits.** Upstream `throw`s are `Result::Err` (`ListError`, `OptionError`,
`UnicodeError`, `RangeError`). The module gate is the parser's plus `clippy::string_slice`, and
arithmetic that goes negative in JS runs in `i64`. `crates/lune-vt/tests/buffer_ops.rs` drives the
buffers with seeded random operations (cursor, resize both ways, scroll, splice, trim, cell edits,
markers, links, alt buffer, memory cleanup) and asserts the size, side-table and disposal-log
bounds after every operation.

**Host duties of the buffer slice (M5; since M6 `CoreTerminal` and `InputHandler` do all of them,
see "lune-vt engine").** The buffer and its services have no timers, no listeners and no clamping
of their own, so anything that owns a `BufferService` directly must:
- clamp `cols >= MINIMUM_COLS (2)` and `rows >= MINIMUM_ROWS (1)` before `BufferService::resize`
  (`CoreTerminal.ts:175-176`; upstream's reflow does not terminate at one column);
- drain `take_disposed_markers()` of **both** buffers after anything that can dispose markers
  (scroll that trims, `clear_markers`, resize, activating the normal buffer) and pass the ids to
  `OscLinkService::on_markers_disposed`;
- call `Buffer::batched_memory_cleanup()` while idle until it returns `false` (the replacement of
  `IdleTaskQueue`, after a resize sets `memory_cleanup_pending()`);
- react to option changes as `BufferSet`'s upstream constructor does: a new `scrollback` calls
  `BufferSet::resize` with the current size, a new `tab_stop_width` calls `setup_tab_stops(None)`;
- forward `BufferActivate::active_ydisp` (and the value `reset` returns) as the `onScroll` event.

**Known differences from xterm.js (each documented in the file header).**
- `CircularList` has no ring: `splice` and a forward `shift_elements` give the logical result in
  the two cases where upstream's ring overwrites elements it has not moved yet; `shift_elements`
  refuses (`ListError::ShiftTooFar`) to grow the list by more than `max_length`, `set_len` and
  `set_max_length` clamp.
- Column shrink with a full scrollback: upstream overwrites the last row through a negative ring
  index; the port keeps the correct row (`added_reflow_smaller_drops_trimmed_new_rows_instead_of_overwriting_the_last_row`).
  The port does reproduce upstream's re-read of rows popped during the same shrink
  (`added_reflow_smaller_reads_popped_rows_again_as_upstream_does`).
- With `scrollback == 0`, `ybase + rows <= lines.len()` can fail after a column-shrink reflow;
  real `@xterm/headless` 6.0.0 does the same (`reflow_without_scrollback_matches_xterm_js`), so
  it is kept.

## lune-vt engine

`crates/lune-vt/src/input_handler.rs`, `core_terminal.rs`, `windows_mode.rs`,
`services/{core,core_mouse,log}_service.rs` and `headless/terminal.rs` are ports of the xterm.js
6.0.0 files of the same names (status of every file and the conventions this section summarises:
`docs/porting-xterm.md`). `engine.rs` holds the host-facing types, which are original. Lunegit's
additions are in `ext/` (parser extensions) and `input/` (encoders), described below under
"Lunegit's additions (M7)".

**Composition.** `Terminal` (`headless/terminal.rs`) wraps one `CoreTerminal`. `CoreTerminal`
holds `parser: EscapeSequenceParser<InputHandler>` and `input_handler: InputHandler`, side by
side so their borrows are disjoint. `InputHandler` is the parser's host (`ParserHost`) and owns
`CoreServices`: options, buffer (`BufferService`, with both buffers), log, core, mouse, unicode,
charset and OSC links, plus upstream's private state, the event queue and the damage. Handlers
are closures registered once by `InputHandler::register_handlers`; `Extension`s register on the
same parser afterwards and run first (newest first). The standard extensions keep their state in
`InputHandler.ext` (`ExtState`, a `// LUNETERM:` field), so parser closures and the encoders read
the same value with no reference back to the host.

**Write path.** `Terminal::write(&[u8])` calls `InputHandler::parse`, which decodes UTF-8
(`Utf8ToUtf32`, streaming), splits input into 131072-byte chunks and feeds the parser. It is
synchronous: when it returns, every byte has been applied. Nothing in `lune-vt` blocks, sleeps,
spawns or calls back.

**Public API (`VtEngine`, `engine.rs`).** `columns`, `rows`, `write`, `resize` (clamped to at least
2 x 1), `scroll_lines`, `scroll_to_top`, `scroll_to_bottom`, `drain_events`, `take_damage`,
`viewport` and `modes`; `Terminal` implements it and adds upstream's headless surface (`new`,
`write_str`, `input`, `reset`, `clear`, `add_marker`, `set_option`, `read_row`, `link_uri`,
`idle`, `set_logger`, `unicode_mut`, `register_*_handler`, `add_extension`, `buffers`, `core`).
The M7 additions on `Terminal` are listed under "Lunegit's additions (M7)".

- **Events** (`HostEvent`) are queued while `write` runs, in the order upstream fires them, and
  drained by the host: `Response(Vec<u8>)` (DA, DSR, DECRQM, DECRQSS replies and `input`; two
  responses are never merged), `Bell`, `TitleChanged`, `Color(Vec<ColorRequest>)` (OSC 4, 10-12,
  104, 110-112; the engine never answers a colour report, the host knows the theme),
  `FocusReportRequested`, `WindowReport(WindowReportKind)`, `A11yText` and `A11yTab` (only with
  `screen_reader_mode`), and `UnknownOsc` and `ClipboardWrite`, which the core never produces
  (M7; the core never produces them, the standard extensions do), `CwdChanged`, `Progress`,
  `CommandMark` and `SyncUpdateInCommand` (M7, standard extensions only), and `Notification`
  (the notification extension, behind `StandardExtensions::notifications`; the C ABI never
  produces it, below). Without
  `install_standard_extensions`, OSC 52, queries included, reaches only the fallback that logs
  (ledger row 13).
- **Bounded queues** (user decisions Q10 and Q11). `TitleChanged` has no payload and at most one
  is queued per drain (read the title from `Viewport::title`, an `Arc<str>` shared with the
  engine, so a snapshot never copies it); `A11yText` merges consecutive characters and stops at
  `A11Y_TEXT_LIMIT` (2048 bytes) per drain. Everything else is exact. With these, the events
  of one drain weigh at most 16 units per byte written plus 4 KiB (checked on every fuzz seed,
  `engine_fuzz.rs` `event_cost`: a payload byte weighs 1, a payload-free event 1); without
  them a few bytes of XTWINOPS 22/23 or REP queued gigabytes. The M7 events fit the same bound:
  `ClipboardWrite`, `CwdChanged` and `Progress` are last-wins per drain, replaced in place (O(1));
  `CommandMark` is exact; `UnknownOsc` is capped at 64 per drain (`UNKNOWN_OSC_LIMIT`) and `Notification` at 8
  (`NOTIFICATION_LIMIT`); each
  payload is at most the bytes that produced it, except `ClipboardWrite`: invalid UTF-8 in the
  decoded data becomes U+FFFD (3 bytes per bad byte), so its text can reach about 2.25x the
  base64 that produced it (`////` gives 9 bytes); the drain bound still holds (`event_cost`
  weighs the real length). The weight is not the memory: a
  queued `HostEvent` is 48 bytes, so 1 MiB of BEL holds about 48 MiB until drained (measured,
  48.5x). M9 sizes its drain cadence for about 50x the bytes of one write.
- **Work per input byte** (fix rounds 1 and 2 of the M6 review; the S7 residuals in M7). Where
  upstream's cost per byte grows with a count the input controls, the engine caps or batches it,
  each in a `// LUNETERM:` block: `OscLinkService` checks the link's last marker before looking
  further (printing inside a link open across the buffer is not O(scrollback) per character), keeps
  at most `MAX_LINKS` (4096) links and `MAX_LINK_MARKERS` (8192) link markers, evicting the least
  recently extended link (its cells keep their attributes and lose the link), disposes an evicted
  link's markers in one walk per buffer and filters a link's marker list once per disposal batch
  (ED 3 and RIS after a buffer-long link are linear, not quadratic); IL, DL, SU and SD move the
  markers once per sequence (`splice_rotate`), not once per shifted line; a cell's combined string
  holds at most `MAX_COMBINED_CODE_POINTS` (64, `buffer_line.rs`); REP prints only the repetitions
  that still change the state (`rep_equivalent_repeats`, tested equal to the full print), and a unit
  that prints alike (a character with its joiners, also in insert mode, inside an open link or with
  screen reader mode) fills whole lines (`print_repeated`, review S7): one insert pass per line in
  insert mode, on lines longer than `cols` too (`BufferLine::insert_cells_repeatedly`, with
  `print`'s fix-up at `cols - 1`), and one data pass plus one extended-attribute run per line
  (`BufferLine::set_cells_repeatedly`; a line's extended attributes and combined strings are
  `RunMap`s of runs, so an underline colour, a link or a cluster costs one entry per run, not a
  tree insert per cell). No log line carries pty output or user input: unknown OSC and DCS log
  their identifier and payload length, the parse and the data/binary events log lengths only.

  **M7 closed three S7 residuals** (security-review finding `rep-slow-path-amplification`, user
  decisions Q18 and Q21), each behaviour-identical to the code it replaced (a differential against
  a test-only copy of it, `docs/porting-xterm.md`, "S7 residuals closed in M7"):
  1. `MarkerSet` has a trim offset and a line index, so a line change no longer walks every live
     marker: `on_trim` visits only the markers it disposes, an insert, delete or rotation only the
     markers on the lines it moves, and link markers are a `VecDeque`. A line change is therefore
     not O(`MAX_LINK_MARKERS` + rows + scrollback) any more (tests count visits: a trim of one line
     over 10,000 markers visits 1; a REP of `a CSI 1048576 b` in an open link at 80x24 with scrollback 1,000
     visits 3,154 markers, against 2,153,500 on the pre-change tree, same test).
  2. A cluster's combined string is a shared `Arc<str>` in a run: a narrow-cluster REP is one
     run and one allocation per line.
  3. A no-wrap insert REP on a line longer than `cols` batches the stuck inserts: at most
     `cols + 4` insert passes however long the line (test), where it was one O(len) pass per
     repetition.

  **Measured after** (`scripts/bench-rep.sh` = `crates/lune-vt/examples/rep_cost.rs`; release,
  aarch64 macOS, rustc 1.92.0; median of 5 fresh terminals, each writing 3 x (unit +
  `CSI 1048576 b`), Unicode 11, ns per input byte; run at the M7 tip 4248f2f, load average 4.30
  before and 4.44 after on an interactive Mac, so compare ratios inside one table, not absolute
  values across tables). Before: the same script at f081990 (load average 3.01 before, 4.31 after).

  | per input byte | 80x24 sb1k | 80x24 sb10k | 300x100 sb1k | 300x100 sb10k |
  |---|---|---|---|---|
  | plain `a`, before / after | 8.7 / 9.5 us | 88.2 / 96.0 us | 25.5 / 27.8 us | 83.2 / 88.9 us |
  | link `a`, before / after | 131.4 / 74.9 us | 10.38 ms / 792.6 us | 182.6 / 106.0 us | 895.3 / 255.9 us |
  | narrow cluster, before / after | 220.3 / 11.3 us | 1.29 ms / 73.9 us | 885.6 / 27.0 us | 1.09 ms / 39.9 us |
  | wide cluster, before / after | 102.1 / 81.0 us | 987.7 / 817.1 us | 376.2 / 283.9 us | 922.7 / 628.1 us |
  | DECALN (control), before / after | 1.6 / 1.6 us | 1.6 / 1.7 us | 27.8 / 27.5 us | 27.8 / 27.3 us |

  | no-wrap insert REP, `e` + U+0301, line length L -> window | before | after |
  |---|---|---|
  | L = 300 -> 80x24 | 273.7 us | 3.8 us |
  | L = 1000 -> 80x24 | 4.67 ms | 12.9 us |
  | L = 2000 -> 80x24 | 21.96 ms | 27.4 us |
  | L = 2000 -> 300x100 | 19.47 ms | 73.9 us |
  | control, L = 80 -> 80x24 | 1.3 us | 690 ns |

  Link REP against plain REP in the same table: 80x24 sb10k 118x before, 8.3x after; 300x100
  sb10k 10.8x before, 2.9x after. Narrow cluster: 14.6x and 13.1x before, 0.77x and 0.45x after
  (cheaper than plain: one allocation per line). Insert REP at L = 2000 -> 80x24 against the
  same-run control: about 40x the control after (27.4 us / 690 ns), linear in L as designed (target:
  at most 2 x control x L / 80 = 34.5 us here, met).

  **What is left.**
  - Link REP missed its step target (at most 4x plain) at 80x24: 8.3x with scrollback 10,000 (the
    ratio is the same at scrollback 1,000, 7.9x: a fixed cost per scrolled line, about 0.6 us by
    inference from the table, not profiled, and no longer growing with scrollback). The user
    accepted this (Q23). At 300x100 the target is met (2.9x).
  - Wide-cluster REP was 8.5x plain at 80x24 sb10k and 7.1x at 300x100 sb10k; M7 fix round 1
    shares a fresh line's combined map copy-on-write (`SharedRunMap`), and a joiner that widens a
    narrow base (VS16 under `11-vs16`, review S1, which skipped both the cut and the line fill)
    now takes the cut, the line fill (insert mode: `BufferLine::insert_cell_strided`) and the
    batched stuck inserts. Same script at the fix-round tip, load average 7.26 (the "after"
    reading printed the same value): wide cluster 0.98x / 1.09x plain at 80x24 / 300x100 sb10k,
    VS16 1.04x / 1.16x, keycap 0.85x / 0.73x; inside an open link wide cluster 0.68x / 0.81x and
    VS16 0.72x / 0.85x the narrow link REP; no-wrap insert VS16 at L = 2000 -> 80x24 16.0 us
    (0.61x `e U+0301`, linear in L). Insert mode with wraparound: VS16 0.75x to 0.79x insert `a`;
    with an underline colour 2.1x to 3.6x insert `a` (300x100 sb1k the worst: the per-key pass
    of the strided insert with extended entries).
  - One REP is still one unit of work of up to 2^20 code points (or `cols x (rows + scrollback + 3)`
    cells): it cannot be sliced, but its cost is now linear in the lines it prints, not quadratic.
    **M7 fix round 2** tightened two cases of the cut floor (review S2, S3): below a scroll
    region a wrap neither scrolls nor trims, so the floor no longer counts the scrollback
    (`cols x (rows + 3)`, plus the long-row passes in insert mode) — before, a full scrollback
    made every one of `max_lines` units a pass over the last row (2.5-50 ms per byte); and a REP
    whose unit's first code point joins without moving the cursor (a lone combining mark the
    cursor sits on) is cut to `MAX_COMBINED_CODE_POINTS + 2`, the point its one growing cell is
    full, instead of running 2^20 code points through `print` (was ~0.9 ms per byte).
  - The pre-M7 `a CR LF` inside a long link (0.55 us per byte at scrollback 1,000 and 5.4 us at
    10,000 against 30 ns with no link; one IL with 8192 markers outside the region about 7 us at
    80x24 and 39 us at 300x100) was not re-measured after step 11: the probe covers REP only.
    The visit-count tests show the walk is gone for trims, inserts, deletes and rotations; the
    figures are the old ones. **M9's flow control still bounds the work per slice (time or line
    changes), not only the bytes**, until that is measured.
  - In insert mode on lines longer than `cols` (after a column shrink without reflow), a REP
    without wraparound still prints up to `len - cols` more repetitions to reach the same state
    (code review F9, state identity); each is no longer an O(len) insert.
  - **M7 fix round 2** (review S2, S3; figures `bench-rep.sh`, load 11.9, ratios within a table):
    after the scrollback was dropped from the below-region floor, plain below-region REP is
    540 ns / 5.9 us (80x24 / 300x100 sb10k), VS16 short-line 6.5 us (was 2.50 ms, ~385x), the
    no-reflow long-row `e U+0301` L=2000 is 363.8 us (was 48.4 ms, ~133x), and the joining-base
    REP is ~400 ns at every size (was ~0.9 ms, ~2000x). Two sub-targets were left open there and
    are met by fix round 3 (next item).
  - **M7 fix round 3** (review S2's two open targets; figures `bench-rep.sh`, load 9.7, ratios
    within a table). Below a scroll region in insert mode every wrap lands on the last row, so
    `print_repeated` compares the state a whole pass reads (the row's first `cols` cells, or the
    whole line and its entries past it when the hidden tail is no longer than what a pass pushes
    into it) before and after each pass, and once a pass leaves it as it found it takes the rest
    of the whole passes at once: nothing to do on a short row, one
    `BufferLine::insert_block_repeatedly` (O(len), proven against its loop) on a long row's tail.
    The line fill's insert passes lost their per-cell side-table work where the writes hide the
    inserts' entries (`insert_cells_repeatedly_then_set`, `insert_cell_strided_then_set`: one
    shift, the fix-ups, the writes), `insert_cells`' shift is one pass (`shift_cells_right`), and
    the `RunMap`s read in one walk (indexed dense reads, cut-run walks, a cursor, range splicing).
    Below-region REP at 80x24 / 300x100 sb10k: plain 101 / 234 ns, `e U+0301` 144 / 295 ns (1.4x /
    1.3x), wide cluster 2.2x / 2.3x, VS16 2.4x / 2.4x, keycap 2.4x / 2.3x, `a` in a link or with an
    underline colour 1.5x (was 9.8-14.2x plain, and 2.5-3.1 ms per byte before round 2). The
    no-reflow long row (after a narrowing: the alt buffer on every platform, the normal buffer
    on Windows 10 or in `windows_mode`; wraparound on) is linear in L: `e U+0301` at
    L = 300 / 1000 / 2000 is 1.9 / 6.0 / 12.5 us (was 363.8 us at 2000, and 48.4 ms before round
    2), VS16 9.2 us, `a` in a link 13.0 us, with an underline colour 12.3 us (the review's 51.6 /
    44.4 ms), against 10.1 us for the step-13 no-wrap insert of `e U+0301` from 2000 in the same
    run (itself 26.7 us before this round). Left: a widened unit (VS16, keycap) on an odd number
    of columns is wrapped by its joiner, whose wrap copies the old row's strings from `cols - 1`
    on, the tail's included (upstream's copy), so on a long row its passes settle only with the
    tail: 53.6 us per byte at L = 2000 narrowed to 79 columns (12.5x plain there; short rows 3.1x
    / 3.4x).
  - **M7 fix round 4** (code review F6, F7): the cut floor of rounds 1-3 was **not**
    state-identical for that widened unit on an odd number of columns over rows longer than
    `cols` that hold combined strings. Each joiner wrap runs upstream's `copyCellsFrom(oldRow,
    cols - 1, 0, 1)`, whose FIXME copy moves every string at a key `>= cols - 1` (stale entries
    and keys past the line's end included) `cols - 1` to the left, so a row's strings settle
    only after about `last key / (cols - 1)` wraps; below a region the cut REP showed other text
    (insert off), at the scroll bottom the oldest lines' hidden entries differed. The floor now
    adds `ceil((K + 1) / (cols - 1)) + 1` lines, `K` the largest combined key on the rows the
    print reaches (each row's last cell too in insert mode), read in O(log n) per row
    (`BufferLine::last_combined_key`); state identity is tested at 7, 9, 21 and 79 columns, below
    the region and at the scroll bottom, insert on and off (dedicated test, the review's seeded
    probe, the matrix, the 300k-seed differential). Cost of this shape (`bench-rep.sh` table
    "REP at odd widths over narrowed rows of strings", Windows mode, sb10k; before = the same
    table built on fix round 3, same run, load ~3.5): below the region without insert VS16 at
    L = 2000 is 0.55 ms per byte at 79 columns (was 0.35 ms, cut too early) and 1.38 ms at 21
    (was 0.71 ms); in insert mode 0.95 / 4.0 ms, unchanged (round 3's insert passes already
    walked the tail); at the scroll bottom ~1.5 / ~0.95 ms, unchanged. Plain `a` in the same
    setup is 0.2-0.3 us (no insert) and 29 us (insert). So the shape is O(L^2 / cols): one O(len)
    copy per wrap until the strings settle, now counted, not cut. It needs a row longer than
    `cols` at an odd width and `11-vs16` (reach corrected in fix round 7, below); an O(len)
    closed form of the copy is not built.
  - **M7 fix round 5** (plan revision 1, step fix5; the F6 residual): the floor is unchanged;
    how its units print changed. (A) Below the region without insert mode, the last row's whole
    passes differ only in the strings the joiner's wrap copies, and `p` passes leave the state
    of the first `p - 1` copies taken in closed form (`BufferLine::shift_combined_classes_repeatedly`,
    `F_k(M)[j]` = the value of the largest held key of `j`'s class in `j..=j + k (cols - 1)`,
    O(K + runs); lemma in `InputHandler::copy_passes_at_once`) and one real pass. (B1) A joiner
    wrap no longer clones the old row (`CellsSource`: the copied cells and the shared strings),
    a copy of a row onto itself is one closed-form self-copy, and the wrapping repetition
    (`print_widened_wrap`, `print`'s steps) shares the head string between the lines that hold no
    other, which the fill's template (now also keyed on a one-run "before" map) takes in O(1).
    (B2) At the scroll bottom the first `ceil(K / (cols - 1)) - 1` scrolling joiner wraps skip
    their strings' copy when at least `max_length` scrolls follow (the lines they land on are
    then discarded; proof in `InputHandler::chain_copies_to_skip`). State identity: the closed
    form against the loop of copies, the direct wrap against `print_inner`, the skip bound at
    `max_length - 1 / 0 / +1`, the matrix, the 300k-seed differential; 12 mutants of the bounds
    (11 in fix round 5, the chain skip's `plain` term in fix round 7) each fail a test (notes.md
    "fix round 5", "fix round 7"). Cost (`bench-rep.sh` table "REP at odd widths over
    narrowed rows of strings", Windows mode, sb10k, VS16, median of 5; before = fix round 4's
    tree with the same example, load ~4.5; after load ~3): below the region without insert at
    L = 2000, 543 us -> 12.1 us per byte at 79 columns and 1.38 ms -> 9.0 us at 21; linear in L
    (1000 -> 2000 at 79 columns: 6.8 -> 12.1 us); but still 6.7x / 12.5x the same unit at the
    even widths 80 / 22 (1.8 us / 721 ns) and 40x / 51x plain `a` — every REP rebuilds the row's
    O(K) string map in the batch and in each real joiner wrap's self-copy, because the fill's
    heads keep the row from being a fixed point of the copy (85% of the time in a profile).
    At the scroll bottom 1.41 ms -> 135 us (79 -> 79), 1.55 ms -> 157 us (2000 -> 79), 462 ->
    83 us (21 -> 21), 955 -> 100 us (2000 -> 21): 1.1-2.1x plain `a` with and without insert
    mode. Insert mode below the region is still O(L^2 / cols) (the copy interleaves with the
    insert shifts; plan revision 1 R1-D3), now 941 us -> 398 us at 2000 -> 79 and 3.98 -> 1.77 ms
    at 2000 -> 21 (the row clone gone; reach and settlement: fix round 7). Where the chain's
    lines survive the REP (too few scrolls after them to discard them), the exact state holds
    `Θ(K^2 / cols)` entries over those lines (K up to a narrowed row's length, `cols` down to
    2): a memory amplification of upstream's copy that no exact algorithm on this representation avoids (R1-D4, for the security review).
  - **M7 fix round 7** (plan revision 2, step fix6; security S4): the reach of this whole family
    (a widened unit with `11-vs16`, a VS16 after a one-cell base with or without joiners before
    or after it: VS16 emoji, keycaps, `a U+0301 U+FE0F`; whose widening joiner wraps its base at
    an odd width over a row longer than `cols`) was understated as "a narrowed row of strings (Windows 10
    without reflow)". Rows longer than `cols` exist after the user narrows the window **in the
    alt buffer on every platform**: it has no scrollback, so it never reflows
    (`Buffer::is_reflow_enabled` requires `has_scrollback`), and in the normal buffer without
    reflow (Windows 10 ConPTY < 21376, `windows_mode`). Rows of plain text reach it as well as
    rows of strings. In insert mode below the region the cost is O(L^2 / cols) per REP that has
    to run the transient: about `(len - cols) / (cols / 2)` passes until the row's old tail has
    left it, then `K / (cols - 1)` more for the copied strings, each O(len). It **settles**: a
    bare repeated REP is free after the first (planner probe P4, scratch crate `rev2-probe`,
    release: 49 ms then 0.00 ms per REP at 9 columns, L = 1000; 20 ms then 0.00 ms at 21). The
    sustained cost needs a standalone unit print between REPs (itself O(L)), which disturbs the
    fixed point and makes about half of such REPs pay again (phase-dependent). Security review
    round 7, alt buffer, defaults + `11-vs16`, first REP / mean per input byte over REPs 11-210:
    L = 1000 at 21 columns 19 ms / 38 us, at 9 columns 49 ms / 209 us; L = 2000 at 21 columns
    80 ms / 145 us, at 9 columns 226 ms / 861 us, at 5 columns 386 ms / 2.91 ms. The surviving
    scroll-bottom chain (R1-D4) can form in the alt buffer too (read from the code, not
    measured); there `max_length = rows`, so its ceiling is `rows x K` entries. Part B (plan
    revision 2 R2-D1 = build, behind a prototype gate): no linear or bounded-settle closed form
    exists (planner probes P1, P5), so the passes are replayed in dense arrays instead
    (`BufferLine::replay_wrap_passes`, called by `InputHandler::replay_wrap_passes` on the last
    row below the region in insert mode, for a row longer than `cols`; in fix round 7 only for
    a unit whose first joiner widens a one-cell base: VS16, a keycap; fix round 8 below). It
    makes each pass's writes, in order, on the slots and on per-key indexes into the strings
    and attributes (the strings over every key the copy reads, past the line's end too), and
    rebuilds the maps once; a pass reads only the row, so the row is compared at checkpoints,
    one and two passes after each, and the passes left are cut to their remainder once it
    repeats (with an odd `h` the pushed cells alternate: period 2, never a fixed point, at 79
    columns). Fix round 7's checkpoints were pass 0, `T = ceil((len - cols) / h) + ceil(K' / c)
    + 2` and its doublings; fix round 8's are passes 0, 1, 2, 4, 8, …. State identity: the replay against the real per-pass calls
    (`replaying_wrap_passes_equals_the_real_per_pass_loop`, 6,000 rounds), the uncapped print
    over plain rows, stale tails, keys past the end and the narrowed alt buffer, the matrix
    and the seeded differential; eight mutants (pass count, truncated transient, keys past the
    end, the stale entry at the base, the copy width, off the last row, without insert mode,
    unmerged maps) each fail a test. Still `O((L + K)^2 / cols)` per REP that runs the
    transient, a constant-factor mitigation: `bench-rep.sh` table "REP in insert mode below a
    region at odd widths" (Windows mode, sb10k, median of 5, before = fix round 6's tree with
    the same example, same session, load ~6.8), VS16 per input byte, 3-REP stream, L -> cols:
    2000 -> 79 532 -> 37 us, 1000 -> 21 446 -> 29 us, 2000 -> 21 1.79 ms -> 85 us, 1000 -> 9
    944 -> 36 us, 2000 -> 9 3.99 ms -> 130 us, 1000 -> 5 2.28 ms -> 117 us, 2000 -> 5 9.58 ms
    -> 457 us (keycap alike); first REP 1.21 -> 0.072 ms (79) up to 23.1 -> 0.73 ms (5);
    sustained (unit + REP pairs 11-60) 9.8 -> 6.4 us (79) up to 2.86 ms -> 322 us (5), all
    below the Q23-accepted link REP (845 us) for VS16 and the keycap. Plain `a` in the same
    setup: 29-30 us. Rows of `cols` cells keep the round-3 batch (no tail to push out; the
    replay's map rebuild cost more there). Not covered in fix round 7: units with a joiner
    before the widening one (`a U+0301 U+FE0F`, `U+2764 U+0301 U+FE0F`, `a U+0301 U+0302
    U+FE0F`) kept the per-pass loop at the pre-fix cost (security review round 8, alt buffer,
    first pair / sustained per byte: 2000 -> 5 394 ms / 3.05 ms, 2000 -> 9 203 ms / 884 us).
  - **M7 fix round 8** (security S4 round 8, code review F9): the replay covers units with
    joiners before the widening one. Those joiners keep width 1 and are added to the base's
    cell at `c = cols - 1` before the widening one wraps it: the cell becomes a combined cell
    whose string (`WrapUnit::pre`, built by the same calls on a scratch line) the copy carries
    to key 0, where the remaining joiners append to it (the head's string), and the clear
    leaves stale at key `c`; `DenseRow::pass` writes `pre` at `c` after the base, before the
    copy. The repeat checks moved to passes 1, 2, 4, 8, … (a row repeating from pass `τ` stops
    by `2τ + 2`; round 7's first checkpoint `T` ran a row that repeated early to `T`: security
    round 8, `\r VS16 CSI 9999 b` at 2000 -> 3, 2.2 -> 658 us per byte). No unit shape of the
    class declines the replay (its guards hold by construction for every unit `rep_alike`
    accepts with a widening joiner); rows whose string keys reach past `len + 2 x runs + 4096`
    still take the per-pass loop. State identity: the primitive differential with six such
    units (one past `MAX_COMBINED_CODE_POINTS`), the uncapped print over plain rows, stale
    tails, keys past the end and the narrowed alt buffer, the batch test and the seeded
    differential; the joiner string written one column off fails all five. `bench-rep.sh` table
    "REP in insert mode below a region on the narrowed alt screen" (the security review's
    shape: alt buffer, `unit REP` pairs, first pair / mean per input byte over pairs 11-210,
    one terminal; before = fix round 7's tree with the same example, same session, macOS arm64,
    load 5.3-6.6), after: every unit (VS16, keycap, `a U+0301 U+FE0F`, `U+2764`/`U+26A0 U+0301
    U+FE0F`, `a U+0301 U+0302 U+FE0F`) 5.3-9.0 us at 1000 -> 21, 14-18 us at 1000 -> 9, 53-67
    us at 2000 -> 9, 196-224 us at 2000 -> 5 (`a U+0301 U+FE0F` was 36 us, 195 us, 794 us,
    2.90 ms; its first pair 375 -> 11.7 ms at 2000 -> 5); VS16 at 2000 -> 5 325 -> 223 us from
    the schedule; `\r VS16 CSI 9999 b` at 2000 -> 3 668 -> 1.6 us. The Q23 link REP in the same
    run: 791 us, so all are below it, the worst (224 us) 3.5x under. Still `O((L + K)^2 / cols)`
    per REP that runs the transient.
- **Damage** (`Damage`) is coalesced: the union of refresh rows, `full` (resize, buffer switch,
  reset), `cursor_moved`, `scrolled`.
- **`Viewport`** is a snapshot of the visible rows (`ViewportCell`: text, width, colours as
  `CellColor`, flags, underline style and colour, link id), the cursor, `viewport_top`, `base_y`,
  `total_lines`, the title, `alt_screen` and the live link table (id to URI). It allocates per
  call; `Terminal::read_row` fills one row into a caller-owned `Vec` for incremental renderers.
  `Modes` has the public mode flags, `cursor_hidden`, the mouse tracking protocol and encoding.

**Host duties done by the engine** (upstream wires these with event listeners; here each is an
explicit call at the point the event would fire, `core_terminal.rs` header lists them): scroll
position after every scroll and buffer switch; resize clamped to 2 x 1 before the buffers see it;
marker disposal forwarded to the OSC link table at the end of every public mutation and before
each link call, so no link outlives its last marker; `scrollback`, `tab_stop_width`, `log_level`,
`windows_mode` and `windows_pty` option changes applied (`set_option`); `reset` (RIS, DECCOLM,
`Terminal::reset`) resets every service; user input scrolls to the bottom; the Windows wrapping
heuristics run while they are enabled.

**What the host (M9's session) still owns.** All done in `crates/luneterm/src/session/`, M9 steps 3 to 8:
- Drain after every write (`drain_events`, `take_damage`): the queues hold one write's output.
  **Done:** `engine.rs` `run_slice` drains after every sub-write (the 1 MiB BEL case, about 48 MiB of
  queued events, is why: events per drain stay at most 64 KiB + 16, tested in T3).
- Write every `Response` to the PTY (`lune-pty`'s `PtyWriter`), and answer `Color` reports,
  `FocusReportRequested` and `WindowReport` from its theme, focus state and cell metrics. **Done:**
  responses queue to the writer thread (`threads.rs`, capped at 64 KiB pending); colour reports, sets
  and restores follow xterm.js against a per-session palette, focus per `send_focus`, window
  reports in xterm.js's format (`host.rs`, D9; window operations stay off, so those are never
  emitted in M9).
- Call `Terminal::idle()` while it returns `true` when nothing else is happening (the
  replacement of `IdleTaskQueue`: the buffer's deferred memory cleanup after a shrink). **Done:**
  `engine.rs` `tick`, after the slice.
- Flow control: feed bounded chunks from the reader thread and decide when to stop reading. The
  engine's `write` has no watermark or time slicing (`WriteBuffer` is replaced, D3). **Done:** the
  inbox and the time-sliced writes, "luneterm app".
- Choose the Unicode version (`unicode_mut()`; V11 is registered and V6 stays active at
  construction) and install a `Logger` if it wants logs (default: discarded). **Done:** Unicode 11
  with VS16 (`vs16: true`), no logger.

**Errors and limits.** Upstream `throw`s are `Result::Err` or, inside a handler, a handler that
stops with its upstream return value. The ported non-test code of `input_handler.rs`,
`core_terminal.rs`, `headless/` and the new services denies indexing, `unwrap`, `expect` and
`panic` (clippy). `crates/lune-vt/tests/engine_fuzz.rs` drives the public API with seeded random
operations (hostile parameters, payload bursts, link floods, resizes to 0x0 and 300x100, option
changes) under `catch_unwind`, checks buffer, title-stack, link-table, event and viewport bounds
after every operation, and replays each seed with 1-byte and random chunking to compare the
screen. Deliberate differences from xterm.js (each is a `// LUNETERM:` block or a header note):
no `parser.reset()` inside RIS (D7); a missing line stops the handler instead of throwing (D8);
the Windows CSI `H` heuristic handler is registered once, which changes the order against an
extension's own CSI `H` handler (D6); `reset` frees the OSC links of the replaced buffers (Q9);
IL, DL, SU, SD, CHT, CBT and REP loops are bounded.

## luneterm-ffi (C ABI)

`crates/luneterm-ffi` exports a C ABI over `lune_vt::Terminal` that mirrors Lunegit's `IVtEngine`
(M8; ABI version 1). The contract is the hand-written header `crates/luneterm-ffi/include/luneterm.h`
(types, constants, ownership and threading rules in comments); this section is the design behind it.
The Lunegit-side swap (P/Invoke, deleting the V8 host) is not part of M8.

- **Handle and guard.** `LunetermEngine*` is an opaque `Box<Mutex<Inner>>` (`Inner` = the
  `Terminal` plus the output slots). Every export, in this order: zeroes its out-parameters, validates
  its other pointers (NULL, alignment, length above `PTRDIFF_MAX`, enums and bools decoded by range
  check, never transmuted), then takes a shared reference to the handle, `try_lock`s it and runs the
  body inside `catch_unwind`; the results are written to the out-parameters only after the body
  returned `Ok`. A rejected argument never touches the engine and never poisons the handle. No
  `unsafe impl Send/Sync` exists: `Terminal: Send` (asserted at compile time in
  `headless/terminal.rs`) is what makes the handle sound to move between threads.
- **Status codes.** `int32_t`, 0 = `LUNETERM_OK`, errors negative: `ERR_NULL_POINTER` -1,
  `ERR_INVALID_ARGUMENT` -2, `ERR_INVALID_UTF8` -3, `ERR_TOO_LARGE` -4 (a viewport text pool beyond
  `uint32_t` offsets), `ERR_PANIC` -5, `ERR_POISONED` -6, `ERR_BUSY` -7.
- **Slots.** The viewport, the drained events and the encoder output (key, char, mouse, focus,
  paste) each have one engine-owned slot. A pointer into a slot stays valid and unchanged until the
  next call that fills *that* slot, or `luneterm_engine_free`; every other call (write, resize,
  scroll, the other getters) leaves it alone. Slots give back capacity beyond `2 x need + a floor`
  after a fill. An empty array or string is a NULL pointer. **Outputs are untrusted and not
  NUL-terminated**: strings are pointer + length in bytes and may contain NUL; titles, link uris,
  cell text and event payloads come from the child process.
- **Viewport.** `luneterm_engine_viewport` converts the screen row by row
  (`Terminal::viewport_with_rows`, which `Terminal::viewport` is built on) straight into the slot, so
  no second full-screen copy exists. `cursor_x` is `0..=cols + 1` (`cols` while a wrap is pending,
  `cols + 1` after a zero-width character is printed there, as @xterm/headless 6.0.0 does); it is not
  clamped, so a host indexing the cells clamps first.
- **Threading.** A handle may be used from any thread (that is how Lunegit's gated engine calls
  arrive), one call at a time. A call made while another is running on the same handle returns
  `ERR_BUSY` without waiting; the host serializes. Freeing a handle another thread may still call is
  a host bug (`free` on a busy handle is `ERR_BUSY` and frees nothing).
- **Aliased out-parameters.** `luneterm_engine_size` has two out-parameters (`cols`, `rows`); they must
  be distinct pointers, aliasing them is undefined behaviour (the header says so too).
- **Panics.** A panic is caught, the call returns `ERR_PANIC` and the handle is poisoned (its state
  may break engine invariants): every later call except `luneterm_engine_free` returns
  `ERR_POISONED`. The payload is dropped, never formatted, and the library installs no panic hook.
  The catch needs unwinding, so the shipping library is built only with
  `cargo build -p luneterm-ffi --profile ffi-release` (inherits `release`, `panic = "unwind"`;
  profiles cannot be set per package). The crate has `#[cfg(panic = "abort")] compile_error!`, so
  `cargo build --release` of it fails by design (and so does `cargo build --release --workspace`).
  The feature `panic-probe` adds an export `luneterm_test_panic`, which the C smoke test uses to
  prove `catch_unwind` works in the shipping profile.
- **Caps (D5).** `LUNETERM_MAX_COLS` 4096, `LUNETERM_MAX_ROWS` 2048, `LUNETERM_MAX_SCROLLBACK`
  100000. A size over a cap (at `luneterm_engine_new` or `_resize`) is `ERR_INVALID_ARGUMENT` and
  changes nothing; below 2 x 1 it is clamped, as the engine does.
  **Memory at the caps is GiB-scale.** A buffer holds `cols x (rows + scrollback)` cells, each at
  least 12 bytes (`CELL_SIZE = 3` u32 in `BufferLine`). At the caps that is
  4096 x (2048 + 100,000) = 417,988,608 cells: **~4.7 GiB at 12 bytes per cell, ~9.7 GiB at the
  measured 25 bytes per cell (both derived, not measured)**, and a child process fills it by
  printing ~420 MB of long lines. A host that must bound memory lower chooses smaller
  `cols`, `rows` and `scrollback` in its config; the library does not lower them (D5 keeps the caps
  as they are, user decision 2026-11-08).
  What was measured, on macOS arm64 with
  `cargo run --profile ffi-release -p luneterm-ffi --example cap_memory` (peak RSS, `getrusage`),
  is the **scrollback-0** case only: 4096 x 2048 with every cell filled is 202 MiB (25 bytes per
  cell, 15.6 for letter + combining mark), and one `luneterm_engine_viewport` call raises the peak to
  439 MiB (slot 224 MiB = 28 bytes per cell, plus the text pool). The viewport call adds only that
  slot and pool, on top of whatever the buffer already holds. The security ledger's REP chain
  construction (`rep-scroll-bottom-chain-memory`, premise "K is bounded by the display") is bounded by
  the column cap; the example's case (b) at 4096 columns peaks at 5.8 MiB. The ledger's 77 MB figure is settled by
  the M8 security review round 10 (the orchestration task's `m08-ffi/security-review.md`, round 10): that construction (normal buffer, Windows mode) is unreachable
  through ABI 1, and at the caps the REP chain adds at most 0.2 MiB over the screen fill. Time is bounded by
  the column cap too: at 4096 columns narrowed to 5, an odd-width REP over rows of strings costs up
  to ~1.13 ms per input byte (41 ms for the first pair); this is user-accepted (Q28) and recorded
  in the security ledger.
- **ABI version policy.** `luneterm_abi_version()` returns `ABI_VERSION` (1, `LUNETERM_ABI_VERSION`
  in the header); a host refuses a library whose value differs. It is bumped on any change to an
  existing export's signature or meaning, a struct layout, a constant's value, or a removal. Adding an
  export does not bump it (the host resolves symbols at load time). M8 defines ABI 1 in full; the
  stub's only export is unchanged. ABI 1 has no consumer yet, so
  `UNKNOWN_OSC` carries its payload length in `value2` (saturated to `INT32_MAX`; `offset` and `len`
  are 0) without a bump: `offset`/`len` are a data-pool range for every event.
- **Construction (D9).** `luneterm_engine_new(const LunetermConfig*, LunetermEngine**)` always
  installs the standard extensions, with `notifications` off (D3 of the bell-notifications plan):
  the C ABI has no notification event and `ABI_VERSION` stays 1, so OSC 777 is still
  `UNKNOWN_OSC` and a plain OSC 9 is silent; `flatten_events` skips a `Notification` so no event
  of kind 0 can reach the host. An all-zero config is the safe default: 2 x 1, no scrollback,
  Unicode 6, XTVERSION unanswered, clipboard writes off. The XTVERSION name is passed as bytes
  (1 to 64 bytes of 0x20..=0x7E, else `ERR_INVALID_ARGUMENT` / `ERR_INVALID_UTF8`; length 0 = not
  answered), never truncated. No logger is installed, so payloads are never logged. Window
  operations stay off and the ABI has no switch for them: `LUNETERM_EVENT_WINDOW_REPORT` exists in the
  header but ABI 1 never emits it (assumed decision Q27).
- **Checks.** `scripts/check-ffi.sh` (macOS only; needs clang, nm, python3) builds the shipping
  library and the `panic-probe` build, compares the function declarations read from clang's AST dump
  with the library's exported symbols both ways, and runs the C smoke test; it fails closed on a
  missing tool or an empty list. Layout is asserted in Rust and in the header (`_Static_assert`).
  `cargo test -p luneterm-ffi` also runs `tests/c_smoke.rs` (C smoke test with `$CC`/`cc`; Linux runs it
  with gcc) and `tests/ffi_fuzz.rs` (seeded: random and hostile writes, resizes at and over the caps,
  encoders with bad enums, NULL and misaligned pointers; `LUNETERM_FFI_FUZZ_SEEDS`, default 60;
  `LUNETERM_FFI_FUZZ_SEED=<n>` replays one). **Windows has no header or export check**: CI builds the
  ffi-release DLL and runs the Rust-side tests only.

**What the C host still owns** (the same duties as the Rust host, "Host duties M9 inherits from M7"
above): drain events after every write (`luneterm_engine_drain_events`, `luneterm_engine_take_damage`);
answer reports (DA, DSR, colour requests, `RESPONSE` payloads go back to the PTY); write every
encoder result to the PTY and send paste text only through `luneterm_engine_paste`; call
`luneterm_engine_idle` while it reports more work; own the clipboard setting
(`luneterm_engine_set_clipboard_write`) and call `luneterm_engine_reset_keyboard_protocol` when the
foreground process changes; serialize calls per handle.

## luneterm app

`crates/luneterm` is lib + bin (`#![deny(unsafe_code)]`: `unsafe` only in the platform modules, each block with a `// SAFETY:` comment: `links/open.rs`, `efficiency/windows.rs`, `ctl/endpoint/windows.rs` and `ctl/token.rs` on Windows, `os_event.rs` and `efficiency/macos.rs` on macOS, `shell_integration/mod.rs` and `ctl/endpoint/unix.rs` on Unix; the PTY calls live in `lune-pty`). The
bin (`main.rs`) calls `lune_pty::restrict_dll_search()`, then hands `luneterm ctl ...` to
`ctl::client` (see "Tab control (ctl)"), and otherwise starts the app: `update::startup()`, then
`luneterm::app::run()`. One session per tab and no
global state (M10: a `Tab` owns a session and everything that used to be per-session on `App`). Modules:

| Module | Holds |
|---|---|
| `app` | winit event loop (user event = session wake), window, frame loop, session wiring, bell and program notifications (`hear`, the inbox list's open state), exit |
| `shell` | Layout of both views (the top bar's: 34 px title band, card radius 8, margins 10, body padding 10; the side pane's: the pane at the left, the title band and the card beside it, no strip), the Windows chrome (caption buttons, drag, edge resize: the 6 px resize zone of a borderless, non-maximized window takes the press before any widget under it, so a resize never clicks one), the band double press (Windows: maximize; macOS: the System Settings action via `double_click`; Linux: drag), scrollbar thumb geometry and drag, the inbox button and its list (`draw_inbox`, `inbox_rows`), the notification toast (`notice_toast`) |
| `notices` | The notification model, pure: `Inbox` (rows, coalescing, `settle`), the D6 `route` table, `ToastSlots` (the live toasts, their arm delay), age labels |
| `double_click` | What a double click on the title band does on macOS: the `AppleActionOnDoubleClick` setting (zoom, minimize or nothing) mapped to a `WindowCommand`; elsewhere maximize |
| `render` | `Renderer`: one `Ui`, `TermView`, `FrameRenderer` and `TextCache`, shared by the window and by dump mode |
| `session` | The three-thread session below, `Update`, `apply_update`, the fake PTY and the protocol tests |
| `input` | `keys` (winit `KeyEvent` to `KeyRoute`), `shortcuts`, `pointer` (mouse, wheel) |
| `spawn` | Shell candidates, cwd, environment, `WindowsPty` |
| `shell_integration` | The scripts the user's shell sources (OSC 7, OSC 133), their checked install under the config directory, and what they add to a spawn (see "Shell integration" above) |
| `tabs` | `Tab` (one session plus its UI-side state, origin, program flag), `TabSet` (order, active, next focus after a close, group membership and every move that keeps a group's tabs together: pure list logic), the close rules (`asks_on_close`, `ClosePlan`), tab names |
| `groups` | What a tab group is: name, one of nine colours, collapsed flag; the colour a new group gets; the name rule. Pure data, no UI and no I/O |
| `pane` | The side pane: which view a window shows, its rects and controls, the tab list (rows, group cards, scrolling) and its drag reorder |
| `ctl` | `luneterm ctl`: words and limits, framing, replies, instance token, endpoint per OS, listener, client, request handler; see "Tab control (ctl)" below |
| `update` | Self-update over lunepack: `startup`/`confirm_healthy`, the `Updater` worker thread and its state machine, the channel a copy follows (`channel`), the 24 h timer and the toast rule (`auto`); see "Updates" below |
| `efficiency` | Efficiency mode in the background: the pure policy `wanted`, the `Controller` (UI thread only), and the OS backends (`windows.rs` EcoQoS + idle class, `macos.rs` utility QoS; none elsewhere); see "Efficiency mode" below |
| `links` | `scan` (OSC 8 runs and bare `http(s)://` text), `open` (the closed URL grammar and the only code that opens anything), hover state |
| `persist` | `settings.json` and `workspace.json` (tab groups included): bounded JSON reader and writer, atomic 0600 writes, versioned, `.bak` on a file it cannot read, `SaveGuard` (ledger 12) |
| `cwd` | The host's check of a directory a shell reported (`syntax`, `validated`) |
| `outline`, `menus`, `palettes`, `settings_page` | The running-outline gate and command tracker; the app and terminal menus as data; the ten palettes (saved by name); the settings page model and its one draw function |
| `title`, `settings`, `fonts`, `dump`, `demo` | Title sanitizer; the settings model (Lunegit's `TerminalSettings` defaults; every field has a `SettingId` and an `Effect`); the six bundled faces plus the detected system fonts; `LUNETERM_DUMP_FRAMES`; the dump mode's placeholder grid |

**Threads (D1).** Each session runs three: `luneterm-reader` blocks on the PTY and fills the inbox;
`luneterm-engine` owns the `Terminal` and the `Box<dyn Pty>`; `luneterm-writer` owns one `PtyWriter`
clone and does every PTY write. The UI thread never calls a `Terminal` or `Pty` method and takes
only locks held for one queue operation (inbox, mailbox, write queue), never across a `Terminal`
call, PTY I/O or the waker. Why a third thread: if the engine wrote the child's input itself, a
child blocked writing output while we block writing its input (a big paste into a program that
echoes it, typing during `cat`) would deadlock; the engine pushes to the write queue and goes on
parsing. Why the engine is off the UI thread: one `write` cannot be interrupted and one REP is one
unit of up to 2^20 code points (see "lune-vt buffer"), so a locked or inline engine would put
milliseconds on a frame. The UI sends `Command`s (key, text, paste, mouse, wheel arrows, focus,
scroll, resize, clipboard gate) and takes `Update`s.

**Flow control and bounds (D2).**

| What | Value |
|---|---|
| PTY read | 64 KiB; the reader waits while 1 MiB is queued, so backpressure reaches the child through the kernel buffer and nothing is dropped |
| Engine slice | 4 ms of engine time, or until a command is pending (the write in flight finishes first) |
| Sub-write size | starts at 16 KiB, bounds 256 B to 64 KiB; halves after a write over 2 ms, doubles after one under 0.5 ms |
| Events | drained, with damage, after every write (at most 64 KiB + 16 events per drain in the tests) |
| Update | at most one outstanding; the next is built only after the UI took the last; hidden window: at most one per 100 ms |
| Writer queues | 64 KiB pending responses (the excess is dropped and counted), 1 MiB pending typed input, 2 pastes; a paste is written in 64 KiB pieces |
| Caps | cols 2..=4096, rows 1..=2048, scrollback at most 100,000 (clamped, not rejected: the app computes the size itself) |
| Exit | the output ends, then the exit status is polled every 20 ms for at most 1 s; `Session::close` joins the engine and writer so the `Pty` drop kills the child; the app waits at most 500 ms |

**Publish protocol (ledger 6).** Engine: `ui_ready = false`, store the Update in the mailbox, then
`wake_pending.swap(true)` and call the waker only if the swap saw `false`. UI `take_update`: clear
`wake_pending`, take the Update, then set `ui_ready = true`. Because the store comes before the swap,
an Update in the mailbox always has a wake pending or a swap still to run, so none is lost; a take
that runs every frame can find an Update before its wake, so there is at most one wake per Update
(not exactly one). Swapping before the store loses a wake in one interleaving
(`session_publish_wake_is_never_lost`). The waker is an `EventLoopProxy` clone sending `UserEvent::Wake`; the loop requests a
redraw when visible and applies the Update without drawing when occluded. The result is vsync
pacing by construction: an Update is built at most once per frame the UI actually consumes, so a
flood costs one grid mirror per frame, not one per read. An `Update` carries the changed viewport
rows (`take_damage`, whole-viewport when the engine says `full`, a scroll, or lines are missing at
scrollback 0), cursor, scroll state from `Terminal::viewport_state()` (O(1), D3), title only when it
changed (`Arc<str>`), bells, the last OSC 52 write, palette changes, cwd, progress and marks.
`apply_update` writes rows into the termview `Grid`, so the renderer rebuilds only dirty rows.

**Frame scheduling (`pacing.rs`, `App::ask_frame`, `settle_flow`).** A frame is drawn on a redraw
request, and every request goes through one pure `Pacer` (no clock; app time is the `Duration` since
`App::start`). It has two lanes. A *paced* ask (PTY output, a pointer move, the wheel, a toolkit
animation `Repaint::Now`, the crossfade, the cursor blink, a deadline, a clipboard read) is served at
`max(not_before, first grid tick after the last paced frame's start)`: at most one frame per slot of a
grid at the display's rate, several pending asks keep the earliest, and the deadline is what
`settle_flow` puts into the control flow (`WaitUntil`), so a deferred ask is never lost (invariant
I1, tested on the pure flow function). An *urgent* ask draws at once and does not take the slot:
a mouse press or release, a key, IME, focus, a resize or scale change, un-occlusion, a dropped file,
a close request, and the one frame after a frame whose input held such an event (a click's tab
switch, a menu opening; the frame after that is paced again). The grid rate is the display's whole
Hz (60 when unknown, clamped to 30..=240), one below that when the present mode queues (`Fifo`),
read at window creation and on `ScaleFactorChanged` and `Moved`. The toolkit's own cadence covers
the progress shine and the other busy loops (`FrameConfig.busy_cadence = Cadence::BUSY`, 30/s; the
rate the user approved for the earlier stopgap `IDLE_ANIMATION_HZ`, now applied by luneui-rs 0.5.0
to busy loops only and removed from the app with `pace_frame`, plan render-performance D2, D6).

**Present mode.** The surface is created with `AutoNoVsync`'s meaning resolved by the app
(`pacing::resolve`: the first of `Immediate`, `Mailbox` the surface lists, else `Fifo`; luneui-rs's
`options()` reports `AutoNoVsync` unresolved), frame latency 2, on every OS (user decision at the
plan gate, 2026-11-11: no per-OS split). `LUNETERM_PRESENT=fifo|novsync` overrides it for a run
(anything else prints the variable's name and exits 2); stderr prints once
`luneterm: present <mode> (asked <ask>), frames <= <hz>/s, caps [<modes>]`. Why: under `Fifo` with
frame latency 2 the main thread blocked ~15 ms per frame in surface acquire whenever the busy tab
was visible, so a click and the new tab's first frame waited behind it. With no wait the pacer's
urgent lane is what makes a switch quick.

*Measured* (artifacts of plan render-performance: `baseline.md`, `after.md`; M1 Pro, Metal, release,
the ink demo with the busy tab, caps `[Fifo, Immediate]` so the mode is `Immediate`): per-frame acquire
wait 15.7 ms median before, 0.03 ms after (p95 0.76 / 1.69 ms in two runs); a steady visible busy
tab `acquire_ms` 921 ms/s before, 1.5 ms/s after; busy tab to an idle tab, click to the new tab's
first frame 27.4 ms median (max 33.8) before, 2.1 ms (max 20.5 and 3.9 in two runs) after; the UI
thread busy 327 ms/s median before, 74 after (visible steady: ~1000 before, 102 after); redraws per
second never above grid + 2 x (presses + autos) + 2; a hidden busy tab with the shine and titles ~40
redraws/s (unchanged: the toolkit's 30/s plus titles), without chrome 0; an idle focused window with a
blinking cursor one redraw per 600 ms. Not all thresholds were met: single frames at a tab switch still
wait up to 16 ms in acquire under `Immediate` (about 1 switch in 14 at its first frame; cause not
established), so the maximum first-frame latency is 20 ms in one run of two, and idle to idle was
5.0 and 13.1 ms at its maximum against an estimate of 5. Under `Fifo` (`LUNETERM_PRESENT=fifo`, grid
one below the display's rate) the pacer did **not** remove the switch delay on macOS (busy to idle
27.3 ms median, per-frame acquire 15.2 ms, as before the work): with a switch every 250 to 1000 ms
an urgent frame queues an extra present that a 59 Hz grid drains too slowly (hypothesis); with a
steady flood and no switching `Fifo` at 59 Hz does not wait (acquire 2.2 ms/s). So `fifo` is an
escape from tearing, not a latency setting.

*Not measured:* Windows (DX12), Linux (Vulkan, GL) and any present mode other than `Immediate` on them
(a run on Windows decides whether Windows stays on no-wait); **tearing** on any
OS (the user's eye check on macOS; Linux X11 without a compositor may tear, and the user accepted
that); ProMotion displays (the grid follows `refresh_rate_millihertz`, not exercised); a flood of
pointer moves with a busy tab (moves are paced, not skipped: the toolkit is immediate-mode and only a
frame knows whether a hover changed).

**Mode 2026 (D10, ledger 14).** While the mode is set the engine publishes nothing, until the mode
is reset or 1000 ms after it was first seen (xterm.js `RenderService.ts`). Unlike xterm.js the
session does not clear the engine's mode on timeout, because that needs bytes injected into the
engine's input, which can land inside a PTY sequence. **Deviation:** after a timeout DECRQM 2026
still reports "set" until the program resets the mode.

**Input routing (D4 to D7).** For 400 ms after a `ctl` request moved the active tab, typed, pasted
and composed input is dropped (not queued) for every tab; mouse and wheel are not (see "Tab control
(ctl)"). The terminal owns the keyboard unless the app has opened a toolkit
text input or overlay (none in M9; M10 sets the flag). Ownership is app state, not
`FrameOutput.focused`, so a click on a caption button cannot steal typing. A terminal-owned key goes
to the app's shortcut table (matched at event time with `Chord::matches`: paste, and Shift+PageUp,
PageDown, Home and End for scrollback), then to the router. It is never fed to the toolkit's
Translator, because `Ui::begin` would move focus on Tab (and dismiss an open overlay on Escape). Pointer, wheel, focus, resize, scale
and modifier changes always reach the Translator.
- **Keys (D5)** are a port of Lunegit's `TerminalKeyRouter` with three deviations: on macOS the
  window sets `OptionAsAlt::Both` with Option-as-Meta (otherwise winit feeds Option+key to the
  dead-key IME before the router sees it); on Windows and Linux Ctrl+Alt+letter whose text is a
  printable character other than the letter is AltGr text and goes as text; `NamedKey::Process`,
  dead keys, synthetic presses and releases are dropped (no kitty release reporting).
- **IME (D6).** While the terminal owns the keyboard the app sets the IME area to the cursor cell
  each frame. The preedit is drawn by the app over the cursor span (theme background rect, mono
  text, underline, at most 256 characters, above the grid); a commit goes to the PTY as typed
  text; `Disabled` clears the preedit.
- **Mouse and wheel (D7).** Reporting through `Terminal::mouse_event` when tracking is on and Shift
  is not held; moves only on a cell change (a pixel change under `?1016`). Wheel with tracking: one
  report per notch, at most 32 per event. Normal buffer without tracking: local scroll, 3 lines per
  notch, pixel deltas divided by cell height with the remainder kept. Alternate screen (or
  scrollback 0) without tracking: one arrow per event by DECCKM, as xterm.js does (Lunegit does
  nothing there, which leaves `less` and `man` unscrollable; deviation, reversible).
  The scrollbar thumb is drawn at accent alpha 0.55 whenever scrollback exists and is dragged with
  `ThumbDrag`.

**Clipboard (D8, ledger 19).** OSC 52 is gated twice: the engine gate (`osc_clipboard`, default on
as Lunegit) and the app gate (`ui.clipboard_write` runs only while the setting is on); the last
write wins, and no read or query path exists. Paste is Cmd+V on macOS, Ctrl+Shift+V elsewhere: it
pushes `ClipboardRequest::Read`, `Event::ClipboardText` arrives on a later frame, and the text goes
out as `Command::Paste`, which the engine thread runs through `Terminal::paste`: the only path from
the clipboard to the PTY. Sanitizing is not done on the UI thread (O(32 MiB) on a frame).

**Host answers (D9).** Colour reports, sets and restores follow xterm.js (`CoreBrowserTerminal.ts`)
against a per-session palette; focus is reported per `send_focus`; XTVERSION is
`Luneterm(<version>)`; Unicode 11 with VS16; `WindowsPty { Conpty, build }` on Windows (the ConPTY
build comes from `RtlGetVersion`, `lune_pty::conpty_build()`). Window reports are answered in
xterm.js's format, but window options stay at the xterm.js and Lunegit defaults (all off), so in M9
they are never emitted; tools that want pixel sizes read `TIOCGWINSZ`, which the PTY size fills.

**Tabs (M10 step 2, D1, D7, D13).** One ordered list (`TabSet`) behind two views: the top bar, whose
strip is the toolkit's, and the side pane, which the app draws (next two parts). The app decides cues
and when to ask. New, close, close others, switch, rename, reorder, duplicate and every group change
go through `TabSet`, which is tested without a window. Shortcuts (`input/shortcuts.rs`): macOS Cmd+T,
Cmd+W, Cmd+Shift+[ and ], Cmd+1 to 9, Cmd+C, Cmd+, , Cmd+B (side pane); elsewhere Ctrl+Shift+T,
Ctrl+Shift+W, Ctrl+Tab, Ctrl+Shift+Tab, Alt+1 to 9, Ctrl+Shift+C, Ctrl+, , Ctrl+Shift+B (plain
Ctrl+B is tmux's prefix). Closing a tab asks only when something would be lost (`tabs::asks_on_close`):
when the foreground probe knows, if a program other than the shell runs; when it cannot (Windows),
only if the shell-integration marks say a command runs (D7), so an idle tab never asks. The
engine thread probes the foreground process and the line discipline (`raw_mode`) at most every
second when idle and every 250 ms while output flows or a command runs, and publishes the result in an `Update` only
when it changed (D3); the UI never touches the `Pty`. The running-outline cue is
`outline::should_show` (a port of Lunegit's `TerminalRunningOutlineGate`) over `CommandTracker`,
which follows the OSC 133 marks and whether the program consumes keystrokes; progress (OSC 9;4)
sets the cue's tone. `scripts/cpu-sample.sh` samples a process's CPU for the outline's budget
(ledger 16).

**Tab groups.** A group is a name, one of nine colours (grey, blue, red, yellow, green, pink, purple,
cyan, orange; per-theme values in `groups.rs`, whose test holds card text to 4.5:1 over dark
backdrops and the opaque theme; a light desktop behind the glass is not covered) and a collapsed
flag. `TabSet` keeps each group's tabs next to one another, gives a group at least one member (the
last one leaving or closing drops it), keeps names unique (compared exactly) and never reuses a tab
or group id. A new group gets the first colour no open group uses, else the least used. A name typed
by hand follows the rule `ctl` has (`label_ok`, at most 64 characters), must be free, and is never
trimmed or cut; a name that fails it disables the editor's confirm.
- *By hand.* Tab menu (strip tab or pane row): Add Tab to Group and, on a grouped tab, Remove from
  Group, Edit Group and Close Group, so the top bar needs no pane. Add Tab to Group has a chevron
  and opens a submenu, which the app builds (the toolkit has none): picking the row (a click, Enter
  or Space; hovering opens nothing) opens it beside the tab menu, which stays open: flush against
  the menu's right edge (its left where the window has no room on the right), the first row level
  with the picked one, shifted up where it would cross the window's bottom. It lists the other open
  groups in tab order, then New Group (the group editor), for the same tab and the groups as the
  menu opened with them, and it lives in the tab menu's state, so whatever closes or replaces that
  menu drops it. Escape and presses act one level at a time: Escape closes the submenu and then the
  tab menu; a press on the tab menu closes the submenu alone and picks nothing; a press outside
  both closes both. A pick in the submenu closes both, and a key still held from the pick that
  opened it repeats into it without picking. The toolkit keeps no row of the lower menu lit and has
  no Left or Right keys between the two. App menu: New Tab Group (a new shell tab in a new group)
  and Show or Hide Side Pane. A card header's menu: Edit Group, Collapse or Expand, New Tab in
  Group, Ungroup (keeps the tabs), Close Group. A colour picked in the editor of an open group
  applies and is saved at once (Cancel keeps it).
- *Close Group* asks every time, whatever its tabs run and whatever the close-confirmation setting
  says, with one answer for all of them. It closes the tabs it asked about that are still members; a
  tab a program adds while the question is open stays, with its group.
- *Where a new tab goes (D22).* The new-tab shortcut, the strip's `+`, the app and terminal menus'
  New Tab, and Duplicate Tab (relative to the menu's tab) put the tab after the last member of the
  reference tab's group when it has one, else at the end. A card's `+` and New Tab in Group add to
  that group's end; the pane's New Tab button adds an ungrouped tab at the end. A tab opened by hand
  into a collapsed group expands it. `ctl tab new` goes by its own rule (Protocol 1).
- *Collapse (D21)* is a flag only the pane draws: it never changes the active tab, is never refused,
  and the bar view ignores it. Next, previous and the number shortcuts run over all tabs in order and
  expand nothing. An action that sends the user to a tab expands its group: the toast's Go to tab,
  an inbox entry, `ctl tab focus` and `tab new --focus`.
- *Top bar drag (D25).* The bar does not show groups, so a drag there never changes membership
  (`TabSet::reorder_bar`): a grouped tab stays inside its group's run, and an ungrouped tab dropped
  inside a run goes to the nearer end of it (a tie: after).

**Two views and the side pane (D20, D26 to D35).** The saved choice (`side_pane`, see "Persistence")
is toggled by the shortcut, the app menu, the hide button in the pane's header and the show button
in the bar's title band. The two buttons share one place (right of the traffic lights on macOS, at
the left elsewhere), so toggling does not move the button. The pane shows only while the window is
at least pane width + 490 px wide (the card keeps 480 plus its margin); narrower, the bar view
shows, the choice stays and the show button is not drawn.
- *Layout.* The pane takes the left column over the window's whole height, 248 px wide by default,
  dragged within 200 to 420 by a 6 px zone on its right edge (a drag stops at window width - 490)
  and saved once, at the release. Its 34 px header holds the traffic lights' room and the hide
  button. On every OS a 34 px title band lies over the terminal column, with the terminal card (no
  strip) under it. The band shows the title centred on the card, clipped 12 px inside the card's
  edges and, where the app draws the caption buttons (Windows), short of them. The header's empty
  part and the band drag the window, and a double press there does what it does on the bar view's
  band. The body reaches the PTY size through the same `Renderer::place` to `Command::Resize` path a
  window resize takes. The pane has no glass layer of its own (it sits on the window panel). The
  Settings page draws in the card with the pane live; a press on a tab row, a New Tab action, an app
  menu row other than Settings, About and the pane toggle, or opening the inbox leaves the page.
- *Footer.* One row: New Tab at the left (a `+` and its label, no outline, as wide as that needs up
  to the next button, the label cut short there), and at the right the bell (only while the inbox has
  entries) beside the app menu. The inbox list and the app menu open upward, over the terminal.
  Toasts stay at the card's bottom right in both views.
- *List.* Loose tab rows and group cards (header with name and tab count, then the member rows unless
  collapsed), scrolling between header and footer. While they overflow the list, rows and cards stop
  3 px short of the scroll thumb's track; while they fit they use the full width. An activation change,
  a go-to or the pane appearing scrolls the active row into view (the card header when the group is
  collapsed). A row activates on the press, as on the strip; the active and the hovered row have a
  close button; a header toggles collapse and has a `+`; secondary press opens the tab or the group
  menu. A collapsed card holding the active tab draws its header selected, and shows one dot when any
  member has an unread notice; an expanded card shows dots on its member rows only. A row has no
  icon: a fixed column left of the title holds the unread dot, so a title does not move when the dot
  comes or goes, and a header's dot and name share the left edges of its members'. Rows show the
  progress (OSC 9;4) shine. Rows, headers and buttons are nodes of the accessibility tree
  (checked against the toolkit's tree in a test, not with a screen reader).
- *Running outline.* The bar view's outline is the strip's. The pane view draws the same cue on the
  terminal card's outline (`shell::card_cue`, a copy of the toolkit's private drawing): the setting,
  the full-screen rule, the tones and the motion level apply as on the strip (reduced motion
  breathes, no motion leaves the still outline), and the settings page covers it.
- *Pane drag (D24).* A tab row drags past the toolkit's threshold; the gaps of the flattened list are
  the slots: inside an expanded card joins that group there, a collapsed card's header joins at its
  end, between blocks makes the tab ungrouped. The last member leaving drops the group without a
  question (nothing closes). A card header drags the whole group, between blocks only. A line (or a
  highlighted collapsed card) marks the slot, the list scrolls near its edges, and Escape, a loss of
  window focus or a release outside the pane cancels. The commit is `TabSet::move_tab` or
  `move_group`; the active tab never changes (a dragged row became active at its press).
- *Not run in a real window yet:* pane, menu and drag behaviour is covered by tests and offscreen
  dumps; the Windows and Linux GUI has not been run with it.

**Persistence (M10 step 5, D5, D17, ledger 12).** `~/.luneterm` (`dirs::home_dir()`, on every OS;
`LUNETERM_CONFIG_DIR` overrides it, as the dump mode and the trial runs do) is created 0700 on
Unix and holds `settings.json` and `workspace.json`, both `{"version": 1, ...}`. Both are read
through the toolkit's bounded JSON reader and written by `persist/json_write.rs`; a write creates a
uniquely named 0600 file with `create_new`, syncs and renames it over the target, so a symlink or a
file already at the temporary name is never followed. A missing file is the defaults and writes
nothing; a file that is not JSON, over the limits, not an object or of an unknown version is copied
to `<name>.bak` (a backup is never overwritten), the defaults run, and the file stays until the user
changes something; a file over the size cap or not a regular file is not read and not saved over.
Enums, the font, the palette and group colours are saved by name, never by index. Nothing read from
either file is executed; the one value that names a program, `shell`, is the user's own setting, and
the workspace holds directories, titles and tab groups only (each directory goes through
`cwd::validated` before a tab starts in it, at most 32 tabs). `SaveGuard` is ledger 12: no workspace
write while any tab shows a failed start, nor after a tab that died within 3 s of its start until the
user acts on the tabs (a program tab's exit does not count).
- *Groups (D15)* are additions inside version 1: a tab's `"group": "<name>"` and a top-level
  `"groups": [{"name", "color", "collapsed"}]`, written only when a saved tab is grouped. An older
  build drops the per-tab key and keeps the top-level one. A bad name, a duplicate entry, an unknown
  colour or members that are apart are repaired on read (gathered after the first member), never
  refused. A restored active tab in a collapsed group stays active and the group stays collapsed.
- *Program tabs* (`ctl tab new -- ...`) are never written; a shell tab opened through `ctl` is saved
  like any tab and comes back with origin `user`.
- *Settings keys* with no counterpart in Lunegit: `tab_control` (bool, default on), `side_pane`
  (bool) and `side_pane_width` (read clamped to 200..=420, a non-number is 248); the last two have no
  Settings page row. With no `side_pane` key the first start decides once and saves it: a new install
  gets the side pane; a config directory that already holds `settings.json`, `workspace.json` or a
  `shell-integration/` directory is an upgrade and keeps the top bar until the user opens the pane.
  A rejected settings file is not saved over.

**Selection and links** are described with the host duties above (engine-thread selection
model; the closed URL grammar and the argv or `ShellExecuteW` open).

**Efficiency mode (copy-select-efficiency, D4 to D12).** The setting "Efficiency mode in the
background" (General > Background, `efficiency_mode`, default on) throttles Luneterm itself while its
window is not focused or is minimized, and never the shells it starts. `efficiency::Controller<O>`
is an `App` field used on the UI thread only (no global, no mutex); an `OsThrottle` backend does the
OS calls, and only on a change of state. Inputs: the setting (`Effect::Efficiency`), window focus
(`WindowEvent::Focused`, first statement of its arm; a minimized window is not focused, there is no
separate minimized input), and `Update.output` (PTY bytes were parsed since the last published
Update; hidden tabs publish at most every 100 ms, so any tab's output reaches it). The rule is
`wanted = enabled && !focused && (quiet || !slows_io)`, `quiet` being no output in any tab for
`QUIET` = 2 s. Backends:

- **Windows:** EcoQoS (`SetProcessInformation(ProcessPowerThrottling)`, control and state mask
  `EXECUTION_SPEED`) plus `IDLE_PRIORITY_CLASS`, Task Manager's own recipe (the green leaf). The
  step 0 probe on a Windows 11 VM showed neither half alone gives the leaf; a child created without a
  priority-class flag inherits the idle class and not the EcoQoS tag. The idle class is
  process-wide, so it also slows the PTY reader: `slows_io` is true and the throttle waits for the
  quiet period (D6), and output lifts it. The leaf therefore shows on an idle background Luneterm,
  not on one that is printing. Lift = control mask 0 (system-managed, not an explicit high QoS) and
  the priority class found at the first apply.
- **macOS:** the UI thread's QoS class to utility (`pthread_set_qos_class_self_np`), restored to the
  class and relative priority found at the first apply; the PTY threads are untouched (`slows_io`
  false, no quiet gate), and a child's QoS is reset by exec (measured). The backend acts on the thread
  that first called it (debug assertion). The calls are declared with `u32` classes, not `libc`'s enum.
- **Linux:** none: `nice` and `SCHED_IDLE` cannot be undone without privileges and there is no OS
  indicator. The row exists on every OS (like Option as Meta) with a Linux description that says it
  has no effect; the backend is never called.

Every child creation runs inside `Controller::lifted`, which lifts the throttle first and re-evaluates
after: `new_tab`'s spawn (a user's or a `ctl` tab's, conhost included) and `open_link`.
`Session::start` needs no lift. On
macOS a thread starts at the default QoS, not its creator's (measured: threads made from a thread at
user-initiated, utility and background all start at default), so the session threads are never
throttled. On Windows the priority class is per process, so a lift around thread creation would
change nothing: the session threads share the process's class and are throttled whenever the process
is, which is why the quiet gate exists (D6). On exit the controller is released first
(`release_for_exit`), so the final workspace and settings saves never run at the idle class. Luneterm is never throttled at the moment
it creates a process. Failure: any OS call that fails lifts what can be lifted, prints one
`luneterm: efficiency mode off: <call> failed (<code>)` line and turns the controller off for the
run; an error never leaves the process throttled. The controller's wake deadline (the end of the quiet
period) is merged into the control flow in `settle_flow`, also while the window is hidden, and
`Controller::tick` runs first in every callback, so a due deadline is consumed and cannot spin the
loop. Tests: fake-backend unit tests for every state change (`efficiency/mod.rs`), and
`tests/efficiency_os.rs` against the real OS (Windows: the own process's class and EcoQoS state, a
lifted child at normal class and a control child that shows the inheritance; macOS: the thread's
QoS and a child that reports its own QoS). Checked by hand so far: the leaf recipe in a PowerShell
probe on the VM (step 0); not yet by hand with the Luneterm window: the leaf in Task Manager, and the
first click after the window was unfocused on a busy Mac.

**Spawn (D11, D12).** Shell: the setting if set, else the Unix login shell (`program: None`, as
Lunegit's `$SHELL -l`); on Windows `pwsh.exe`, then `powershell.exe`, then `%ComSpec%`. A candidate
that is not found moves on to the next; any other error stops. cwd: the setting if it is an existing
directory, else home. Environment: Lunegit's overlay with `TERM_PROGRAM=Luneterm` (iTerm2 spoofing
behind the setting), and when none of `LC_ALL`, `LC_CTYPE`, `LANG` is set (an empty value counts as
unset) `LC_CTYPE=UTF-8` on macOS or `C.UTF-8` on Linux, because a Finder-launched shell would
otherwise run in the C locale. When a tab's child exits the tab closes and the others stay (the last
tab takes the app with it); if no candidate spawns the grid shows "Failed to start terminal" with the
error kind and stays open.

**Program tabs** (`ctl tab new -- <program> <args...>`, `spawn::program_spawn`). Neither `shell_args`
nor shell integration applies, and a program that cannot be started makes no tab (the caller hears
it).
- *Unix.* The shell is the `shell` setting, else an absolute `$SHELL`, else `/bin/sh`. zsh, bash, sh,
  dash, ksh and fish run `<shell> -l -i -c <constant script> [luneterm] <cwd> <program> <args...>`
  (fish takes no `$0`): the script changes to the directory (exit 127 if it cannot) and `exec`s the
  program, so the program has the login environment and `PATH` of a new tab, after the user's rc
  files ran. The directory and every word are separate arguments, never text in the script. A program
  name starting with `-` is refused under such a shell (`exec` would read it as its option). Any
  other shell is bypassed: the program starts directly with the app's environment plus the additions.
- *Windows.* No login shell. The program is started directly: an absolute path as it is, a bare name
  searched in the child's `PATH` (as it is, then with each `PATHEXT` extension); empty and relative
  `PATH` entries, relative paths with a separator and a `:` in the file name are refused. A `.bat` or
  `.cmd` file (in any case, trailing dots and spaces ignored) is refused: Windows would hand it to
  `cmd.exe`, which parses the arguments again.

**Shell integration (M10 step 7, D12).** With the `shell_integration` setting on (the default) and a
config directory, `spawn::launch_with` writes `<config dir>/shell-integration` (scripts ported from
Lunegit's `TerminalShellIntegrator`, text constant) and starts a zsh, bash or PowerShell through them,
so the shell reports its directory (OSC 7, OSC 9;9) and command marks (OSC 133). This changes the
spawn, so the paragraph above holds only with it off or for any other shell: zsh gets `ZDOTDIR`
(ours) and `LUNETERM_USER_ZDOTDIR` (the user's) added to the environment; a bash with no arguments or
`-l` runs as `bash --rcfile <ours> -i` (for the login shell, with `$SHELL` as the program, since
arguments need one); PowerShell with no arguments gets `-NoExit -Command <read our script>` (the
script's path single-quoted, every quote doubled). On Unix the directory, its three subdirectories
and each script are `lstat`-checked before use (real, owned by the effective user, no group or other
write); a failed check, or any error, skips integration for that spawn and the shell starts as
without it. The directory is created 0700 and the scripts 0600, written through a temporary name and
renamed. On Windows the directory is under `%USERPROFILE%`, whose user-only default ACL is assumed
(plan assumption A3), and only PowerShell is integrated. `shell_integration = false` leaves each
candidate's argv alone and adds only the environment below (the `LUNETERM_*` tab variables included;
a test pins the exact keys).

**Environment (M10).** Read by the app: `LUNETERM_CONFIG_DIR` (replaces `~/.luneterm`;
`ctl::client::config_dir` reads it for the app and for `ctl`, and a relative one is passed to
children as an absolute path), `LUNETERM_DUMP_FRAMES` (dump mode, no window) with
`LUNE_REQUIRE_GPU` (a missing adapter becomes a failure), `SHELL` (the login shell's name when
integration needs one, and the shell a program tab goes through), `PATH` and `PATHEXT` (finding a
program tab's program on Windows), `ZDOTDIR` and `HOME` (where the user's zsh files are), `LC_ALL`,
`LC_CTYPE` and `LANG` (the locale fallback). `luneterm ctl` also reads `LUNETERM_INSTANCE`. Every
tab's child gets `LUNETERM_BIN` (this run's executable, canonical, resolved once at start; under a
lunepack install the version's own copy), `LUNETERM_INSTANCE` (this run's token) and
`LUNETERM_TAB_ID`, replacing any inherited values; an empty one reads as unset. Added to the child's
inherited environment besides those: `TERM=xterm-256color`,
`COLORTERM=truecolor`, `TERM_PROGRAM` and `TERM_PROGRAM_VERSION` (`Luneterm` and the crate version,
or `iTerm.app` and `3.6.11` behind `report_iterm2`), `LC_CTYPE` when no locale variable is set, and,
for zsh with shell integration on, `ZDOTDIR` (ours) and `LUNETERM_USER_ZDOTDIR` (the user's). The
Windows child gets the inherited environment plus these, not portable-pty's registry merge (Q7).

**Title and bell (D13, D14).** The engine keeps titles up to 10 MB; the app scans at most 4,096
characters, drops Unicode Cc and the bidi controls (U+061C, U+200E, U+200F, U+202A to U+202E,
U+2066 to U+2069), trims, caps at 80 characters, and uses "Luneterm" when empty; the result goes to
the window title and, in the bar view, the title band (a 10 MB title takes under 10 ms in the test).
A name given by hand (the rename dialog, the saved workspace, `ctl tab new --title`) is cleaned the
same way but may be 128 characters (`title::MAX_NAME_CHARS`). A bell in an
unfocused window asks for user attention (`Informational`), at most every 2 s (`BELL_INTERVAL`),
and so does a program message in a background tab of one (the D6 table below); a bell in a
background tab also leaves an inbox row (next part).

**Bell and notifications (bell-notifications plan).** A bell in a background tab shows an amber dot
on that tab; a program message (plain OSC 9, or `OSC 777;notify;<title>;<body>`) shows an in-app
toast and is kept in an inbox (the bell in the bar's header or the pane's footer). Nothing goes to
the OS but the attention request above (the bell's, and a message's in an unfocused window).
- *Path.* Bytes reach `lune-vt`: `ext/osc_notify.rs` turns a message into
  `HostEvent::Notification { title, body }`, only with `StandardExtensions::notifications` on (the
  app passes `true`; `luneterm-ffi` passes `false`, so through the C ABI `OSC 777` stays
  `UNKNOWN_OSC`, a plain OSC 9 stays silent, `flatten_events` skips the variant, and `ABI_VERSION`
  stays 1). The session engine collects them in the Update (`Update.notifications`, raw text).
  `Tab::apply` is the one place program text becomes app text: `title::sanitize_text` makes
  `Effects.messages` (and the bell count becomes `Effects.bells`). `pump` hands both to `hear`,
  which asks the `route` table and fills the `Inbox` (`notices.rs`, in memory), the single store.
  The tab dot (`StripTab.attention` = the tab has an unread row and is not the active one; the
  pane's rows use it too, and a collapsed card shows one dot), the inbox button's bell (yellow while
  any row is unread, plain once all are read; it rings for 1 s when a new row arrives in a focused
  window, at most once per 3 s, under full motion only; the count stays in its accessible name), the
  list (a luneui popover) and the toast slots are all read from it.
- *OSC 9 grammar (D4, D5).* The payload is a message unless its first field (up to the first `;`, or
  all of it) is only ASCII digits, which keeps every ConEmu form (`9;4;...` progress, `9;9;...`
  cwd, a bare `9;404`) out; `9;3 tests failed` is a message. `OSC 777` is `notify` only,
  case-sensitive, the body is everything after the second `;`; both fields empty is nothing. An
  aborted OSC emits nothing.
- *Routing (D6).* `route(is_bell, focused, active)`:

  | | focused, active tab | focused, other tab | unfocused, active tab | unfocused, other tab |
  |---|---|---|---|---|
  | bell | nothing | row + dot | OS attention | row + dot + OS attention |
  | message | nothing | row + dot + toast | OS attention | row + dot + OS attention |

  OS attention goes through the existing `BellLimiter`. No toast is shown for something that arrived
  while unfocused, nor on return (the dot and the yellow bell show it). With the setting "Notifications from
  programs" off (`osc_notifications`, Terminal > Behavior, default on) messages are dropped in `pump`
  (no row, toast or attention; rows already there stay); bells are not affected. `Inbox::settle` runs
  at the end of `pump` and before each frame: while focused it marks the active tab's rows read, and
  rows of closed tabs go, so no hook is needed where the active tab changes. A bell folds into the
  tab's unread "Bell ×N" row; an identical message folds into the tab's newest unread row.
- *Sanitising (D9).* Per field, from at most `MAX_SCAN_CHARS` (4,096) chars: every whitespace char
  becomes one space and runs collapse, `is_control()` and the bidi controls of `title.rs` are
  dropped, trim, then at most `NOTICE_TITLE_CHARS` (80) for the title and `NOTICE_BODY_CHARS`
  (240) for the body; a message with both empty is dropped. Titles keep their own rule above.
- *Caps (D10).* One OSC payload at most `MAX_PAYLOAD_BYTES` (4,096, in `ext/osc_notify.rs`), over it
  dropped whole, never cut; `NOTIFICATION_LIMIT` (8) events per engine drain;
  `NOTIFICATIONS_CAP` (16) per Update, the oldest dropped; `INBOX_CAP` (50) rows and `TAB_CAP` (20)
  per tab; toasts: `TOAST_LIVE_CAP` (3) alive, one per tab, a tab's toast text replaced at most
  once per `TOAST_MIN_INTERVAL` (1 s), each lives `TOAST_LIFE` (6 s, luneui holds it while the
  pointer is on it).
- *Invariants.* The text is inert: luneui draws it as plain text (never markup, never a link), and
  it is never copied, written to the clipboard, sent to the PTY, logged (the `Debug` of a notice
  prints char counts, the engine's one debug line a byte count) or persisted (the inbox is not in
  `workspace.json`). The source is always named: the toast header and every list row carry the
  tab's current title (a toast keeps the title it had when pushed). Arm delay (D14): a program
  controls when a toast appears, so `Go to tab` does nothing until that toast has been drawn and
  has stood still, with its text unchanged, for `TOAST_ARM` (400 ms): `ToastSlots::sync` restarts
  the clock on the first frame that lists the toast and on every change of its rect (the stack
  slides when a neighbour leaves), and `press_target` is the one gate in front of the activation.
  It is judged at the press, not at the frame that reports the click (luneui reports it on the
  release): `PressClock` keeps the latest primary press until the frame that saw its release, and
  forgets it on a focus loss; a click with no press kept (a key) is judged at its frame. The list
  follows the same rule (`notices::armed_since`): luneui reports a row press as an index into the
  frame the user saw, so `DrawnRows` keeps the ids of the rows the last frame drew and since when,
  and a row press activates the row drawn at that index only when that list had stood still for
  `TOAST_ARM` at the press and the click's own frame changes nothing in it; otherwise it does
  nothing and the list stays open (so a row press right after opening the list waits too).
  While the pointer is over a toast, `terminal_hit` keeps every pointer path out of the terminal; a
  drag that began in the terminal keeps its capture. While the inbox list is open `overlay_open()`
  is true, so no key or pointer event reaches the terminal; it closes on Escape, a press outside, a
  focus loss, and when a dialog, the page or a menu opens.

**Updates (auto-update plan; publishing and keys: `docs/updates.md`).** The app installs and updates
through lunepack (`lune-rs/lunepack` at one pinned rev, facade crate `lunepack` with its `blocking`
API; HTTPS by ureq and rustls inside lunepack, no new HTTP code in the app). `update::startup()` is
the first thing `main` does for an app start, after `restrict_dll_search` and the `ctl` dispatch,
which never reaches it (lunepack's lifecycle: a version that was
rolled back exits at once); `update::confirm_healthy()` runs once on the UI thread after the first
frame is presented and a tab has a session (D8), and only when `startup` said a confirmation is
pending. A copy lunepack did not install (a `cargo run`, an old portable zip, no
`lunepack-update.json` beside the executable) is `Unavailable`: no worker thread, no network, the
About rows are disabled with "Updates need the Luneterm installer". The root of trust is the one in the
installed copy's update config (`lunepack-update.json`, written by `lunepack pack` from the `[root]` table of
`packaging/lunepack.toml`); the app compiles in no root key and does not compare it with anything.
*Worker.* `Updater` owns one thread, `luneterm-update`, over `lunepack::blocking`. The UI thread
sends `Check { auto }`, `Download` and `Restart`; the worker posts every `UpdateState` through a sink
(`EventLoopProxy::send_event`, so it never blocks the UI thread). One operation at a time: a command
sent while one runs is dropped (an `AtomicBool` taken by compare-exchange on the UI thread, released
by the worker before it posts a command's last state); the operation's `CancelToken` is stored before
the command is sent, so a cancel cannot miss it, and is never held across a lunepack call. Quit
cancels the operation and waits for the worker at most 1 s (`exiting`). States: `Unavailable`, `Idle`,
`Checking`, `UpToDate`, `Available { version, auto }`, `Downloading(pct)` (posted at most 4 times a
second), `Ready(version)` (downloaded and staged), `Failed(text)` (lunepack's message with every URL
cut to scheme, host and path, at most 240 chars). Restart runs `apply_and_restart` with
`UiChoice::Never` (Luneterm shows its own toast; lunepack's update window is never opened), waiting
for this process, then the normal quit path saves the workspace and settings. Restarting closes all
shells, which the toast and About say.
*Rules around it (`update::auto`, pure, tested without a window).* The setting "Check for updates
automatically" (`auto_update_check`, on by default, a missing key reads on; Settings > General > Updates) runs a
timer: a check at launch (or when the setting is switched on) and every 24 h; switching it off drops
the deadline. A check the timer started that finds a version raises the glass toast "Luneterm
<version> is available" with Update and Dismiss (20 s; each version once per run); Update downloads and
stages, then a second toast and About offer "Restart to update". A manual "Check for updates" never
toasts: it shows its result in About. No request is made except by a check or a download.
*Settings pages.* General is the first settings page and where Settings opens (`Section::ALL = [General,
Appearance, Terminal, About]`; the Settings menu item and command open it). It holds the two app-wide rows, the
update toggle (group Updates, disabled when `UpdatePanel::toggle_enabled` is false) and efficiency mode (group
Background); About draws no group: its version line carries a Secondary "Check for updates" button, the status text and,
when `UpdatePanel.action` is set, the Primary Update / Restart to update button, inline. General is a Luneterm-only
deviation from Lunegit's settings layout; no settings.json key changed with the move (the file is flat, the open page is
not persisted).
*Channel.* A pack bakes a channel (`stable` for a stable tag, `beta` for a release candidate), and an update replaces the tree's update
config with the new pack's, so a beta user who receives a stable pack would follow stable. The first
run of an installed copy therefore records the tree's channel as `update_channel` in `settings.json`
and every `Updater` is built with it (`update::channel`): a copy keeps its install channel. A garbled
value reads as not recorded and is rewritten from the tree. `settings.json` outlives an uninstall, and a
setup cannot run over an install (lunepack refuses: "already installed"), so changing channel is an
uninstall and the other channel's setup; that setup must reset the record. The tree's channel cannot tell
it from an update (a beta copy's tree names stable after every stable pack), so lunepack's own signal
decides: a per-user first install, or a reinstall over a removed tree, queues an `Installed` lifecycle
event that `lunepack::startup()` delivers once (`Launch::fresh_install`); an update queues `Updated`. On
that start the tree's channel is recorded whatever was persisted, and saved at once. A per-machine
install queues no events, so it keeps the persisted channel across a reinstall (docs/updates.md).
*Trust.* The root public key lives in one place, the `[root]` table of `packaging/lunepack.toml`, and
reaches an installed copy through the update config of its pack. It is a throwaway TEST root until the
owner's key ceremony (docs/updates.md); `scripts/release-pack.py check-root` refuses to pack or to
build the macOS input while it is in. Feed and manifest parsing and signature checks are lunepack's; the
app treats a feed as untrusted input and shows only lunepack's message text.
*Release side (`scripts/`, `.github/workflows/publish-updates.yml`).* Build jobs produce one build input
per OS (`make-build-input.py`: the app tree + lunepack runtime, UI and, on Windows, the launcher stub,
built from the pinned source by `build-lunepack.{sh,ps1}`, whose dependency graph check is fail-closed).
Publishing is lunepack's standard `lunepack publish` flow (its `docs/PUBLISHING.md`): `tools-linux`
builds the CLI from the pin and reports its hash; `pack` (environment `release-pack`) runs
`release-pack.py pack`, which takes the build inputs from the GitHub release, checks them (SHA-256,
safe extraction, `BUILD-INFO.txt`) and runs `publish fetch`, `pack` and `setup` for the three targets;
`push` (environment `release-push`, the owner approves) runs `publish push`, which writes the two
feeds (`stable`, `beta`; newest 10 versions each, a release candidate to `beta` only) to R2 in
lunepack's order; `attach` puts the setup files on the release. A weekly `refresh` (environment
`release-refresh`) keeps the feeds from expiring. The release key and the R2 tokens are secrets of the
three environments, never of the repository; every key job checks the CLI hash and builds nothing.
One config, `packaging/lunepack.toml`, serves every OS (`[install.<os>]`) and `lunepack publish`
(`[publish]`); the channel is chosen at pack time (`pack --channel beta`). Names, setup and recovery are in
`docs/updates.md`. The release binary grows by about 2.7 MiB (macOS arm64, measured on the probe
build when lunepack was added).

**Fonts and look (D15, D16; M10 D9a; rznmono-1000).** The six bundled faces live in `crates/luneterm/assets`:
RznMono 1.003 Regular, Bold, Italic and Bold Italic (the terminal font, real faces for every
style termview asks for; it carries the Nerd Fonts icons and the 128 `calt` ligatures, and its
`CREDITS.txt` is bundled) and RznSans Regular and Medium, with their licences. RobotoMono Nerd Font Mono
is no longer bundled. The Settings font list is the built-in RznMono and the installed system fonts:
the curated names of `fonts::SYSTEM_MONO_FONTS` (23 popular monospace families) that the loaded font
database has an upright face of (`fonts::system_fonts`, once when the window's `TextCache` is built;
a font installed later shows after a restart; no OS enumeration, no new crate). The combo shows disabled
`BUILT-IN` and `SYSTEM` heading rows (no `SYSTEM` heading when none is found); the rows are
`settings_page::font_rows`, and the generic option path covers RznMono only. A saved name that is not
curated or not installed draws as RznMono and stays in the file unchanged. RznMono is `TermFonts::icon_family`
for every system font, so Nerd icons a system font lacks still draw; other missing characters keep
cosmic-text's fallback. luneui-rs's `TextCache` loads no system fonts, so the app loads them once at start
(`fonts::text_cache`, 59 to 64 ms on the Apple M1 Pro) for emoji and CJK fallback. Cell
height is `ceil(natural_line_height)`: 18 px at scale 1 and 37 at scale 2 for the bundled face at
14 px, the default size since the luneui-rs 0.4.0 work (13 px gave 17 and 34). At 1040 x 660 the dump
scenes' grid is 125 x 31 at scale 1 and 117 x 30 at scale 2 (read from `Renderer::grid_size`). Glass: `near_black::MATERIAL`
(UltraDark vibrancy, near-black layers; `docs/glass-styling.md`) for the default style, Lunegit's layers under the liquid styles; Windows draws its own caption buttons.

**Windows (D17).** `main` hardens the DLL search before any winit or wgpu object ("lune-pty", above),
the release build uses the GUI subsystem (`windows_subsystem = "windows"`), and CI uploads
`luneterm.exe` as `luneterm-windows-x64`.

**Measured (M9).** Apple M1 Pro unless noted; debug builds parse at roughly 6 MB/s, so the timing
bounds are 5x the debug median, not a product claim.
- *Protocol latency* (debug, send 2 B then take the Update): p50 27 to 43 us, p99 under 100 us.
- *Mirror:* `session_mirror_differential` (an Update-applied `Grid` equals `viewport()` after every
  Update; chunking, commands, resize parity, hidden publishes, the fake clock past 2026 timeouts,
  scrollback 0/5/50/1000): 20,000 seeds pass in release (191 s); seed 2985 found the scrollback-0
  lines-missing state and is pinned.
- *Floods* (E6, 256 MiB of plain text, debug, three pacers): 37 to 40 s each, RSS growth +8, +0, +0
  MiB, inbox high water 1024 KiB, reader waits 258; 16 MiB of BEL 4.1 s; 4 MiB of `ESC[6n` 0.8 to 2.0 s
  with the response queue at its 65,536-byte cap (292k to 366k dropped). Every bound of the table
  held.
- *Keys during a flood* (E6k, 20 keys typed behind the 1 MiB inbox): macOS debug median 254 to 261
  ms, p95 260 to 274 ms (the debug engine drains 1 MiB in about 250 ms); Linux Docker median 366 ms;
  Windows CI (16 MiB floods) median 20 ms, p95 20 ms.
- *Mode 2026* (E5): nothing published before the reset, one Update after (406 to 409 ms from GO
  with the child's pauses); without a reset the first Update comes 1007 to 1022 ms after the set.
- *Memory with scrollback* (this Mac, release, a real session on `awk` output, 200 x 50, the
  session plus its UI grid, no window or GPU; RSS by `ps -o rss=`, which read a 64 MiB allocation
  as +64 MiB first): scrollback 5,000 adds 16.0 MiB (plain) / 15.8 MiB (a colour change every 5
  cells), scrollback 100,000 adds 272.3 / 271.9 MiB, about 14 bytes per cell of the 20.0 M cells
  (`docs/lunegit-bug-ledger.md` row 7). The process idles at 113 MB in a debug build with a window
  (GPU, fonts and atlas included).
- *Frame time and the headless render:* `lune-termview`'s `frame_time` is unchanged (the table under
  "lune-termview"); the E8 PNG of a real session is byte-identical to a reference replay on macOS
  and on Windows CI (1040 x 660).
- *Binary:* macOS arm64 release 11,495,056 bytes, the Windows exe 13,858,304.
- *Windows CI* (run 42477613074 at b535456): the e2e harness ran 10 cases with 0 failures; E2 to E5
  and E9 need a Unix tty (`stty`) and skip there; the GPU initialized on the runner.

**M10 closed what M9 left:** tabs, selection and copy, the settings file and page, the kitty reset
and the consumers of cwd, progress and marks, and the Windows GUI run (a Windows VM, step 8; the
results are in `docs/luneui-rs-handoff.md`). Real IME typing is still verified only by hand (no
input method was installed on the VM).

## Tab control (ctl)

`luneterm ctl` lets a program of the same user list, open, close, focus and group tabs of a running
Luneterm. The contract is Protocol 1 (the orch side's) with its revision 3 amendment, `tab close
<id> [--any-origin]` (task `tab-groups-and-control`); it is not restated here. The grammar and limits
in code are `ctl/args.rs`, the exit codes `ctl::Code`. A request comes from any process of the user,
so it is untrusted: lengths are checked before allocation, input over a limit is refused, never cut.

**Client** (`ctl/client.rs`; `main` dispatches it before `update::startup()`, which reads the
arguments itself). It opens no window, reads no settings, loads no fonts and never runs the updater.
It parses the words (and checks that `--cwd` is a directory) before it connects, uses the instance
`LUNETERM_INSTANCE` names, else the only reachable one (none or several: exit 4), and ends within
2 s (exit 1). Success is the JSON line on stdout and exit 0; otherwise one `luneterm: <reason>` line
on stderr, control characters removed. It is the app binary, a GUI-subsystem program in Windows
release builds: callers that wait on the process (Git Bash, Python `subprocess`, checked on a Windows
VM) get its output and exit code. PowerShell's `& luneterm.exe ctl` does not wait for a GUI program
and shows nothing (probed); an interactive cmd prompt is expected to behave the same (not probed).

**Endpoint** (`ctl/endpoint`). An instance is named by a token, 16 bytes from the OS (`/dev/urandom`,
`BCryptGenRandom`) as 22 base64url characters, new for each run; it names, the OS authenticates.
Without a token, a config directory or a passing check, control is off for that run: one stderr line
(a reason, no path) and the app runs normally.
- *Unix:* a stream socket at `<config dir>/run/<token>.sock`. The config directory must not be group
  or other writable; `run` must be a real directory (not a link), owned by the effective user, with
  no group or other bit; it is created 0700 when missing and never repaired. The socket is bound as
  `.sock.tmp` and renamed, so a `.sock` name is a listening socket. A bind removes only the user's own
  sockets that refuse a connect and temporary ones older than 60 s. The server asks the kernel for
  the peer's uid (`getpeereid`, `SO_PEERCRED`) and drops another user's connection unanswered; the
  client checks the directory and the socket's owner. A `sun_path` that does not fit is refused.
- *Windows:* `\\.\pipe\luneterm-ctl-<token>`, owner = the user's own SID and a protected DACL with
  one allow ACE for it, `FILE_FLAG_FIRST_PIPE_INSTANCE`, `PIPE_REJECT_REMOTE_CLIENTS`, one instance
  reused (disconnected, never closed, so the name never lapses). The server refuses a client process
  whose token user differs; the client opens at identification level and refuses a server whose token
  user differs. Pipe names are the machine's, so a scan (no `LUNETERM_INSTANCE`) can list other
  users' pipes: it tries each without waiting, then gives the busy ones turns of 250 ms each, for
  the whole budget while none has answered and for one turn more once one has. So another user's
  busy pipe does not end the run, and a second Luneterm of this user that stays busy longer than
  that turn is not counted. The list stops at 4096 names: someone who makes more can still crowd the
  scan, and `LUNETERM_INSTANCE` is the sure way. Not tested: a non-elevated `ctl` against an
  elevated Luneterm of the same user (if reading the other process's token is denied, the connection
  is refused), and a pipe of another user on a real second account.
- No network listener exists.

**Wire** (`ctl/wire.rs`). Request: `LTC1`, a u32 little-endian word count, each word as u32 length and
UTF-8; at most 300 words and 128 KiB for the whole frame. Reply: one status byte (the exit code; 4 is
the client's alone), u32 length, UTF-8 body of at most 4 MiB (the JSON line or the reason). The server
parses the words with the parser the client ran.

**Threads and deadlines.** `ctl-listener` accepts one connection at a time (read 1 s, write 1 s) and
hands the request to `ctl-handler` (1.5 s, else the reply is 1; while one is unanswered no other is
handed over). The handler is a `Bridge`: it checks a `tab new`'s directory on its own thread (300 ms,
no answer counts as no), posts `UserEvent::Control` to the UI thread and waits. The UI acts only if
it takes the call within 1.2 s (`Call::take`), so a request given up on is never acted on later; for
a taken one the bridge waits up to 1.45 s. One case remains: a request taken in time whose work (a
slow spawn) outlasts the server's 1.5 s is carried out while its client is told 1.

**Handler** (`ctl/handler.rs`, a pure function over `TabSet`, the setting and the clock). For a
`tab new` the app starts the child before it answers; the other effects follow the answer.
- With `tab_control` off, every request but `status` is refused (3) and nothing changes; the listener
  runs either way, and the setting is read per request, so it needs no restart.
- `tab new` (origin `control`): at the end, or after the group's last member; active, revealed and
  raised only with `--focus`. At most 64 tabs of origin `control` are open at once (the next is 1).
  No window yet, or a program not found or not started, is 1; a `--cwd` that is not an existing
  directory and a refused batch file are 2.
- `tab close`: unknown id 5, checked before origin; a `user` tab without `--any-origin` is 6;
  otherwise it closes with no confirmation, and closing the last tab quits. Dialogs and menus about
  only gone tabs close, as when a child exits.
- `tab focus`: expands the group, activates, restores the window and asks the OS to focus it; on
  macOS the app also unhides and activates itself (`raise.rs`: winit's calls do neither for a hidden
  app), on this and `tab new --focus` only. Only the normal window and app calls: the OS may decline
  to raise the window, and the call still exits 0. It does not close the Settings page.
- `tab group`, `ungroup`: either origin.
- `tab list`: every tab in order, collapsed members included; `title` and `cwd` have control
  characters removed, and `cwd` is `null` unless absolute and at most 4096 bytes; `input_idle_ms`
  counts keyboard, paste and IME input only.
- *Input hold:* a `tab focus` or `tab new --focus` that changed the active tab drops keyboard, paste
  and IME input for 400 ms, so a background program cannot redirect what the user is typing.

**What `--any-origin` allows:** any program of the same user can close any tab, ending what runs in
it with no question. Same-user code can already kill those processes; turning the setting off stops it.

**N1 to N4** (fixed requirements):
- *N1, no input.* The handler sees a tab only through `ControlTab` (origin, title, cwd, pid, last
  input time), with no session; a test scans the module's code for session and content names. The one
  data a caller hands a program is its argument vector at creation (above).
- *N2, no content.* Replies carry only the fields above; nothing reads the grid, scrollback,
  selection or clipboard.
- *N3, local and same user.* The endpoint checks above; `UserEvent::Control` is made only by the
  listener's bridge, so nothing a program prints (no escape sequence) can trigger an action.
- *N4, the setting.* Terminal > Behavior, "Tab control from programs", on by default; off leaves
  existing tabs and groups as they are.

**Interpretations beyond Protocol 1** (as built; the numbering is the plan's CC list):
- CC1: no separate console binary on Windows (see Client).
- CC2: batch files are refused on Windows (see "Spawn", program tabs).
- CC3: the login environment exists only for the sh family and fish; rc files run first.
- CC4: "256 arguments after `--`" counts the program; "64 KiB" is the UTF-8 bytes of those words
  summed. Every word is valid Unicode without NUL. Group names are at most 64 and `--title` at most
  128 scalar values, with no control or bidi character, no U+2028 or U+2029 and no whitespace at an
  end. Ids are plain decimals up to u64. A set `LUNETERM_INSTANCE` that is not 8 to 64 of
  `A-Za-z0-9_-` is exit 4; an empty one counts as unset. Also exit 2: a `--cwd` with a control
  character, over 4096 bytes or starting with two separators; an empty program word after `--`; an
  option value that is exactly `--cwd`, `--group`, `--title`, `--focus` or `--`.
- CC5: the 64-tab cap, the quit on closing the last tab and the best-effort raise.
- CC6: the input hold.
- CC7: a `ctl` shell tab is saved and restored as a user tab, a program tab is not saved;
  `LUNETERM_BIN` is this run's executable.

## Extension points

| Point | Mechanism | Example |
|---|---|---|
| Escape-sequence behaviour | `lune_vt::Extension<C>` (`Terminal::add_extension`) registers handlers on the `EscapeSequenceParser<C>` itself (newest-first, like xterm.js `register*Handler`); there is no separate registry | kitty keyboard, XTVERSION, OSC 7 cwd, OSC 9;4 progress (all shipped in `lune-vt/src/ext/`, installed together by `Terminal::install_standard_extensions`; a new one is a new file there, registered by `StandardSet`) |
| PTY strategy | `lune_pty::PtyBackend` | portable-pty backend; later a session-daemon backend |
| OS glass | `lune_ui::material::Material` applied by `lune_ui::material::apply` (luneui-rs `platform.material`; its `Backdrop` trait documents the capability levels) | macOS vibrancy / Liquid Glass, Windows acrylic/Mica, KDE blur |
| Theme | `lune_ui::theme::Theme` value handed to the `Painter` | `lunegit_dark::THEME` |
| Paint | `lune_termview::Pass` ordered passes: cell backgrounds, text (mask then colour), decorations, cursor, selection, link hover | new overlay = new pass at its z-position |
| Host embedding | `luneterm-ffi` C ABI (`include/luneterm.h`, "luneterm-ffi (C ABI)"), versioned by `luneterm_abi_version()`; a new export is added without a version bump | Lunegit P/Invoke |

## Split points

Where the repo can be cut later (nothing is split yet):

- `lune-vt` can move to a library repo once the FFI (ABI 1, M8), its second consumer, has settled
  its API in use by Lunegit. It
  takes `tools/xterm-oracle`, the goldens and the conformance harness with it.
- `lune-ui` moved to its own project, `~/Projects/luneui-rs` (decided 2026-11-03). Luneterm
  consumes it as a git dependency pinned at one rev (M9, roadmap D22; the app and `lune-termview`
  use it). The in-tree toolkit and the spike were retired in M10 step 12; a toolkit bump edits
  the rev in the root `Cargo.toml` and in `scripts/check-deps.sh` in one reviewed change. `lune-termview` stays in Luneterm (luneui-rs D8).
  `luneterm` is lib + bin: the tabs, settings and links build on the same `session`, `input` and `shell` modules.
  Where `lune-pty` goes is undecided.

Invariants that keep a split cheap:

1. No `lune-*` crate depends on a `luneterm*` crate.
2. No `lune-*` crate holds Luneterm identity (names, env values, copy, paths).
3. Each library's tests and test data live in its crate directory, or in a tool named for it.
4. Every crate sets `license` (inherited from the workspace: Apache-2.0).

`scripts/check-lib-boundaries.sh` enforces them with grep rules: no `luneterm` in `lune-*`
manifests or files (the allow list is `scripts/lib-boundaries.allow`); no clock or environment
reads in `lune-termview/src` (rule 3; tests and examples may); every manifest inherits the
licence. It runs locally and on the Linux CI job.

## Threading

One engine per session, owned by that session (`Send` since M8, so a session may move it to a worker thread or hand it across the C ABI; not `Sync`). In the app (M9) the engine lives on its own thread
with a reader and a writer thread beside it ("luneterm app"); the renderer paints on vsync from the
dirty rows of the Updates the UI takes, and honours synchronized output (mode 2026), which Lunegit
parses but does not use for painting.

PTY threads (`lune-pty`):

- **Reader.** `Pty::take_reader()` moves the blocking reader out once; the caller runs it on its
  read thread. It holds no shared state and returns `Ok(0)` at EOF.
- **Writers.** `Pty::writer()` returns a `PtyWriter`, cloneable and `Send + Sync`: a mutex around
  the single portable-pty writer, held for one `write_all` + `flush` only. The engine's replies
  (DSR and the like) and the UI's keystrokes can both write through clones.
- **Windows exit watcher.** ConPTY never closes its output when the child exits. A watcher thread
  waits on a duplicate of the process handle, then closes the pseudoconsole (off the UI and reader
  threads), which is what gives the reader its `Ok(0)`.
- **Drop** never blocks: an unfinished child is killed and everything is closed and reaped on a
  detached thread.

Outside the sessions the app has threads for persistence, checks of directories and, per feature:
the update worker (`luneterm-update`, "Updates" in "luneterm app"): UI-side state is an atomic busy
flag and a mutex-held cancel token, each held for a store or a clone only, and its results reach the
UI as user events. And the control listener (`ctl-listener` and `ctl-handler`, "Tab control (ctl)"):
they share no tab state with the UI thread; a request crosses as `UserEvent::Control` with a one-shot
reply channel and an atomic that settles whether the UI or the waiting thread owns it. Dropping the
server at exit stops it without waiting for the handler.

Efficiency mode (see "luneterm app") changes the UI thread's QoS class to utility on macOS while the
window is in the background, and nothing else about threads there: on macOS the session threads are
created unthrottled (a new thread starts at the default QoS, not its creator's, measured) and keep
their QoS. On Windows it changes the whole process's priority class and EcoQoS state, so the session
threads are throttled with it, only after a quiet period and never while a tab prints.
