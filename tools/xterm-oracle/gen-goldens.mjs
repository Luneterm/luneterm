// Golden generator for crates/lune-vt-conformance.
//
// Reads the corpus files (crates/lune-vt-conformance/corpus/*.cases), replays every case through
// real @xterm/headless 6.0.0 and writes the goldens (crates/lune-vt-conformance/goldens/).
// The file formats are specified in crates/lune-vt-conformance/FORMAT.md; keep the two in step.
//
// Usage:
//   node gen-goldens.mjs                    write all goldens (deterministic: no clock, sorted output)
//   node gen-goldens.mjs --check-lunegit    only self-check the 27 Lunegit vectors against the oracle
//                                           (exit 1 when a non-divergent vector differs)

import { readFileSync, writeFileSync, mkdirSync, readdirSync, unlinkSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTerminal, writeChunk, resizeTerminal, parserInternals } from './oracle.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'crates', 'lune-vt-conformance');
const CORPUS = join(ROOT, 'corpus');
const GOLDENS = join(ROOT, 'goldens');

const UPSTREAM_BUG = 'upstream Utf8ToUtf32 split-sequence bug (TextDecoder.ts:161)';
const UPSTREAM_DEL_QUIRK = 'upstream DCS passthrough DEL quirk (EscapeSequenceParser.ts:346, :744)';

// ---------------------------------------------------------------------------------------------
// String escaping (FORMAT.md "String escaping"; the Rust side implements the same rule).

/** Code points 0x20-0x7E except `"` and `\` verbatim, everything else `\u{X}`; wrapped in quotes. */
export function escapeCodePoints(cps) {
  let out = '"';
  for (const cp of cps) {
    if (cp >= 0x20 && cp <= 0x7e && cp !== 0x22 && cp !== 0x5c) out += String.fromCharCode(cp);
    else out += `\\u{${cp.toString(16).toUpperCase()}}`;
  }
  return out + '"';
}

const codePointsOf = s => Array.from(s, ch => ch.codePointAt(0));
const escapeString = s => escapeCodePoints(codePointsOf(s));
const hex = (n, width) => n.toString(16).padStart(width, '0');
const bytesToHex = bytes => (bytes.length === 0 ? '-' : Buffer.from(bytes).toString('hex'));

// ---------------------------------------------------------------------------------------------
// Corpus files (FORMAT.md "Corpus files").

class CorpusError extends Error {}

/** Tokenises the text after `input`: "..." strings, hex:... bytes, optional *N, and `|`. */
function inputBytes(text, where) {
  const chunks = [[]];
  const push = bytes => { for (const b of bytes) chunks[chunks.length - 1].push(b); };
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (ch === ' ' || ch === '\t') { i++; continue; }
    if (ch === '|') { chunks.push([]); i++; continue; }
    let bytes;
    if (ch === '"') {
      const out = [];
      i++;
      for (;;) {
        if (i >= text.length) throw new CorpusError(`${where}: unterminated string`);
        const c = text[i];
        if (c === '"') { i++; break; }
        if (c !== '\\') {
          const cp = text.codePointAt(i);
          out.push(...Buffer.from(String.fromCodePoint(cp)));
          i += cp > 0xffff ? 2 : 1;
          continue;
        }
        const e = text[i + 1];
        i += 2;
        if (e === 'e') out.push(0x1b);
        else if (e === 'a') out.push(0x07);
        else if (e === 'b') out.push(0x08);
        else if (e === 't') out.push(0x09);
        else if (e === 'n') out.push(0x0a);
        else if (e === 'r') out.push(0x0d);
        else if (e === '\\') out.push(0x5c);
        else if (e === '"') out.push(0x22);
        else if (e === 'x') {
          const h = text.slice(i, i + 2);
          if (!/^[0-9a-fA-F]{2}$/.test(h)) throw new CorpusError(`${where}: bad \\x escape`);
          out.push(parseInt(h, 16));
          i += 2;
        } else if (e === 'u' && text[i] === '{') {
          const end = text.indexOf('}', i);
          const h = text.slice(i + 1, end);
          if (end < 0 || !/^[0-9a-fA-F]{1,6}$/.test(h)) throw new CorpusError(`${where}: bad \\u escape`);
          out.push(...Buffer.from(String.fromCodePoint(parseInt(h, 16))));
          i = end + 1;
        } else throw new CorpusError(`${where}: unknown escape \\${e}`);
      }
      bytes = out;
    } else if (text.startsWith('hex:', i)) {
      let j = i + 4;
      while (j < text.length && /[0-9a-fA-F]/.test(text[j])) j++;
      const h = text.slice(i + 4, j);
      if (h.length % 2 !== 0) throw new CorpusError(`${where}: odd-length hex`);
      bytes = Array.from(Buffer.from(h, 'hex'));
      i = j;
    } else throw new CorpusError(`${where}: unexpected '${ch}' in input`);
    if (text[i] === '*') {
      let j = i + 1;
      while (j < text.length && /[0-9]/.test(text[j])) j++;
      const n = Number(text.slice(i + 1, j));
      if (j === i + 1 || n > 100000) throw new CorpusError(`${where}: bad repeat count`);
      i = j;
      const once = bytes;
      bytes = [];
      for (let k = 0; k < n; k++) bytes.push(...once);
    }
    push(bytes);
  }
  return chunks;
}

