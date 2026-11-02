//! Port of xterm.js src/common/buffer/BufferLine.ts @ f447274 (tests: BufferLine.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D7):
//!  - `_data` is a `Vec<u32>` of `length x CELL_SIZE` slots, `[content, fg, bg]` per cell with the
//!    bit layout of `buffer/Constants.ts`. It reproduces `Uint32Array` semantics exactly: a read
//!    past the end yields 0, a write past the end is a no-op, and a read of an index that
//!    overflows `usize` is 0 as well. `Vec::capacity()` stands in for the typed array's
//!    `buffer.byteLength` (the underlying `ArrayBuffer`), so `resize` and `cleanup_memory` keep
//!    upstream's reuse/shrink behaviour and return values.
//!  - `_combined` and `_extendedAttrs` are ordered maps (`BTreeMap`: ascending integer keys, as
//!    `Object.keys` on integer-like keys; `// LUNETERM:` (review S7 round 4) `_extendedAttrs` is a
//!    [`RunMap`], the same entries stored as runs of keys with one value, so a line in one
//!    underline colour or link is one run). Writes to them are as unchecked as JS object writes:
//!    a `set_cell` of a combined cell at an index `>= len()` stores the string even though the
//!    data write is ignored. An `undefined` map value is represented by an absent key (every
//!    reader treats the two alike).
//!  - [`copy_cells_from`](BufferLine::copy_cells_from) copies every combined key `>= src_col`,
//!    not only those of the copied range, as upstream does (`:501-508`, a `FIXME` there). Do not
//!    "fix" it: reflow relies on the resulting values.
//!  - The test-only `TestBufferLine` subclass is replaced by the sibling test module reading the
//!    private fields.
//!  - `ExtendedAttrs` is a `Copy` value instead of a shared reference (equivalent, see
//!    `attribute_data.rs`).
//!  - Where upstream would read the string of a combined cell whose map entry is missing it gets
//!    `undefined` (and throws on `.charCodeAt`); here that string is empty and its code is 0.
//!  - `insert_cells` and `delete_cells` return early on a zero-length line (upstream computes
//!    `pos %= 0` = `NaN` and ends up writing at most a `"NaN"` map key, which nothing reads).
//!  - `// LUNETERM:` (review S4) [`add_codepoint_to_cell`](BufferLine::add_codepoint_to_cell) stores
//!    at most [`MAX_COMBINED_CODE_POINTS`] code points per cell. Upstream appends every combining
//!    character, so `a` followed by any number of U+0301 grows one cell without bound, beyond
//!    what the scrollback limit bounds, and every read of the cell copies it. A code point past
//!    the cap is dropped; the width it asks for is still applied, so layout and the cursor are
//!    as upstream and only the cell's text is shorter. `get_string` (and so REP and the
//!    renderer) sees the capped text.
//!  - `// LUNETERM:` (review S7) [`insert_cells_repeatedly`](BufferLine::insert_cells_repeatedly)
//!    is a run of `insert_cells` calls at advancing positions (each followed by the input
//!    handler's fix-up of a wide cell at `cols - 1`) done in one pass, with the same cells and
//!    side-table entries (stale ones included); REP uses it in insert mode.
//!    [`set_cells_repeatedly`](BufferLine::set_cells_repeatedly) is a run of `set_cell` calls
//!    in one pass; REP's line fill and `insert_cells`' fill use it.
//!  - A width above 2 (never stored by the input handler) is kept as is; `content >> 22` is an
//!    unsigned shift here where JS sign-extends a value with bit 31 set.

use std::collections::BTreeMap;

use crate::buffer::attribute_data::{AttributeData, ExtendedAttrs};
use crate::buffer::cell_data::{CellData, CharData};
use crate::buffer::constants::{
    Attributes, BgFlags, Content, NULL_CELL_CHAR, NULL_CELL_CODE, NULL_CELL_WIDTH,
    WHITESPACE_CELL_CHAR,
};
use crate::input::text_decoder::string_from_code_point;

/// typed array slots taken by one cell
pub const CELL_SIZE: usize = 3;

// Cell member indices (upstream `const enum Cell`).
const CELL_CONTENT: usize = 0;
const CELL_FG: usize = 1; // currently simply holds all known attrs
const CELL_BG: usize = 2; // currently unused

/// Upstream `DEFAULT_ATTR_DATA` (`Object.freeze(new AttributeData())`).
pub const DEFAULT_ATTR_DATA: AttributeData = AttributeData {
    fg: 0,
    bg: 0,
    extended: ExtendedAttrs::new(0, 0),
};

/// Factor when to cleanup underlying array buffer after shrinking.
pub const CLEANUP_THRESHOLD: usize = 2;

/// The last UTF-16 code unit of `s` (`s.charCodeAt(s.length - 1)`), 0 for an empty string.
fn last_code_unit(s: &str) -> u32 {
    s.encode_utf16().last().map_or(0, u32::from)
}

/// LUNETERM: most code points one cell's combined string holds (module notes, review S4).
/// The longest real grapheme clusters (emoji ZWJ sequences, stacked diacritics) use about a
/// dozen; Unicode's stream-safe format allows a base plus 30 non-starters.
pub const MAX_COMBINED_CODE_POINTS: usize = 64;

/// LUNETERM: (review S7) `map.insert(k, v)` for every `(k, v)` of `updates`, whose keys are
/// ascending and distinct. A batch that is large next to the map rebuilds it in one ordered
/// merge (`collect` of a sorted sequence builds the tree in bulk, O(entries)) instead of one
/// tree insert per entry; the map ends with the same entries either way, and nothing reads a
/// map's node layout.
fn insert_sorted<V>(map: &mut BTreeMap<usize, V>, updates: Vec<(usize, V)>) {
    debug_assert!(updates.windows(2).all(|pair| match pair {
        [(a, _), (b, _)] => a < b,
        _ => true,
    }));
    if updates.len() < 16 || updates.len().saturating_mul(4) < map.len() {
        for (key, value) in updates {
            map.insert(key, value);
        }
        return;
    }
    let old = std::mem::take(map);
    let mut merged = Vec::with_capacity(old.len() + updates.len());
    let mut old = old.into_iter().peekable();
    for (key, value) in updates {
        while let Some(entry) = old.next_if(|(old_key, _)| *old_key < key) {
            merged.push(entry);
        }
        // an entry at the same key is replaced
        old.next_if(|(old_key, _)| *old_key == key);
        merged.push((key, value));
    }
    merged.extend(old);
    *map = merged.into_iter().collect();
}

