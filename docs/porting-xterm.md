# Porting xterm.js

Pinned: xterm.js **6.0.0**, commit `f447274f430fd22513f6adbf9862d19524471c04`
(`lune_vt::XTERM_JS_COMMIT`). Fetch the source with `scripts/fetch-xterm-ref.sh` into
`reference/xterm.js` (gitignored). Licence: MIT — `licenses/xterm.js-LICENSE` must ship with the app.

## Rules

1. **Tests first.** Port `Foo.test.ts` to Rust tests, watch them fail, then port `Foo.ts` until
   they pass. A module is done when all of its upstream tests pass. Keep test names recognisable
   (`describe`/`it` text → `snake_case` fn names).
2. **Mirror the layout.** `src/common/parser/Params.ts` → `crates/lune-vt/src/parser/params.rs`.
   One TS file → one Rust file. Each file starts with
   `//! Port of xterm.js src/common/<path> @ f447274.`
3. **Port behaviour, not idioms.** Keep algorithms, constants and edge-case handling identical;
   replace the DI container, event emitters and disposables with owned structs and returned events.
4. **No behaviour changes inside ported files.** Fixes and Lunegit additions go in extensions or in
   clearly marked `// LUNETERM:` blocks with a reason, so upstream diffs stay mechanical.
5. **Differential tests.** For anything beyond unit level, generate expected output with
   `tools/xterm-oracle` (real xterm.js) and compare. Add Lunegit's 27 tmux conformance vectors
   (`Lunegit.Tests/Terminal/IVtEngineConformanceTests.cs` + `TestData/vt-goldens`) early.
6. **Upstream updates.** Bump the pin deliberately: diff `src/common` between tags, port each change
   with its test, update the commit constant and this file.

## Suggested order

1. `parser/`: Constants, Params, OscParser, DcsParser, EscapeSequenceParser.
2. `input/`: UnicodeV6 (+ unicode11 tables), TextDecoder, XParseColor.
3. `buffer/`: AttributeData, CellData, BufferLine, CircularList, BufferReflow, Buffer, BufferSet.
4. `services/`: Options, Unicode, Charset, Buffer, Core, CoreMouse, OscLink.
5. `InputHandler.ts` and `CoreTerminal.ts`: wire everything; run the oracle and tmux vectors.
6. Lunegit additions as extensions: `driver.js` features, `XtermVtInputEncoder`, OSC scanners.

## Running the oracle

```bash
cd tools/xterm-oracle && pnpm install
node oracle.mjs <cols> <rows> <input-file> [resizeCols resizeRows]   # JSON screen dump
scripts/regen-vt-goldens.sh      # regenerate crates/lune-vt-conformance/goldens (needs node + pnpm)
```

The goldens are committed, so `cargo test` never needs Node. `regen-vt-goldens.sh` runs
`tools/xterm-oracle/gen-goldens.mjs` against the pinned `@xterm/headless` 6.0.0 (parser-trace
mode: a fresh upstream `EscapeSequenceParser` + `Utf8ToUtf32`; screen mode: attributes, modes,
responses, title, links), then self-checks Lunegit's 27 vectors. It is deterministic: a second run
leaves `git diff -- crates/lune-vt-conformance/goldens` empty. Formats, the divergent-golden rule
and the runners: `crates/lune-vt-conformance/FORMAT.md`.

## Conventions the parser port established

- **Tests live in a sibling file (plan M4 D7).** `foo.rs` + `foo_tests.rs`, included as
  `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`, so tests keep private access (upstream's
  `TestEscapeSequenceParser` reads protected state) without a 3,000-line source file. Test names
  follow rule 1.