/** Reads a .cases file into { random?, cases: [{name, source, size, resize, input, grid, divergent}] }. */
function readCorpus(file) {
  const path = join(CORPUS, file);
  const lines = readFileSync(path, 'utf8').split('\n');
  const result = { random: null, cases: [] };
  let cur = null;
  lines.forEach((raw, idx) => {
    const line = raw.replace(/\r$/, '');
    const where = `${file}:${idx + 1}`;
    if (line === '' || line.startsWith('#')) return;
    const sp = line.indexOf(' ');
    const key = sp < 0 ? line : line.slice(0, sp);
    const rest = sp < 0 ? '' : line.slice(sp + 1);
    if (key === 'random') {
      const m = /^(\d+) seed (0x[0-9a-fA-F]+|\d+)$/.exec(rest);
      if (!m) throw new CorpusError(`${where}: bad random directive`);
      result.random = { count: Number(m[1]), seed: Number(m[2]) };
      return;
    }
    if (key === 'case') {
      if (!/^[a-z0-9_]+$/.test(rest)) throw new CorpusError(`${where}: bad case name '${rest}'`);
      cur = { name: rest, source: `${file}:${idx + 1}`, size: null, resize: null, input: [[]], grid: [], divergent: null, line: idx + 1 };
      result.cases.push(cur);
      return;
    }
    if (!cur) throw new CorpusError(`${where}: '${key}' before any case`);
    if (key === 'source') cur.source = rest;
    else if (key === 'size' || key === 'resize') {
      const m = /^(\d+) (\d+)$/.exec(rest);
      if (!m) throw new CorpusError(`${where}: bad ${key}`);
      cur[key] = [Number(m[1]), Number(m[2])];
    } else if (key === 'divergent') cur.divergent = rest;
    else if (key === 'input') {
      const parts = inputBytes(rest, where);
      cur.input[cur.input.length - 1].push(...parts[0]);
      for (const p of parts.slice(1)) cur.input.push(p);
    } else if (key === 'grid') {
      if (!/^"[^"]*"$/.test(rest)) throw new CorpusError(`${where}: bad grid row`);
      cur.grid.push(rest);
    } else throw new CorpusError(`${where}: unknown key '${key}'`);
  });
  const names = new Set();
  for (const c of result.cases) {
    if (names.has(c.name)) throw new CorpusError(`${file}: duplicate case ${c.name}`);
    names.add(c.name);
  }
  return result;
}

// ---------------------------------------------------------------------------------------------
// Seeded random parser cases.

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const UTF8_SNIPPETS = [
  [0xc2, 0x80], [0xc2, 0x90], [0xc2, 0x98], [0xc2, 0x9b], [0xc2, 0x9c], [0xc2, 0x9d], [0xc2, 0x9e],
  [0xc2, 0x9f], [0xc3, 0xa9], [0xe2, 0x80, 0x99], [0xe4, 0xbd, 0xa0], [0xf0, 0x9f, 0x9a, 0x80],
  [0xef, 0xbb, 0xbf], [0xed, 0xa0, 0x80], [0xf4, 0x90, 0x80, 0x80], [0xc0, 0x80],
];

function randomCase(rng, index) {
  const pick = arr => arr[Math.floor(rng() * arr.length)];
  const len = 1 + Math.floor(rng() * 120);
  const bytes = [];
  while (bytes.length < len) {
    const r = rng();
    if (r < 0.22) bytes.push(0x20 + Math.floor(rng() * 95));
    else if (r < 0.37) bytes.push(0x1b);
    else if (r < 0.47) bytes.push(pick([0x5b, 0x5d, 0x50, 0x5e, 0x5f, 0x58, 0x5c, 0x28, 0x29, 0x23, 0x25]));
    else if (r < 0.57) bytes.push(pick([0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3b, 0x3a]));
    else if (r < 0.65) bytes.push(0x20 + Math.floor(rng() * 16));
    else if (r < 0.73) bytes.push(pick([0x00, 0x07, 0x08, 0x09, 0x0a, 0x0d, 0x18, 0x1a, 0x1c, 0x7f, 0x0e, 0x0f]));
    else if (r < 0.78) bytes.push(pick([0x3f, 0x3e, 0x3c, 0x3d]));
    else if (r < 0.83) bytes.push(0x40 + Math.floor(rng() * 63));
    else if (r < 0.94) bytes.push(...pick(UTF8_SNIPPETS));
    else bytes.push(0x80 + Math.floor(rng() * 128));
  }
  const cuts = new Set();
  const nCuts = Math.floor(rng() * 5);
  for (let k = 0; k < nCuts; k++) cuts.add(Math.floor(rng() * (bytes.length + 1)));
  const sorted = [...cuts].sort((x, y) => x - y);
  const chunks = [];
  let prev = 0;
  for (const c of sorted) { chunks.push(bytes.slice(prev, c)); prev = c; }
  chunks.push(bytes.slice(prev));
  const name = `rand_${String(index + 1).padStart(4, '0')}`;
  return { name, source: `generated: mulberry32, ${chunks.length} chunk(s)`, size: null, resize: null, input: chunks, grid: [], divergent: null };
}

// ---------------------------------------------------------------------------------------------
// Seeded random screen cases (FORMAT.md "Random screen cases"). Grammar-biased: every token is a
// whole registered sequence (or text), with parameters drawn from a pool that includes extremes;
// one in twelve tokens is cut short so the next token interrupts it.

