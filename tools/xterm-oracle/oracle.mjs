// Reference oracle: replays a byte stream through real @xterm/headless 6.0.0 and prints the
// resulting screen as JSON, for comparison against lune-vt in differential tests.
//
// Usage: node oracle.mjs <cols> <rows> <input-file> [resizeCols resizeRows]
//
// The helpers below are shared with gen-goldens.mjs (golden generator for lune-vt-conformance).

import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import xtermHeadless from '@xterm/headless';
import unicode11 from '@xterm/addon-unicode11';

const { Terminal } = xtermHeadless;
const { Unicode11Addon } = unicode11;

/** A headless terminal with the Unicode 11 width tables active (the shipping configuration). */
export function createTerminal(cols, rows) {
  const term = new Terminal({ cols, rows, allowProposedApi: true });
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = '11';
  return term;
}

/** Writes one chunk and resolves when the parser has consumed it. */
export function writeChunk(term, bytes) {
  return new Promise(resolve => term.write(bytes, resolve));
}

/** Resizes and drains the write queue (an empty write flushes pending reflow work). */
export async function resizeTerminal(term, cols, rows) {
  term.resize(cols, rows);
  await writeChunk(term, '');
}

/**
 * Constructors of fresh upstream parser internals, taken from a throwaway terminal. The bundle keeps
 * private names, so this reaches through `_core._inputHandler` (probed against 6.0.0).
 */
export function parserInternals() {
  const ih = createTerminal(80, 24)._core._inputHandler;
  return {
    EscapeSequenceParser: ih._parser.constructor,
    Utf8ToUtf32: ih._utf8Decoder.constructor,
  };
}

async function main() {
  const [cols, rows, file, resizeCols, resizeRows] = process.argv.slice(2);
  if (!file) {
    console.error('usage: node oracle.mjs <cols> <rows> <input-file> [resizeCols resizeRows]');
    process.exit(2);
  }

  const term = createTerminal(+cols, +rows);
  await writeChunk(term, readFileSync(file));
  if (resizeCols) await resizeTerminal(term, +resizeCols, +resizeRows);

  const buf = term.buffer.active;
  const lines = [];
  for (let y = 0; y < buf.length; y++) {
    const line = buf.getLine(y);
    lines.push({ text: line.translateToString(true), wrapped: line.isWrapped });
  }

  console.log(JSON.stringify({
    cols: term.cols,
    rows: term.rows,
    cursor: { x: buf.cursorX, y: buf.cursorY },
    baseY: buf.baseY,
    screen: buf.type,
    lines,
  }, null, 2));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