/// LUNETERM: (review S7 round 4) upstream's `_extendedAttrs` (cell index -> extended
/// attributes) stored as runs of consecutive keys holding one value. A line's cells mostly
/// share their attributes, so a line printed in one underline colour (or inside one link,
/// whose id lives here too) is one run instead of a tree entry per cell: filling, cloning and
/// dropping it no longer cost an allocation per handful of cells. It answers every read as
/// the per-key map does, keys past the line's end included (a write there is stored, as a JS
/// object write is).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RunMap<V> {
    /// first key -> (last key, value). Runs are disjoint and non-empty, and two runs that
    /// touch (`last + 1 == next first`) hold different values: a set of entries has exactly
    /// one form, so two maps are equal exactly when they hold the same entries.
    runs: BTreeMap<usize, (usize, V)>,
}

impl<V> Default for RunMap<V> {
    fn default() -> Self {
        RunMap {
            runs: BTreeMap::new(),
        }
    }
}

impl<V: Copy + PartialEq> RunMap<V> {
    /// The value at `key`.
    fn get(&self, key: &usize) -> Option<&V> {
        self.runs
            .range(..=*key)
            .next_back()
            .filter(|(_, (last, _))| *key <= *last)
            .map(|(_, (_, value))| value)
    }

    #[cfg(test)]
    fn contains_key(&self, key: &usize) -> bool {
        self.get(key).is_some()
    }

    /// The number of keys held.
    fn len(&self) -> usize {
        self.runs
            .iter()
            .fold(0, |sum: usize, (&first, &(last, _))| {
                sum.saturating_add((last - first).saturating_add(1))
            })
    }

    fn clear(&mut self) {
        self.runs.clear();
    }

    fn insert(&mut self, key: usize, value: V) {
        // the common cases first: the key already holds the value, or it extends the run
        // that ends just before it (and does not reach the next run)
        let before = self
            .runs
            .range(..=key)
            .next_back()
            .map(|(&first, &(last, held))| (first, last, held));
        if let Some((first, last, _)) = before.filter(|&(_, _, held)| held == value) {
            if key <= last {
                return;
            }
            if last.checked_add(1) == Some(key) {
                let joins_next = key
                    .checked_add(1)
                    .and_then(|next| self.runs.get(&next))
                    .is_some_and(|&(_, next_value)| next_value == value);
                if !joins_next {
                    if let Some(run) = self.runs.get_mut(&first) {
                        run.0 = key;
                    }
                    return;
                }
            }
        }
        self.assign(key, key, Some(value));
    }

    fn remove(&mut self, key: &usize) {
        self.assign(*key, *key, None);
    }

    /// Keeps the keys below `end` only.
    fn truncate(&mut self, end: usize) {
        match end {
            0 => self.runs.clear(),
            _ => self.assign(end, usize::MAX, None),
        }
    }

    /// Every key in `first..=last` holds `value` afterwards (`None`: no entry).
    fn assign(&mut self, first: usize, last: usize, value: Option<V>) {
        if first > last {
            return;
        }
        let covering = self
            .runs
            .range(..=first)
            .next_back()
            .is_some_and(|(_, &(end, held))| end >= last && Some(held) == value);
        if covering {
            // (a run already holds the value at every key)
            return;
        }
        // a run that starts before `first` and reaches it keeps its keys before `first` and
        // after `last`
        let before = self
            .runs
            .range(..first)
            .next_back()
            .map(|(&start, &(end, held))| (start, end, held));
        if let Some((start, end, held)) = before.filter(|&(_, end, _)| end >= first) {
            // (start < first <= end, so neither bound overflows)
            self.runs.insert(start, (first - 1, held));
            if end > last {
                self.runs.insert(last + 1, (end, held));
            }
        }
        // a run that starts inside goes, but keeps its keys after `last`
        while let Some((&start, _)) = self.runs.range(first..=last).next() {
            if let Some((end, held)) = self.runs.remove(&start) {
                if end > last {
                    self.runs.insert(last + 1, (end, held));
                }
            }
        }
        // the new run, merged with a run it touches that holds the same value (after a
        // removal the gap separates the neighbours, so nothing merges)
        let Some(value) = value else {
            return;
        };
        let (mut first, mut last) = (first, last);
        let before = self
            .runs
            .range(..first)
            .next_back()
            .map(|(&start, &(end, held))| (start, end, held));
        if let Some((start, _, _)) =
            before.filter(|&(_, end, held)| end.checked_add(1) == Some(first) && held == value)
        {
            self.runs.remove(&start);
            first = start;
        }
        let after = last
            .checked_add(1)
            .and_then(|next| self.runs.get(&next).map(|&(end, held)| (next, end, held)));
        if let Some((next, end, _)) = after.filter(|&(_, _, held)| held == value) {
            self.runs.remove(&next);
            last = end;
        }
        self.runs.insert(first, (last, value));
    }
}

/// Typed array based bufferline implementation.
///
/// There are 2 ways to insert data into the cell buffer:
/// - `set_cell_from_codepoint` + `add_codepoint_to_cell`
///   Use these for data that is already UTF32.
///   Used during normal input in `InputHandler` for faster buffer access.
/// - `set_cell`
///   This method takes a `CellData` object and stores the data in the buffer.
///   Use `CellData::from_char_data` to create the `CellData` object (e.g. from a string).
///
/// To retrieve data from the buffer use either one of the primitive methods
/// (if only one particular value is needed) or `load_cell`. For `load_cell` in a loop
/// allocations can be avoided by reusing the `CellData` object.
///
/// Upstream memory layout:
///
/// ```text
///   |             uint32_t             |        uint32_t         |        uint32_t         |
///   |             `content`            |          `FG`           |          `BG`           |
///   | wcwidth(2) comb(1) codepoint(21) | flags(8) R(8) G(8) B(8) | flags(8) R(8) G(8) B(8) |
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferLine {
    data: Vec<u32>,
    combined: BTreeMap<usize, String>,
    extended_attrs: RunMap<ExtendedAttrs>,
    length: usize,
    pub is_wrapped: bool,
}

impl BufferLine {
    /// A line of `cols` cells, each a copy of `fill_cell_data` (default: the null cell).
    ///
    /// `cols` must be a sane terminal width (the caller clamps it, as `CoreTerminal` does);
    /// upstream throws a `RangeError` from `new Uint32Array` for an absurd one.
    pub fn new(cols: usize, fill_cell_data: Option<&CellData>, is_wrapped: bool) -> Self {
        let mut line = BufferLine {
            data: vec![0; cols.saturating_mul(CELL_SIZE)],
            combined: BTreeMap::new(),
            extended_attrs: RunMap::default(),
            length: cols,
            is_wrapped,
        };
        let default_cell;
        let cell = match fill_cell_data {
            Some(cell) => cell,
            None => {
                default_cell = CellData::from_char_data(&(
                    0,
                    NULL_CELL_CHAR.to_string(),
                    NULL_CELL_WIDTH,
                    NULL_CELL_CODE,
                ));
                &default_cell
            }
        };
        for i in 0..cols {
            line.set_cell(i, cell);
        }
        line
    }