const SCREEN_SIZES = [
  [20, 8], [16, 6], [10, 4], [8, 3], [12, 5], [2, 1], [2, 2], [3, 1], [20, 1], [5, 1], [2, 6], [6, 2],
];
const RESIZE_SIZES = [[10, 4], [20, 8], [5, 3], [2, 1], [3, 2], [12, 1], [16, 8], [8, 6]];
const SMALL_PARAMS = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '10', '12', '20', '22', '23', '25'];
const EXTREME_PARAMS = ['0', '1', '2', '65535', '2147483647'];
const CAPPED_PARAMS = ['0', '1', '2', '65535'];
const ANSI_MODES = ['0', '2', '4', '12', '20', '99'];
const DEC_MODES = [
  '0', '1', '3', '5', '6', '7', '8', '9', '12', '25', '45', '47', '66', '69', '80', '95', '1000', '1001',
  '1002', '1003', '1004', '1005', '1006', '1007', '1015', '1016', '1047', '1048', '1049', '2004', '2026',
  '99999',
];
const SGR_SIMPLE = [
  ...Array.from({ length: 10 }, (_, i) => String(i)), '21', '22', '23', '24', '25', '27', '28', '29',
  '30', '31', '32', '33', '34', '35', '36', '37', '39', '40', '41', '42', '43', '44', '45', '46', '47',
  '49', '53', '55', '59', '90', '91', '97', '100', '101', '107', '65535', '2147483647', '', '10', '56',
];
const TEXT_WORDS = ['a', 'bc', 'hello', 'xyz', 'Q', 'text', '0123456789', 'The quick brown fox', '~|{}`'];
const WIDE_TEXT = [
  '你', '好', '한', '글', '\u{1F600}', '\u{1F44D}', '\u{1F44D}\u{1F3FD}', '\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}',
  '\u{1F1EF}\u{1F1F5}', 'e\u{301}', 'a\u{301}\u{302}', '\u{301}', '\u{26A0}\u{FE0F}', '\u{26A0}\u{FE0E}',
  '\u{200D}', '\u{FE0F}', '\u{3042}', 'ａ', '\u{AD}', '\u{200B}', 'é', '€', '\u{2019}',
];
const CHARSET_FLAGS = '0AB4C5RQKYE6ZH7='.split('');
// [prefix, intermediates, final, parameter kind]; kinds: cnt count, cap capped count, pos two
// positions, none, sgr, ansi, dec, erase3, erase2, style, winops, margins, prot, sel, dsr.
const CSI_TABLE = [
  ['', '', '@', 'cnt'], ['', ' ', '@', 'cnt'], ['', '', 'A', 'cnt'], ['', ' ', 'A', 'cnt'],
  ['', '', 'B', 'cnt'], ['', '', 'C', 'cnt'], ['', '', 'D', 'cnt'], ['', '', 'E', 'cnt'],
  ['', '', 'F', 'cnt'], ['', '', 'G', 'cnt'], ['', '', 'H', 'pos'], ['', '', 'I', 'cap'],
  ['', '', 'J', 'erase3'], ['?', '', 'J', 'erase3'], ['', '', 'K', 'erase2'], ['?', '', 'K', 'erase2'],
  ['', '', 'L', 'cap'], ['', '', 'M', 'cap'], ['', '', 'P', 'cnt'], ['', '', 'S', 'cap'],
  ['', '', 'T', 'cap'], ['', '', 'X', 'cnt'], ['', '', 'Z', 'cap'], ['', '', '`', 'cnt'],
  ['', '', 'a', 'cnt'], ['', '', 'b', 'cap'], ['', '', 'c', 'none'], ['>', '', 'c', 'none'],
  ['', '', 'd', 'cnt'], ['', '', 'e', 'cnt'], ['', '', 'f', 'pos'], ['', '', 'g', 'tabclear'],
  ['', '', 'h', 'ansi'], ['?', '', 'h', 'dec'], ['?', '', 'h', 'dec'], ['', '', 'l', 'ansi'], ['?', '', 'l', 'dec'],
  ['', '', 'm', 'sgr'], ['', '', 'm', 'sgr'], ['', '', 'm', 'sgr'], ['', '', 'n', 'dsr'],
  ['?', '', 'n', 'dsr'], ['', '!', 'p', 'none'], ['', ' ', 'q', 'style'], ['', '', 'r', 'pos'],
  ['', '', 's', 'margins'], ['', '', 't', 'winops'], ['', '', 'u', 'none'], ['', "'", '}', 'cnt'],
  ['', "'", '~', 'cnt'], ['', '"', 'q', 'prot'], ['', '$', 'p', 'modeq'], ['?', '$', 'p', 'modeq'],
  ['', '', 'Z', 'cap'], ['', '', 'z', 'cnt'], ['?', '', 'c', 'none'], ['=', '', 'c', 'none'],
];
const ESC_SIMPLE = ['7', '8', 'D', 'E', 'H', 'M', '=', '>', 'n', 'o', '|', '}', '~', '%@', '%G', '#8', 'Z', 'x', '#3'];
const OSC_COLOURS = ['rgb:00/ff/00', 'rgb:f/0/8', '#123456', '#abc', 'rgb:ffff/0000/8000', 'nonsense', '', '?'];

