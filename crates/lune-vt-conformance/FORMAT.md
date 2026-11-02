# Conformance formats

`lune-vt-conformance` holds the data that pins `lune-vt` to xterm.js 6.0.0: corpus files (inputs,
hand-written), golden files (inputs plus the expected result, generated from the reference
xterm.js), and a reader for the goldens. The Rust side never needs Node: goldens are committed.

Regenerate with `scripts/regen-vt-goldens.sh` (needs Node and pnpm; runs
`tools/xterm-oracle/gen-goldens.mjs`). The generator is deterministic, so a second run must leave
`git diff -- crates/lune-vt-conformance/goldens` empty. `.gitattributes` forces LF for both
directories; the reader also tolerates a trailing `\r`.

## Runners

- `src/trace.rs` renders a `lune-vt` decoder + parser run in the parser-trace format above;
  `tests/parser_trace.rs` replays every parser golden (chunked and unsplit) and compares it with
  `expect`. `lune-vt/tests/no_panic.rs` includes `trace.rs` and `escape.rs` by path (the engine
  crate cannot depend on this one) for its seeded fuzz and chunk-invariance check.
- `src/screen.rs` renders a `lune_vt::Terminal` run (Unicode 11 active, default options, as the
  oracle's `createTerminal`) in the screen-dump format and Lunegit's grid, reading only the public
  API and `core()` accessors. `tests/screen_goldens.rs` replays every screen golden as recorded,
  unsplit, one byte per write and with three seeded random splits; all must equal `expect`.
  `tests/lunegit_vectors.rs` checks the 27 Lunegit vectors: the 21 non-divergent equal the tmux
  grid, the 6 divergent differ from it and their `lunegit_<name>` screen golden matches, and one
  byte per write gives the recorded chunking's grid. Every golden must pass (there is no
  expected-failure list since milestone M6 step 8).
- Known upstream chunk dependence: xterm.js 6.0.0 drops a DEL (0x7F) that is the first code after
  a DCS hook or starts a chunk inside a DCS payload and keeps one anywhere later. `lune-vt` keeps
  it everywhere (ledger row 18), so the fuzz's chunk-invariance comparison needs no exception
  (`no_panic.rs`), and goldens the quirk touches are marked `divergent` (below).

## Golden files

A golden file starts with the line `lune-vt-golden 1` and holds one or more records. All files in
`goldens/parser/`, `goldens/screen/` and `goldens/lunegit/` ending in `.golden` are read, in file
name order. A record is:

```
kind parser|screen|grid
name <id>                 [a-z0-9_]+, unique within its kind
source <text>             free text (corpus line or origin)
size <cols> <rows>        optional
resize <cols> <rows>      optional; applied after all chunks
divergent <reason>        optional; see "Divergent goldens"
chunk <lowercase hex>     one or more; an empty chunk is `chunk -`
expect
<expected line>...        zero or more, never blank
end
```

The header lines (`kind` first, then any of `name source size resize divergent` in any order)
precede the first `chunk`; `chunk` lines are consecutive; `expect` follows the last chunk; the
record ends at the first line equal to `end`. Blank lines between records are ignored. Anything
else is a format error: unknown header key, missing `kind`/`name`/`chunk`/`expect`/`end`, odd-length
or non-hex chunk, a blank expected line, a wrong magic line. The reader reports file and line and
never panics.

Inputs are stored as hex chunks so that line-ending conversion cannot corrupt bytes. A case with
several chunks is fed chunk by chunk to one parser or terminal (state carries across chunks).

### Kinds and where they live

| kind | directory | meaning of `expect` |
| --- | --- | --- |
| `parser` | `goldens/parser/` | parser event trace (below) |
| `screen` | `goldens/screen/` | screen dump (below); also holds `lunegit_<name>` oracle dumps of the 27 Lunegit vectors |
| `grid` | `goldens/lunegit/` | the visible grid in Lunegit's normalisation, one escaped string per row; `expect` is the tmux 3.6b result, not the oracle's |

### Divergent goldens

`divergent <reason>` means the reference xterm.js is not the behaviour `lune-vt` aims for.
- Parser and screen goldens: the result depends on how the input is chunked. The reason is
  `upstream Utf8ToUtf32 split-sequence bug (TextDecoder.ts:161)`: upstream's decoder drops a
  character when a UTF-8 sequence is split right after a `0x80` continuation byte (`E2 80 | 99`
  loses U+2019). `lune-vt` fixes this (ledger entry 3), so `expect` holds the trace or dump of the
  **unsplit** input, which is what a correct decoder produces for any chunking.
- Parser goldens, second reason: `upstream DCS passthrough DEL quirk (EscapeSequenceParser.ts:346,
  :744)`: upstream's table makes DEL a no-op in DCS passthrough while its put loop keeps it, so a
  DEL straight after the hook (or at a chunk start) vanishes. `lune-vt` keeps it (ledger row 18).
  The generator derives `expect` from the oracle's own parser constructed with upstream's table
  plus that one entry changed (DEL in DCS passthrough = DCS put), unsplit, so everything else is
  upstream's. The oracle's stock output is never edited. A case showing both bugs carries both
  reasons joined by `; `. Single-chunk cases can be divergent for this reason.
