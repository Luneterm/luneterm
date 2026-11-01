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

## lune-pty

`Pty` / `PtyBackend` (`crates/lune-pty/src/lib.rs`) and the one backend, `PortablePtyBackend`
(`portable.rs`; `unix.rs` and `windows.rs` hold the platform parts). `Pty` has `read`, `write_all`,
`resize`, `try_exit_code` and `kill`, plus `take_reader()`, `writer()` (a cloneable `PtyWriter`),
`process_id()`, `raw_mode()` and `foreground_process()`.

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
  M9 session writes to the PTY); `lune-pty` does not, since
  a second answer would be injected input. Tests and the ConPTY harness answer it in their reader.
- **Unix EOF** arrives only after every holder of the slave closes it, so a background job can
  delay it past the shell's exit.

**DLL search hardening (decision D4, Windows).** portable-pty loads `conpty.dll` by bare name. The
first spawn therefore calls `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` once
(`windows.rs`), and spawn fails if the call fails. The effect is process-wide and permanent: later
bare-name `LoadLibrary` calls skip the current directory and `PATH`. For M9: check whether wgpu's
DX12 backend loads `dxcompiler.dll` (or anything else) by bare name from `PATH`; it is unverified
whether it does. `crates/lune-pty/tests/dll_search.rs` proves the plant is not loaded after a spawn.

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
`docs/porting-xterm.md`). `engine.rs` holds the host-facing types, which are original.

**Composition.** `Terminal` (`headless/terminal.rs`) wraps one `CoreTerminal`. `CoreTerminal`
holds `parser: EscapeSequenceParser<InputHandler>` and `input_handler: InputHandler`, side by
side so their borrows are disjoint. `InputHandler` is the parser's host (`ParserHost`) and owns
`CoreServices`: options, buffer (`BufferService`, with both buffers), log, core, mouse, unicode,
charset and OSC links, plus upstream's private state, the event queue and the damage. Handlers
are closures registered once by `InputHandler::register_handlers`; `Extension`s register on the
same parser afterwards and run first (newest first).

**Write path.** `Terminal::write(&[u8])` calls `InputHandler::parse`, which decodes UTF-8
(`Utf8ToUtf32`, streaming), splits input into 131072-byte chunks and feeds the parser. It is
synchronous: when it returns, every byte has been applied. Nothing in `lune-vt` blocks, sleeps,
spawns or calls back.

**Public API (`VtEngine`, `engine.rs`).** `columns`, `rows`, `write`, `resize` (clamped to at least
2 x 1), `scroll_lines`, `scroll_to_top`, `scroll_to_bottom`, `drain_events`, `take_damage`,
`viewport` and `modes`; `Terminal` implements it and adds upstream's headless surface (`new`,
`write_str`, `input`, `reset`, `clear`, `add_marker`, `set_option`, `read_row`, `link_uri`,
`idle`, `set_logger`, `unicode_mut`, `register_*_handler`, `add_extension`, `buffers`, `core`).
The input encoders (key and mouse encoding) are M7.

- **Events** (`HostEvent`) are queued while `write` runs, in the order upstream fires them, and
  drained by the host: `Response(Vec<u8>)` (DA, DSR, DECRQM, DECRQSS replies and `input`; two
  responses are never merged), `Bell`, `TitleChanged`, `Color(Vec<ColorRequest>)` (OSC 4, 10-12,
  104, 110-112; the engine never answers a colour report, the host knows the theme),
  `FocusReportRequested`, `WindowReport(WindowReportKind)`, `A11yText` and `A11yTab` (only with
  `screen_reader_mode`), and `UnknownOsc` and `ClipboardWrite`, which the core never produces
  (M7). OSC 52, queries included, reaches only the fallback that logs (ledger row 13).