function randomScreenCase(rng, index) {
  const pick = arr => arr[Math.floor(rng() * arr.length)];
  const chance = p => rng() < p;
  const int = (lo, hi) => lo + Math.floor(rng() * (hi - lo + 1));
  const b = text => Array.from(Buffer.from(text, 'utf8'));
  // RIS and OSC 8 links never share a case: lune-vt frees a reset buffer's links (decision Q9),
  // upstream leaks them, so the url ids after a reset differ by design.
  const allowRis = chance(0.15);
  const allowLinks = !allowRis;

  const param = pool => {
    const r = rng();
    if (r < 0.12) return '';
    if (r < 0.55) return pick(SMALL_PARAMS.slice(0, 9));
    if (r < 0.85) return pick(pool);
    return String(int(1, 40));
  };
  const joined = (n, pool) => {
    let out = param(pool);
    for (let i = 1; i < n; i++) out += (chance(0.12) ? ':' : ';') + param(pool);
    return out;
  };
  const sgrParams = () => {
    const parts = [];
    const n = int(1, 4);
    for (let i = 0; i < n; i++) {
      const r = rng();
      if (r < 0.45) parts.push(pick(SGR_SIMPLE));
      else if (r < 0.7) {
        const base = pick(['38', '48', '58']);
        const sep = chance(0.3) ? ':' : ';';
        if (chance(0.5)) parts.push([base, '5', pick(['0', '7', '8', '15', '16', '231', '255', '256', '300', ''])].join(sep));
        else {
          const lead = sep === ':' && chance(0.5) ? [base, '2', ''] : [base, '2'];
          const rgb = [0, 1, 2].map(() => pick(['0', '1', '128', '255', '256', '65535', '']));
          parts.push([...lead, ...rgb].join(sep));
        }
      } else if (r < 0.85) parts.push('4:' + pick(['0', '1', '2', '3', '4', '5', '6', '', '65535']));
      else parts.push(pick(['1', '3', '4', '7', '9', '53', '0']));
    }
    return parts.join(';');
  };
  const csi = () => {
    const [prefix, inter, final, kind] = pick(CSI_TABLE);
    let params;
    switch (kind) {
      case 'cnt': params = chance(0.2) ? '' : joined(chance(0.1) ? 2 : 1, EXTREME_PARAMS); break;
      case 'cap': params = chance(0.2) ? '' : joined(1, CAPPED_PARAMS); break;
      case 'pos': params = chance(0.15) ? '' : joined(int(1, 2), chance(0.5) ? EXTREME_PARAMS : SMALL_PARAMS); break;
      case 'none': params = chance(0.7) ? '' : pick(['0', '1', '2']); break;
      case 'sgr': params = chance(0.1) ? '' : sgrParams(); break;
      case 'ansi': params = joined(chance(0.15) ? 2 : 1, ANSI_MODES); break;
      case 'dec': params = joined(chance(0.2) ? 2 : 1, DEC_MODES); break;
      case 'modeq': params = chance(0.1) ? '' : pick(prefix === '?' ? DEC_MODES : ANSI_MODES); break;
      case 'erase3': params = pick(['', '0', '2', '3', '4']); break; // not 1: see FORMAT.md (upstream throws)
      case 'erase2': params = pick(['', '0', '1', '2', '3']); break;
      case 'tabclear': params = pick(['', '0', '1', '2', '3', '4']); break;
      case 'dsr': params = pick(['', '5', '6', '0', '15', '25', '26', '53', '1']); break;
      case 'style': params = pick(['', '0', '1', '2', '3', '4', '5', '6', '7', '3;1']); break;
      case 'margins': params = chance(0.4) ? '' : joined(2, SMALL_PARAMS); break;
      case 'winops': params = pick(['', '14', '14;2', '16', '18', '19', '22', '22;0', '22;1', '22;2', '23', '23;0', '23;2', '8;5;5', '1', '99']); break;
      case 'prot': params = pick(['', '0', '1', '2']); break;
      default: params = '';
    }
    return b(`\x1b[${prefix}${params}${inter}${final}`);
  };
  const esc = () => {
    const r = rng();
    if (r < 0.3) return b(`\x1b${pick(['(', ')', '*', '+', '-', '.', '/'])}${pick(CHARSET_FLAGS)}`);
    if (allowRis && r < 0.4) return b('\x1bc');
    return b(`\x1b${pick(ESC_SIMPLE)}`);
  };
  const text = () => {
    const r = rng();
    if (r < 0.55) return b(pick(TEXT_WORDS));
    if (r < 0.9) return b(pick(WIDE_TEXT) + (chance(0.3) ? pick(WIDE_TEXT) : '') + (chance(0.3) ? pick(TEXT_WORDS) : ''));
    return pick([[0x80], [0xc0, 0x80], [0xe2], [0xf4, 0x90, 0x80, 0x80], [0xed, 0xa0, 0x80]]);
  };
  const control = () => {
    const r = rng();
    if (r < 0.5) return b(pick(['\r\n', '\r\n', '\n', '\r', '\t', '\b', '\x07', '\x0b', '\x0c', '\x00', '\x7f', '\x0e', '\x0f']));
    if (r < 0.8) return b(pick(['\u{84}', '\u{85}', '\u{88}']));
    return b(pick(['\x18', '\x1a', '\x05', '\x1f', '\x01']));
  };
  const oscText = () => {
    const r = rng();
    if (r < 0.1) return 't'.repeat(int(50, 200));
    if (r < 0.2) return pick(WIDE_TEXT) + pick(TEXT_WORDS);
    return pick(['title', 'a b', 'x;y', '', 'some longer title']);
  };
  const osc = () => {
    const r = rng();
    let body;
    if (r < 0.2) body = `${pick(['0', '1', '2'])};${oscText()}`;
    else if (r < 0.35) body = `4;${pick(['0', '1', '7', '15', '16', '255', '256', 'x', ''])};${pick(OSC_COLOURS)}` + (chance(0.3) ? `;${pick(['2', '9'])};${pick(OSC_COLOURS)}` : '');
    else if (r < 0.6 && allowLinks) {
      body = chance(0.4) ? '8;;' : `8;${pick(['id=a', 'id=b:x=y', 'x=y', 'id=', 'id=a:id=b', ''])};${pick(['http://a.example/', 'http://b.example/?q=1&r=2', 'file:///x', 'u'.repeat(int(1, 80))])}`;
    } else if (r < 0.7) body = `${pick(['10', '11', '12'])};${pick(OSC_COLOURS)}`;
    else if (r < 0.8) body = pick(['104', '104;1', '104;1;2;300', '110', '111', '112', '110;x']);
    else if (r < 0.9) body = pick(['52;c;?', '52;c;aGVsbG8=', '9;note', '7;file:///tmp', '777;x', '133;A', '3;a=b', '99999999999;x']);
    else body = pick(['', '0', ';', '8', '4', '10']);
    const t = rng();
    const end = t < 0.6 ? [0x07] : t < 0.9 ? [0x1b, 0x5c] : [0xc2, 0x9c];
    return [...b(`\x1b]${body}`), ...end];
  };
  const dcs = () => {
    const req = pick(['m', 'r', ' q', '"q', '"p', 's', 'z', '']);
    const r = rng();
    if (r < 0.8) return b(`\x1bP$q${req}\x1b\\`);
    return b(`\x1bP${pick(['1;2|abc', '$qm', '+q4e', '>|x', ''])}\x1b\\`);
  };
  const token = () => {
    const r = rng();
    if (r < 0.27) return text();
    if (r < 0.37) return control();
    if (r < 0.40) return b(pick(['\x1b[?1049h', '\x1b[?47h', '\x1b[?1047h', '\x1b[?1049h\x1b[2J']));
    if (r < 0.70) return csi();
    if (r < 0.82) return esc();
    if (r < 0.93) return osc();
    return dcs();
  };

  const bytes = [];
  const count = int(4, 30);
  for (let i = 0; i < count; i++) {
    const t = token();
    if (chance(1 / 12) && t.length > 2) bytes.push(...t.slice(0, int(1, t.length - 1)));
    else bytes.push(...t);
  }
  const cuts = new Set();
  const nCuts = chance(0.5) ? 0 : int(1, 3);
  // Cut points: random offsets, now and then right after "E2 80" (the split upstream mishandles).
  const afterE280 = [];
  for (let k = 0; k + 1 < bytes.length; k++) if (bytes[k] === 0xe2 && bytes[k + 1] === 0x80) afterE280.push(k + 2);
  for (let k = 0; k < nCuts; k++) cuts.add(afterE280.length > 0 && chance(0.4) ? pick(afterE280) : int(0, bytes.length));
  const chunks = [];
  let prev = 0;
  for (const c of [...cuts].sort((x, y) => x - y)) { chunks.push(bytes.slice(prev, c)); prev = c; }
  chunks.push(bytes.slice(prev));
  const size = pick(SCREEN_SIZES);
  const resize = chance(0.4) ? pick(RESIZE_SIZES) : null;
  const name = `srand_${String(index + 1).padStart(4, '0')}`;
  return { name, source: `generated: mulberry32, ${chunks.length} chunk(s)`, size, resize, input: chunks, grid: [], divergent: null };
}

