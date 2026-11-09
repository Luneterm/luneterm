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
`lune-vt`, `lune-pty`; `luneterm-ffi` → `lune-vt`. The engine depends on nothing, so it can be
tested, fuzzed and embedded on its own. No `lune-*` crate depends on an app crate (`luneterm*`).
`scripts/check-deps.sh` reads `cargo metadata --locked` and fails on any package outside the
workspace, crates.io and that one luneui-rs rev (`[patch]`, source replacement and branch or tag
pins included); CI runs it and builds with `--locked`.

## lune-ui (toolkit)

The app and `lune-termview` use **luneui-rs** (`~/Projects/luneui-rs`, `lune-ui` 0.2.0: `core`,
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
  OSC 133 marks with the synchronized-update latch. They are handlers on the parser's own registry
  (no second dispatch path); an extension that registers CSI `t` bypasses the window-option guard
  (`extension.rs`).
- **Events** the host receives from `drain_events`: `CwdChanged(String)`, `Progress(Progress {
  state, percent })`, `CommandMark(CommandMark { kind, exit_code })`, `SyncUpdateInCommand`,
  `ClipboardWrite(String)` and `UnknownOsc { ident, len }`. `Modes` gains `kitty_keyboard_flags`
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
  viewport cells and `Command::CopySelection`; an Update carries `selection` (buffer coordinates,
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
  `CommandMark` and `SyncUpdateInCommand` (M7, standard extensions only). Without
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
  `CommandMark` is exact; `UnknownOsc` is capped at 64 per drain (`UNKNOWN_OSC_LIMIT`); each
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
  installs the standard extensions. An all-zero config is the safe default: 2 x 1, no scrollback,
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

`crates/luneterm` is lib + bin (`#![forbid(unsafe_code)]`; the Windows calls live in `lune-pty`). The
bin is `luneterm::app::run()` after `lune_pty::restrict_dll_search()`. One session per tab and no
global state (M10: a `Tab` owns a session and everything that used to be per-session on `App`). Modules:

| Module | Holds |
|---|---|
| `app` | winit event loop (user event = session wake), window, frame loop, session wiring, bell, exit |
| `shell` | Layout (34 px title band, card radius 8, margins 10, body padding 10), the Windows chrome (caption buttons, drag, double-press maximize, edge resize), scrollbar thumb geometry and drag |
| `render` | `Renderer`: one `Ui`, `TermView`, `FrameRenderer` and `TextCache`, shared by the window and by dump mode |
| `session` | The three-thread session below, `Update`, `apply_update`, the fake PTY and the protocol tests |
| `input` | `keys` (winit `KeyEvent` to `KeyRoute`), `shortcuts`, `pointer` (mouse, wheel) |
| `spawn` | Shell candidates, cwd, environment, `WindowsPty` |
| `shell_integration` | The scripts the user's shell sources (OSC 7, OSC 133), their checked install under the config directory, and what they add to a spawn (see "Shell integration" above) |
| `tabs` | `Tab` (one session plus its UI-side state), `TabSet` (order, active, next focus after a close: pure list logic), the close rules (`asks_on_close`, `ClosePlan`), tab names |
| `links` | `scan` (OSC 8 runs and bare `http(s)://` text), `open` (the closed URL grammar and the only code that opens anything), hover state |
| `persist` | `settings.json` and `workspace.json`: bounded JSON reader and writer, atomic 0600 writes, versioned, `.bak` on a file it cannot read, `SaveGuard` (ledger 12) |
| `cwd` | The host's check of a directory a shell reported (`syntax`, `validated`) |
| `outline`, `menus`, `palettes`, `settings_page` | The running-outline gate and command tracker; the app and terminal menus as data; the ten palettes (saved by name); the settings page model and its one draw function |
| `title`, `settings`, `fonts`, `dump`, `demo` | Title sanitizer; the settings model (Lunegit's `TerminalSettings` defaults; every field has a `SettingId` and an `Effect`); the eight bundled faces plus system fonts; `LUNETERM_DUMP_FRAMES`; the dump mode's placeholder grid |

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

**Mode 2026 (D10, ledger 14).** While the mode is set the engine publishes nothing, until the mode
is reset or 1000 ms after it was first seen (xterm.js `RenderService.ts`). Unlike xterm.js the
session does not clear the engine's mode on timeout, because that needs bytes injected into the
engine's input, which can land inside a PTY sequence. **Deviation:** after a timeout DECRQM 2026
still reports "set" until the program resets the mode.

**Input routing (D4 to D7).** The terminal owns the keyboard unless the app has opened a toolkit
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

**Tabs (M10 step 2, D1, D7, D13).** The strip is the toolkit's; the app decides cues and when to ask.
New, close, close others, switch, rename, reorder and duplicate go through `TabSet`, which is tested
without a window. Shortcuts (`input/shortcuts.rs`): macOS Cmd+T, Cmd+W, Cmd+Shift+[ and ], Cmd+1 to
9, Cmd+C, Cmd+, ; elsewhere Ctrl+Shift+T, Ctrl+Shift+W, Ctrl+Tab, Ctrl+Shift+Tab, Alt+1 to 9,
Ctrl+Shift+C, Ctrl+, . Closing a tab asks only when something would be lost (`tabs::asks_on_close`):
when the foreground probe knows, if a program other than the shell runs; when it cannot (Windows),
only if the shell-integration marks say a command runs (D7), so an idle tab never asks. The
engine thread probes the foreground process and the line discipline (`raw_mode`) at most every
second when idle and every 250 ms while output flows or a command runs, and publishes the result in an `Update` only
when it changed (D3); the UI never touches the `Pty`. The running-outline cue is
`outline::should_show` (a port of Lunegit's `TerminalRunningOutlineGate`) over `CommandTracker`,
which follows the OSC 133 marks and whether the program consumes keystrokes; progress (OSC 9;4)
sets the cue's tone. `scripts/cpu-sample.sh` samples a process's CPU for the outline's budget
(ledger 16).

**Persistence (M10 step 5, D5, D17, ledger 12).** `<config dir>/Luneterm` (`dirs::config_dir()`;
`LUNETERM_CONFIG_DIR` overrides it, as the dump mode and the trial runs do) is created 0700 on
Unix and holds `settings.json` and `workspace.json`, both `{"version": 1, ...}`. Both are read
through the toolkit's bounded JSON reader and written by `persist/json_write.rs`; a write creates a
uniquely named 0600 file with `create_new`, syncs and renames it over the target, so a symlink or a
file already at the temporary name is never followed. A missing file is the defaults and writes
nothing; a file that is not JSON, over the limits, not an object or of an unknown version is copied
to `<name>.bak` (a backup is never overwritten), the defaults run, and the file stays until the user
changes something; a file over the size cap or not a regular file is not read and not saved over.
Enums, the font and the palette are saved by name, never by index. Nothing read from either file is
executed; the one value that names a program, `shell`, is the user's own setting, and the workspace
holds directories and titles only (each directory goes through `cwd::validated` before a tab
starts in it, at most 32 tabs). `SaveGuard` is ledger 12: no workspace write while any tab shows
a failed start, nor after a tab that died within 3 s of its start until the user acts on the tabs.

**Selection and links** are described with the host duties above (engine-thread selection
model; the closed URL grammar and the argv or `ShellExecuteW` open).

**Spawn (D11, D12).** Shell: the setting if set, else the Unix login shell (`program: None`, as
Lunegit's `$SHELL -l`); on Windows `pwsh.exe`, then `powershell.exe`, then `%ComSpec%`. A candidate
that is not found moves on to the next; any other error stops. cwd: the setting if it is an existing
directory, else home. Environment: Lunegit's overlay with `TERM_PROGRAM=Luneterm` (iTerm2 spoofing
behind the setting), and when none of `LC_ALL`, `LC_CTYPE`, `LANG` is set (an empty value counts as
unset) `LC_CTYPE=UTF-8` on macOS or `C.UTF-8` on Linux, because a Finder-launched shell would
otherwise run in the C locale. When the child exits the window closes and the app quits; if no
candidate spawns the grid shows "Failed to start terminal" with the error kind and stays open.

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
renamed. On Windows the directory is under `%APPDATA%`, whose user-only default ACL is assumed
(plan assumption A3), and only PowerShell is integrated. `shell_integration = false` leaves the
environment and argv exactly as before (a test pins it).

**Environment (M10).** Read by the app: `LUNETERM_CONFIG_DIR` (replaces the platform config
directory; read once, in `app::run`), `LUNETERM_DUMP_FRAMES` (dump mode, no window) with
`LUNE_REQUIRE_GPU` (a missing adapter becomes a failure), `SHELL` (the login shell's name when
integration needs one), `ZDOTDIR` and `HOME` (where the user's zsh files are), `LC_ALL`, `LC_CTYPE`
and `LANG` (the locale fallback). Added to the child's inherited environment: `TERM=xterm-256color`,
`COLORTERM=truecolor`, `TERM_PROGRAM` and `TERM_PROGRAM_VERSION` (`Luneterm` and the crate version,
or `iTerm.app` and `3.6.11` behind `report_iterm2`), `LC_CTYPE` when no locale variable is set, and,
for zsh with shell integration on, `ZDOTDIR` (ours) and `LUNETERM_USER_ZDOTDIR` (the user's). The
Windows child gets the inherited environment plus these, not portable-pty's registry merge (Q7).

**Title and bell (D13, D14).** The engine keeps titles up to 10 MB; the app scans at most 4,096
characters, drops Unicode Cc and the bidi controls (U+061C, U+200E, U+200F, U+202A to U+202E,
U+2066 to U+2069), trims, caps at 80 characters, and uses "Luneterm" when empty; the result goes to
the window title and the title band (a 10 MB title takes under 10 ms in the test). A bell asks for
user attention (`Informational`) only while the window is unfocused, at most every 2 s.

**Fonts and look (D15, D16; M10 D9a).** The eight bundled faces live in `crates/luneterm/assets`:
RznMono 0.007 Regular, Bold, Italic and Bold Italic (the terminal font, real faces for every
style termview asks for), RobotoMono Nerd Font Mono Regular and Bold (the fallback for the Nerd
icons RznMono does not have until its stage 4, and a terminal font option) and RznSans Regular and
Medium, with their licences. luneui-rs's `TextCache` loads no system fonts, so the app loads them once at start
(`fonts::text_cache`, 59 to 64 ms on the Apple M1 Pro) for emoji and CJK fallback. Cell
height is `ceil(natural_line_height)`: 17 px at scale 1 and 34 at scale 2 for the bundled face at
13 px. At 1040 x 660 the body is 1000 x 596 at (20, 44), a 125 x 35 grid. Glass: `near_black::MATERIAL`
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
- *Binary:* macOS arm64 release 11,495,056 bytes, the Windows exe 13,858,304 (STATE.md has the table
  and the fonts' share).
- *Windows CI* (run 42477613074 at b535456): the e2e harness ran 10 cases with 0 failures; E2 to E5
  and E9 need a Unix tty (`stty`) and skip there; the GPU initialized on the runner.

**M10 closed what M9 left:** tabs, selection and copy, the settings file and page, the kitty reset
and the consumers of cwd, progress and marks, and the Windows GUI run (claude-vm-win, step 8; the
results are in `docs/luneui-rs-handoff.md`). Real IME typing is still verified only by hand (no
input method was installed on the VM).

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
4. Every crate sets `license` (inherited from the workspace: MIT OR Apache-2.0).

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
