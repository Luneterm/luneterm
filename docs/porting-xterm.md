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
  unchecked as JS object writes; `translate_to_string`'s `out_columns` index UTF-16 code units.
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
  --test buffer_ops`.

## Port status

One row per file of `git ls-files 'src/common/*'` at f447274, excluding `*.test.ts`: **54**
entries. "Ported" names the Rust file (under `crates/lune-vt/src/`); "replaced" says why no Rust
file exists; "pending" names the milestone that decides (roadmap M6 InputHandler and the engine
API, M7 Lunegit additions). Update a row when its milestone lands.

| Upstream (`src/common/`) | Status |
|---|---|
| `CircularList.ts` | `circular_list.rs` (`CircularList<T, O: ListObserver>`: events become observer calls, storage grows lazily, D1/D2) |
| `Clone.ts` | pending M6 (imported by `CoreService`) |
| `Color.ts` | `color.rs` (`css::to_color` has no canvas path; unsupported formats are `Err`) |
| `CoreTerminal.ts` | pending M6 |
| `InputHandler.ts` | pending M6 |
| `MultiKeyMap.ts` | replaced: imported only by `src/browser/` (`ColorContrastCache`), not by `src/common` |
| `Platform.ts` | replaced: user-agent sniffing that only fed a browser option's default (`rightClickSelectsWord`) and `TaskQueue`; the option is not ported (D3, D5) |
| `SortedList.ts` | pending M6 (imported only by `DecorationService`) |
| `TaskQueue.ts` | replaced: `Buffer::resize` sets a pending flag and the host drives `Buffer::batched_memory_cleanup` when idle (D5) |
| `TypedArrayUtils.ts` | replaced: imported only by its own test; Rust has slices and `Vec` |
| `Types.ts` | pending M6 (interfaces; Rust types replace most of them) |
| `WindowsMode.ts` | pending M6 (imported by `CoreTerminal`) |
| `buffer/AttributeData.ts` | `buffer/attribute_data.rs` (`AttributeData`, `ExtendedAttrs` as a `Copy` value) |
| `buffer/Buffer.ts` | `buffer/buffer.rs` (including the column reflow; deviations listed in its header) |
| `buffer/BufferLine.ts` | `buffer/buffer_line.rs` (`Vec<u32>` storage with typed-array edge semantics, D7) |
| `buffer/BufferRange.ts` | `buffer/buffer_range.rs` |
| `buffer/BufferReflow.ts` | `buffer/buffer_reflow.rs` |
| `buffer/BufferSet.ts` | `buffer/buffer_set.rs` (`onBufferActivate` is a return value, D1) |
| `buffer/CellData.ts` | `buffer/cell_data.rs` |
| `buffer/Constants.ts` | `buffer/constants.rs` |
| `buffer/Marker.ts` | `buffer/marker.rs` (`MarkerId`, `MarkerSet`: the three `addMarker` listeners, D1/D4) |
| `buffer/Types.ts` | replaced: interfaces; the Rust structs are the types (D6) |
| `data/Charsets.ts` | `data/charsets.rs` |
| `data/EscapeSequences.ts` | `data/escape_sequences.rs` |
| `input/Keyboard.ts` | pending M7 (ported or recorded as superseded) |
| `input/TextDecoder.ts` | `input/text_decoder.rs` (`Utf8ToUtf32` with the D4 fix; `StringToUtf32` replaced, D5; `stringFromCodePoint` and `utf32ToString` ported) |
| `input/UnicodeV6.ts` | `input/unicode_v6.rs` |
| `input/WriteBuffer.ts` | pending M6 (or a recorded replacement) |
| `input/XParseColor.ts` | `input/x_parse_color.rs` |
| `parser/Constants.ts` | `parser/constants.rs` |
| `parser/DcsParser.ts` | `parser/dcs_parser.rs` (no async handlers, D3) |
| `parser/EscapeSequenceParser.ts` | `parser/escape_sequence_parser.rs` (no async handlers, D3) |
| `parser/OscParser.ts` | `parser/osc_parser.rs` (no async handlers, D3) |
| `parser/Params.ts` | `parser/params.rs` |
| `parser/Types.ts` | `parser/types.rs` (`IParams`, `ISubParser`, `IOscParser`, `IDcsParser`, `IEscapeSequenceParser` are the concrete types; stack-state types not ported, D3) |
| `public/AddonManager.ts` | pending M6 (headless API) |
| `public/BufferApiView.ts` | pending M6 (headless API) |
| `public/BufferLineApiView.ts` | pending M6 (headless API) |
| `public/BufferNamespaceApi.ts` | pending M6 (headless API) |
| `public/ParserApi.ts` | pending M6 (headless API); `lune_vt::Extension` is the parser's extension point meanwhile |
| `public/UnicodeApi.ts` | pending M6 (headless API) |
| `services/BufferService.ts` | `services/buffer_service.rs` (`onResize` and `onScroll` are return values, D1) |
| `services/CharsetService.ts` | `services/charset_service.rs` |
| `services/CoreMouseService.ts` | pending M6 |
| `services/CoreService.ts` | pending M6 |
| `services/DecorationService.ts` | pending M6 (headless API) |
| `services/InstantiationService.ts` | replaced: DI container; Rust services are plain structs (rule 3) |
| `services/LogService.ts` | pending M6 |
| `services/OptionsService.ts` | `services/options_service.rs` (typed `TerminalOptions` with the 21 options the core reads, D3) |
| `services/OscLinkService.ts` | `services/osc_link_service.rs` (marker disposal arrives through `on_markers_disposed`, D1) |
| `services/ServiceRegistry.ts` | replaced: DI registry; Rust services are plain structs (rule 3) |
| `services/Services.ts` | replaced: DI service interfaces; the Rust structs are the interfaces (rule 3) |
| `services/UnicodeService.ts` | `services/unicode_service.rs` (`onChange` is the return value of `set_active_version`) |
| `tsconfig.json` | replaced: TypeScript build configuration |

## Ported outside `lune-vt`

Files from other parts of the xterm.js tree follow the same rules (marker line, tables and
comments in upstream order, deviations only in `// LUNETERM:` blocks), but live in the crate that
uses them and stay flat. In `lune-vt` that means `src/addons/` only.