    /// Number of cells (upstream `length`).
    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The number of combined strings held (upstream `Object.keys(_combined).length`); for
    /// bounding the side tables in tests.
    pub fn combined_len(&self) -> usize {
        self.combined.len()
    }

    /// The number of extended attribute entries held (upstream `Object.keys(_extendedAttrs).length`).
    pub fn extended_attrs_len(&self) -> usize {
        self.extended_attrs.len()
    }

    /// `_data[index * CELL_SIZE + field]` with `Uint32Array` read semantics: 0 out of range.
    fn read(&self, index: usize, field: usize) -> u32 {
        index
            .checked_mul(CELL_SIZE)
            .and_then(|slot| slot.checked_add(field))
            .and_then(|slot| self.data.get(slot))
            .copied()
            .unwrap_or(0)
    }

    /// `_data[index * CELL_SIZE + field] = value` with `Uint32Array` write semantics: ignored
    /// out of range.
    fn write(&mut self, index: usize, field: usize, value: u32) {
        let slot = index
            .checked_mul(CELL_SIZE)
            .and_then(|slot| slot.checked_add(field));
        if let Some(entry) = slot.and_then(|slot| self.data.get_mut(slot)) {
            *entry = value;
        }
    }

    /// Get cell data CharData.
    /// @deprecated (upstream)
    pub fn get(&self, index: usize) -> CharData {
        let content = self.read(index, CELL_CONTENT);
        let cp = content & Content::CODEPOINT_MASK;
        let is_combined = content & Content::IS_COMBINED_MASK != 0;
        let chars = if is_combined {
            self.combined.get(&index).cloned().unwrap_or_default()
        } else if cp != 0 {
            string_from_code_point(cp)
        } else {
            String::new()
        };
        let code = if is_combined {
            last_code_unit(&chars)
        } else {
            cp
        };
        (
            self.read(index, CELL_FG),
            chars,
            content >> Content::WIDTH_SHIFT,
            code,
        )
    }

    /// Set cell data from CharData.
    /// @deprecated (upstream)
    ///
    /// Upstream decides on the string's UTF-16 length (`.length > 1`), so an astral single
    /// character is stored as a combined string here, unlike [`CellData::set_from_char_data`].
    /// The combined cell's code point slot holds the *index* (`index | IS_COMBINED_MASK | ...`),
    /// as upstream writes it.
    pub fn set(&mut self, index: usize, value: &CharData) {
        self.write(index, CELL_FG, value.0);
        let width_bits = value.2 << Content::WIDTH_SHIFT;
        if value.1.encode_utf16().count() > 1 {
            self.combined.insert(index, value.1.clone());
            // `index | ...` is an int32 operation: only the low 32 bits survive.
            let content = (index as u32) | Content::IS_COMBINED_MASK | width_bits;
            self.write(index, CELL_CONTENT, content);
        } else {
            // `''.charCodeAt(0)` is NaN, and `NaN | x` is `x`.
            let unit = value.1.encode_utf16().next().map_or(0, u32::from);
            self.write(index, CELL_CONTENT, unit | width_bits);
        }
    }

    /// primitive getters
    /// use these when only one value is needed, otherwise use `load_cell`
    pub fn get_width(&self, index: usize) -> u32 {
        self.read(index, CELL_CONTENT) >> Content::WIDTH_SHIFT
    }

    /// Test whether content has width.
    pub fn has_width(&self, index: usize) -> u32 {
        self.read(index, CELL_CONTENT) & Content::WIDTH_MASK
    }

    /// Get FG cell component.
    pub fn get_fg(&self, index: usize) -> u32 {
        self.read(index, CELL_FG)
    }

    /// Get BG cell component.
    pub fn get_bg(&self, index: usize) -> u32 {
        self.read(index, CELL_BG)
    }

    /// Test whether contains any chars.
    /// Basically an empty has no content, but other cells might differ in FG/BG
    /// from real empty cells.
    pub fn has_content(&self, index: usize) -> u32 {
        self.read(index, CELL_CONTENT) & Content::HAS_CONTENT_MASK
    }

    /// Get codepoint of the cell.
    /// To be in line with `code` in CharData this either returns
    /// a single UTF32 codepoint or, for a combined string, the **last UTF-16 code unit** of the
    /// string (`charCodeAt(length - 1)`; a low surrogate when the last char is astral).
    pub fn get_code_point(&self, index: usize) -> u32 {
        let content = self.read(index, CELL_CONTENT);
        if content & Content::IS_COMBINED_MASK != 0 {
            return self
                .combined
                .get(&index)
                .map_or(0, |chars| last_code_unit(chars));
        }
        content & Content::CODEPOINT_MASK
    }

    /// Test whether the cell contains a combined string.
    pub fn is_combined(&self, index: usize) -> u32 {
        self.read(index, CELL_CONTENT) & Content::IS_COMBINED_MASK
    }

    /// Returns the string content of the cell.
    pub fn get_string(&self, index: usize) -> String {
        let content = self.read(index, CELL_CONTENT);
        if content & Content::IS_COMBINED_MASK != 0 {
            return self.combined.get(&index).cloned().unwrap_or_default();
        }
        if content & Content::CODEPOINT_MASK != 0 {
            return string_from_code_point(content & Content::CODEPOINT_MASK);
        }
        // return empty string for empty cells
        String::new()
    }

    /// Get state of protected flag.
    pub fn is_protected(&self, index: usize) -> u32 {
        self.read(index, CELL_BG) & BgFlags::PROTECTED
    }

