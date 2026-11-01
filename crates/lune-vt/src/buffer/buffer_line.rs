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
//!    `Object.keys` on integer-like keys). Writes to them are as unchecked as JS object writes:
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
//!  - LUNETERM (review S4): [`add_codepoint_to_cell`](BufferLine::add_codepoint_to_cell) stores
//!    at most [`MAX_COMBINED_CODE_POINTS`] code points per cell. Upstream appends every combining
//!    character, so `a` followed by any number of U+0301 grows one cell without bound, beyond
//!    what the scrollback limit bounds, and every read of the cell copies it. A code point past
//!    the cap is dropped; the width it asks for is still applied, so layout and the cursor are
//!    as upstream and only the cell's text is shorter. `get_string` (and so REP and the
//!    renderer) sees the capped text.
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
    extended_attrs: BTreeMap<usize, ExtendedAttrs>,
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
            extended_attrs: BTreeMap::new(),
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
            for i in 0..n {
                self.set_cell(pos + i, fill_cell_data);
            }
        } else {
            for i in pos..self.length {
                self.set_cell(i, fill_cell_data);
            }
        }

        // handle fullwidth at line end: reset last cell if it is first cell of a wide char
        if self.get_width(self.length - 1) == 2 {
            self.set_cell_from_codepoint(self.length - 1, 0, 1, fill_cell_data);
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
            self.extended_attrs.retain(|&key, _| key < cols);
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
