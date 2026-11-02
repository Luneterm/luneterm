// Throughput benchmark: @xterm/headless 6.0.0 vs lune-vt (crates/lune-vt/examples/throughput.rs).
//
//   node bench.mjs --generate <file>   write the deterministic ~16 MiB input stream
//   node bench.mjs --run <file>        time @xterm/headless over it (5 runs, median)
//
// Both sides write the same bytes in 64 KiB chunks into an 80x24 terminal with Unicode 11 active.
// Output is machine-readable `key=value` lines read by scripts/bench-vt.sh. Throughput is MiB/s.

import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createTerminal } from './oracle.mjs';

const TARGET_BYTES = 16 * 1024 * 1024;
const CHUNK = 64 * 1024;
const COLS = 80;
const ROWS = 24;
const RUNS = 5;

// ---- input generation (deterministic: mulberry32, fixed seed) ----

function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS = ['alpha', 'build', 'cargo', 'diff', 'error', 'fn', 'git', 'hello', 'index', 'join',
  'kernel', 'lines', 'merge', 'node', 'open', 'parse', 'query', 'render', 'state', 'test', 'unit',
  'vector', 'warn', 'xterm', 'yield', 'zero'];

function generate() {
  const r = rng(0x1ce7e4d);
  const int = n => Math.floor(r() * n);
  const pick = a => a[int(a.length)];
  const word = () => pick(WORDS);
  const out = [];
  let size = 0;
  const push = s => {
    const b = Buffer.from(s, 'utf8');
    out.push(b);
    size += b.length;
  };

  const plainLines = n => {
    for (let i = 0; i < n; i++) {
      const words = [];
      let len = int(110);
      while (len > 0) {
        const w = word();
        words.push(w);
        len -= w.length + 1;
      }
      push(words.join(' ') + '\r\n');
    }
  };

  const lsOutput = n => {
    for (let i = 0; i < n; i++) {
      let line = '';
      for (let c = 0; c < 4; c++) {
        const name = word() + (int(3) ? '' : '.rs');
        switch (int(5)) {
          case 0: line += `\x1b[1;34m${name}/\x1b[0m  `; break;
          case 1: line += `\x1b[0;32m${name}\x1b[0m  `; break;
          case 2: line += `\x1b[38;5;${int(256)}m${name}\x1b[39m  `; break;
          case 3: line += `\x1b[38;2;${int(256)};${int(256)};${int(256)};48;5;${int(256)}m${name}\x1b[0m  `; break;
          default: line += `\x1b[4;31m${name}\x1b[24;39m  `; break;
        }
      }
      push(line + '\r\n');
    }
  };

  const EMOJI = ['\u{1F600}', '\u{1F680}', '\u{1F4A9}', '\u{1F44D}', '\u{2764}\u{FE0F}', '\u{1F468}\u{200D}\u{1F4BB}', '\u{1F1EF}\u{1F1F5}'];
  const wideText = n => {
    for (let i = 0; i < n; i++) {
      let line = '';
      let w = 0;
      while (w < 70) {
        switch (int(4)) {
          case 0: line += word() + ' '; w += 6; break;
          case 1: { const c = 0x4e00 + int(0x5000); line += String.fromCodePoint(c); w += 2; break; }
          case 2: line += pick(EMOJI); w += 2; break;
          default: line += String.fromCodePoint(0x3041 + int(0x50)); w += 2; break;
        }
      }
      push(line + '\r\n');
    }
  };

  const redraw = n => {
    for (let f = 0; f < n; f++) {
      push('\x1b[H');
      for (let row = 1; row <= ROWS; row++) {
        let line = `\x1b[${row};1H`;
        if (row === 1 || row === ROWS) line += `\x1b[7m ${word()} ${f} \x1b[27m`;
        else line += `\x1b[3${1 + int(7)}m${word()} ${word()}\x1b[0m`;
        line += '\x1b[K';
        line += `\x1b[${row};${40 + int(30)}H${word()}`;
        push(line);
      }
      if (f % 8 === 7) push('\x1b[2J');
    }
  };

  const scrollRegion = n => {
    push('\x1b[5;20r\x1b[20;1H');
    for (let i = 0; i < n; i++) {
      push(`${word()} ${word()} ${i}\r\n`);
      if (i % 16 === 0) push('\x1b[2S');
      if (i % 24 === 0) push('\x1b[T');
      if (i % 40 === 0) push('\x1b[10;1H\x1b[M\x1b[20;1H');
    }
    push('\x1b[r\x1b[24;1H');
  };

  const kinds = [
    () => plainLines(400),
    () => lsOutput(400),
    () => wideText(300),
    () => redraw(40),
    () => scrollRegion(600),
  ];
  for (let i = 0; size < TARGET_BYTES; i++) kinds[i % kinds.length]();
  return Buffer.concat(out);
}