// ---------------------------------------------------------------------------------------------
// Parser-trace mode.

const { EscapeSequenceParser, Utf8ToUtf32 } = parserInternals();

const identHex = n => hex(n >>> 0, 8);
const oscId = id => (id === -1 || id === undefined || id === null ? '-1' : id > 2147483646 ? 'max' : String(id));

// ParserState.DCS_PASSTHROUGH (constants.ts) and the table index `state << 8 | code`.
const DCS_PASSTHROUGH = 13;

/**
 * Upstream's table with the one entry lune-vt changes (ledger row 18): DEL in DCS_PASSTHROUGH is
 * DCS_PUT (as the read-ahead loop already treats it) instead of IGNORE. Built from upstream's own
 * table object, so everything else is upstream's. Used only to derive the expected trace of the
 * cases the quirk touches; the stock oracle output is never altered.
 */
function dcsDelFixedTable() {
  const stock = new EscapeSequenceParser()._transitions;
  const table = new stock.constructor(stock.table.length);
  table.table.set(stock.table);
  const put = stock.table[(DCS_PASSTHROUGH << 8) | 0x41]; // 'A': DCS_PUT -> DCS_PASSTHROUGH
  if ((put & 0x0f) !== DCS_PASSTHROUGH || stock.table[(DCS_PASSTHROUGH << 8) | 0x7f] === put) {
    throw new Error('oracle: unexpected DCS_PASSTHROUGH table layout');
  }
  table.table[(DCS_PASSTHROUGH << 8) | 0x7f] = put;
  return table;
}