| Upstream file @ f447274 | Rust file | Deviations |
|---|---|---|
| `addons/addon-webgl/src/CustomGlyphs.ts` (box drawing, blocks, shades, legacy computing, powerline) | `crates/lune-termview/src/custom_glyphs.rs`, drawn through the original `glyph_canvas.rs` | Stroke snap generalised for even line widths; pattern fill on a colourless mask; `has_custom_char`; NaN coordinates skip their instruction. Upstream has no test file, so the tests are original. |
| `addons/addon-unicode11/src/UnicodeV11.ts` (wcwidth tables and `UnicodeV11` provider, addon 0.9.0) | `crates/lune-vt/src/addons/unicode_v11.rs` | None. Tables generated from the TS file; the Playwright test is not portable, so `wcwidth_v11_emoji_test` (10 x U+1F923 = 20 cells, against 10 for V6: `v6_is_ten_before_activation`) and `version_is_11_and_registered_after_6` in `addons/unicode_v11_tests.rs` cover it. |
| `addons/addon-unicode11/src/Unicode11Addon.ts` | replaced | Activation is `UnicodeService::register(Box::new(UnicodeV11::new()))` followed by `set_active_version("11")` (M5 D6). |

On a version bump (rule 6), diff each file in this table against its new tag too, not only
`src/common`: `git diff <old>..<new> -- addons/addon-webgl/src/CustomGlyphs.ts` in
`reference/xterm.js`, then port each change with a test and update the marker line.
