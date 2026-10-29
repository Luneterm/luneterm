// Reference oracle: replays a byte stream through real @xterm/headless 6.0.0 and prints the
// resulting screen as JSON, for comparison against lune-vt in differential tests.
//
// Usage: node oracle.mjs <cols> <rows> <input-file> [resizeCols resizeRows]

import { readFileSync } from 'node:fs';
import xtermHeadless from '@xterm/headless';
import unicode11 from '@xterm/addon-unicode11';

const { Terminal } = xtermHeadless;
const { Unicode11Addon } = unicode11;

const [cols, rows, file, resizeCols, resizeRows] = process.argv.slice(2);
if (!file) {
  console.error('usage: node oracle.mjs <cols> <rows> <input-file> [resizeCols resizeRows]');
  process.exit(2);
}

const term = new Terminal({ cols: +cols, rows: +rows, allowProposedApi: true });
term.loadAddon(new Unicode11Addon());
term.unicode.activeVersion = '11';

await new Promise(resolve => term.write(readFileSync(file), resolve));
if (resizeCols) {
  term.resize(+resizeCols, +resizeRows);
  await new Promise(resolve => term.write('', resolve));
}

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