/**
 * Feeds the chunks through a fresh parser + decoder and returns the normalised trace lines.
 * `fixDel` swaps in the table of `dcsDelFixedTable()`.
 */
function traceParser(chunks, fixDel = false) {
  const parser = fixDel ? new EscapeSequenceParser(dcsDelFixedTable()) : new EscapeSequenceParser();
  const decoder = new Utf8ToUtf32();
  const ev = [];
  const add = (kind, text) => {
    const last = ev[ev.length - 1];
    if ((kind === 'print' || kind === 'osc put' || kind === 'dcs put') && last && last.kind === kind) {
      last.cps.push(...text);
    } else if (kind === 'print' || kind === 'osc put' || kind === 'dcs put') {
      ev.push({ kind, cps: [...text] });
    } else ev.push({ kind, line: text });
  };
  parser.setPrintHandler((data, start, end) => add('print', Array.from(data.subarray(start, end))));
  parser.setExecuteHandlerFallback(code => add('exec', `exec ${hex(code, 2)}`));
  parser.setCsiHandlerFallback((ident, params) => add('csi', `csi ${identHex(ident)} ${JSON.stringify(params.toArray())}`));
  parser.setEscHandlerFallback(ident => add('esc', `esc ${identHex(ident)}`));
  parser.setOscHandlerFallback((ident, action, payload) => {
    if (action === 'START') add('osc start', `osc ${oscId(ident)} start`);
    else if (action === 'PUT') add('osc put', codePointsOf(payload));
    else add('osc end', `osc end ${payload ? 1 : 0}`);
  });
  parser.setDcsHandlerFallback((ident, action, payload) => {
    if (action === 'HOOK') add('dcs hook', `dcs ${identHex(ident)} hook ${JSON.stringify(payload.toArray())}`);
    else if (action === 'PUT') add('dcs put', codePointsOf(payload));
    else add('dcs unhook', `dcs unhook ${payload ? 1 : 0}`);
  });
  parser.setErrorHandler(state => {
    add('error', `error ${state.currentState} ${state.code.toString(16)}`);
    return state;
  });
  for (const chunk of chunks) {
    const input = Uint8Array.from(chunk);
    const target = new Uint32Array(input.length + 4);
    const n = decoder.decode(input, target);
    parser.parse(target, n);
  }
  return ev.map(e => {
    if (e.line !== undefined) return e.line;
    const tag = e.kind;
    return `${tag} ${escapeCodePoints(e.cps)}`;
  });
}

const sameLines = (a, b) => a.length === b.length && a.every((l, i) => l === b[i]);

function parserGolden(c) {
  const chunked = traceParser(c.input);
  const flat = [c.input.flat()];
  // `expect` is what lune-vt must produce for any chunking: the unsplit trace of upstream with the
  // DEL entry fixed (equal to the stock trace unless one of the two quirks shows).
  const expect = traceParser(flat, true);
  const reasons = [];
  if (c.input.length > 1 && !sameLines(traceParser(flat), chunked)) reasons.push(UPSTREAM_BUG);
  if (!sameLines(traceParser(c.input, true), chunked)) reasons.push(UPSTREAM_DEL_QUIRK);
  if (reasons.length === 0 && !sameLines(expect, chunked)) {
    throw new Error(`oracle: ${c.name}: chunked trace differs from the unsplit trace for an unknown reason`);
  }
  const divergent = reasons.length ? reasons.join('; ') : null;
  return { kind: 'parser', name: c.name, source: c.source, size: null, resize: null, divergent, chunks: c.input, expect };
}

// ---------------------------------------------------------------------------------------------
// Screen mode.

const CM = { 0x1000000: 'p16', 0x2000000: 'p256', 0x3000000: 'rgb' };
function colour(mode, value) {
  if (mode === 0) return 'default';
  const m = CM[mode];
  // Palette modes keep only the low byte meaningful: a previous RGB value's upper bytes can linger
  // in the underline-colour word (upstream masks with PCOLOR_MASK when reading it).
  return m === 'rgb' ? `rgb:${hex(value & 0xffffff, 6)}` : `${m}:${value & 0xff}`;
}

/** Style tokens of one cell, in the fixed order of FORMAT.md; default tokens are omitted. */
function cellTokens(cell) {
  const t = [];
  const fg = colour(cell.getFgColorMode(), cell.getFgColor());
  const bg = colour(cell.getBgColorMode(), cell.getBgColor());
  if (fg !== 'default') t.push(`fg=${fg}`);
  if (bg !== 'default') t.push(`bg=${bg}`);
  if (cell.isBold()) t.push('b');
  if (cell.isDim()) t.push('d');
  if (cell.isItalic()) t.push('i');
  // Raw underline style bits: the public `underlineStyle` getter reports DASHED whenever a url id is set.
  const ext = cell.extended._ext;
  const ubits = (ext & 0x1c000000) >>> 26;
  if (cell.isUnderline() || ubits !== 0) t.push(`u=${ubits}`);
  const ucMode = ext & 0x3000000;
  if (ucMode !== 0) t.push(`uc=${colour(ucMode, ext & 0xffffff)}`);
  if (cell.isBlink()) t.push('bl');
  if (cell.isInverse()) t.push('inv');
  if (cell.isInvisible()) t.push('invis');
  if (cell.isStrikethrough()) t.push('s');
  if (cell.isOverline()) t.push('o');
  if (cell.getWidth() !== 1) t.push(`w=${cell.getWidth()}`);
  const url = cell.extended.urlId;
  if (url) t.push(`url=${url}`);
  return t.join(' ');
}