    /// Load data at `index` into `cell`. This is used to access cells in a way that's more
    /// friendly to allocation as it significantly reduces the amount of new objects needed.
    ///
    /// As upstream, `cell.combined_data` is only touched when the cell is combined and
    /// `cell.extended` only when the cell has extended attributes (a missing map entry reads as
    /// the empty string / default attributes).
    pub fn load_cell<'a>(&self, index: usize, cell: &'a mut CellData) -> &'a mut CellData {
        cell.content = self.read(index, CELL_CONTENT);
        cell.fg = self.read(index, CELL_FG);
        cell.bg = self.read(index, CELL_BG);
        if cell.content & Content::IS_COMBINED_MASK != 0 {
            cell.combined_data = self.combined.get(&index).cloned().unwrap_or_default();
        }
        if cell.bg & BgFlags::HAS_EXTENDED != 0 {
            cell.extended = self.extended_attrs.get(&index).copied().unwrap_or_default();
        }
        cell
    }

    /// Set data at `index` to `cell`.
    pub fn set_cell(&mut self, index: usize, cell: &CellData) {
        if cell.content & Content::IS_COMBINED_MASK != 0 {
            self.combined.insert(index, cell.combined_data.clone());
        }
        if cell.bg & BgFlags::HAS_EXTENDED != 0 {
            self.extended_attrs.insert(index, cell.extended);
        }
        self.write(index, CELL_CONTENT, cell.content);
        self.write(index, CELL_FG, cell.fg);
        self.write(index, CELL_BG, cell.bg);
    }

    /// Set cell data from input handler.
    /// Since the input handler see the incoming chars as UTF32 codepoints,
    /// it gets an optimized access method.
    pub fn set_cell_from_codepoint(
        &mut self,
        index: usize,
        code_point: u32,
        width: u32,
        attrs: &AttributeData,
    ) {
        if attrs.bg & BgFlags::HAS_EXTENDED != 0 {
            self.extended_attrs.insert(index, attrs.extended);
        }
        self.write(
            index,
            CELL_CONTENT,
            code_point | (width << Content::WIDTH_SHIFT),
        );
        self.write(index, CELL_FG, attrs.fg);
        self.write(index, CELL_BG, attrs.bg);
    }

    /// Add a codepoint to a cell from input handler.
    /// During input stage combining chars with a width of 0 follow and stack
    /// onto a leading char. Since we already set the attrs
    /// by the previous `set_cell_from_codepoint` call, we can omit it here.
    pub fn add_codepoint_to_cell(&mut self, index: usize, code_point: u32, width: u32) {
        let mut content = self.read(index, CELL_CONTENT);
        if content & Content::IS_COMBINED_MASK != 0 {
            // we already have a combined string, simply add
            let combined = self.combined.entry(index).or_default();
            // LUNETERM: capped (module notes); the width below still applies
            if combined.chars().count() < MAX_COMBINED_CODE_POINTS {
                combined.push_str(&string_from_code_point(code_point));
            }
        } else if content & Content::CODEPOINT_MASK != 0 {
            // normal case for combining chars:
            //  - move current leading char + new one into combined string
            //  - set combined flag
            let mut chars = string_from_code_point(content & Content::CODEPOINT_MASK);
            chars.push_str(&string_from_code_point(code_point));
            self.combined.insert(index, chars);
            content &= !Content::CODEPOINT_MASK; // set codepoint in buffer to 0
            content |= Content::IS_COMBINED_MASK;
        } else {
            // should not happen - we actually have no data in the cell yet
            // simply set the data in the cell buffer with a width of 1
            content = code_point | (1 << Content::WIDTH_SHIFT);
        }
        if width != 0 {
            content &= !Content::WIDTH_MASK;
            content |= width << Content::WIDTH_SHIFT;
        }
        self.write(index, CELL_CONTENT, content);
    }

    pub fn insert_cells(&mut self, pos: usize, n: usize, fill_cell_data: &CellData) {
        if self.length == 0 {
            return;
        }
        let pos = pos % self.length;

        // handle fullwidth at pos: reset cell one to the left if pos is second cell of a wide char
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }

        if n < self.length - pos {
            let mut cell = CellData::new();
            for i in (0..self.length - pos - n).rev() {
                self.load_cell(pos + i, &mut cell);
                self.set_cell(pos + n + i, &cell);
            }
            // LUNETERM: (review S7) `for i in 0..n { self.set_cell(pos + i, fill) }`, batched
            self.set_cells_repeatedly(pos, 1, n, fill_cell_data, fill_cell_data);
        } else {
            // LUNETERM: (review S7) `for i in pos..self.length { self.set_cell(i, fill) }`
            self.set_cells_repeatedly(pos, 1, self.length - pos, fill_cell_data, fill_cell_data);
        }

        // handle fullwidth at line end: reset last cell if it is first cell of a wide char
        if self.get_width(self.length - 1) == 2 {
            self.set_cell_from_codepoint(self.length - 1, 0, 1, fill_cell_data);
        }
    }

    /// LUNETERM: (review S7) `times` repetitions of a `w`-cell unit from `pos`: for each
    /// `p = pos + r × w`, `set_cell(p, head)` and then `set_cell(p + i, stub)` for `i` in
    /// `1..w`. The same cells and entries (data writes past the end are ignored, map entries
    /// are stored at any key, as `set_cell` does), with the cell data written in one pass, the
    /// combined strings inserted as one sorted batch and extended attributes the cells share
    /// as one run, instead of a map insert per cell (review S7 round 4: an underline colour
    /// made every printed cell a `BTreeMap` insert). REP's line fill and `insert_cells`' fill
    /// use it.
    pub fn set_cells_repeatedly(
        &mut self,
        pos: usize,
        w: usize,
        times: usize,
        head: &CellData,
        stub: &CellData,
    ) {
        let end = w
            .checked_mul(times)
            .and_then(|cells| pos.checked_add(cells));
        let Some(end) = end.filter(|_| w != 0) else {
            for r in 0..times {
                let p = pos.saturating_add(r.saturating_mul(w));
                self.set_cell(p, head);
                for i in 1..w {
                    self.set_cell(p.saturating_add(i), stub);
                }
            }
            return;
        };
        let head_slots = [head.content, head.fg, head.bg];
        let stub_slots = [stub.content, stub.fg, stub.bg];
        let (lo, hi) = (pos.min(self.length), end.min(self.length));
        if let Some(slots) = self.data.get_mut(lo * CELL_SIZE..hi * CELL_SIZE) {
            if w == 1 {
                for cell in slots.chunks_exact_mut(CELL_SIZE) {
                    cell.copy_from_slice(&head_slots);
                }
            } else {
                for (i, cell) in slots.chunks_exact_mut(CELL_SIZE).enumerate() {
                    cell.copy_from_slice(if i % w == 0 { &head_slots } else { &stub_slots });
                }
            }
        }
        // the entries, in key order: per cell, the head's or the stub's where flagged
        let is_head = |p: usize| (p - pos) % w == 0;
        let head_combined = head.content & Content::IS_COMBINED_MASK != 0;
        let stub_combined = stub.content & Content::IS_COMBINED_MASK != 0 && w > 1;
        if head_combined || stub_combined {
            let updates = (pos..end)
                .filter_map(|p| match (is_head(p), head_combined, stub_combined) {
                    (true, true, _) => Some((p, head.combined_data.clone())),
                    (false, _, true) => Some((p, stub.combined_data.clone())),
                    _ => None,
                })
                .collect();
            insert_sorted(&mut self.combined, updates);
        }
        let head_extended = head.bg & BgFlags::HAS_EXTENDED != 0;
        let stub_extended = stub.bg & BgFlags::HAS_EXTENDED != 0 && w > 1;
        if head_extended && (w == 1 || stub_extended && stub.extended == head.extended) {
            // every cell takes the same attributes: one run
            if let Some(last) = end.checked_sub(1).filter(|&last| last >= pos) {
                self.extended_attrs.assign(pos, last, Some(head.extended));
            }
        } else if head_extended || stub_extended {
            for p in pos..end {
                match (is_head(p), head_extended, stub_extended) {
                    (true, true, _) => self.extended_attrs.insert(p, head.extended),
                    (false, _, true) => self.extended_attrs.insert(p, stub.extended),
                    _ => {}
                }
            }
        }
    }

    /// LUNETERM: (review S7) the insert-mode shifts of printing `times` characters of `n`
    /// cells from `pos` (`InputHandler::print`'s insert path, `edge` = its `cols - 1`), in one
    /// pass:
    ///
    /// ```text
    /// for j in 0..times {
    ///     self.insert_cells(pos + j * n, n, fill);
    ///     if self.get_width(edge) == 2 {
    ///         self.set_cell_from_codepoint(edge, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
    ///     }
    /// }
    /// ```
    ///
    /// It leaves the same cells and the same `combined` and `extended_attrs` entries,
    /// including the stale ones each step's shift leaves behind (`copy_cells_from` can carry a
    /// stale combined string back into view, so they are state). Costs O(len) plus the entries
    /// written, where the loop costs O(times × len).
    ///
    /// Why it is the loop: with `pos + times × n <= len`, step `j` inserts at
    /// `pos_j = pos + j × n`. At its start the cells from `pos_j` on are the old cells from
    /// `pos` on, shifted by `j × n` (a step's line-end fix-up rewrites the last cell, which the
    /// next step shifts out), so the cells end as one insert of `times × n` (one call below
    /// does that, with the same fix-ups at `pos - 1` and at the end). Each step's shift copies
    /// old cell `c`'s entries to `c + (j + 1) × n`, so key `k` was last written by the
    /// largest step, the smallest `c` on `k`'s residue class in the window the steps reach
    /// (`k - J × n ..= k - n`, `J` = the steps whose shift reaches `k`), that carries the
    /// entry's flag; the fill of a later step at `k` comes after (`k < pos + times × n`), and
    /// the last cell also takes the line-end fix-up's entry at every step that shifts a wide
    /// cell onto it. The fill cell must not be wide: a wide fill would fire the `pos - 1`
    /// fix-up of every later step.
    ///
    /// The `edge` fix-up never fires at the last cell or past it (the insert's own fix-up
    /// leaves no wide last cell; past the end the width reads 0). Left of the last cell (a
    /// line longer than `cols`, left by a column shrink without reflow) with
    /// `edge + 1 >= pos + times × n`, the cell at `edge` after step `j`'s shift is old cell
    /// `s = edge - (j + 1) × n` or a fill (never wide, and only at the last step), and no
    /// other step touches a cell after it has moved past `edge`. So the fix-up rewrites each
    /// wide old cell on `edge`'s residue class in `edge - times × n ..= edge - n` once, as it
    /// passes `edge`, and that cell moves on as a narrow cell with no combined string and the
    /// edge attributes' extended entry. Hence: key `edge` takes the fix-up's entry after its
    /// shift, as the last cell takes the line-end fix-up's; every key right of `edge` sees the
    /// sources as rewritten (a source that reaches it on `edge`'s class has passed `edge`, and
    /// the rewritten ones are only on that class), keys left of it see the old ones; and a
    /// rewritten cell ends as the fix-up's cell. Outside these conditions this runs the loop
    /// itself.
    pub fn insert_cells_repeatedly(
        &mut self,
        pos: usize,
        n: usize,
        times: usize,
        fill_cell_data: &CellData,
        edge: usize,
        edge_attrs: &AttributeData,
    ) {
        let len = self.length;
        // whether the edge fix-up can fire: only left of the last cell
        let edge_fix = edge.checked_add(1).is_some_and(|next| next < len);
        let total = n.checked_mul(times);
        let end = total.and_then(|total| pos.checked_add(total));
        let Some((total, end)) = total.zip(end).filter(|&(total, end)| {
            total != 0
                && end <= len
                && fill_cell_data.get_width() != 2
                && (!edge_fix || edge + 1 >= end)
        }) else {
            for j in 0..times {
                self.insert_cells(pos.saturating_add(j.saturating_mul(n)), n, fill_cell_data);
                if self.get_width(edge) == 2 {
                    self.set_cell_from_codepoint(edge, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
                }
            }
            return;
        };
        // the old cells a step can shift: `pos..sources_end` (n <= total <= len - pos)
        let sources_end = len - n;
        let fill_extended = fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0;
        let (mut any_combined, mut any_extended, mut any_wide) = (false, false, false);
        for c in pos..sources_end {
            let content = self.read(c, CELL_CONTENT);
            any_combined |= content & Content::IS_COMBINED_MASK != 0;
            any_extended |= self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
            any_wide |= content >> Content::WIDTH_SHIFT == 2;
        }
        // Whether a step's shift or fix-up leaves an entry the one insert does not: a combined
        // string (no fill removes one); an extended entry or a wide cell's fix-ups when keys
        // past the fills keep shifted cells (`end < len`; the edge fix-up needs that too); an
        // extended entry under fills that write none. With `end == len` every shifted key is
        // filled later, and a fill with extended attributes overwrites the entry.
        let leaves_entries = any_combined
            || (end < len && (any_extended || any_wide))
            || (any_extended && !fill_extended);
        if !leaves_entries {
            // only the fills write entries, at the keys the one insert fills; its cells are
            // the loop's (the fix-ups' cells are shifted out or filled)
            self.insert_cells(pos, total, fill_cell_data);
            return;
        }
        // the sources the edge fix-up rewrites as they pass `edge`, as indexes: the wide ones
        // on its residue class in `edge - times × n ..= edge - n`
        let edge_extended =
            (edge_attrs.bg & BgFlags::HAS_EXTENDED != 0).then_some(edge_attrs.extended);
        let mut rewritten = Vec::new();
        if edge_fix {
            let mut source = edge.checked_sub(n);
            for _ in 0..times {
                let Some(s) = source.filter(|&s| s >= pos) else {
                    break;
                };
                if self.read(s, CELL_CONTENT) >> Content::WIDTH_SHIFT == 2 {
                    rewritten.push(s - pos);
                }
                source = s.checked_sub(n);
            }
        }
        let fill_combined = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
        if !any_combined
            && !any_extended
            && !fill_combined
            && !fill_extended
            && edge_extended.is_none()
        {
            // no shift copies an entry and no fill or fix-up writes one: the entries stay, and
            // the cells are the one insert's with each rewritten source ending as the
            // fix-up's cell
            self.insert_cells(pos, total, fill_cell_data);
            self.write_edge_fix_ups(pos + total, &rewritten, edge_attrs);
            return;
        }
        // per source `pos + i`, as `load_cell` reads them: its combined string and extended
        // attributes where flagged, and whether it is wide
        let mut combined = Vec::with_capacity(sources_end - pos);
        let mut extended = Vec::with_capacity(sources_end - pos);
        let mut wide = Vec::with_capacity(sources_end - pos);
        for c in pos..sources_end {
            let content = self.read(c, CELL_CONTENT);
            let bg = self.read(c, CELL_BG);
            combined.push(
                (content & Content::IS_COMBINED_MASK != 0)
                    .then(|| self.combined.get(&c).cloned().unwrap_or_default()),
            );
            extended.push(
                (bg & BgFlags::HAS_EXTENDED != 0)
                    .then(|| self.extended_attrs.get(&c).copied().unwrap_or_default()),
            );
            wide.push(content >> Content::WIDTH_SHIFT == 2);
        }

        // the sources as the keys right of `edge` see them
        let rewritten_sources = (!rewritten.is_empty()).then(|| {
            let (mut c, mut e, mut w) = (combined.clone(), extended.clone(), wide.clone());
            for &i in &rewritten {
                if let Some(slot) = c.get_mut(i) {
                    *slot = None;
                }
                if let Some(slot) = e.get_mut(i) {
                    *slot = edge_extended;
                }
                if let Some(slot) = w.get_mut(i) {
                    *slot = false;
                }
            }
            (c, e, w)
        });

        // The one insert copies a rewritten source's old entries to its final key, and the
        // line-end fix-up's entry when that key is the last cell; the loop writes neither
        // there (a final key right of `edge`; at `edge` itself the loop's last shift copies
        // the old entries too). Those keys get their entries back before the pass below.
        let restore: Vec<(usize, Option<String>, Option<ExtendedAttrs>)> = rewritten
            .iter()
            .map(|&i| pos + i + total)
            .filter(|&k| k > edge && k < len)
            .map(|k| {
                (
                    k,
                    self.combined.get(&k).cloned(),
                    self.extended_attrs.get(&k).copied(),
                )
            })
            .collect();

        self.insert_cells(pos, total, fill_cell_data);

        for (k, chars, attrs) in restore {
            match chars {
                Some(chars) => self.combined.insert(k, chars),
                None => self.combined.remove(&k),
            };
            match attrs {
                Some(attrs) => self.extended_attrs.insert(k, attrs),
                None => self.extended_attrs.remove(&k),
            };
        }
        self.write_edge_fix_ups(pos + total, &rewritten, edge_attrs);

        // per source `pos + i`: the first source at or after it on its residue class (mod n)
        // that holds an entry, as an index
        let next_held = |held: &dyn Fn(usize) -> bool| {
            let mut next = vec![usize::MAX; sources_end - pos];
            for i in (0..next.len()).rev() {
                let found = if held(i) {
                    i
                } else {
                    next.get(i + n).copied().unwrap_or(usize::MAX)
                };
                if let Some(slot) = next.get_mut(i) {
                    *slot = found;
                }
            }
            next
        };
        // (empty where no source holds such an entry: every lookup then finds none)
        let held_combined = |c: &[Option<String>]| match any_combined {
            true => next_held(&|i| c.get(i).is_some_and(Option::is_some)),
            false => Vec::new(),
        };
        let held_extended = |e: &[Option<ExtendedAttrs>], any: bool| match any {
            true => next_held(&|i| e.get(i).is_some_and(Option::is_some)),
            false => Vec::new(),
        };
        let next_combined = held_combined(&combined);
        let next_extended = held_extended(&extended, any_extended);
        let rewritten_next = rewritten_sources.as_ref().map(|(c, e, _)| {
            (
                held_combined(c),
                held_extended(e, any_extended || edge_extended.is_some()),
            )
        });
        // A filled key (`k < end`, left of every rewritten source's key) takes no entry from
        // the shifts when the fill writes its own or no source holds one; only the edge
        // fix-up may still write there.
        let skip_filled = (fill_combined || !any_combined) && (fill_extended || !any_extended);
        let mut combined_updates = Vec::new();
        for k in pos + n..len {
            if k < end && skip_filled && !(edge_fix && k == edge) {
                continue;
            }
            // the steps whose shift reaches key k, and the window of their sources: indexes
            // `low..=high` on k's residue class
            let steps = times.min((k - pos) / n);
            let (low, high) = (k - steps * n - pos, k - n - pos);
            let in_fill = k < end;
            let (combined, extended, wide, next_combined, next_extended) =
                match (&rewritten_sources, &rewritten_next) {
                    (Some((c, e, w)), Some((nc, ne))) if k > edge => (c, e, w, nc, ne),
                    _ => (&combined, &extended, &wide, &next_combined, &next_extended),
                };
            let last = |next: &[usize]| next.get(low).copied().filter(|&i| i <= high);
            if !(in_fill && fill_combined) {
                if let Some(Some(chars)) = last(next_combined).and_then(|i| combined.get(i)) {
                    // (an equal entry is left in place: the same state without a copy)
                    if self.combined.get(&k) != Some(chars) {
                        combined_updates.push((k, chars.clone()));
                    }
                }
            }
            if in_fill && fill_extended {
                continue;
            }
            // the window's last entry where a fix-up after each shift writes `fix` over a
            // wide cell (with no fix-up entry, the shifts' last entry)
            let fixed_up = |fix: Option<ExtendedAttrs>| match fix {
                Some(fix) => (low..=high).step_by(n).find_map(|i| {
                    if wide.get(i) == Some(&true) {
                        Some(fix)
                    } else {
                        extended.get(i).copied().flatten()
                    }
                }),
                None => last(next_extended).and_then(|i| extended.get(i).copied().flatten()),
            };
            let value = if k == len - 1 && !in_fill {
                // the line-end fix-up also writes here, after the shift, when it is wide
                fixed_up(fill_extended.then_some(fill_cell_data.extended))
            } else if edge_fix && k == edge {
                // and the edge fix-up here
                fixed_up(edge_extended)
            } else {
                last(next_extended).and_then(|i| extended.get(i).copied().flatten())
            };
            if let Some(value) = value {
                self.extended_attrs.insert(k, value);
            }
        }
        insert_sorted(&mut self.combined, combined_updates);
    }

    /// The cells of `insert_cells_repeatedly`'s rewritten sources (indexes from `pos`, moved
    /// to `end + i`, at `edge` or right of it): the edge fix-up's cell.
    fn write_edge_fix_ups(&mut self, end: usize, rewritten: &[usize], edge_attrs: &AttributeData) {
        for &i in rewritten {
            let k = end.saturating_add(i);
            self.write(
                k,
                CELL_CONTENT,
                NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
            );
            self.write(k, CELL_FG, edge_attrs.fg);
            self.write(k, CELL_BG, edge_attrs.bg);
        }
    }

    pub fn delete_cells(&mut self, pos: usize, n: usize, fill_cell_data: &CellData) {
        if self.length == 0 {
            return;
        }
        let pos = pos % self.length;
        if n < self.length - pos {
            let mut cell = CellData::new();
            for i in 0..self.length - pos - n {
                self.load_cell(pos + n + i, &mut cell);
                self.set_cell(pos + i, &cell);
            }
            for i in self.length - n..self.length {
                self.set_cell(i, fill_cell_data);
            }
        } else {
            for i in pos..self.length {
                self.set_cell(i, fill_cell_data);
            }
        }

        // handle fullwidth at pos:
        // - reset pos-1 if wide char
        // - reset pos if width==0 (previous second cell of a wide char)
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }
        if self.get_width(pos) == 0 && self.has_content(pos) == 0 {
            self.set_cell_from_codepoint(pos, 0, 1, fill_cell_data);
        }
    }

    /// `getWidth(end - 1)`; `end == 0` reads index -1 upstream: undefined, width 0. Evaluated
    /// where upstream evaluates it, i.e. after the start-side reset above may have changed it.
    fn width_before(&self, end: usize) -> u32 {
        end.checked_sub(1).map_or(0, |i| self.get_width(i))
    }

    pub fn replace_cells(
        &mut self,
        start: usize,
        end: usize,
        fill_cell_data: &CellData,
        respect_protect: bool,
    ) {
        let mut start = start;

        // full branching on respectProtect==true, hopefully getting fast JIT for standard case
        if respect_protect {
            if start != 0 && self.get_width(start - 1) == 2 && self.is_protected(start - 1) == 0 {
                self.set_cell_from_codepoint(start - 1, 0, 1, fill_cell_data);
            }
            if end < self.length && self.width_before(end) == 2 && self.is_protected(end) == 0 {
                self.set_cell_from_codepoint(end, 0, 1, fill_cell_data);
            }
            while start < end && start < self.length {
                if self.is_protected(start) == 0 {
                    self.set_cell(start, fill_cell_data);
                }
                start += 1;
            }
            return;
        }

        // handle fullwidth at start: reset cell one to the left if start is second cell of a
        // wide char
        if start != 0 && self.get_width(start - 1) == 2 {
            self.set_cell_from_codepoint(start - 1, 0, 1, fill_cell_data);
        }
        // handle fullwidth at last cell + 1: reset to empty cell if it is second part of a wide
        // char
        if end < self.length && self.width_before(end) == 2 {
            self.set_cell_from_codepoint(end, 0, 1, fill_cell_data);
        }

        while start < end && start < self.length {
            self.set_cell(start, fill_cell_data);
            start += 1;
        }
    }

    /// Resize BufferLine to `cols` filling excess cells with `fill_cell_data`.
    /// The underlying array buffer will not change if there is still enough space
    /// to hold the new buffer line data.
    /// Returns a boolean indicating, whether a `cleanup_memory` call would free
    /// excess memory (true after shrinking > CLEANUP_THRESHOLD).
    ///
    /// "Buffer" is the `Vec`'s capacity (see the module notes). Growing past the capacity
    /// allocates exactly `cols * CELL_SIZE` slots, as the fresh `Uint32Array` does.
    pub fn resize(&mut self, cols: usize, fill_cell_data: &CellData) -> bool {
        if cols == self.length {
            return self.excess_buffer(self.data.len());
        }
        let uint32_cells = cols.saturating_mul(CELL_SIZE);
        if cols > self.length {
            if self.data.capacity() >= uint32_cells {
                // optimization: avoid alloc and data copy if buffer has enough room
                self.data.resize(uint32_cells, 0);
            } else {
                // slow path: new alloc and full data copy
                let mut data = Vec::with_capacity(uint32_cells);
                data.extend_from_slice(&self.data);
                data.resize(uint32_cells, 0);
                self.data = data;
            }
            for i in self.length..cols {
                self.set_cell(i, fill_cell_data);
            }
        } else {
            // optimization: just shrink the view on existing buffer
            self.data.truncate(uint32_cells);
            // Remove any cut off combined data
            self.combined.retain(|&key, _| key < cols);
            // remove any cut off extended attributes
            self.extended_attrs.truncate(cols);
        }
        self.length = cols;
        self.excess_buffer(uint32_cells)
    }

    /// `used * 4 * CLEANUP_THRESHOLD < buffer.byteLength` (in `u32` slots: `used` slots against
    /// the capacity).
    fn excess_buffer(&self, used: usize) -> bool {
        used.saturating_mul(CLEANUP_THRESHOLD) < self.data.capacity()
    }

    /// Cleanup underlying array buffer.
    /// A cleanup will be triggered if the array buffer exceeds the actual used
    /// memory by a factor of CLEANUP_THRESHOLD.
    /// Returns 0 or 1 indicating whether a cleanup happened.
    pub fn cleanup_memory(&mut self) -> u32 {
        if self.excess_buffer(self.data.len()) {
            let mut data = Vec::with_capacity(self.data.len());
            data.extend_from_slice(&self.data);
            self.data = data;
            return 1;
        }
        0
    }

    /// fill a line with fill_cell_data
    pub fn fill(&mut self, fill_cell_data: &CellData, respect_protect: bool) {
        // full branching on respectProtect==true, hopefully getting fast JIT for standard case
        if respect_protect {
            for i in 0..self.length {
                if self.is_protected(i) == 0 {
                    self.set_cell(i, fill_cell_data);
                }
            }
            return;
        }
        self.combined.clear();
        self.extended_attrs.clear();
        for i in 0..self.length {
            self.set_cell(i, fill_cell_data);
        }
    }

    /// alter to a full copy of line
    pub fn copy_from(&mut self, line: &BufferLine) {
        if self.length != line.length {
            self.data = line.data.clone();
        } else {
            // reuse the buffer if lengths are equal
            self.data.clear();
            self.data.extend_from_slice(&line.data);
        }
        self.length = line.length;
        self.combined = line.combined.clone();
        self.extended_attrs = line.extended_attrs.clone();
        self.is_wrapped = line.is_wrapped;
    }

    // `clone` is the derived `Clone`: a fresh exact-size buffer, both maps, `is_wrapped`.

    pub fn get_trimmed_length(&self) -> usize {
        for i in (0..self.length).rev() {
            let content = self.read(i, CELL_CONTENT);
            if content & Content::HAS_CONTENT_MASK != 0 {
                return i + (content >> Content::WIDTH_SHIFT) as usize;
            }
        }
        0
    }

    pub fn get_no_bg_trimmed_length(&self) -> usize {
        for i in (0..self.length).rev() {
            let content = self.read(i, CELL_CONTENT);
            if content & Content::HAS_CONTENT_MASK != 0
                || self.read(i, CELL_BG) & Attributes::CM_MASK != 0
            {
                return i + (content >> Content::WIDTH_SHIFT) as usize;
            }
        }
        0
    }

    /// Copy `len` cells of `src` starting at `src_col` to `self` starting at `dest_col`.
    ///
    /// Out-of-range reads of `src` copy 0, out-of-range writes to `self` are ignored (typed
    /// array semantics). Extended attributes are copied per cell; **combined strings are copied
    /// for every key `>= src_col` of `src`** (ascending), not only for the copied range, as
    /// upstream does.
    pub fn copy_cells_from(
        &mut self,
        src: &BufferLine,
        src_col: usize,
        dest_col: usize,
        len: usize,
        apply_in_reverse: bool,
    ) {
        if apply_in_reverse {
            for cell in (0..len).rev() {
                self.copy_cell_from(src, src_col, dest_col, cell);
            }
        } else {
            for cell in 0..len {
                self.copy_cell_from(src, src_col, dest_col, cell);
            }
        }

        // Move any combined data over as needed, FIXME: repeat for extended attrs
        for (&key, value) in src.combined.range(src_col..) {
            self.combined
                .insert((key - src_col).saturating_add(dest_col), value.clone());
        }
    }

    fn copy_cell_from(&mut self, src: &BufferLine, src_col: usize, dest_col: usize, cell: usize) {
        let src_index = src_col.saturating_add(cell);
        let dest_index = dest_col.saturating_add(cell);
        for field in 0..CELL_SIZE {
            self.write(dest_index, field, src.read(src_index, field));
        }
        if src.read(src_index, CELL_BG) & BgFlags::HAS_EXTENDED != 0 {
            match src.extended_attrs.get(&src_index) {
                Some(&attrs) => {
                    self.extended_attrs.insert(dest_index, attrs);
                }
                None => {
                    self.extended_attrs.remove(&dest_index);
                }
            }
        }
    }

    /// [`copy_cells_from`](Self::copy_cells_from) with `src` being this line itself (upstream
    /// `line.copyCellsFrom(line, ...)`, used by reflow): the same loop over the line's own data
    /// and maps. The cell copy reads cells it has already overwritten when the ranges overlap and
    /// `apply_in_reverse` does not match the direction, and the combined strings are read live
    /// from a snapshot of the keys taken up front, so a forward overlapping shift cascades
    /// (`copy_cells_within(1, 2, 2, false)` over combined cells at 1 and 2 leaves cell 1's string
    /// at 1, 2 and 3). Keys written by the loop are not visited.
    pub fn copy_cells_within(
        &mut self,
        src_col: usize,
        dest_col: usize,
        len: usize,
        apply_in_reverse: bool,
    ) {
        if apply_in_reverse {
            for cell in (0..len).rev() {
                self.copy_cell_within(src_col, dest_col, cell);
            }
        } else {
            for cell in 0..len {
                self.copy_cell_within(src_col, dest_col, cell);
            }
        }

        // Move any combined data over as needed, FIXME: repeat for extended attrs
        let keys: Vec<usize> = self
            .combined
            .range(src_col..)
            .map(|(&key, _)| key)
            .collect();
        for key in keys {
            if let Some(value) = self.combined.get(&key).cloned() {
                self.combined
                    .insert((key - src_col).saturating_add(dest_col), value);
            }
        }
    }

    fn copy_cell_within(&mut self, src_col: usize, dest_col: usize, cell: usize) {
        let src_index = src_col.saturating_add(cell);
        let dest_index = dest_col.saturating_add(cell);
        for field in 0..CELL_SIZE {
            let value = self.read(src_index, field);
            self.write(dest_index, field, value);
        }
        if self.read(src_index, CELL_BG) & BgFlags::HAS_EXTENDED != 0 {
            match self.extended_attrs.get(&src_index).copied() {
                Some(attrs) => {
                    self.extended_attrs.insert(dest_index, attrs);
                }
                None => {
                    self.extended_attrs.remove(&dest_index);
                }
            }
        }
    }

    /// Translates the buffer line to a string.
    ///
    /// * `trim_right` - Whether to trim any empty cells on the right.
    /// * `start_col` - The column to start the string (0-based inclusive), default 0.
    /// * `end_col` - The column to end the string (0-based exclusive), default `len()`.
    /// * `out_columns` - if specified, this vector is cleared and filled with column numbers such
    ///   that the UTF-16 code unit `i` of the returned string is displayed at column
    ///   `out_columns[i]`; `out_columns[units]` (`units` = the string's UTF-16 length) is where
    ///   the character following the string will be displayed.
    ///
    /// **`out_columns` is indexed by UTF-16 code units, not by `char`s or bytes** (upstream
    /// contract): when a single cell is translated to multiple code units (e.g. a surrogate pair
    /// for an astral char) the corresponding entries have the same column number.
    ///
    /// The cost is `O(end_col - start_col)` even past `len()` (cells there read as spaces).
    pub fn translate_to_string(
        &self,
        trim_right: bool,
        start_col: Option<usize>,
        end_col: Option<usize>,
        out_columns: Option<&mut Vec<usize>>,
    ) -> String {
        let mut start_col = start_col.unwrap_or(0);
        let mut end_col = end_col.unwrap_or(self.length);
        let mut out_columns = out_columns;
        if trim_right {
            end_col = end_col.min(self.get_trimmed_length());
        }
        if let Some(columns) = out_columns.as_deref_mut() {
            columns.clear();
        }
        let mut result = String::new();
        while start_col < end_col {
            let content = self.read(start_col, CELL_CONTENT);
            let cp = content & Content::CODEPOINT_MASK;
            let units = if content & Content::IS_COMBINED_MASK != 0 {
                let chars = self
                    .combined
                    .get(&start_col)
                    .map_or("", |chars| chars.as_str());
                result.push_str(chars);
                chars.encode_utf16().count()
            } else if cp != 0 {
                // as `string_from_code_point`
                let ch = char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER);
                result.push(ch);
                ch.len_utf16()
            } else {
                result.push_str(WHITESPACE_CELL_CHAR);
                WHITESPACE_CELL_CHAR.len()
            };
            if let Some(columns) = out_columns.as_deref_mut() {
                for _ in 0..units {
                    columns.push(start_col);
                }
            }
            // always advance by at least 1
            start_col =
                start_col.saturating_add(((content >> Content::WIDTH_SHIFT) as usize).max(1));
        }
        if let Some(columns) = out_columns {
            columns.push(start_col);
        }
        result
    }
}

#[cfg(test)]
#[path = "buffer_line_tests.rs"]
mod tests;