- **No async handlers (D3).** The `_parseStack` / `Promise` machinery
  (`EscapeSequenceParser.ts`, `ISubParserStackState`, `ParserStackType`, `ResumableHandlersType`)
  is not ported; handlers are synchronous and return `bool` (the engine's `write` is synchronous).
  Upstream's async tests are recorded as "replaced: no async handlers" in the test file's header;
  their sync-ordering cases stay.
- **Handler, logger and mouse-encoder boxes are `Send` (M8 D1/D2).** Every boxed handler alias
  (`parser/types.rs`), `Box<dyn DcsHandler/OscHandler>`, `Box<dyn Logger>` and the
  `CoreMouseEncoding`/`restrict` boxes carry `+ Send`, and so do the `impl Fn*` parameters that
  feed them, so a `Terminal` can move between threads (the C ABI hands it to arbitrary host
  threads). Signature only: no behaviour changed in any ported file. `Sync` is not required. A
  compile-time assertion in `headless/terminal.rs` keeps `Terminal: Send`; tests capture state
  with `Arc<Mutex<_>>`/atomics, not `Rc`.
- **`throw` becomes `Result::Err`, never a panic (D5).** `IdentifierError` for `_identifier`,
  `ParamsError` for `Params`. `StringToUtf32` (UTF-16 input) is "replaced: Rust input is bytes";
  a `&str` goes through `Utf8ToUtf32`.
- **No-panic gate.** `parser/mod.rs` denies `clippy::indexing_slicing`, `unwrap_used`,
  `expect_used`, `panic`, `unreachable`, `todo`, `unimplemented` outside tests. Every slice taken
  from input is `.get(..)`; arithmetic that cannot overflow in JS saturates.
- **Handler API (D1).** The parser is generic over the host state; see `docs/architecture.md`
  ("lune-vt parser"). Extensions register on the parser (`lune_vt::Extension`), not beside it.
- **Deliberate divergence (D4).** `Utf8ToUtf32` tracks the interim byte count explicitly instead
  of upstream's `& 0x3F` sentinel, which drops a character split after a `0x80` continuation byte
  (`E2 80 | 99` loses U+2019). A `// LUNETERM:` block in `input/text_decoder.rs`; whole-buffer
  decoding is byte-for-byte upstream. Goldens whose upstream output depends on chunking are
  marked `divergent` and expect the unsplit trace.
- **Deliberate divergence (user decision Q8, ledger row 18).** xterm.js 6.0.0 drops a DEL (0x7F)
  that is the first code after a DCS hook or starts a chunk inside a DCS payload, and keeps one
  anywhere later: the table maps DEL to IGNORE in DCS_PASSTHROUGH, but the DCS_PUT read-ahead
  loop includes it. `EscapeSequenceParser::parse` has a `// LUNETERM:` block that turns that one
  table lookup into DCS_PUT, so the payload keeps DEL however the host splits its writes. Input
  that has no DEL at such a position parses exactly as upstream. The other read-ahead loops
  (print, OSC put, params) match their table entries code for code;
  `every_code_in_every_state_is_chunk_invariant` shows it for every state and code. The upstream
  test `state dcs_passthrough / ignore` is adapted (`state_dcs_passthrough_del_is_put_not_ignored`),
  and the generator marks goldens the quirk touches `divergent` (FORMAT.md).
- **Parser tests.** `cargo test -p lune-vt-conformance --test parser_trace` replays every parser
  golden chunked and unsplit; `cargo test -p lune-vt --test no_panic` is the seeded random-input
  fuzz (debug, so overflow checks fire) with chunk invariance and size bounds; `--test
  payload_limits` pins the OSC/DCS payload limit and the param limits. Fuzz knobs:
  `LUNE_VT_FUZZ_SEEDS=<n>` runs seeds `0..n` (default 1000), `LUNE_VT_FUZZ_SEED=<s>` replays one.
  `lune-vt` has no dev-dependencies: `no_panic.rs` includes the harness's `trace.rs` and
  `escape.rs` by `#[path]`.

## Conventions the buffer port established

- **Events become an observer or return values (M5 D1).** `CircularList<T, O: ListObserver>`
  calls `on_trim` / `on_insert` / `on_delete` synchronously at each upstream `fire`;
  `Buffer.lines` is `CircularList<BufferLine, MarkerSet>`, so markers move exactly where upstream
  moves them. Every other emitter (`onBufferActivate`, `onResize`, `onScroll`, `onOptionChange`,
  `onChange`) is a return value, produced exactly when upstream fires. Marker disposal is a log
  the host drains (`Buffer::take_disposed_markers()`).
- **Lazy storage (D2).** `CircularList` is a `VecDeque<Option<T>>` that grows on demand, never
  preallocated to `rows + scrollback`; `get(i >= len)` is `None`; upstream `throw`s are
  `Result::Err`. `shift_elements` moves elements and clones only into the vacated slots whose
  values upstream's aliasing leaves there.
- **Typed options passed by reference (D3).** `TerminalOptions` has the 21 options the core
  reads; methods that read `rawOptions` take `opts: &TerminalOptions` (a live read, as upstream).
  Renderer and browser options are not ported.
- **JS value semantics in `BufferLine` (D7).** Out-of-range `_data` reads give 0 and writes are
  no-ops (a `Uint32Array`); `_combined` and `_extendedAttrs` are ordered maps whose writes are as
  unchecked as JS object writes (both are stored as runs of keys with one value, `RunMap`, the
  same entries: review S7 round 4 for `_extendedAttrs`, M7 step 12 for `_combined`, whose values
  are shared `Arc<str>`; the public `String` accessors are unchanged); `translate_to_string`'s `out_columns` index UTF-16 code units.
  Upstream quirks (`copyCellsFrom` copies every combined key `>= srcCol`) are kept.
- **Preconditions, not guards (D8).** `cols >= 2` and `rows >= 1` are the caller's job
  (`CoreTerminal.ts:175-176`); upstream locks up at one column and so would a guard-less port.
- **Idle work is driven by the host (D5).** `Buffer::batched_memory_cleanup()` replaces the
  `IdleTaskQueue`; `lune-vt` has no timers.
- **No-panic gate.** As the parser's, plus `clippy::string_slice`; arithmetic that can go
  negative in JS is computed in `i64` (D9).
- **Buffer tests.** `cargo test -p lune-vt` runs the ported upstream tests (Buffer, BufferLine,
  BufferReflow, BufferSet, BufferRange, CircularList, Color, UnicodeV6, OptionsService,
  UnicodeService, OscLinkService) and `tests/buffer_ops.rs`, the seeded random-operation test
  (cursor, resize both ways, scroll, splice, trim, cell edits, markers, links, alt buffer,
  reflow). Knobs: `LUNE_VT_BUFFER_FUZZ_SEEDS=<n>` (default 300), `LUNE_VT_BUFFER_FUZZ_SEED=<s>`
  replays one. Longer run: `LUNE_VT_BUFFER_FUZZ_SEEDS=5000 cargo test -p lune-vt --release
  --test buffer_ops`. Two differentials against test-only copies of replaced code (M7 steps 11
  and 12, below) have their own knobs: `LUNE_VT_MARKER_DIFF_SEEDS=<n>` / `LUNE_VT_MARKER_DIFF_SEED=<s>`
  (default 200 seeds) and `LUNE_VT_COMBINED_DIFF_SEEDS=<n>` / `LUNE_VT_COMBINED_DIFF_SEED=<s>`
  (default 200 seeds).

## Conventions the engine port established

- **Host layout (M6 D1).** The parser is generic over a `'static` host (`EscapeSequenceParser<C: ParserHost + 'static>`), so `InputHandler` is the host: it owns `CoreServices` (options, buffer, log, core, mouse, unicode, charset, osc_link) and upstream's private fields (attributes, titles and their stacks, `DirtyRowTracker`, `preceding_join_state`, the parse buffer, the event queue, the damage). `CoreTerminal { parser, input_handler }` holds the parser beside it, so borrows are disjoint; `InputHandler::parse(&mut self, parser, data)` takes the parse buffer out with `mem::take` for the call. Handlers are closures registered by `InputHandler::register_handlers`. No `Rc<RefCell<_>>` anywhere.
- **Events (M6 D2).** Every upstream `fire` becomes an explicit call at the same point, a `HostEvent` pushed in fire order (`Response`, `Bell`, `TitleChanged`, `Color`, `FocusReportRequested`, `WindowReport`, `A11yText`, `A11yTab`; M7 adds `UnknownOsc`, `ClipboardWrite`, `CwdChanged`, `Progress`, `CommandMark` and `SyncUpdateInCommand`, produced only by the standard extensions), or a field of the coalesced `Damage` (refresh rows, cursor move, scroll, full redraw). `Terminal::drain_events` and `take_damage` hand them to the host; the engine has no callbacks. `onLineFeed`, `onWriteParsed`, `onRequestSyncScrollBar` and `onUserInput` have no consumer and are dropped (reasons in the file headers). Two events are coalesced because upstream hands listeners shared strings while a queue would copy them (user decisions Q10 and Q11): `TitleChanged` carries no payload and at most one is queued per drain (the host reads `Viewport::title`), and `A11yText` merges consecutive characters up to `A11Y_TEXT_LIMIT` (2048 bytes) per drain. Titles and their stacks are `Arc<str>`. The bound this buys is tested on every fuzz seed: `drain_events` carries at most 16 units per byte written plus 4 KiB (a payload byte weighs 1, a payload-free event 1; `engine_fuzz.rs` `event_cost`).
- **`WriteBuffer` is replaced (M6 D3).** `Terminal::write` calls `InputHandler::parse` directly, which still splits input into 131072-byte chunks (upstream `InputHandler.ts:471-473` splits a `Uint8Array` at 131072 bytes and a string at 131072 UTF-16 units). Time slicing, the discard watermark and callbacks are the host's: M9 feeds bounded chunks from the reader thread.
- **Logging (M6 D4).** `LogService` writes to an optional `Logger`; with none, messages are discarded without being formatted. A library never prints; upstream defaults to `console`. `Terminal::set_logger` sets the sink and `log_level` follows the option.
- **RIS skips `parser.reset()` (M6 D7).** The parser is borrowed by `parse` while the ESC handler runs, and upstream's call changes nothing: the state is GROUND right after the ESC dispatch, collect and params are cleared on the next sequence, the OSC and DCS parsers are idle, and the join state is cleared before the dispatch. A `// LUNETERM:` block in `input_handler.rs` says so; the `oracle_state_ris_*` tests (RIS followed in the same chunk by CSI, OSC and text, RIS at a chunk end, RIS inside a pending OSC) equal `@xterm/headless` 6.0.0.
- **Reset frees links (user decision Q9).** `core_reset` disposes both buffers' markers before replacing them, where upstream leaks them (links alive at a reset stay in `OscLinkService` forever). The only observable difference: an OSC 8 with the same `id=` and URI registered after RIS gets a new `url=` id instead of the old one. Deleting the two `clear_all_markers` lines restores upstream's behaviour.
- **Windows heuristics registered once (M6 D6).** `CoreTerminal::new` registers the CSI `H` handler (it returns `false`) after `InputHandler`'s, gated by a flag that `set_option` and `reset` recompute as `_handleWindowsPtyOptionChange` does. Deviation: a CSI `H` handler an extension registers before the heuristics turn on runs after the heuristic handler, not before; no extension registers CSI `H`.
- **JS throws and `undefined` (M6 D8).** One helper per access kind: `line_mut` for `lines.get(..)!` (on `None` the handler stops with its upstream return value; upstream throws and abandons the rest of the chunk, Rust carries on with the next sequence; reachable only through the M5 scrollback-0 soft invariant), `Params::get` for `params.params[i]` (stale values past `length()` are kept, `None` is `undefined`), `i64` for cursor and param arithmetic. `input_handler.rs`, `core_terminal.rs`, `headless/` and the new services carry the no-panic lint gate.
- **Bounded loops where upstream's are not.** IL, DL, SU and SD stop after as many iterations as the region has rows (further ones change nothing), CHT and CBT stop once the cursor no longer moves, and REP caps the repeated data at `MAX_REP_CODE_POINTS` (1 << 20 code points; upstream allocates up to 2^31 - 1 times the text). Of the capped repetitions REP prints only as many as can still change the state, at most a line's worth of units times `rows + max lines + 3` (review S3; `rep_equivalent_repeats` names the conditions, and where they do not hold, e.g. a joining character that moves the cursor, it prints them all); `rep_cut_to_the_buffer_leaves_the_state_of_the_uncapped_print` compares the cut REP with the full print over 25 setups and 5 sizes (300x100 included) with up to 6 counts. Without wraparound a unit with joiners is cut at `cols + MAX_COMBINED_CODE_POINTS + 3` units (a stuck wide unit stacks its joiners onto one cell until its string is full; review S7). In insert mode on a line longer than `cols` every insert also shifts the hidden tail `[cols, len)`, so the floor grows by the excess: `len - cols` more units without wraparound, and `ceil(excess / (per_line * w)) + 1` more lines when the cursor is below the scroll region, where every wrap rewrites the last row (code review F9; proof in the doc comment); `rep_cut_on_lines_longer_than_cols_leaves_the_state_of_the_uncapped_print` covers lines of `cols + 1`, `3 cols` and more cells than the floor before F9 shifts out. By default the two REP equivalence tests run 300x100 for a handful of cases and skip the largest small-size count; `LUNE_VT_REP_MATRIX=full` runs the whole matrix (code review F10). A unit that prints alike every time (a base of width 1 or 2 that never joins, joiners that keep its width, the same properties in every repetition, wraparound on; `rep_fill`) is written line by line by `print_repeated`: the same cells, side-table entries, wraps, dirty rows, join state, insert-mode shifts (one `BufferLine::insert_cells_repeatedly` per line, equal to the per-character inserts including stale side-table entries and, on a line longer than `cols`, `print`'s fix-up of a wide cell at `cols - 1`; review S7 round 4), cell writes (`BufferLine::set_cells_repeatedly`: one pass and one run of extended attributes per line), link lines (one add per line and wrap) and screen reader text as `print`; the tests assert which path each case takes (`rep_fill_leaves_the_state_of_print_over_existing_content` covers content already on screen, non-ASCII charsets, lines longer than `cols` and extended attributes; `inserting_repeatedly_equals_the_loop_of_inserts`; `setting_cells_repeatedly_equals_the_loop_of_set_cell`; `run_map_holds_the_entries_of_a_per_key_map`; `rep_inside_an_open_link_looks_up_markers_once_per_line`; reviews S3 residual and S7, user decisions Q16, Q17). IL, DL, SU and SD tell the markers about their per-line splices as one composed event (`CircularList::splice_rotate`, `ListObserver::on_rotate`, review S6): one walk of the marker list per sequence, same marker lines and disposal order (`on_rotate_equals_the_per_event_calls`, `splice_rotate_equals_the_splice_loop`). Each has a `// LUNETERM:` block; the first two are not visible in any output, REP is visible only for counts past about a million.
- **Caps on retained state (review S2, S4; user decisions 2026-11-01).** `OscLinkService` keeps at most `MAX_LINKS` (4096) links and `MAX_LINK_MARKERS` (8192) markers held by links; past either it evicts the least recently extended link (registered, or given a new line) and disposes its markers. The link being registered or extended is never evicted, so one link across the whole buffer keeps every line. Cells that carry an evicted id keep their attributes and render without a link (`get_link_data` is `None`). Upstream frees a link only when all its lines are trimmed, so 7-byte `ESC ] 8 ; ; a BEL` repeated on one line grew the table without bound. `BufferLine::add_codepoint_to_cell` stores at most `MAX_COMBINED_CODE_POINTS` (64) code points per cell and drops the rest; the width a dropped character asks for still applies, so layout matches upstream and only the text is shorter (also what REP and the renderer read). `add_line_to_link` checks the link's last marker first and otherwise scans the marker lists once (review F1/S1); the answer is upstream's, the cost per printed character no longer grows with the link's line count. An evicted link's markers are disposed in one walk per buffer, and `on_markers_disposed` filters each affected link's list once per batch (review F6; upstream's per-marker removal is quadratic for ED 3 or RIS after a buffer-long link). After 2^32 - 1 registrations the id counter wraps and skips ids still in use (review F5; upstream would overwrite the live entry), and the eviction loop removes an age key every iteration. Tests: `osc_link_service_tests.rs` (caps), `osc_8_links_repeated_on_one_line_stay_capped`, `osc_8_print_inside_a_long_open_link_looks_up_markers_once_per_line`, `print_caps_the_combining_marks_of_one_cell`, `a_wrapped_id_counter_skips_ids_still_in_use`, `enforce_caps_ends_even_when_an_age_key_outlived_its_entry`, `evicting_a_long_link_walks_the_marker_list_once`, `forwarding_the_disposal_of_a_long_link_filters_its_list_once`; the engine fuzz checks both link caps on every operation.
- **S7 residuals closed in M7 (steps 11 to 13; security-review finding `rep-slow-path-amplification`, user decisions Q18 and Q21).** Three engine-internal changes, each behaviour-identical, each proven against a test-only copy of the code it replaced, plus a timing-free cost counter and the recorded probe (`scripts/bench-rep.sh`, numbers in `docs/architecture.md`, "Work per input byte"). (1) `MarkerSet` keeps a trim offset and a line index (`by_id` map, `by_line` set of `(line + offset, id)` keys, `i128` so a marker near `i64::MAX` saturates as before): `on_trim` bumps the offset and disposes only the markers below it, and an insert, delete or rotation touches only the markers on the affected lines; `get` and `iter` return `Marker` by value. OSC link markers are a `VecDeque`, and `add_line_to_link` on a cache miss looks at `min(link markers, markers on line y)` entries. Differential: `marker_set_equals_the_reference_walk` against `ReferenceMarkerSet`, a verbatim copy of the Vec-based set (`get` for every id, `iter` order and values, `len`, every `take_disposed`; the generator aims at, and one either side of, marker lines, detached markers, lines near `i64::MAX`, trims that dispose everything); cost: `on_trim_visits_only_the_disposed_markers`, `insert_and_delete_visit_only_the_markers_from_the_index_on`, `on_rotate_visits_only_the_markers_between_the_lines`, `a_cache_miss_checks_the_fewer_of_the_link_markers_and_the_markers_on_the_line`, `rep_inside_an_open_link_over_a_full_scrollback_visits_few_markers_per_line`, `forwarding_one_trimmed_oldest_marker_is_constant_time`. (2) A cell's combined string is `Arc<str>` in a `RunMap` (`RunMap<V: Clone + PartialEq>`): a REP of a narrow cluster is one run and one allocation per line, a wide cluster one entry per head cell with no allocation. Differential: `buffer_line_equals_the_reference_combined_map` against `ReferenceBufferLine` (a verbatim copy of the per-key combined map line; data, extended attributes, per-key combined entries including stale keys and keys past `len`, and every read, after every operation of a seeded random sequence); sharing: `rep_of_a_narrow_cluster_shares_one_string_per_line`. (3) `BufferLine::stuck_insert_repeatedly` and `InputHandler::print_stuck_inserts`: with wraparound off and insert mode on, a REP of a narrow unit prints until the cursor is stuck at `cols - 1`, then applies the remaining repetitions as one batched insert of copies of the head cell instead of one O(len) insert each. The F9 floor above stays (state identity); only the cost of reaching it changed. Proof in the primitive's doc comment; `stuck_inserting_repeatedly_equals_the_loop_of_stuck_inserts` (5000 seeded rounds, whole-line equality) and `rep_cut_on_lines_longer_than_cols_leaves_the_state_of_the_uncapped_print` (asserts which path each case takes); bound: `rep_no_wrap_insert_on_a_long_line_makes_a_bounded_number_of_insert_passes` (at most `cols + 4` insert passes).
- **Logging without payloads (user decisions 2026-11-01 and 2026-11-02).** No log line carries pty output or user input. The OSC and DCS fallbacks log the identifier and the payload length, never the payload (upstream logs the data; an unhandled OSC 52 carries clipboard contents); `InputHandler::parse` logs `parsing data (N bytes)` and `CoreService::trigger_data_event`/`trigger_binary_event` log `sending data|binary (N bytes)` at debug, where upstream logs the data at debug and its code points at trace (those trace lines are dropped). `fallbacks_log_unknown_sequences`, `trace_logging_never_carries_output_or_input_payloads` (`headless/terminal_tests.rs`, a sink at `Trace` over an OSC 52 write and `input("hunter2-password\r")`) and `events_log_the_length_never_the_data` check it.
- **Differential harness (M6 D11, D12).** `lune-vt-conformance/src/screen.rs` renders a `lune_vt::Terminal` run in the oracle's screen-dump format (through `Terminal::core()` read accessors) and Lunegit's grid. `tests/screen_goldens.rs` replays every screen golden recorded, unsplit, one byte per write and with three seeded random splits; `tests/lunegit_vectors.rs` checks the 27 tmux vectors; `tests/handler_coverage.rs` checks that every registration upstream makes (50 CSI, 12 execute codes, 17 plain ESC and 112 charset designators, 12 OSC, DCS `$q`) appears in a screen golden and reaches a real handler (a capturing logger at `Debug` sees no `Unknown ... code`). During steps 4 to 7 a `PENDING` list named the goldens expected to fail and the test asserted the failing set equalled it exactly; it was empty and removed at step 8, so every golden must pass. The corpus is 166 hand cases and 200 random cases (`random 200 seed 0x5c4ee7` in `crates/lune-vt-conformance/goldens/screen/random.golden`, 9 of them marked `divergent` for the known `Utf8ToUtf32` split bug) plus the 27 Lunegit screens; the goldens stay under 1 MiB (595,886 bytes at step 9).
- **Handler tests.** `input_handler_tests.rs` ports the 169 `it` cases of `InputHandler.test.ts` (133 ported, 33 adapted, 3 replaced as async-handler cases, M4 D3; the table is in the file's header and the milestone notes), and adds oracle tables whose expected values come from running `@xterm/headless` 6.0.0 (`oracle_*`, `sgr_matches_the_oracle`, `osc_and_decrqss_equal_the_oracle`).
- **Engine fuzz.** `crates/lune-vt/tests/engine_fuzz.rs`: per seed a random terminal and 36 random operations over the public API, each under `catch_unwind`, with bounds checked after every operation (line counts, title stacks <= 10, link table against live markers, event bytes, viewport shape) and chunk invariance (1-byte and random chunkings give the same screen dump and `Viewport`). Knobs: `LUNE_VT_ENGINE_FUZZ_SEEDS=<n>` (default 60), `LUNE_VT_ENGINE_FUZZ_SEED=<s>` replays one. Known upstream behaviours it tolerates or reproduces: the alt buffer is bounded by the peak row count seen, not the current one (shrinking never trims the alt list, upstream too); with autowrap off a wide character that does not fit at the right margin gives a chunk-dependent screen (upstream too; seeds whose writes contain `CSI ? ... 7 ... l` and differ only there are counted, not failed); `ED 1` with the cursor in the last column of the last row, no scrollback, throws upstream (`lines.get(j + 1)!` lacks `ybase`, `InputHandler.ts:1218`) and here stops that handler at the same point.
- **Throughput.** `scripts/bench-vt.sh` runs `Terminal::write` against `@xterm/headless` on the same generated 16 MiB stream and prints both; the numbers are in `STATE.md`.

## Port status

One row per file of `git ls-files 'src/common/*'` at f447274, excluding `*.test.ts`: **54**
entries. "Ported" names the Rust file (under `crates/lune-vt/src/`); "replaced" says why no Rust
file exists; "pending" names the milestone that decides (none is left since M7). Update a row
when its milestone lands. The Lunegit additions (M7) are not xterm.js files and are listed under
"Ported from Lunegit" below.

| Upstream (`src/common/`) | Status |
|---|---|
| `CircularList.ts` | `circular_list.rs` (`CircularList<T, O: ListObserver>`: events become observer calls, storage grows lazily, D1/D2) |
| `Clone.ts` | replaced: a deep-copy helper; Rust `Clone` copies `Modes`, `DecPrivateModes` and the options (M6 D5) |
| `Color.ts` | `color.rs` (`css::to_color` has no canvas path; unsupported formats are `Err`) |
| `CoreTerminal.ts` | `core_terminal.rs` (the parser beside the `InputHandler`; each upstream listener is an explicit call; `WriteBuffer` replaced; Windows heuristics registered once, a `// LUNETERM:` note on handler order; reset disposes both buffers' markers, user decision Q9; M6 D1, D2, D3, D6) |
| `InputHandler.ts` | `input_handler.rs` (the `ParserHost`; owns the services; every handler upstream registers; events are a queue plus coalesced damage; bounded loops for IL/DL/SU/SD/REP, no `parser.reset()` in RIS, a missing line stops the handler instead of throwing; M6 D1, D2, D7, D8) |
| `MultiKeyMap.ts` | replaced: imported only by `src/browser/` (`ColorContrastCache`), not by `src/common` |
| `Platform.ts` | replaced: user-agent sniffing that only fed a browser option's default (`rightClickSelectsWord`) and `TaskQueue`; the option is not ported (D3, D5) |
| `SortedList.ts` | replaced: imported only by `DecorationService` (M6 D5) |
| `TaskQueue.ts` | replaced: `Buffer::resize` sets a pending flag and the host drives `Buffer::batched_memory_cleanup` when idle (D5) |
| `TypedArrayUtils.ts` | replaced: imported only by its own test; Rust has slices and `Vec` |
| `Types.ts` | replaced: interfaces; the Rust structs are the types, `Modes` and `DecPrivateModes` live in `services/core_service.rs`, the host-facing types in `engine.rs` (M6 D5) |
| `WindowsMode.ts` | `windows_mode.rs` (`update_windows_mode_wrapped_state`; a missing line leaves the state untouched) |
| `buffer/AttributeData.ts` | `buffer/attribute_data.rs` (`AttributeData`, `ExtendedAttrs` as a `Copy` value) |
| `buffer/Buffer.ts` | `buffer/buffer.rs` (including the column reflow; deviations listed in its header) |
| `buffer/BufferLine.ts` | `buffer/buffer_line.rs` (`Vec<u32>` storage with typed-array edge semantics, D7; at most 64 code points per combined cell, review S4; extended attributes and combined strings stored as runs, review S7 round 4 and M7 step 12; batched stuck insert for REP, M7 step 13) |
| `buffer/BufferRange.ts` | `buffer/buffer_range.rs` |
| `buffer/BufferReflow.ts` | `buffer/buffer_reflow.rs` |
| `buffer/BufferSet.ts` | `buffer/buffer_set.rs` (`onBufferActivate` is a return value, D1) |
| `buffer/CellData.ts` | `buffer/cell_data.rs` |
| `buffer/Constants.ts` | `buffer/constants.rs` |
| `buffer/Marker.ts` | `buffer/marker.rs` (`MarkerId`, `MarkerSet`: the three `addMarker` listeners, D1/D4; a trim offset and a line index instead of a walk of every marker per event, same marker lines and disposal order, M7 step 11) |
| `buffer/Types.ts` | replaced: interfaces; the Rust structs are the types (D6) |
| `data/Charsets.ts` | `data/charsets.rs` |
| `data/EscapeSequences.ts` | `data/escape_sequences.rs` |
| `input/Keyboard.ts` | replaced: it maps DOM keyboard events (`keyCode`, `key`, `isMac`, `macOptionIsMeta`), which the input API rejects (roadmap D8); superseded by `input/key_encoder.rs` and `input/legacy_key_encoder.rs` (Lunegit's port, "Ported from Lunegit" below), cases adapted. Its 59 test cases are listed in the header of `input/key_encoder_tests.rs` and run as `keyboard_ts_*`: 57 adapted to `TerminalKeyEvent` (in three of them, Alt+Esc and the `Space` key with Alt or Ctrl+Alt, Lunegit's encoder disagrees with `Keyboard.ts` and the test pins Lunegit's bytes, M7 D4) and 2 replaced as DOM-only (mac Alt+a is `undefined`; mobile arrow events) |
| `input/TextDecoder.ts` | `input/text_decoder.rs` (`Utf8ToUtf32` with the D4 fix; `StringToUtf32` replaced, D5; `stringFromCodePoint` and `utf32ToString` ported) |
| `input/UnicodeV6.ts` | `input/unicode_v6.rs` |
| `input/WriteBuffer.ts` | replaced: `Terminal::write` is synchronous; time slicing, the discard watermark and callbacks are the host's job (M6 D3). Its test cases that are not about scheduling are `Terminal::write` tests in `headless/terminal_tests.rs` |
| `input/XParseColor.ts` | `input/x_parse_color.rs` |
| `parser/Constants.ts` | `parser/constants.rs` |
| `parser/DcsParser.ts` | `parser/dcs_parser.rs` (no async handlers, D3) |
| `parser/EscapeSequenceParser.ts` | `parser/escape_sequence_parser.rs` (no async handlers, D3) |
| `parser/OscParser.ts` | `parser/osc_parser.rs` (no async handlers, D3) |
| `parser/Params.ts` | `parser/params.rs` |
| `parser/Types.ts` | `parser/types.rs` (`IParams`, `ISubParser`, `IOscParser`, `IDcsParser`, `IEscapeSequenceParser` are the concrete types; stack-state types not ported, D3) |
| `public/AddonManager.ts` | replaced: no JS addons; `lune_vt::Extension` through `Terminal::add_extension` (M6 D5) |
| `public/BufferApiView.ts` | replaced: the viewport read API, `Terminal::viewport` and `Terminal::read_row` (M6 D5) |
| `public/BufferLineApiView.ts` | replaced: `Terminal::read_row`, one `ViewportCell` per column (M6 D5) |
| `public/BufferNamespaceApi.ts` | replaced: `Terminal::buffers()` returns the `BufferSet` (M6 D5) |
| `public/ParserApi.ts` | replaced: `Terminal::register_{csi,esc,osc,dcs}_handler` and `dispose_handler`, with the `t` window-option guard; `lune_vt::Extension` is the extension point (M6 D5) |
| `public/UnicodeApi.ts` | replaced: `Terminal::unicode_mut()` returns the `UnicodeService` (M6 D5) |
| `services/BufferService.ts` | `services/buffer_service.rs` (`onResize` and `onScroll` are return values, D1) |
| `services/CharsetService.ts` | `services/charset_service.rs` |
| `services/CoreMouseService.ts` | `services/core_mouse_service.rs` (`consume_wheel_event` also takes `rows`; `trigger_mouse_event` returns a `MouseReport` with a `Suppressed` case; the protocol-change event is a return value) |
| `services/CoreService.ts` | `services/core_service.rs` (`trigger_data_event` and `trigger_binary_event` return the payload, `None` when `disable_stdin`) |
| `services/DecorationService.ts` | replaced: renderer-side, unused by the core and the headless terminal; `lune-termview` and the app own decorations (M6 D5) |
| `services/InstantiationService.ts` | replaced: DI container; Rust services are plain structs (rule 3) |
| `services/LogService.ts` | `services/log_service.rs` (level gate; an optional `Logger` sink, default discard; arguments are formatted only when the level allows; `setTraceLogger`/`traceCall` not ported; M6 D4) |
| `services/OptionsService.ts` | `services/options_service.rs` (typed `TerminalOptions` with the 21 options the core reads, D3) |
| `services/OscLinkService.ts` | `services/osc_link_service.rs` (marker disposal arrives through `on_markers_disposed`, D1; link and marker caps with eviction, a cached last marker in `add_line_to_link`, review S2 and F1/S1) |
| `services/ServiceRegistry.ts` | replaced: DI registry; Rust services are plain structs (rule 3) |
| `services/Services.ts` | replaced: DI service interfaces; the Rust structs are the interfaces (rule 3) |
| `services/UnicodeService.ts` | `services/unicode_service.rs` (`onChange` is the return value of `set_active_version`) |
| `tsconfig.json` | replaced: TypeScript build configuration |

## Ported outside `lune-vt`

Files from other parts of the xterm.js tree follow the same rules (marker line, tables and
comments in upstream order, deviations only in `// LUNETERM:` blocks), but live in the crate that
uses them and stay flat. In `lune-vt` that means `src/addons/` and `src/headless/` only.

| Upstream file @ f447274 | Rust file | Deviations |
|---|---|---|
| `addons/addon-webgl/src/CustomGlyphs.ts` (box drawing, blocks, shades, legacy computing, powerline) | `crates/lune-termview/src/custom_glyphs.rs`, drawn through the original `glyph_canvas.rs` | Stroke snap generalised for even line widths; pattern fill on a colourless mask; `has_custom_char`; NaN coordinates skip their instruction. Upstream has no test file, so the tests are original. |
| `addons/addon-unicode11/src/UnicodeV11.ts` (wcwidth tables and `UnicodeV11` provider, addon 0.9.0) | `crates/lune-vt/src/addons/unicode_v11.rs` | None. Tables generated from the TS file; the Playwright test is not portable, so `wcwidth_v11_emoji_test` (10 x U+1F923 = 20 cells, against 10 for V6: `v6_is_ten_before_activation`) and `version_is_11_and_registered_after_6` in `addons/unicode_v11_tests.rs` cover it. |
| `addons/addon-unicode11/src/Unicode11Addon.ts` | replaced | Activation is `UnicodeService::register(Box::new(UnicodeV11::new()))` followed by `set_active_version("11")` (M5 D6). |
| `headless/Terminal.ts` and `headless/public/Terminal.ts` (tests: `headless/public/Terminal.test.ts`; `WriteBuffer.test.ts` per M6 D3) | `crates/lune-vt/src/headless/terminal.rs` (`lune_vt::Terminal`) | One Rust type: the core/public split upstream only hides JS internals. Emitters are `drain_events` and `take_damage`; `write` is synchronous; `allowProposedApi` gates nothing; `dispose` is `Drop`; `onLineFeed` and `onWriteParsed` have no consumer. Unicode 11 is registered at construction, version 6 stays active (D10). Each replaced and adapted upstream test is listed in the header of `headless/terminal_tests.rs`. |

On a version bump (rule 6), diff each file in this table against its new tag too, not only
`src/common`: `git diff <old>..<new> -- addons/addon-webgl/src/CustomGlyphs.ts` in
`reference/xterm.js`, then port each change with a test and update the marker line.

## Ported from Lunegit

Lunegit's terminal additions are not part of xterm.js 6.0.0. They live in `crates/lune-vt/src/ext/`
(parser extensions, installed once by `Terminal::install_standard_extensions`; `Terminal::new` and
every upstream-equal golden stay plain xterm.js) and `crates/lune-vt/src/input/` (encoders), and
cite Lunegit HEAD 33a1dd4 in their headers. Their expected values are not hand-typed: the key
encoders, `Osc52Parser` and the three OSC scanners are run by `tools/lunegit-oracle` over Lunegit's
prebuilt DLLs (`scripts/regen-lunegit-goldens.sh`, `LUNEGIT_BIN`) into
`crates/lune-vt-conformance/goldens/lunegit-input/` (format: `crates/lune-vt-conformance/FORMAT.md`,
"Lunegit input goldens"); the goldens record the DLLs' SHA-256 and CI never runs the tool. Lunegit's
own tests for each file are ported with it (counts in each Rust test file's header).

| Lunegit source | Rust file (`crates/lune-vt/src/`) | Deviations |
|---|---|---|
| `driver.js` kitty keyboard flags, modifyOtherKeys, their resets | `ext/keyboard_protocol.rs` | State lives on the host (`InputHandler.ext`, a `// LUNETERM:` field), not in a closure, so the encoders read it. RIS and `Terminal::reset` clear it through `core_reset`, DECSTR through a handler returning `false`; `Terminal::reset_keyboard_protocol()` is the host hook (ledger row 11). Everything stored is bounded (8 entries per screen, flags masked to 1 and 4, level 0 to 2). |
| `driver.js` XTVERSION | `ext/xtversion.rs` | The name comes from the host (`StandardExtensions::xtversion`, 1 to 64 bytes of printable ASCII, else `ExtensionError::InvalidXtversion`); `lune-vt` carries no identity. |
| `driver.js` unknown-OSC report | `ext/unknown_osc.rs` | Identifier and payload length only, at most 64 per drain (`UNKNOWN_OSC_LIMIT`). |
| `driver.js` VS16 width provider | `ext/vs16.rs` | `UnicodeV11Vs16` built from the engine's own packed layout, pinned against `UnicodeV11` for every code point; opt-in (`vs16`). |
| `Osc52Parser.cs` and `driver.js` OSC 52 | `ext/osc52.rs` | Streamed and capped as it arrives; a runtime gate (`set_clipboard_write`); the selection target is ignored; queries consumed, never answered. |
| `OscCwdScanner.cs`, `Osc94ProgressScanner.cs`, `TerminalCommandScanner.cs` | `ext/osc_cwd.rs`, `ext/osc_progress.rs`, `ext/osc_command.rs` | OSC handlers on the parser, not byte scanners (framing below). A cwd containing a control character after decoding is dropped (a tightening). `D;<digits>` carries the exit code (Lunegit ignores it). |
| `TerminalInput.cs` (`TerminalKey`, `TerminalKeyEvent`, `TerminalModifiers`) | `input/key_event.rs` | None for the keys. |
| `TerminalKeyEncoder.cs` | `input/key_encoder.rs` | Equal to the golden for every row. Two cases Lunegit does not cover (a modifyOtherKeys shifted code point, or a kitty flag-4 base-layout code point, that is not a Unicode scalar) send nothing. |
| `XtermVtInputEncoder.cs`, key and char parts (itself a port of XTerm.NET `KeyboardInputGenerator`, MIT, `THIRD_PARTY_NOTICES.md`) | `input/legacy_key_encoder.rs` | None. The mouse part is not ported (below). |
| `TerminalInput.cs` mouse types; `XtermVtInputEncoder.cs` mouse part | `input/mouse_event.rs`, over the ported `CoreMouseService` | Encoding comes from the engine's own DECSET state (plan D5). A position outside the terminal sends nothing where Lunegit clamped it; the default (X10-style) encoding sends nothing from zero-based column or row 223 on where Lunegit clamped; a drag's held button comes from the event, not state kept between events; press or release with no button or a wheel as the button, and a drag with a wheel as the button, send nothing. |
| (not in Lunegit) xterm.js `CoreBrowserTerminal.ts:270-295` | `Terminal::focus_event` | `ESC [ I` / `ESC [ O` only after `CSI ? 1004 h`. |
| `PtyTerminalSession.cs:351-368` (wraps only) and xterm.js `browser/Clipboard.ts` | `input/paste.rs` | The sanitizer of ledger row 19; an empty paste sends nothing; xterm.js's `ignoreBracketedPasteMode` option is not ported. |

Framing differences of the OSC extensions (the parser's framing against Lunegit's byte scanners; one
state machine on untrusted input, plan D3): CAN and SUB abort an OSC; a C1 control (U+0080 to
U+009F, a C1 ST included) aborts it instead of ending it; ESC ends it successfully, so an ST cut after the ESC
is complete one step earlier; a payload past the parser's `PAYLOAD_LIMIT` is dropped by the parser. The
parser also drops C0 controls inside an OSC before any handler sees them, so for OSC 52 the
base64 whitespace tolerance of `Osc52Parser` is moot and the golden rows with VT, FF or NUL inside
the base64 (Malformed in Lunegit) decode here (`the_engine_handler_agrees_with_the_parser_on_every_golden_row`
lists the difference). Per-form caps are Lunegit's, discard-whole on overflow: 4096 bytes for OSC 7
and 9;9, 64 for 9;4 and 133 (less the identifier the parser has consumed), 1,398,104 base64 characters
for 52. A leading-zero identifier (`OSC 007`) is accepted (the parser reads it as 7).

Synchronized-output latch (ledger row 17, plan D13): set by a DECSET whose parameters include 2026
as a plain number (a list such as `?1;2026h` counts, as in Lunegit's scanner; `?2026:1h` does not),
cleared by 133 A, C or D. Differences: `CSI ? 2026 SP h` (an intermediate) does not latch here;
Lunegit's scanner latched on it. `Params` keeps 32 parameters, so a 2026 past the 32nd does not
latch (Lunegit's scanner has no such limit).

File URIs in OSC 7 and 9;9 follow `OscCwdScanner.ParseFileUri` as .NET's `System.Uri` behaves on
Unix (probed against .NET 10); URIs the golden does not hold are an approximation, and IPv6 or
`c|` drive hosts and user-info with dots or queries are not verified against the golden.

Behaviour differences of the encoders from Lunegit, in one place: invalid Unicode scalar values in
`Character` events send nothing in every mode (golden-covered); the mouse rows above; an empty paste
sends nothing (xterm.js alone would send an empty bracket pair); paste strips C0 and C1 controls in
bracketed mode (user decision Q22).