function modeValue(v) {
  if (v === undefined) return '-';
  if (v === true) return '1';
  if (v === false) return '0';
  const s = String(v);
  if (/\s/.test(s)) throw new Error(`mode value has whitespace: ${s}`);
  return s;
}

/** Screen dump lines (FORMAT.md "Screen dump"). `events` collects onData / title. */
function dumpScreen(term, events) {
  const core = term._core;
  const buf = term.buffer.active;
  const out = [];
  out.push(`cursor ${buf.cursorX} ${buf.cursorY}`);
  out.push(`buffer ${buf.type}`);
  out.push(`base-y ${buf.baseY}`);
  out.push(`title ${escapeString(events.title)}`);
  const modes = {};
  for (const [k, v] of Object.entries(term.modes)) modes[`public.${k}`] = v;
  for (const [k, v] of Object.entries(core.coreService.decPrivateModes)) modes[`dec.${k}`] = v;
  for (const [k, v] of Object.entries(core.coreService.modes)) modes[`core.${k}`] = v;
  modes['mouse.protocol'] = core.coreMouseService.activeProtocol;
  modes['mouse.encoding'] = core.coreMouseService.activeEncoding;
  out.push('modes ' + Object.keys(modes).sort().map(k => `${k}=${modeValue(modes[k])}`).join(' '));
  for (const r of events.responses) out.push(`response ${escapeString(r)}`);
  const urlIds = new Set();
  const runs = [];
  for (let y = 0; y < buf.length; y++) {
    const line = buf.getLine(y);
    const width = Math.min(line.length, term.cols);
    out.push(`line ${y} ${line.isWrapped ? 1 : 0} ${escapeString(line.translateToString(true, 0, width))}`);
    let runStart = 0;
    let runTokens = '';
    const flush = end => {
      if (runTokens !== '') runs.push(`attr ${y} ${runStart} ${end - runStart} ${runTokens}`);
    };
    for (let x = 0; x < width; x++) {
      const cell = line.getCell(x);
      const tokens = cellTokens(cell);
      if (cell.extended.urlId) urlIds.add(cell.extended.urlId);
      if (tokens !== runTokens) {
        flush(x);
        runStart = x;
        runTokens = tokens;
      }
    }
    flush(width);
  }
  out.push(...runs);
  for (const id of [...urlIds].sort((a, b) => a - b)) {
    const entry = core._oscLinkService._dataByLinkId.get(id);
    out.push(`link ${id} ${escapeString(entry ? entry.data.uri : '')}`);
  }
  return out;
}

/** Writes the chunks (each awaited), optionally resizes, and dumps. */
async function runScreen(c, chunks) {
  const [cols, rows] = c.size ?? [80, 24];
  const term = createTerminal(cols, rows);
  const events = { responses: [], title: '' };
  term.onData(d => events.responses.push(d));
  term.onTitleChange(t => { events.title = t; });
  for (const chunk of chunks) await writeChunk(term, Uint8Array.from(chunk));
  if (c.resize) await resizeTerminal(term, c.resize[0], c.resize[1]);
  return { term, events };
}

/**
 * True when the oracle shows the same screen for the input in one write and for one write per UTF-8
 * character. Upstream's `print` depends on where `write` calls end for some inputs (a wide
 * character and a joiner at the right margin with autowrap off), and lune-vt reproduces that, so
 * the replays of the screen runner (one byte per write, random splits) could not equal any one
 * dump. Such cases are not kept (FORMAT.md "Random screen cases").
 */
async function chunkInvariantInOracle(c) {
  const flat = c.input.flat();
  const groups = [];
  for (const byte of flat) {
    if (groups.length === 0 || (byte & 0xc0) !== 0x80) groups.push([byte]);
    else groups[groups.length - 1].push(byte);
  }
  const dumpOf = async chunks => {
    const { term, events } = await runScreen(c, chunks);
    return dumpScreen(term, events);
  };
  return sameLines(await dumpOf([flat]), await dumpOf(groups));
}

async function screenGolden(c, name) {
  const dumpOf = async chunks => {
    const { term, events } = await runScreen(c, chunks);
    return dumpScreen(term, events);
  };
  let expect = await dumpOf(c.input);
  let divergent = null;
  if (c.input.length > 1) {
    const whole = await dumpOf([c.input.flat()]);
    if (!sameLines(whole, expect)) {
      expect = whole;
      divergent = UPSTREAM_BUG;
    }
  }
  return { kind: 'screen', name, source: c.source, size: c.size ?? [80, 24], resize: c.resize, divergent, chunks: c.input, expect };
}

// ---------------------------------------------------------------------------------------------
// Grid mode (Lunegit normalisation: VtConformanceSupport.cs Normalize).

/** Visible rows of the active buffer: cells joined (a wide glyph's tail skipped), rtrim, no trailing blank rows. */
export function gridRows(term) {
  const buf = term.buffer.active;
  const rows = [];
  for (let y = 0; y < term.rows; y++) {
    const line = buf.getLine(buf.baseY + y);
    let s = '';
    // Visible width is term.cols: the alternate buffer keeps stale cells beyond it after a shrink.
    for (let x = 0; x < Math.min(line.length, term.cols); x++) {
      const cell = line.getCell(x);
      if (cell.getWidth() === 0) continue;
      s += cell.getChars() || ' ';
    }
    rows.push(s.replace(/[ \t]+$/, ''));
  }
  while (rows.length > 0 && rows[rows.length - 1] === '') rows.pop();
  return rows;
}