- **Bounded queues** (user decisions Q10 and Q11). `TitleChanged` has no payload and at most one
  is queued per drain (read the title from `Viewport::title`, an `Arc<str>` shared with the
  engine, so a snapshot never copies it); `A11yText` merges consecutive characters and stops at
  `A11Y_TEXT_LIMIT` (2048 bytes) per drain. Everything else is exact. With these, the events
  of one drain weigh at most 16 units per byte written plus 4 KiB (checked on every fuzz seed,
  `engine_fuzz.rs` `event_cost`: a payload byte weighs 1, a payload-free event 1); without
  them a few bytes of XTWINOPS 22/23 or REP queued gigabytes. The weight is not the memory: a
  queued `HostEvent` is 48 bytes, so 1 MiB of BEL holds about 48 MiB until drained (measured,
  48.5x). M9 sizes its drain cadence for about 50x the bytes of one write.
- **Work per input byte** (fix rounds 1 and 2 of the M6 review). Where upstream's cost per
  byte grows with a count the input controls, the engine caps or batches it, each in a
  `// LUNETERM:` block: `OscLinkService` checks the link's last marker before walking all of
  them (printing inside a link open across the buffer is no longer O(scrollback) per
  character), keeps at most `MAX_LINKS` (4096) links and `MAX_LINK_MARKERS` (8192) link markers,
  evicting the least recently extended link (its cells keep their attributes and lose the
  link), disposes an evicted link's markers in one walk per buffer and filters a link's marker
  list once per disposal batch (ED 3 and RIS after a buffer-long link are linear, not
  quadratic); IL, DL, SU and SD move the markers once per sequence (`splice_rotate`), not once
  per shifted line; a cell's combined string holds at most `MAX_COMBINED_CODE_POINTS` (64,
  `buffer_line.rs`); REP prints only the repetitions that still change the state
  (`rep_equivalent_repeats`, tested equal to the full print), and a single character that
  prints alike fills whole lines (`print_repeated`). No log line carries pty output or user
  input: unknown OSC and DCS log their identifier and payload length, the parse and the
  data/binary events log lengths only.

  **Residual, accepted (not capped):** every line change (a scroll, a trim, one IL/DL/SU/SD
  sequence, a print onto a new line inside an open link) walks the buffer's live markers, as
  upstream's per-marker listeners do. Link markers are bounded by `MAX_LINK_MARKERS` plus the
  link being extended, which can mark every line (rows + scrollback), so the walk is
  O(`MAX_LINK_MARKERS` + rows + scrollback) per line change. Measured (release, 80x24,
  `a CR LF` inside one link open across the buffer): about 0.55 µs per byte at scrollback 1,000
  and 5.4 µs at 10,000, against about 30 ns with no link; one IL with 8192 markers outside the
  region costs about 7 µs at 80x24 and 39 µs at 300x100. So the work of a chunk is
  O(bytes x (rows + scrollback)) with a long link, not O(bytes): **M9's flow control must bound
  the work per slice (time or line changes), not only the bytes.**
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

**What the host (M9's session) still owns.**
- Drain after every write (`drain_events`, `take_damage`): the queues hold one write's output.
- Write every `Response` to the PTY (`lune-pty`'s `PtyWriter`), and answer `Color` reports,
  `FocusReportRequested` and `WindowReport` from its theme, focus state and cell metrics.
- Call `Terminal::idle()` while it returns `true` when nothing else is happening (the
  replacement of `IdleTaskQueue`: the buffer's deferred memory cleanup after a shrink).
- Flow control: feed bounded chunks from the reader thread and decide when to stop reading. The
  engine's `write` has no watermark or time slicing (`WriteBuffer` is replaced, D3).
- Choose the Unicode version (`unicode_mut()`; V11 is registered and V6 stays active at
  construction) and install a `Logger` if it wants logs (default: discarded).

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

## Extension points

| Point | Mechanism | Example |
|---|---|---|
| Escape-sequence behaviour | `lune_vt::Extension<C>` (`Terminal::add_extension`) registers handlers on the `EscapeSequenceParser<C>` itself (newest-first, like xterm.js `register*Handler`); there is no separate registry | kitty keyboard, XTVERSION, OSC 7 cwd, OSC 9;4 progress |
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
