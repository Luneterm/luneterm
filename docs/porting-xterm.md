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
```