async function oracleGrid(c) {
  const { term } = await runScreen(c, c.input);
  return gridRows(term).map(escapeString);
}

function gridGolden(c) {
  return { kind: 'grid', name: c.name, source: c.source, size: c.size, resize: c.resize, divergent: c.divergent, chunks: c.input, expect: c.grid };
}

// ---------------------------------------------------------------------------------------------
// Output.

function renderGolden(g) {
  const out = [`kind ${g.kind}`, `name ${g.name}`, `source ${g.source}`];
  if (g.size) out.push(`size ${g.size[0]} ${g.size[1]}`);
  if (g.resize) out.push(`resize ${g.resize[0]} ${g.resize[1]}`);
  if (g.divergent) out.push(`divergent ${g.divergent}`);
  for (const chunk of g.chunks) out.push(`chunk ${bytesToHex(chunk)}`);
  out.push('expect', ...g.expect, 'end');
  return out.join('\n') + '\n';
}

function writeGoldenFile(dir, file, goldens) {
  mkdirSync(join(GOLDENS, dir), { recursive: true });
  const text = 'lune-vt-golden 1\n' + goldens.map(renderGolden).join('');
  writeFileSync(join(GOLDENS, dir, file), text);
}

function clearGoldens() {
  for (const dir of ['parser', 'screen', 'lunegit']) {
    const d = join(GOLDENS, dir);
    if (!existsSync(d)) continue;
    for (const f of readdirSync(d)) if (f.endsWith('.golden')) unlinkSync(join(d, f));
  }
}

async function checkLunegit(cases, quiet) {
  let matched = 0;
  let nonDivergent = 0;
  const failures = [];
  const divergentReport = [];
  for (const c of cases) {
    const actual = await oracleGrid(c);
    const same = sameLines(actual, c.grid);
    if (c.divergent) divergentReport.push(`  ${c.name}: oracle ${same ? 'matches' : 'differs from'} the tmux grid (expected: differs)`);
    else {
      nonDivergent++;
      if (same) matched++;
      else failures.push(`  ${c.name}\n    oracle: ${actual.join(' / ')}\n    tmux:   ${c.grid.join(' / ')}`);
    }
  }
  if (!quiet || failures.length) {
    console.log(`${matched}/${nonDivergent} Lunegit vectors match the oracle`);
    for (const l of divergentReport) console.log(l);
    for (const f of failures) console.error(`MISMATCH:\n${f}`);
  }
  return failures.length === 0;
}

async function main() {
  const lunegit = readCorpus('lunegit.cases').cases;
  if (process.argv.includes('--check-lunegit')) {
    process.exit((await checkLunegit(lunegit, false)) ? 0 : 1);
  }
  const parserCorpus = readCorpus('parser.cases');
  const screenCorpus = readCorpus('screen.cases');

  const parserCases = [...parserCorpus.cases];
  const randomCases = [];
  if (parserCorpus.random) {
    const rng = mulberry32(parserCorpus.random.seed);
    for (let i = 0; i < parserCorpus.random.count; i++) randomCases.push(randomCase(rng, i));
  }

  clearGoldens();
  const handGoldens = parserCases.map(parserGolden);
  const randomGoldens = randomCases.map(parserGolden);
  writeGoldenFile('parser', 'cases.golden', handGoldens);
  writeGoldenFile('parser', 'random.golden', randomGoldens);
  const screenGoldens = [];
  for (const c of screenCorpus.cases) screenGoldens.push(await screenGolden(c, c.name));
  writeGoldenFile('screen', 'cases.golden', screenGoldens);
  const randomScreens = [];
  let rejected = 0;
  if (screenCorpus.random) {
    const rng = mulberry32(screenCorpus.random.seed);
    while (randomScreens.length < screenCorpus.random.count) {
      if (rejected > 20 * screenCorpus.random.count) throw new Error('oracle: too many random screen cases rejected');
      const c = randomScreenCase(rng, randomScreens.length);
      if (!(await chunkInvariantInOracle(c))) { rejected++; continue; }
      randomScreens.push(await screenGolden(c, c.name));
    }
  }
  writeGoldenFile('screen', 'random.golden', randomScreens);
  const lunegitScreens = [];
  for (const c of lunegit) lunegitScreens.push(await screenGolden(c, `lunegit_${c.name}`));
  writeGoldenFile('screen', 'lunegit.golden', lunegitScreens);
  writeGoldenFile('lunegit', 'vectors.golden', lunegit.map(gridGolden));

  console.log(`parser: ${parserCases.length} + ${randomCases.length} random (${[...handGoldens, ...randomGoldens].filter(g => g.divergent).length} chunk-divergent); screen: ${screenGoldens.length} + ${randomScreens.length} random (${randomScreens.filter(g => g.divergent).length} chunk-divergent, ${rejected} rejected) + ${lunegitScreens.length} lunegit; grid: ${lunegit.length}`);
  const ok = await checkLunegit(lunegit, true);
  process.exit(ok ? 0 : 1);
}

await main();