- Grid goldens (the 6 Lunegit vectors listed in the corpus): xterm.js does not reflow the cursor's
  own wrapped line, so the oracle's grid differs from tmux's; `expect` stays the tmux grid and the
  paired `screen` golden records what xterm.js actually produces.

## String escaping

Used for every quoted string in expected lines, by the generator and by the Rust runner. A string
is wrapped in `"`; code points 0x20 to 0x7E other than `"` and `\` appear verbatim; every other
code point (including `"`, `\`, controls, and everything above 0x7E) is written `\u{X}` with
uppercase hex digits and no padding. There is no other escape, so a quoted string contains no
literal `"` or `\` except as part of `\u{22}` and `\u{5C}`.

## Parser trace (`kind parser`)

The generator drives a fresh upstream `EscapeSequenceParser` and `Utf8ToUtf32` per case with
fallback handlers only (the constructor's `ESC \` swallow stays, so a bare `ESC \` produces no
event). Each chunk goes through the decoder and then `parse`; bytes are UTF-8, so the parser
sees code points. Events, one per line:

```
print "<s>"               printable run
exec <hex>                C0/C1 control, lowercase hex, at least 2 digits
csi <ident> <params>
esc <ident>
osc <id> start
osc put "<s>"
osc end <0|1>             1 = terminated normally, 0 = aborted
dcs <ident> hook <params>
dcs put "<s>"
dcs unhook <0|1>
error <state> <code hex>  parse-state number in decimal, offending code point in lowercase hex
```

- `error` is the parser's error handler: it fires when the transition table has no entry for a
  code in the current state (for example DEL in the ground state, or a code point above 0xFF while
  an escape sequence is open). The state is the upstream `ParserState` number in decimal. Parsing
  continues afterwards, and the position is deliberately not part of the line.
- Adjacent `print` events are merged into one; adjacent `osc put` events and adjacent `dcs put`
  events are merged. Nothing else is merged. (Chunk boundaries are therefore invisible.)
- `<ident>` is the upstream identifier as an unsigned 32-bit value, 8 lowercase hex digits: the
  collected bytes (a CSI/DCS prefix first, then intermediates) shifted in 8 bits at a time, then
  the final byte in the low byte, so `ESC [ ? h` is `00003f68` and `DCS $ q` is `00002471`. With
  more than three collected bytes upstream's 32-bit shift wraps; the value shown is that wrapped
  result. The generator
  never uses upstream `identToString`, which loops forever on negative idents.
- `<id>` of an OSC is decimal for 0 to 2147483646, `-1` when the sequence carried no id, and `max`
  above that. Upstream accumulates the id as a JS number (400 digits become Infinity); `lune-vt`
  saturates, and `max` is the shared spelling of both.
- `<params>` is upstream `Params.toArray()` rendered as JSON without spaces: `[1,2,[3,-1]]`. A
  parameter with sub-parameters is an array whose first element is the parameter, `-1` is an empty
  value. For DCS the params are captured at hook time (upstream mutates the object afterwards).
- `print`, `osc put` and `dcs put` payloads are code points rendered with the string escaping above.

## Screen dump (`kind screen`)

The generator writes the chunks into `new Terminal({cols, rows, allowProposedApi: true})` with the
Unicode 11 addon active (default `80 24`), waits for each write, applies `resize` and waits again,
then dumps the **active** buffer. Lines, in this order:

```
cursor <x> <y>
buffer normal|alternate
base-y <n>
title "<s>"                            last OSC 0/2 title, "" if none
modes <key>=<value> ...                sorted by key, space separated
response "<s>"                         one per terminal data event, in order
line <y> <wrapped 0|1> "<s>"           every buffer line (scrollback included), y from 0
attr <y> <x> <len> <token> ...         attribute runs, ordered by line then x
link <id> "<uri>"                      every url id used by a cell, ascending
```

- `modes` keys: `public.<k>` (the public `term.modes`), `dec.<k>` (`coreService.decPrivateModes`),
  `core.<k>` (`coreService.modes`), `mouse.protocol`, `mouse.encoding`. Values: `1`/`0` for
  booleans, `-` for `undefined`, otherwise the value's string form (no whitespace).
- `line` text is `translateToString(true)` limited to the terminal's current width: after a
  shrink the alternate buffer keeps stale cells beyond `cols`, which are not part of the screen.
- `attr` covers cells `x` in `[x, x+len)` of line `y`. Maximal runs of consecutive cells with an
  identical, non-empty token set are written; runs with no tokens (all default) are omitted.
  Tokens, in this fixed order, each omitted when default:
  `fg=<c>` `bg=<c>` `b` (bold) `d` (dim) `i` `u=<n>` `uc=<c>` `bl` (blink) `inv` `invis` `s`
  (strikethrough) `o` (overline) `w=<n>` (cell width when not 1) `url=<id>`.
  `<c>` is `p16:<n>`, `p256:<n>` or `rgb:<rrggbb>` (the default colour is omitted).
  `u=<n>` is the raw underline style bits `(_ext & 0x1C000000) >> 26`, written when the underline
  flag or the bits are set; the public `underlineStyle` getter is not used because it reports
  dashed (5) whenever a url id is set. `uc` is the raw underline-colour word's mode and value, not
  the fallback to the foreground colour; upstream leaves `rgb:ffffff` after `SGR 59`.
- `link` uris come from the OSC 8 link service.

## Grid (`kind grid`)

Lunegit's normalisation: the visible rows of the active buffer (the last `rows` lines), each joined
from its cells with a wide glyph's tail cell skipped, right-trimmed, then trailing empty rows
dropped. Each expected line is the escaped row string, `""` for an interior empty row.

## Corpus files

Under `corpus/`, read only by the generator. `#` starts a comment line; keys:

```
random <count> seed <n>   (parser.cases, screen.cases) extra generated cases, mulberry32
case <name>               starts a case
source <text>             optional origin note
size <cols> <rows>        screen and grid cases
resize <cols> <rows>      screen and grid cases
divergent <reason>        grid cases (lunegit.cases): xterm.js is known to differ from tmux
grid "<row>"              grid cases: one expected row each, string-escaped
input <token>...          may repeat; tokens concatenate
```

Input tokens: `"..."` is UTF-8 text with the escapes `\e \a \b \t \n \r \\ \" \xHH` (a raw byte)
and `\u{H}` (a code point as UTF-8); `hex:<digits>` is raw bytes; either may be followed by `*N` to
repeat it; `|` ends the current chunk (`| |` yields an empty chunk).

Random cases are named `rand_0001` and up: up to 120 bytes of weighted byte soup (escape
structure characters, digits, intermediates, C0 controls, UTF-8 snippets, stray high bytes) cut
into 1 to 5 chunks at random offsets.

### Random screen cases

`random <count> seed <n>` in `screen.cases` adds `<count>` generated screen cases, named
`srand_0001` and up, written to `goldens/screen/random.golden` (same record format as any screen
golden; the generator's source line says how many chunks the input has). Each case is built from
whole tokens drawn from a grammar of the registered sequences, so most of the input reaches a
real handler:

- size one of 12 small geometries (largest 20x8; 2x1, 2x2, 3x1, 5x1 and 20x1 give the one-row
  and two-column edge cases); 40% of cases also `resize` afterwards;
- 4 to 30 tokens: text (ASCII words; wide, combining, ZWJ, emoji, VS15/16 clusters; stray or
  invalid UTF-8 bytes), C0 and C1 controls, CSI sequences for every registered identifier,
  ESC sequences including all charset designators, OSC (titles, colours, OSC 8 links, OSC 52 and
  other unregistered ids; BEL, `ESC \` or C1 ST terminators), DCS DECRQSS, and one token in
  twelve is cut short so the next token interrupts it;
- CSI parameters come from a pool that includes the extremes `0 1 2 65535 2147483647`, empty
  values, and `:` sub-parameter separators; the finals that loop once per unit (`L M S T I Z b`)
  are capped at 65535 because the oracle would take minutes (and lune-vt bounds them, plan M6
  step 5);
- 1 to 4 chunks, cut at random offsets and, now and then, right after `E2 80` (upstream's split
  UTF-8 bug): such cases are `divergent` exactly like the hand cases and hold the unsplit dump;
- RIS (`ESC c`) and OSC 8 links never appear in the same case: lune-vt frees a reset buffer's
  links (decision Q9), upstream leaks them, so url ids after a reset differ by design.

- a drawn case is dropped (and another drawn) when the oracle's dump of the input in one write
  differs from its dump with one write per UTF-8 character: upstream's `print` depends on where
  `write` calls end for some inputs (a wide character and a joiner at the right margin with
  autowrap off, for example `ESC [ 1 ; 1 5 H ESC [ ? 7 l` then U+1F468 U+200D U+1F469 U+200D
  U+1F467), `lune-vt` reproduces that, and the screen runner's replays (one byte per write,
  random splits) require every chunking to give one dump;
- `CSI 1 J` and `CSI ? 1 J` are not generated: with the cursor on the last row at the last column
  upstream reads `lines.get(j + 1)!` without adding `ybase` (`InputHandler.ts:1218`), gets
  `undefined` and throws, which stops the oracle's write loop. Hand cases cover ED 1 with the
  cursor elsewhere.

Generation is deterministic (mulberry32, no clock). A case that makes the oracle itself throw
would stop the generator; pick another seed.