// ---- measurement ----

function fnv1a(buf) {
  let h = 0x811c9dc5;
  for (let i = 0; i < buf.length; i++) h = Math.imul(h ^ buf[i], 0x01000193) >>> 0;
  return h.toString(16).padStart(8, '0');
}

const median = a => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

async function timeWrite(data) {
  const term = createTerminal(COLS, ROWS);
  const t0 = process.hrtime.bigint();
  let last;
  for (let off = 0; off < data.length; off += CHUNK) {
    const chunk = data.subarray(off, Math.min(off + CHUNK, data.length));
    last = new Promise(resolve => term.write(chunk, resolve));
  }
  await last;
  const secs = Number(process.hrtime.bigint() - t0) / 1e9;
  return { secs, term };
}

// Same, but through the core's synchronous parse (no WriteBuffer time slicing): the pure engine cost.
function timeWriteSync(data) {
  const term = createTerminal(COLS, ROWS);
  const t0 = process.hrtime.bigint();
  for (let off = 0; off < data.length; off += CHUNK) {
    term._core.writeSync(data.subarray(off, Math.min(off + CHUNK, data.length)));
  }
  const secs = Number(process.hrtime.bigint() - t0) / 1e9;
  return { secs, term };
}

function screenSummary(term) {
  const b = term.buffer.active;
  return `cursor=${b.cursorX},${b.cursorY} base_y=${b.baseY} total_lines=${b.length}`;
}

async function run(file) {
  const data = readFileSync(file);
  const mib = data.length / (1024 * 1024);
  console.log(`engine=@xterm/headless ${createRequire(import.meta.url)('@xterm/headless/package.json').version}`);
  console.log(`node=${process.version} platform=${process.platform}-${process.arch}`);
  console.log(`bytes=${data.length} fnv1a=${fnv1a(data)} chunk=${CHUNK} grid=${COLS}x${ROWS} unicode=11`);

  const timed = [];
  let term;
  for (let i = 0; i < RUNS; i++) {
    const r = await timeWrite(data);
    timed.push(mib / r.secs);
    term = r.term;
    if (i < RUNS - 1) term.dispose();
  }
  console.log(`write_api_mib_s=${timed.map(x => x.toFixed(2)).join(',')} median=${median(timed).toFixed(2)}`);
  console.log(`final_screen=${screenSummary(term)}`);
  term.dispose();

  const sync = [];
  for (let i = 0; i < RUNS; i++) {
    const r = timeWriteSync(data);
    sync.push(mib / r.secs);
    r.term.dispose();
  }
  console.log(`core_write_sync_mib_s=${sync.map(x => x.toFixed(2)).join(',')} median=${median(sync).toFixed(2)}`);
}

const [mode, file] = process.argv.slice(2);
if (mode === '--generate' && file) {
  const data = generate();
  writeFileSync(file, data);
  console.log(`generated ${file}: ${data.length} bytes fnv1a=${fnv1a(data)}`);
} else if (mode === '--run' && file) {
  await run(file);
} else {
  console.error('usage: node bench.mjs --generate <file> | --run <file>');
  process.exit(2);
}
