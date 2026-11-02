//! Port of xterm.js src/common/buffer/BufferLine.ts @ f447274 (tests: BufferLine.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D7):
//!  - `_data` is a `Vec<u32>` of `length x CELL_SIZE` slots, `[content, fg, bg]` per cell with the
//!    bit layout of `buffer/Constants.ts`. It reproduces `Uint32Array` semantics exactly: a read
//!    past the end yields 0, a write past the end is a no-op, and a read of an index that
//!    overflows `usize` is 0 as well. `Vec::capacity()` stands in for the typed array's
//!    `buffer.byteLength` (the underlying `ArrayBuffer`), so `resize` and `cleanup_memory` keep
//!    upstream's reuse/shrink behaviour and return values.
//!  - `_combined` and `_extendedAttrs` are ordered maps (ascending integer keys, as
//!    `Object.keys` on integer-like keys), each a [`RunMap`]: the same entries stored as runs of
//!    keys with one value (`// LUNETERM:` review S7 round 4 for `_extendedAttrs`, so a line in
//!    one underline colour or link is one run; plan M7 step 12, S7 residual 2, for `_combined`,
//!    whose strings are shared `Arc<str>` values, so a line filled with one cluster is one run
//!    and one allocation, not a `String` per cell; M7 fix round 1: lines share the `_combined`
//!    map copy-on-write, [`SharedRunMap`], so lines REP fills afresh with a wide cluster share one
//!    map of a run per cell). Writes to them are as unchecked as JS
//!    object writes: a `set_cell` of a combined cell at an index `>= len()` stores the string
//!    even though the data write is ignored. An `undefined` map value is represented by an absent key (every
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
use std::sync::Arc;

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

#[cfg(test)]
thread_local! {
    /// Combined strings allocated by this thread's lines (tests pin allocations per line).
    static COMBINED_ALLOCATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of combined strings this thread's lines have allocated so far.
#[cfg(test)]
pub(crate) fn combined_allocations() -> u64 {
    COMBINED_ALLOCATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// Passes over a line's cells right of an insert made by this thread's lines: one per
    /// `insert_cells` call and one per batched stuck insert (plan M7 step 13 pins the passes
    /// per REP).
    static INSERT_PASSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of insert passes this thread's lines have made so far.
#[cfg(test)]
pub(crate) fn insert_passes() -> u64 {
    INSERT_PASSES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn count_insert_pass() {
    INSERT_PASSES.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
thread_local! {
    /// Insert-mode line fills this thread's lines took as one shift and the writes (M7 fix
    /// round 3: `insert_cells_repeatedly_then_set`, `insert_cell_strided_then_set`).
    static ONE_SHIFT_FILLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of insert-mode line fills taken as one shift and the writes so far.
#[cfg(test)]
pub(crate) fn one_shift_fills() -> u64 {
    ONE_SHIFT_FILLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn count_one_shift_fill() {
    ONE_SHIFT_FILLS.with(|count| count.set(count.get() + 1));
}

/// LUNETERM: (M7 fix round 3) equality of side-table values with a cheap first check: the same
/// allocation for a shared string (`Arc<str>`'s `==` compares the bytes every time).
trait SameValue {
    fn same_value(&self, other: &Self) -> bool;
}

impl SameValue for Arc<str> {
    fn same_value(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other) || same_text(self, other)
    }
}

/// `a == b`, short strings (a cell's text is a few bytes) compared in place rather than through
/// a `memcmp` call.
fn same_text(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && match a.len() {
            0..=16 => a.bytes().zip(b.bytes()).all(|(x, y)| x == y),
            _ => a == b,
        }
}

impl SameValue for ExtendedAttrs {
    fn same_value(&self, other: &Self) -> bool {
        self == other
    }
}

#[cfg(test)]
impl SameValue for u8 {
    fn same_value(&self, other: &Self) -> bool {
        self == other
    }
}

/// Whether `held` is `chars`: the same allocation, or an equal string.
fn same_str(held: Option<&Arc<str>>, chars: &Arc<str>) -> bool {
    held.is_some_and(|held| held.same_value(chars))
}

/// A combined string as the shared value the line stores (plan M7 step 12): every allocation
/// of one goes through here.
fn shared_str(chars: &str) -> Arc<str> {
    #[cfg(test)]
    COMBINED_ALLOCATIONS.with(|count| count.set(count.get() + 1));
    Arc::from(chars)
}

/// LUNETERM: (review S7 round 4) upstream's `_extendedAttrs` (cell index -> extended
/// attributes) stored as runs of consecutive keys holding one value. A line's cells mostly
/// share their attributes, so a line printed in one underline colour (or inside one link,
/// whose id lives here too) is one run instead of a tree entry per cell: filling, cloning and
/// dropping it no longer cost an allocation per handful of cells. It answers every read as
/// the per-key map does, keys past the line's end included (a write there is stored, as a JS
/// object write is). Plan M7 step 12 (S7 residual 2) stores `_combined` the same way, with
/// `Arc<str>` values: values compare by content, so equal strings at touching keys merge
/// into one run whichever allocation they came from.
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

impl<V: Clone + PartialEq + SameValue> RunMap<V> {
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

    /// Every key held with its value, in key order (one entry per key, as the per-key map).
    #[cfg(test)]
    fn entries(&self) -> Vec<(usize, V)> {
        self.runs
            .iter()
            .flat_map(|(&first, (last, value))| (first..=*last).map(move |k| (k, value.clone())))
            .collect()
    }

    /// The values at the keys `lo..hi`, in order (one walk over the runs there, where `get`
    /// per key searches the tree each time).
    fn dense(&self, lo: usize, hi: usize) -> Vec<Option<V>> {
        let mut values = vec![None; hi.saturating_sub(lo)];
        if values.is_empty() {
            return values;
        }
        let reaching = self
            .runs
            .range(..lo)
            .next_back()
            .filter(|(_, (last, _))| *last >= lo);
        for (&first, (last, value)) in reaching.into_iter().chain(self.runs.range(lo..hi)) {
            let (from, to) = (first.max(lo), last.saturating_add(1).min(hi));
            for slot in values.iter_mut().take(to - lo).skip(from - lo) {
                *slot = Some(value.clone());
            }
        }
        values
    }

    /// `insert` of every `(key, value)` of `updates`, whose keys are ascending and distinct,
    /// with the keys that touch and hold one value assigned as one range.
    fn assign_ascending(&mut self, updates: &[(usize, V)]) {
        let mut rest = updates;
        while let Some(((first, value), tail)) = rest.split_first() {
            let mut last = *first;
            let mut len = 1;
            for (key, next) in tail {
                if last.checked_add(1) != Some(*key) || next != value {
                    break;
                }
                last = *key;
                len += 1;
            }
            self.assign(*first, last, Some(value.clone()));
            rest = rest.get(len..).unwrap_or_default();
        }
    }

    /// The keys from `start` on, in order (one per key).
    fn keys_from(&self, start: usize) -> Vec<usize> {
        let mut keys = Vec::new();
        // a run that starts before `start` may reach it
        if let Some((_, &(last, _))) = self.runs.range(..start).next_back() {
            if last >= start {
                keys.extend(start..=last);
            }
        }
        for (&first, &(last, _)) in self.runs.range(start..) {
            keys.extend(first..=last);
        }
        keys
    }

    /// The number of keys held.
    fn len(&self) -> usize {
        self.runs.iter().fold(0, |sum: usize, (&first, (last, _))| {
            sum.saturating_add((*last - first).saturating_add(1))
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
            .filter(|(_, (_, held))| *held == value)
            .map(|(&first, &(last, _))| (first, last));
        if let Some((first, last)) = before {
            if key <= last {
                return;
            }
            if last.checked_add(1) == Some(key) {
                let joins_next = key
                    .checked_add(1)
                    .and_then(|next| self.runs.get(&next))
                    .is_some_and(|(_, next_value)| *next_value == value);
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
            .is_some_and(|(_, (end, held))| *end >= last && value.as_ref() == Some(held));
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
            .filter(|(_, (end, _))| *end >= first)
            .map(|(&start, (end, held))| (start, *end, held.clone()));
        if let Some((start, end, held)) = before {
            // (start < first <= end, so neither bound overflows)
            if end > last {
                self.runs.insert(last + 1, (end, held.clone()));
            }
            self.runs.insert(start, (first - 1, held));
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
            .filter(|(_, (end, held))| end.checked_add(1) == Some(first) && *held == value)
            .map(|(&start, _)| start);
        if let Some(start) = before {
            self.runs.remove(&start);
            first = start;
        }
        let after = last.checked_add(1).and_then(|next| {
            self.runs
                .get(&next)
                .filter(|(_, held)| *held == value)
                .map(|&(end, _)| (next, end))
        });
        if let Some((next, end)) = after {
            self.runs.remove(&next);
            last = end;
        }
        self.runs.insert(first, (last, value));
    }

    /// For every key `k >= src_col` of `src`, in ascending order, the entry at
    /// `(k - src_col).saturating_add(dest_col)` takes `src`'s value at `k` (`src` is another
    /// map). The shift is monotone, so each run of `src` lands on one range of keys (a run whose
    /// keys saturate ends at `usize::MAX`, which the last such key wins, as in key order), and
    /// assigning the shifted runs in order is the per-key loop.
    fn copy_shifted_from(&mut self, src: &RunMap<V>, src_col: usize, dest_col: usize) {
        let shift = |k: usize| (k - src_col).saturating_add(dest_col);
        let reaching = src
            .runs
            .range(..src_col)
            .next_back()
            .filter(|(_, (last, _))| *last >= src_col)
            .map(|(_, (last, value))| (src_col, *last, value));
        let inside = src
            .runs
            .range(src_col..)
            .map(|(&first, (last, value))| (first, *last, value));
        for (first, last, value) in reaching.into_iter().chain(inside) {
            self.assign(shift(first), shift(last), Some(value.clone()));
        }
    }

    /// (M7 fix round 3) `insert(k, v)` for every `(k, v)` of `updates` (ascending, distinct
    /// keys in `lo..=hi`, `hi < usize::MAX`), with the runs over `lo..=hi` rebuilt once from
    /// the old values and the updates instead of a tree update per key: the runs there are
    /// cut out of the tree (a run that reaches in from before `lo` is cut at `lo`, and the part
    /// past `hi` of the run that holds `hi` is kept), the range's runs built in key order with
    /// touching equal values merged, and those that touch the runs left on either side merged
    /// with them. The same entries, and the one form a set of entries has.
    fn replace_range(&mut self, lo: usize, hi: usize, updates: Vec<(usize, V)>) {
        // per key of `lo..=hi`: its value after the updates, an index into `values`
        let (mut values, mut index) = self.dense_indexed(lo, hi + 1);
        for (key, value) in updates {
            if let Some(slot) = key.checked_sub(lo).and_then(|offset| index.get_mut(offset)) {
                *slot = Some(values.len());
                values.push(value);
            }
        }
        let mut built: Vec<(usize, usize, usize)> = Vec::new();
        for (offset, id) in index.iter().enumerate() {
            let (key, Some(id)) = (lo + offset, *id) else {
                continue;
            };
            match built.last_mut() {
                Some((_, last, held))
                    if last.checked_add(1) == Some(key) && values.get(*held) == values.get(id) =>
                {
                    *last = key;
                }
                _ => built.push((key, key, id)),
            }
        }
        let mut runs: Vec<(usize, (usize, V))> = Vec::with_capacity(built.len() + 1);
        for (first, last, id) in built {
            if let Some(value) = values.get(id) {
                runs.push((first, (last, value.clone())));
            }
        }
        self.splice_runs(lo, hi, runs);
    }

    /// (M7 fix round 3) every key of `lo..=hi` (`hi < usize::MAX`) takes its value from `runs`
    /// (ascending, disjoint runs inside the range, touching ones of equal value merged; a key
    /// they do not cover holds no entry), the others keep theirs: the runs there are cut out of
    /// the tree (a run that reaches in from before `lo` is cut at `lo`, and the part past `hi`
    /// of the run that holds `hi` is kept), `runs` put in, and those that touch the runs left
    /// on either side with the same value merged with them, so the map keeps its one form.
    fn splice_runs(&mut self, lo: usize, hi: usize, mut runs: Vec<(usize, (usize, V))>) {
        // cut the range out
        let mut inside = self.runs.split_off(&lo);
        let mut after = inside.split_off(&(hi + 1));
        let mut tail = None;
        if let Some(mut entry) = self.runs.last_entry() {
            let (end, held) = entry.get_mut();
            if *end >= lo {
                if *end > hi {
                    tail = Some((hi + 1, *end, held.clone()));
                }
                *end = lo - 1;
            }
        }
        if let Some((_, (end, held))) = inside.last_key_value() {
            if *end > hi {
                tail = Some((hi + 1, *end, held.clone()));
            }
        }
        drop(inside);
        // the run left before `lo`, taken in if the first new run touches it with its value
        if let Some((first, (_, value))) = runs.first_mut() {
            let touching = self.runs.last_key_value().is_some_and(|(_, (end, held))| {
                end.checked_add(1) == Some(*first) && held == value
            });
            if touching {
                if let Some((start, _)) = self.runs.pop_last() {
                    *first = start;
                }
            }
        }
        // the part past `hi` kept, or the next run, if the last new run touches it
        let from_after = tail.is_none();
        let next = match tail {
            Some(tail) => Some(tail),
            None => after
                .first_key_value()
                .filter(|&(&first, _)| first == hi + 1)
                .map(|(&first, (end, value))| (first, *end, value.clone())),
        };
        if let Some((first, end, value)) = next {
            match runs.last_mut() {
                Some((_, (last, held))) if last.checked_add(1) == Some(first) && *held == value => {
                    *last = end;
                    if from_after {
                        after.remove(&first);
                    }
                }
                _ if !from_after => runs.push((first, (end, value))),
                _ => {}
            }
        }
        self.runs.extend(runs);
        self.runs.append(&mut after);
    }

    /// `insert(k, v)` for every `(k, v)` of `updates`, whose keys are ascending and distinct.
    /// When no run holds or touches a key in `first - 1 ..= last + 1` (the updates' span, as
    /// on a line the batch writes afresh), the updates' own runs (touching equal values
    /// merged) cannot meet an old run, so they go in as they are, a large batch next to the
    /// map in one ordered merge (`collect` of a sorted sequence builds the tree in bulk) instead
    /// of a tree update per key; otherwise key by key. The same entries either way.
    fn insert_ascending(&mut self, updates: Vec<(usize, V)>) {
        debug_assert!(updates.windows(2).all(|pair| match pair {
            [(a, _), (b, _)] => a < b,
            _ => true,
        }));
        let (Some(&(first, _)), Some(&(last, _))) = (updates.first(), updates.last()) else {
            return;
        };
        let (low, high) = (first.saturating_sub(1), last.saturating_add(1));
        let reaches_low = self
            .runs
            .range(..low)
            .next_back()
            .is_some_and(|(_, (end, _))| *end >= low);
        if reaches_low || self.runs.range(low..=high).next().is_some() {
            if updates.len() < 16 || last == usize::MAX {
                for (key, value) in updates {
                    self.insert(key, value);
                }
            } else {
                self.replace_range(first, last, updates);
            }
            return;
        }
        let mut new_runs: Vec<(usize, (usize, V))> = Vec::new();
        for (key, value) in updates {
            if let Some((_, (end, held))) = new_runs.last_mut() {
                if end.checked_add(1) == Some(key) && *held == value {
                    *end = key;
                    continue;
                }
            }
            new_runs.push((key, (key, value)));
        }
        if new_runs.len() < 16 || new_runs.len().saturating_mul(4) < self.runs.len() {
            self.runs.extend(new_runs);
            return;
        }
        let mut tail = self.runs.split_off(&first);
        let mut batch: BTreeMap<usize, (usize, V)> = new_runs.into_iter().collect();
        self.runs.append(&mut batch);
        self.runs.append(&mut tail);
    }
}

impl<V: Clone + PartialEq + SameValue> RunMap<V> {
    /// (M7 fix round 3) [`dense`](Self::dense) as indexes into one list of the values: one
    /// copy per run there (one for consecutive runs of equal values), where `dense` copies the
    /// value per key (a string is an `Arc`, whose copies cost an atomic count each).
    fn dense_indexed(&self, lo: usize, hi: usize) -> (Vec<V>, Vec<Option<usize>>) {
        let mut values = Vec::new();
        let mut index = vec![None; hi.saturating_sub(lo)];
        if index.is_empty() {
            return (values, index);
        }
        let reaching = self
            .runs
            .range(..lo)
            .next_back()
            .filter(|(_, (last, _))| *last >= lo);
        for (&first, (last, value)) in reaching.into_iter().chain(self.runs.range(lo..hi)) {
            let (from, to) = (first.max(lo), last.saturating_add(1).min(hi));
            // (runs apart that hold one value share its copy: a wide cluster's line holds a
            // run per cell)
            if !values.last().is_some_and(|held: &V| held.same_value(value)) {
                values.push(value.clone());
            }
            let id = values.len() - 1;
            for slot in index.iter_mut().take(to - lo).skip(from - lo) {
                *slot = Some(id);
            }
        }
        (values, index)
    }

    /// (M7 fix round 3) the values at ascending keys from `lo` on, each found in amortized
    /// O(1) by walking the runs once (`get` searches the tree per key).
    fn cursor(&self, lo: usize) -> RunCursor<'_, V> {
        let runs: Box<dyn Iterator<Item = (usize, usize, &V)> + '_> =
            Box::new(self.runs_cut(lo, None));
        RunCursor {
            runs: runs.peekable(),
        }
    }

    /// The runs over the keys `lo..hi` (`None`: every key from `lo` on), cut to them:
    /// `(first, last, value)` in key order.
    fn runs_cut(
        &self,
        lo: usize,
        hi: Option<usize>,
    ) -> impl Iterator<Item = (usize, usize, &V)> + '_ {
        let empty = hi.is_some_and(|hi| lo >= hi);
        let reaching = self
            .runs
            .range(..lo)
            .next_back()
            .filter(|(_, (last, _))| !empty && *last >= lo);
        let inside = match (empty, hi) {
            (true, _) => self.runs.range(0..0),
            (false, Some(hi)) => self.runs.range(lo..hi),
            (false, None) => self.runs.range(lo..),
        };
        reaching
            .into_iter()
            .chain(inside)
            .map(move |(&first, (last, value))| {
                let last = hi.map_or(*last, |hi| (*last).min(hi - 1));
                (first.max(lo), last, value)
            })
    }

    /// The runs over the keys `lo..hi` (`None`: every key from `lo` on), cut to them:
    /// `(first, last, value index)` in key order, with the values (one copy for consecutive
    /// runs of equal values).
    fn runs_in(&self, lo: usize, hi: Option<usize>) -> RangeRuns<V> {
        let mut runs = Vec::new();
        let mut values: Vec<V> = Vec::new();
        for (first, last, value) in self.runs_cut(lo, hi) {
            if !values.last().is_some_and(|held: &V| held.same_value(value)) {
                values.push(value.clone());
            }
            runs.push((first, last, values.len() - 1));
        }
        (runs, values)
    }

    /// Whether `self` and `other` hold the same entries at the keys `lo..hi` (`None`: every key
    /// from `lo` on): the same runs there, as both maps have the one form of their entries.
    fn same_in(&self, other: &Self, lo: usize, hi: Option<usize>) -> bool {
        let mut mine = self.runs_cut(lo, hi);
        let mut theirs = other.runs_cut(lo, hi);
        loop {
            match (mine.next(), theirs.next()) {
                (None, None) => return true,
                (Some((a_first, a_last, a)), Some((b_first, b_last, b)))
                    if a_first == b_first && a_last == b_last && a.same_value(b) => {}
                _ => return false,
            }
        }
    }
}

/// (M7 fix round 3) [`RunMap::cursor`]: the value at each of a run of ascending keys.
struct RunCursor<'a, V> {
    runs: std::iter::Peekable<Box<dyn Iterator<Item = (usize, usize, &'a V)> + 'a>>,
}

impl<'a, V> RunCursor<'a, V> {
    /// The value at `key`; `key` must not be below a key asked before.
    fn at(&mut self, key: usize) -> Option<&'a V> {
        while self.runs.next_if(|&(_, last, _)| last < key).is_some() {}
        self.runs
            .peek()
            .filter(|&&(first, _, _)| first <= key)
            .map(|&(_, _, value)| value)
    }
}

/// LUNETERM: (M7 fix round 1, wide cluster REP) a [`RunMap`] that lines share until one of
/// them changes it (copy on write), with no allocation while empty. A line printed with a wide
/// cluster holds a string at every other key (the stubs hold none), a run per cell that the
/// tree stores one by one; REP's line fill gives every fresh line the same entries
/// ([`BufferLine::set_cells_repeatedly_shared`]), so the lines share one map instead of
/// building and dropping a tree each. It answers every read as the map it holds (none: no
/// entry), and two compare equal when they hold the same entries.
#[derive(Clone, Debug)]
struct SharedRunMap<V>(Option<Arc<RunMap<V>>>);

impl<V> Default for SharedRunMap<V> {
    fn default() -> Self {
        SharedRunMap(None)
    }
}

impl<V: PartialEq> PartialEq for SharedRunMap<V> {
    fn eq(&self, other: &Self) -> bool {
        fn runs<V>(map: &SharedRunMap<V>) -> Option<&BTreeMap<usize, (usize, V)>> {
            map.0
                .as_deref()
                .map(|map| &map.runs)
                .filter(|r| !r.is_empty())
        }
        runs(self) == runs(other)
    }
}

impl<V: Eq> Eq for SharedRunMap<V> {}

impl<V: Clone + PartialEq + SameValue> SharedRunMap<V> {
    /// The map to change: this line's own (a shared one is copied first).
    fn make_mut(&mut self) -> &mut RunMap<V> {
        Arc::make_mut(self.0.get_or_insert_with(Default::default))
    }

    fn get(&self, key: &usize) -> Option<&V> {
        self.0.as_deref()?.get(key)
    }

    /// The largest key held: the last run's last key, `O(log n)`.
    fn last_key(&self) -> Option<usize> {
        self.0
            .as_deref()?
            .runs
            .last_key_value()
            .map(|(_, (last, _))| *last)
    }

    #[cfg(test)]
    fn entries(&self) -> Vec<(usize, V)> {
        self.0.as_deref().map(RunMap::entries).unwrap_or_default()
    }

    fn keys_from(&self, start: usize) -> Vec<usize> {
        self.0
            .as_deref()
            .map(|map| map.keys_from(start))
            .unwrap_or_default()
    }

    fn dense(&self, lo: usize, hi: usize) -> Vec<Option<V>> {
        match self.0.as_deref() {
            Some(map) => map.dense(lo, hi),
            None => vec![None; hi.saturating_sub(lo)],
        }
    }

    fn dense_indexed(&self, lo: usize, hi: usize) -> (Vec<V>, Vec<Option<usize>>) {
        match self.0.as_deref() {
            Some(map) => map.dense_indexed(lo, hi),
            None => (Vec::new(), vec![None; hi.saturating_sub(lo)]),
        }
    }

    fn runs_in(&self, lo: usize, hi: Option<usize>) -> RangeRuns<V> {
        self.0
            .as_deref()
            .map(|map| map.runs_in(lo, hi))
            .unwrap_or_default()
    }

    /// [`RunMap::splice_runs`]. A map shared with another line is not copied whole for it: the
    /// new map takes only the old runs outside the range (with the one reaching past `hi`).
    fn splice_runs(&mut self, lo: usize, hi: usize, runs: Vec<(usize, (usize, V))>) {
        let fresh = match &self.0 {
            Some(map) if Arc::strong_count(map) > 1 => {
                let reaching_past = map
                    .runs
                    .range(lo..=hi)
                    .next_back()
                    .filter(|(_, (end, _))| *end > hi);
                let kept = map
                    .runs
                    .range(..lo)
                    .chain(reaching_past)
                    .chain(map.runs.range(hi.saturating_add(1)..))
                    .map(|(&first, (end, value))| (first, (*end, value.clone())))
                    .collect();
                Some(RunMap { runs: kept })
            }
            _ => None,
        };
        if let Some(fresh) = fresh {
            self.0 = Some(Arc::new(fresh));
        }
        if !runs.is_empty() || !self.is_empty() {
            self.make_mut().splice_runs(lo, hi, runs);
        }
    }

    /// [`RunMap::cursor`] (an empty one where no map is held).
    fn with_cursor<R>(&self, lo: usize, f: impl FnOnce(&mut RunCursor<'_, V>) -> R) -> R {
        let empty = RunMap::default();
        let map = self.0.as_deref().unwrap_or(&empty);
        f(&mut map.cursor(lo))
    }

    /// [`RunMap::same_in`]; one shared map is the same everywhere.
    fn same_in(&self, other: &Self, lo: usize, hi: Option<usize>) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) if Arc::ptr_eq(a, b) => true,
            _ => {
                let empty = RunMap::default();
                let a = self.0.as_deref().unwrap_or(&empty);
                let b = other.0.as_deref().unwrap_or(&empty);
                a.same_in(b, lo, hi)
            }
        }
    }

    fn len(&self) -> usize {
        self.0.as_deref().map_or(0, RunMap::len)
    }

    fn is_empty(&self) -> bool {
        self.0.as_deref().is_none_or(|map| map.runs.is_empty())
    }

    fn clear(&mut self) {
        self.0 = None;
    }

    fn insert(&mut self, key: usize, value: V) {
        self.make_mut().insert(key, value);
    }

    fn remove(&mut self, key: &usize) {
        if !self.is_empty() {
            self.make_mut().remove(key);
        }
    }

    fn truncate(&mut self, end: usize) {
        if !self.is_empty() {
            self.make_mut().truncate(end);
        }
    }

    fn assign(&mut self, first: usize, last: usize, value: Option<V>) {
        // (M7 fix round 3) a run that holds the value at every key already: nothing to change,
        // and a map shared with another line is not copied for it
        let covered = self.0.as_deref().is_some_and(|map| {
            first <= last
                && map
                    .runs
                    .range(..=first)
                    .next_back()
                    .is_some_and(|(_, (end, held))| *end >= last && value.as_ref() == Some(held))
        });
        if !covered && (value.is_some() || !self.is_empty()) {
            self.make_mut().assign(first, last, value);
        }
    }

    fn copy_shifted_from(&mut self, src: &SharedRunMap<V>, src_col: usize, dest_col: usize) {
        if let Some(src) = src.0.as_deref().filter(|map| !map.runs.is_empty()) {
            self.make_mut().copy_shifted_from(src, src_col, dest_col);
        }
    }

    fn insert_ascending(&mut self, updates: Vec<(usize, V)>) {
        if !updates.is_empty() {
            self.make_mut().insert_ascending(updates);
        }
    }

    /// The runs `(first key, (last key, value))`, in key order.
    #[cfg(test)]
    fn runs(&self) -> impl Iterator<Item = (&usize, &(usize, V))> {
        self.0
            .as_deref()
            .into_iter()
            .flat_map(|map| map.runs.iter())
    }
}

/// The cells [`BufferLine::insert_cell_strided_then_set`] writes after its inserts.
struct Overlay<'a> {
    head: &'a CellData,
    stub: &'a CellData,
    template: &'a mut Option<CombinedTemplate>,
}

/// LUNETERM: (M7 fix round 1) a cell's slots and its side-table entries, flagged or stale
/// ([`BufferLine::cell_state`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CellState {
    slots: [u32; CELL_SIZE],
    combined: Option<Arc<str>>,
    extended: Option<ExtendedAttrs>,
}

/// LUNETERM: (M7 fix round 3) everything a line holds for the cells `lo..hi`: their slots and
/// both side tables' entries at those keys, flagged or stale ([`BufferLine::range_state`]).
/// Two are equal exactly when the line holds the same cells and entries there.
/// The side tables are kept whole (the combined map shared, so a line that changes none of its
/// strings is compared in O(1)) and compared at those keys only.
#[derive(Clone, Debug)]
pub(crate) struct RangeState {
    lo: usize,
    keys_end: Option<usize>,
    slots: Vec<u32>,
    combined: SharedRunMap<Arc<str>>,
    extended: RunMap<ExtendedAttrs>,
}

impl PartialEq for RangeState {
    fn eq(&self, other: &Self) -> bool {
        let (lo, keys_end) = (self.lo, self.keys_end);
        (lo, keys_end) == (other.lo, other.keys_end)
            && self.slots == other.slots
            && self.combined.same_in(&other.combined, lo, keys_end)
            && self.extended.same_in(&other.extended, lo, keys_end)
    }
}

/// A side table's runs over a range of keys: `(first, last, index into the values)`, and the
/// values ([`RunMap::runs_in`]).
type RangeRuns<V> = (Vec<(usize, usize, usize)>, Vec<V>);

/// LUNETERM: (M7 fix round 1) the combined strings a [`BufferLine::set_cells_repeatedly_shared`]
/// call left on a line that held none, with the call's arguments: another line that holds none
/// gets the same strings from the same call, so it shares them.
#[derive(Debug)]
pub(crate) struct CombinedTemplate {
    pos: usize,
    w: usize,
    times: usize,
    head: CellData,
    stub: CellData,
    combined: SharedRunMap<Arc<str>>,
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
    combined: SharedRunMap<Arc<str>>,
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
            combined: SharedRunMap::default(),
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

    /// LUNETERM: (M7 fix round 4, code review F6) the largest key of the combined strings,
    /// stale entries and keys past the line's end included, in `O(log n)`: how far upstream's
    /// copy of the strings from a column on (`copy_cells_from`'s FIXME) can reach.
    pub fn last_combined_key(&self) -> Option<usize> {
        self.combined.last_key()
    }

    /// The combined strings as runs `(first key, last key, string)`, for tests that pin how
    /// they are shared.
    #[cfg(test)]
    pub(crate) fn combined_runs(&self) -> Vec<(usize, usize, Arc<str>)> {
        self.combined
            .runs()
            .map(|(&first, (last, chars))| (first, *last, chars.clone()))
            .collect()
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
            self.combined
                .get(&index)
                .map_or_else(String::new, |chars| String::from(&**chars))
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
            self.combined.insert(index, shared_str(&value.1));
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
            return self
                .combined
                .get(&index)
                .map_or_else(String::new, |chars| String::from(&**chars));
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
            // (the cell's own buffer reused: the same string as a fresh copy)
            cell.combined_data.clear();
            if let Some(chars) = self.combined.get(&index) {
                cell.combined_data.push_str(chars);
            }
        }
        if cell.bg & BgFlags::HAS_EXTENDED != 0 {
            cell.extended = self.extended_attrs.get(&index).copied().unwrap_or_default();
        }
        cell
    }

    /// LUNETERM: (M7 fix round 1) everything the line holds for cell `index`: its slots and the
    /// entries at its key in both side tables, flagged or stale.
    pub(crate) fn cell_state(&self, index: usize) -> CellState {
        CellState {
            slots: [
                self.read(index, CELL_CONTENT),
                self.read(index, CELL_FG),
                self.read(index, CELL_BG),
            ],
            combined: self.combined.get(&index).cloned(),
            extended: self.extended_attrs.get(&index).copied(),
        }
    }

    /// LUNETERM: (M7 fix round 3) everything the line holds for the cells `lo..hi`: their slots
    /// and the entries at their keys in both side tables, flagged or stale. One copy of the
    /// slots and of the runs there, where `cell_state` per cell searches both maps each time.
    pub(crate) fn range_state(&self, lo: usize, hi: usize) -> RangeState {
        self.state_with(lo, hi, Some(hi.max(lo)))
    }

    /// The slots of the cells `lo..hi` and the entries at the keys `lo..keys_end` (`None`:
    /// every key from `lo` on).
    fn state_with(&self, lo: usize, hi: usize, keys_end: Option<usize>) -> RangeState {
        let hi = hi.max(lo);
        let (from, to) = (lo.saturating_mul(CELL_SIZE), hi.saturating_mul(CELL_SIZE));
        // (a slot past the data reads 0, as `read` does)
        let mut slots = self
            .data
            .get(from.min(self.data.len())..to.min(self.data.len()))
            .unwrap_or_default()
            .to_vec();
        slots.resize(to - from, 0);
        RangeState {
            lo,
            keys_end,
            slots,
            combined: self.combined.clone(),
            extended: self.extended_attrs.clone(),
        }
    }

    /// LUNETERM: (M7 fix round 3) everything the line holds: its cells' slots and every entry of
    /// both side tables, those at keys past its end included ([`BufferLine::range_state`] of the
    /// whole line and beyond).
    pub(crate) fn line_state(&self) -> RangeState {
        self.state_with(0, self.length, None)
    }

    /// LUNETERM: (M7 fix round 1) cell `index` as `state` holds it ([`BufferLine::cell_state`]).
    pub(crate) fn set_cell_state(&mut self, index: usize, state: &CellState) {
        self.write(index, CELL_CONTENT, state.slots[0]);
        self.write(index, CELL_FG, state.slots[1]);
        self.write(index, CELL_BG, state.slots[2]);
        match &state.combined {
            Some(chars) => self.combined.insert(index, chars.clone()),
            None => self.combined.remove(&index),
        }
        match state.extended {
            Some(attrs) => self.extended_attrs.insert(index, attrs),
            None => self.extended_attrs.remove(&index),
        }
    }

    /// Set data at `index` to `cell`.
    pub fn set_cell(&mut self, index: usize, cell: &CellData) {
        if cell.content & Content::IS_COMBINED_MASK != 0 {
            // (a key that already holds the string keeps it: the same entries, no allocation)
            let held = self.combined.get(&index);
            if held.is_none_or(|held| **held != *cell.combined_data) {
                self.combined.insert(index, shared_str(&cell.combined_data));
            }
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
            // we already have a combined string, simply add (a missing entry reads as empty);
            // the shared string is replaced by a new one, never changed in place
            let held = self.combined.get(&index);
            // LUNETERM: capped (module notes); the width below still applies
            if held.map_or(0, |chars| chars.chars().count()) < MAX_COMBINED_CODE_POINTS {
                let mut chars = held.map_or_else(String::new, |chars| String::from(&**chars));
                chars.push_str(&string_from_code_point(code_point));
                self.combined.insert(index, shared_str(&chars));
            }
        } else if content & Content::CODEPOINT_MASK != 0 {
            // normal case for combining chars:
            //  - move current leading char + new one into combined string
            //  - set combined flag
            let mut chars = string_from_code_point(content & Content::CODEPOINT_MASK);
            chars.push_str(&string_from_code_point(code_point));
            self.combined.insert(index, shared_str(&chars));
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
        #[cfg(test)]
        count_insert_pass();
        if self.length == 0 {
            return;
        }
        let pos = pos % self.length;

        // handle fullwidth at pos: reset cell one to the left if pos is second cell of a wide char
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }

        if n < self.length - pos {
            // LUNETERM: (M7 fix round 3) `for i in (0..self.length - pos - n).rev() {
            // self.set_cell(pos + n + i, self.load_cell(pos + i)) }`, batched
            self.shift_cells_right(pos, n);
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

    /// LUNETERM: (M7 fix round 3) `insert_cells`' shift, `for i in (0..len - pos - n).rev() {
    /// set_cell(pos + n + i, load_cell(pos + i)) }`, in one pass: the slots moved at once and
    /// each side table's entries written in one sorted batch, those a key already holds left
    /// out (`set_cell` stores an equal string again; the entries are the same), where the loop
    /// copies a string and searches both maps per cell. The loop reads every source before it
    /// writes it (key `pos + n + i` is the source of the earlier iteration `i + n`), so every
    /// key `k` in `pos + n..len` takes the old cell at `k - n`: its slots, and its entries where
    /// it is flagged, as `load_cell` reads them (the empty string, default attributes, for a
    /// flagged cell without one); other keys and entries stay. With `n = 0` every cell is
    /// copied onto itself, which gives a flagged cell without an entry the empty one.
    /// Needs `pos + n < len`.
    fn shift_cells_right(&mut self, pos: usize, n: usize) {
        let len = self.length;
        let Some(m) = len.checked_sub(pos).and_then(|room| room.checked_sub(n)) else {
            return;
        };
        if m == 0 {
            return;
        }
        if self.data.len() < len.saturating_mul(CELL_SIZE) {
            let mut cell = CellData::new();
            for i in (0..m).rev() {
                self.load_cell(pos + i, &mut cell);
                self.set_cell(pos + n + i, &cell);
            }
            return;
        }
        let (mut any_combined, mut any_extended) = (false, false);
        for c in pos..pos + m {
            any_combined |= self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK != 0;
            any_extended |= self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
        }
        // (when every flagged source's key already holds its string at its target, as on a
        // line filled with one unit, nothing to write: two walks over the runs, no copies)
        let unchanged = any_combined
            && self.combined.with_cursor(pos, |sources| {
                self.combined.with_cursor(pos + n, |targets| {
                    (0..m).all(|i| {
                        self.read(pos + i, CELL_CONTENT) & Content::IS_COMBINED_MASK == 0
                            || sources.at(pos + i).is_some_and(|chars| {
                                targets
                                    .at(pos + n + i)
                                    .is_some_and(|held| held.same_value(chars))
                            })
                    })
                })
            });
        if any_combined && !unchanged {
            // (sources `pos + i` and keys `pos + n + i` read in one walk)
            let (strings, held) = self.combined.dense_indexed(pos, len);
            let string = |offset: usize| {
                held.get(offset)
                    .copied()
                    .flatten()
                    .and_then(|id| strings.get(id))
            };
            let mut empty: Option<Arc<str>> = None;
            let mut updates = Vec::new();
            for i in 0..m {
                if self.read(pos + i, CELL_CONTENT) & Content::IS_COMBINED_MASK == 0 {
                    continue;
                }
                let (source, now) = (string(i), string(n + i));
                // (the same allocation: nothing to compare)
                if source.is_some() && source.map(Arc::as_ptr) == now.map(Arc::as_ptr) {
                    continue;
                }
                let chars = match source {
                    Some(chars) => chars,
                    None => empty.get_or_insert_with(|| shared_str("")),
                };
                if !same_str(now, chars) {
                    updates.push((pos + n + i, chars.clone()));
                }
            }
            self.combined.insert_ascending(updates);
        }
        if any_extended {
            let held = self.extended_attrs.dense(pos, len);
            let mut updates = Vec::new();
            for i in 0..m {
                if self.read(pos + i, CELL_BG) & BgFlags::HAS_EXTENDED == 0 {
                    continue;
                }
                let attrs = held.get(i).copied().flatten().unwrap_or_default();
                if held.get(n + i).copied().flatten() != Some(attrs) {
                    updates.push((pos + n + i, attrs));
                }
            }
            self.extended_attrs.insert_ascending(updates);
        }
        self.data.copy_within(
            pos * CELL_SIZE..(pos + m) * CELL_SIZE,
            (pos + n) * CELL_SIZE,
        );
    }

    /// LUNETERM: (review S7) `times` repetitions of a `w`-cell unit from `pos`: for each
    /// `p = pos + r × w`, `set_cell(p, head)` and then `set_cell(p + i, stub)` for `i` in
    /// `1..w`. The same cells and entries (data writes past the end are ignored, map entries
    /// are stored at any key, as `set_cell` does), with the cell data written in one pass and
    /// the side-table entries the cells share as one run, instead of a map insert per cell
    /// (review S7 round 4: an underline colour made every printed cell a `BTreeMap` insert;
    /// plan M7 step 12: a combined string was a `String` per cell). The head's and the stub's
    /// strings are one `Arc<str>` each per call. REP's line fill and `insert_cells`' fill use
    /// it.
    pub fn set_cells_repeatedly(
        &mut self,
        pos: usize,
        w: usize,
        times: usize,
        head: &CellData,
        stub: &CellData,
    ) {
        self.set_cells_repeatedly_with(pos, w, times, head, stub, true);
    }

    /// LUNETERM: (M7 fix round 1, wide cluster REP) [`set_cells_repeatedly`](Self::set_cells_repeatedly),
    /// with the combined strings shared between the lines it fills afresh. On a line that holds
    /// no combined string (no entry at any key), the call leaves exactly the entries it writes,
    /// which depend on its arguments only; so the first such call keeps those entries in
    /// `template`, and a later call with the same arguments on a line that holds none takes
    /// them from it (one shared map, [`SharedRunMap`]) instead of writing them. Every other
    /// call is `set_cells_repeatedly` itself. The same cells and entries either way.
    pub(crate) fn set_cells_repeatedly_shared(
        &mut self,
        pos: usize,
        w: usize,
        times: usize,
        head: &CellData,
        stub: &CellData,
        template: &mut Option<CombinedTemplate>,
    ) {
        // (the one-pass form only: `set_cells_repeatedly_with` skips the strings there)
        let one_pass = w != 0
            && w.checked_mul(times)
                .and_then(|cells| pos.checked_add(cells))
                .is_some();
        if !one_pass || !self.combined.is_empty() {
            self.set_cells_repeatedly(pos, w, times, head, stub);
            return;
        }
        let same = template.as_ref().filter(|t| {
            (t.pos, t.w, t.times) == (pos, w, times) && t.head == *head && t.stub == *stub
        });
        if let Some(template) = same {
            self.set_cells_repeatedly_with(pos, w, times, head, stub, false);
            self.combined = template.combined.clone();
            return;
        }
        self.set_cells_repeatedly(pos, w, times, head, stub);
        if !self.combined.is_empty() {
            *template = Some(CombinedTemplate {
                pos,
                w,
                times,
                head: head.clone(),
                stub: stub.clone(),
                combined: self.combined.clone(),
            });
        }
    }

    /// [`set_cells_repeatedly`](Self::set_cells_repeatedly), the combined strings written only
    /// with `write_combined`.
    fn set_cells_repeatedly_with(
        &mut self,
        pos: usize,
        w: usize,
        times: usize,
        head: &CellData,
        stub: &CellData,
        write_combined: bool,
    ) {
        let end = w
            .checked_mul(times)
            .and_then(|cells| pos.checked_add(cells));
        let Some(end) = end.filter(|_| w != 0) else {
            debug_assert!(
                write_combined,
                "the strings are skipped in the one-pass form only"
            );
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
        let head_combined = write_combined && head.content & Content::IS_COMBINED_MASK != 0;
        let stub_combined =
            write_combined && stub.content & Content::IS_COMBINED_MASK != 0 && w > 1;
        // (M7 fix round 3) the string the line holds at `pos` when equal (an allocation less,
        // and later comparisons find the same one)
        let string = |line: &BufferLine, chars: &str| match line.combined.get(&pos) {
            Some(held) if **held == *chars => held.clone(),
            _ => shared_str(chars),
        };
        let head_chars = head_combined.then(|| string(self, &head.combined_data));
        if let Some(chars) = head_chars
            .as_ref()
            .filter(|_| w == 1 || stub_combined && stub.combined_data == head.combined_data)
        {
            // every cell takes the same string: one run
            if let Some(last) = end.checked_sub(1).filter(|&last| last >= pos) {
                self.combined.assign(pos, last, Some(chars.clone()));
            }
        } else if head_combined || stub_combined {
            let stub_chars = stub_combined.then(|| string(self, &stub.combined_data));
            // (M7 fix round 3) when every key already holds its string (as a line the same fill
            // wrote does), nothing to write: one walk over the runs, no copy of the strings
            let held_all = self.combined.with_cursor(pos, |cursor| {
                (pos..end).all(|p| {
                    let chars = if is_head(p) { &head_chars } else { &stub_chars };
                    chars
                        .as_ref()
                        .is_none_or(|chars| cursor.at(p).is_some_and(|held| held.same_value(chars)))
                })
            });
            if !held_all {
                // (M7 fix round 3) a key that already holds the string is left as it is: the
                // same entries, without a search of the map per key
                let (held_strings, held) = self.combined.dense_indexed(pos, end);
                let updates = (pos..end)
                    .filter_map(|p| {
                        let chars = if is_head(p) { &head_chars } else { &stub_chars };
                        let now = held
                            .get(p - pos)
                            .copied()
                            .flatten()
                            .and_then(|id| held_strings.get(id));
                        chars
                            .as_ref()
                            .filter(|chars| !same_str(now, chars))
                            .map(|chars| (p, chars.clone()))
                    })
                    .collect();
                self.combined.insert_ascending(updates);
            }
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
        // (M7 fix round 3: each table read in one walk, not searched per key)
        let (mut strings, source_combined) = match any_combined {
            true => self.combined.dense_indexed(pos, sources_end),
            false => (Vec::new(), Vec::new()),
        };
        // (a flagged source without an entry reads as the empty string)
        let mut empty = None;
        let source_extended = match any_extended {
            true => self.extended_attrs.dense(pos, sources_end),
            false => Vec::new(),
        };
        let mut combined = Vec::with_capacity(sources_end - pos);
        let mut extended = Vec::with_capacity(sources_end - pos);
        let mut wide = Vec::with_capacity(sources_end - pos);
        for (i, c) in (pos..sources_end).enumerate() {
            let content = self.read(c, CELL_CONTENT);
            let bg = self.read(c, CELL_BG);
            let flagged = content & Content::IS_COMBINED_MASK != 0;
            combined.push(match source_combined.get(i).copied().flatten() {
                Some(id) if flagged => Some(id),
                _ if flagged => Some(*empty.get_or_insert_with(|| {
                    strings.push(shared_str(""));
                    strings.len() - 1
                })),
                _ => None,
            });
            extended.push((bg & BgFlags::HAS_EXTENDED != 0).then(|| {
                source_extended
                    .get(i)
                    .copied()
                    .flatten()
                    .unwrap_or_default()
            }));
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
        let restore: Vec<(usize, Option<Arc<str>>, Option<ExtendedAttrs>)> = rewritten
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
        let held_combined = |c: &[Option<usize>]| match any_combined {
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
        // (M7 fix round 3) the strings the keys hold now, read in one walk
        let (now_strings, now_combined) = match any_combined {
            true => self.combined.dense_indexed(pos + n, len),
            false => (Vec::new(), Vec::new()),
        };
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
                let id = last(next_combined).and_then(|i| combined.get(i).copied().flatten());
                if let Some(chars) = id.and_then(|id| strings.get(id)) {
                    // (an equal entry is left in place: the same state without a copy)
                    let held = now_combined
                        .get(k - pos - n)
                        .copied()
                        .flatten()
                        .and_then(|id| now_strings.get(id));
                    if !same_str(held, chars) {
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
        self.combined.insert_ascending(combined_updates);
    }

    /// LUNETERM: (M7 fix round 3, review S2) [`insert_cells_repeatedly`](Self::insert_cells_repeatedly)`(pos,
    /// n, times, fill, edge, edge_attrs)` and then
    /// [`set_cells_repeatedly_shared`](Self::set_cells_repeatedly_shared)`(pos, n, times, head,
    /// stub, template)`, REP's insert-mode line fill of a unit `n` cells wide. The same cells
    /// and entries as the two calls. Where the inserts' entries under the writes do not
    /// survive them and right of the writes are those of one shift by all the inserted cells,
    /// it is step 0's fix-up at `pos - 1`, that one shift ([`shift_cells_right`]), the
    /// line-end fix-up and the writes: O(len + entries written), with no pass that works out
    /// each key's last writer. Otherwise the two calls.
    ///
    /// Let `end = pos + n × times` (at most `len`) and `c` a class of offsets from `pos` (mod
    /// `n`; `c = 0` is the head's, the others the stub's). It is the two calls when `edge >=
    /// len - 1` (the edge fix-up can only hit the last cell, which the line-end fix-up has
    /// made narrow), the fill is not wide, and per side table:
    ///  - under the writes, per class: the written cell carries the table's flag (its entry is
    ///    written last at every key of the class), or no old cell the inserts can shift
    ///    (`pos..len - n`) on the class carries it and the fill does not (if the fill does,
    ///    every class must be of the first kind: it is written at every key);
    ///  - right of the writes, per class: the shifted old cells on it carry the flag all or
    ///    none; and for the extended table, the fill carries none or no shifted cell on the
    ///    last key's class is wide.
    ///
    /// Why:
    ///  - Cells: everything the inserts write under the writes (fills, shifts, fix-ups at
    ///    `pos + j × n - 1` for `j >= 1`) is overwritten, except step 0's fix-up at `pos - 1`,
    ///    which reads that cell before anything else writes it. Right of the writes, key `k`
    ///    ends with the old cell at `k - n × times` (each step moves it `n`, and it stays right
    ///    of every later insert), the last key as the line-end fix-up leaves its final cell (a
    ///    cell it rewrote before is shifted out by the next step).
    ///  - Entries: a shift moves a cell `n` keys right, so a key takes entries only from old
    ///    cells and fills on its own class, and the fix-ups write only the fill's extended
    ///    entry (the edge fix-up never fires, see above). Under the writes, a class of the
    ///    second kind takes no entry from the inserts and none from the writes (unflagged
    ///    there); on one of the first kind the writes' are last. Right of the writes, key `k`'s
    ///    last write is its final cell's: with every cell on its class flagged, that cell's
    ///    entry, which the one shift copies; with none flagged, none, which the one shift
    ///    leaves too. On the last key the line-end fix-up writes the fill's extended entry only
    ///    after a wide cell, which the condition rules out unless the fill has none.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_cells_repeatedly_then_set(
        &mut self,
        pos: usize,
        n: usize,
        times: usize,
        fill_cell_data: &CellData,
        edge: usize,
        edge_attrs: &AttributeData,
        cells: (&CellData, &CellData),
        template: &mut Option<CombinedTemplate>,
    ) {
        let (head, stub) = cells;
        let len = self.length;
        let end = n
            .checked_mul(times)
            .and_then(|cells| pos.checked_add(cells))
            .filter(|&end| n != 0 && times != 0 && end <= len);
        let quiet = end.is_some_and(|end| {
            edge.saturating_add(1) >= len
                && fill_cell_data.get_width() != 2
                && self.data.len() >= len.saturating_mul(CELL_SIZE)
                && self.writes_hide_inserts(pos, n, end, fill_cell_data, (head, stub))
        });
        let Some(end) = end.filter(|_| quiet) else {
            self.insert_cells_repeatedly(pos, n, times, fill_cell_data, edge, edge_attrs);
            self.set_cells_repeatedly_shared(pos, n, times, head, stub, template);
            return;
        };
        #[cfg(test)]
        {
            count_insert_pass();
            count_one_shift_fill();
        }
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }
        if end < len {
            self.shift_cells_right(pos, end - pos);
            if self.get_width(len - 1) == 2 {
                self.set_cell_from_codepoint(len - 1, 0, 1, fill_cell_data);
            }
        }
        self.set_cells_repeatedly_shared(pos, n, times, head, stub, template);
    }

    /// The side-table conditions of
    /// [`insert_cells_repeatedly_then_set`](Self::insert_cells_repeatedly_then_set)'s one
    /// pass, for inserts of `n` cells from `pos` that end at `end <= len` and the cells
    /// written after them (a unit of one or two cells; wider ones take the two calls).
    fn writes_hide_inserts(
        &self,
        pos: usize,
        n: usize,
        end: usize,
        fill_cell_data: &CellData,
        cells: (&CellData, &CellData),
    ) -> bool {
        // (a unit is one or two cells wide: the head's class and the stub's)
        if n == 0 || n > 2 {
            return false;
        }
        let len = self.length;
        let classes = [cells.0, cells.1];
        let hides_combined = classes.map(|cell| cell.is_combined() != 0);
        let hides_extended = classes.map(|cell| cell.bg & BgFlags::HAS_EXTENDED != 0);
        let fill_combined = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
        let fill_extended = fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0;
        let all = |hides: [bool; 2]| hides[0] && (n == 1 || hides[1]);
        if (fill_combined && !all(hides_combined)) || (fill_extended && !all(hides_extended)) {
            return false;
        }
        // per class: whether some / every shifted old cell carries each flag, and whether one
        // is wide
        let mut some = [(false, false, false); 2];
        let mut every = [(true, true); 2];
        for c in pos..len.saturating_sub(n) {
            let class = usize::from(n == 2 && (c - pos) % 2 == 1);
            let content = self.read(c, CELL_CONTENT);
            let combined = content & Content::IS_COMBINED_MASK != 0;
            let extended = self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
            if let (Some(some), Some(every)) = (some.get_mut(class), every.get_mut(class)) {
                some.0 |= combined;
                some.1 |= extended;
                some.2 |= content >> Content::WIDTH_SHIFT == 2;
                every.0 &= combined;
                every.1 &= extended;
            }
        }
        let per_class =
            (0..n).all(|class| {
                let (
                    Some(&(some_combined, some_extended, _)),
                    Some(&(every_combined, every_extended)),
                ) = (some.get(class), every.get(class))
                else {
                    return false;
                };
                let hides = (
                    hides_combined.get(class) == Some(&true),
                    hides_extended.get(class) == Some(&true),
                );
                let under = (hides.0 || !some_combined) && (hides.1 || !some_extended);
                let right = end == len
                    || ((every_combined || !some_combined) && (every_extended || !some_extended));
                under && right
            });
        let last_wide = some
            .get((len - 1 - pos) % n)
            .is_some_and(|&(_, _, wide)| wide);
        per_class && (end == len || !fill_extended || !last_wide)
    }

    /// LUNETERM: (M7 fix round 1) `InputHandler::print`'s insert-mode shifts for `times`
    /// repetitions of a unit that inserts one cell and advances `stride` cells (a narrow base a
    /// joiner widens to two: its stub overwrites the cell the insert shifted), `edge` = its
    /// `cols - 1`, in one pass:
    ///
    /// ```text
    /// for j in 0..times {
    ///     self.insert_cells(pos + j * stride, 1, fill);
    ///     if self.get_width(edge) == 2 {
    ///         self.set_cell_from_codepoint(edge, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
    ///     }
    /// }
    /// ```
    ///
    /// The same cells and `combined` and `extended_attrs` entries, stale ones included, at
    /// O(len + entries written) where the loop costs O(times × len). A stride of 1 is
    /// [`insert_cells_repeatedly`](Self::insert_cells_repeatedly).
    ///
    /// Why it is the loop, for `stride >= 2`, the last insert at `last = pos + (times - 1) ×
    /// stride` with `last + 1 < len`, a fill that is not wide and `edge > last` where the edge
    /// fix-up can fire (left of the last cell); otherwise this runs the loop. Let `s_j = pos +
    /// j × stride` and `c_i` the old cell at `i`.
    ///  - Shifts: step `j` writes every key `k` with `s_j < k < len` from the cell at `k - 1`,
    ///    which is `c_{k-1-j}` (by induction: an old cell at `i` is shifted by every step while
    ///    `i + j >= s_j`, and `k - 1 - j` meets that for every earlier step). So key `k > pos`
    ///    is written by the steps `j < J(k) = min(times, (k - 1 - pos) / stride + 1)`, from the
    ///    sources `k - J(k) ..= k - 1`, the smallest index last; later steps start right of it.
    ///  - Fills: step `j` writes the fill at `s_j` after the shifts that reach it, and no later
    ///    step writes there; a fill key keeps the fill's cell and the shifts' entries but where
    ///    the fill writes its own.
    ///  - Fix-ups: step 0's at `pos - 1` is applied first. Step `j >= 1`'s at `s_j - 1` finds
    ///    the cell the shifts left there for good (`c_{k-J(k)}`, `k = s_j - 1`); if it is wide,
    ///    it ends as the fix-up's cell with the fill's extended entry. The line-end fix-up after
    ///    each step rewrites the cell shifted onto `len - 1`, which the next step drops unread:
    ///    seen only in that key's entries (the fill's after each wide source) and the final last
    ///    cell. The edge fix-up after step `j` rewrites a wide `c_{edge-1-j}` as it lands on
    ///    `edge` (every step reaches it, as `edge > last`); it moves on rewritten, so keys right
    ///    of `edge` read the sources in `edge - times ..= edge - 1` as rewritten and keys left of
    ///    it as they were; key `edge` takes the edge attributes' entry after each wide source.
    pub fn insert_cell_strided(
        &mut self,
        pos: usize,
        stride: usize,
        times: usize,
        fill_cell_data: &CellData,
        edge: usize,
        edge_attrs: &AttributeData,
    ) {
        self.insert_cell_strided_with(pos, stride, times, fill_cell_data, edge, edge_attrs, None);
    }

    /// LUNETERM: (M7 fix round 1) [`insert_cell_strided`](Self::insert_cell_strided) and then
    /// [`set_cells_repeatedly_shared`](Self::set_cells_repeatedly_shared)`(pos, stride, times,
    /// head, stub, template)` over the cells the inserts made room for, in one pass where the
    /// line holds flagged cells (the written range's entries computed once, not written by the
    /// shifts and then overwritten). The same cells and entries as the two calls.
    ///
    /// (M7 fix round 3, review S2) Where the inserts change neither side table, it is step 0's
    /// fix-up at `pos - 1`, one shift by all the inserted cells ([`shift_cells_right`]), the
    /// line-end fix-up and the writes: O(len + entries written). That holds for `stride >= 2`,
    /// `end = pos + stride × times <= len`, `edge >= len - 1` (the edge fix-up can only hit
    /// the last cell, which the line-end fix-up has made narrow), a fill that is not wide, and
    /// per side table either no old cell at `pos..len - 1` carries its flag and neither does
    /// the fill, or every key of `pos..len` holds one entry `S` and the fill carries none or
    /// `S`:
    ///  - Entries: the inserts write a table only where a shifted cell or the fill carries its
    ///    flag (the fix-ups write the fill's extended entry): in the first case nowhere; in the
    ///    second only `S` (a flagged old cell carries its key's entry), onto keys that hold `S`.
    ///  - Cells: what the inserts write under the writes is overwritten, but step 0's fix-up at
    ///    `pos - 1`, which reads that cell before anything else writes it; a fix-up at `s_j -
    ///    1` (`j >= 1`) stays there, under the writes, as later inserts start right of it. Key
    ///    `k >= end` ends with the old cell at `k - times`: it is right of every insert when it
    ///    comes (`k - times + j >= pos + j × stride` for every step `j < times`), so every step
    ///    moves it one; the last key as the line-end fix-up leaves its final cell.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_cell_strided_then_set(
        &mut self,
        pos: usize,
        stride: usize,
        times: usize,
        fill_cell_data: &CellData,
        edge: usize,
        edge_attrs: &AttributeData,
        cells: (&CellData, &CellData),
        template: &mut Option<CombinedTemplate>,
    ) {
        let len = self.length;
        let end = stride
            .checked_mul(times)
            .and_then(|cells| pos.checked_add(cells))
            .filter(|&end| stride >= 2 && times != 0 && end <= len);
        let tables = end
            .filter(|_| {
                edge.saturating_add(1) >= len
                    && fill_cell_data.get_width() != 2
                    && self.data.len() >= len.saturating_mul(CELL_SIZE)
            })
            .and_then(|end| Some((end, self.strided_tables(pos, fill_cell_data)?)));
        if let Some((end, written)) = tables {
            #[cfg(test)]
            {
                count_insert_pass();
                count_one_shift_fill();
            }
            // (with one string `s`: the keys a flagged cell passes, or the fill writes, take it)
            let updates = written.map(|chars| {
                let fill_combined = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
                let (_, held) = self.combined.dense_indexed(pos, len);
                // flagged cells at `pos..pos + i`, for the windows of shifted sources
                let mut flagged = Vec::with_capacity(len - pos + 1);
                flagged.push(0usize);
                for c in pos..len {
                    let here =
                        c + 1 < len && self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK != 0;
                    let before = flagged.last().copied().unwrap_or(0);
                    flagged.push(before + usize::from(here));
                }
                // the keys that hold `s` after the inserts, as runs (the others hold none)
                let mut runs: Vec<(usize, (usize, Arc<str>))> = Vec::new();
                for k in pos..len {
                    let offset = k - pos;
                    let filled = fill_combined && offset % stride == 0 && offset / stride < times;
                    let passed = offset > 0 && {
                        let steps = times.min((offset - 1) / stride + 1);
                        let (low, high) = (offset - steps, offset);
                        flagged.get(high).copied().unwrap_or(0)
                            > flagged.get(low).copied().unwrap_or(0)
                    };
                    if filled || passed || held.get(offset).copied().flatten().is_some() {
                        match runs.last_mut() {
                            Some((_, (last, _))) if *last + 1 == k => *last = k,
                            _ => runs.push((k, (k, chars.clone()))),
                        }
                    }
                }
                runs
            });
            if pos != 0 && self.get_width(pos - 1) == 2 {
                self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
            }
            if end < len {
                match updates {
                    // (the entries are the updates: the slots only)
                    Some(_) => self.data.copy_within(
                        pos * CELL_SIZE..(len - times) * CELL_SIZE,
                        (pos + times) * CELL_SIZE,
                    ),
                    None => self.shift_cells_right(pos, times),
                }
                if self.get_width(len - 1) == 2 {
                    self.set_cell_from_codepoint(len - 1, 0, 1, fill_cell_data);
                }
            }
            if let Some(runs) = updates {
                self.combined.splice_runs(pos, len - 1, runs);
            }
            self.set_cells_repeatedly_shared(pos, stride, times, cells.0, cells.1, template);
            return;
        }
        let overlay = Overlay {
            head: cells.0,
            stub: cells.1,
            template,
        };
        self.insert_cell_strided_with(
            pos,
            stride,
            times,
            fill_cell_data,
            edge,
            edge_attrs,
            Some(overlay),
        );
    }

    /// The side-table condition of [`insert_cell_strided_then_set`](Self::insert_cell_strided_then_set)'s
    /// one pass, and what its inserts write: `None` where it does not hold; `Some(None)` where
    /// the inserts change neither table (per table, no cell at `pos..len - 1` and not the fill
    /// carries its flag, or every key of `pos..len` holds one entry and the fill carries none
    /// or that one); `Some(Some(s))` where the extended table is so and every combined string
    /// at `pos..len` is `s`, every flagged cell at `pos..len - 1` holds one, and the fill
    /// carries none or `s`, so the inserts write `s` only, at the keys a flagged cell passes.
    fn strided_tables(&self, pos: usize, fill_cell_data: &CellData) -> Option<Option<Arc<str>>> {
        let len = self.length;
        let (mut any_combined, mut any_extended) = (false, false);
        for c in pos..len.saturating_sub(1) {
            any_combined |= self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK != 0;
            any_extended |= self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
        }
        let fill_combined = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
        let fill_extended = fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0;
        let last = len.saturating_sub(1);
        let extended_ok = (!any_extended && !fill_extended)
            || match self.extended_attrs.runs_in(pos, Some(len)) {
                (runs, held) if runs.as_slice() == [(pos, last, 0)] => held
                    .first()
                    .is_some_and(|held| !fill_extended || *held == fill_cell_data.extended),
                _ => false,
            };
        if !extended_ok {
            return None;
        }
        if !any_combined && !fill_combined {
            return Some(None);
        }
        let (runs, held) = self.combined.runs_in(pos, Some(len));
        let [held] = held.as_slice() else {
            return None;
        };
        if fill_combined && **held != *fill_cell_data.combined_data {
            return None;
        }
        if runs.as_slice() == [(pos, last, 0)] {
            return Some(None);
        }
        // every flagged cell that can shift holds the string (the runs are in key order)
        let mut run = runs.iter().peekable();
        for c in pos..len.saturating_sub(1) {
            if self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK == 0 {
                continue;
            }
            while run.next_if(|&&(_, end, _)| end < c).is_some() {}
            if run.peek().is_none_or(|&&(first, _, _)| first > c) {
                return None;
            }
        }
        Some(Some(held.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_cell_strided_with(
        &mut self,
        pos: usize,
        stride: usize,
        times: usize,
        fill_cell_data: &CellData,
        edge: usize,
        edge_attrs: &AttributeData,
        overlay: Option<Overlay<'_>>,
    ) {
        // the two calls, where this does not take the general pass
        let set_after = |line: &mut BufferLine, overlay: Option<Overlay<'_>>| {
            if let Some(Overlay {
                head,
                stub,
                template,
            }) = overlay
            {
                line.set_cells_repeatedly_shared(pos, stride, times, head, stub, template);
            }
        };
        if stride == 1 {
            self.insert_cells_repeatedly(pos, 1, times, fill_cell_data, edge, edge_attrs);
            set_after(self, overlay);
            return;
        }
        let len = self.length;
        let edge_fix = edge.checked_add(1).is_some_and(|next| next < len);
        let last = times
            .checked_sub(1)
            .and_then(|steps| steps.checked_mul(stride))
            .and_then(|offset| pos.checked_add(offset));
        // (with the cells written after, the written range ends on the line)
        let span = stride.saturating_mul(times);
        let one_pass = last.is_some_and(|last| {
            last.saturating_add(1) < len
                && (!edge_fix || edge > last)
                && fill_cell_data.get_width() != 2
                && self.data.len() >= len.saturating_mul(CELL_SIZE)
                && (overlay.is_none() || pos.saturating_add(span) <= len)
        });
        if !one_pass {
            for j in 0..times {
                self.insert_cells(
                    pos.saturating_add(j.saturating_mul(stride)),
                    1,
                    fill_cell_data,
                );
                if self.get_width(edge) == 2 {
                    self.set_cell_from_codepoint(edge, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
                }
            }
            set_after(self, overlay);
            return;
        }
        #[cfg(test)]
        count_insert_pass();
        // step 0's fix-up at `pos - 1`
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }

        // what the sources hold: combined strings, extended attributes, wide cells
        let (mut any_combined, mut any_extended, mut any_wide) = (false, false, false);
        for c in pos..len - 1 {
            let content = self.read(c, CELL_CONTENT);
            any_combined |= content & Content::IS_COMBINED_MASK != 0;
            any_extended |= self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
            any_wide |= content >> Content::WIDTH_SHIFT == 2;
        }
        let fill_combined_flag = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
        let fill_extended_flag = fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0;
        // the cells the inserts leave, but those the writes after overwrite
        let moved_from = match overlay {
            Some(_) => pos + span,
            None => pos,
        };
        if !any_combined && !any_extended && !any_wide && !fill_combined_flag && !fill_extended_flag
        {
            // no shift copies an entry, no fill writes one and no fix-up fires (nothing wide):
            // only the cells move
            self.move_cells_strided(pos, stride, times, fill_cell_data, moved_from);
            set_after(self, overlay);
            return;
        }
        if !any_combined && !any_wide && !fill_combined_flag {
            // no shift copies a string and no fix-up fires (nothing wide): the cells move as
            // above, and each key's extended entry is the fill's, else its window's smallest
            // flagged source's (written last), else unchanged; with the cells written after,
            // theirs where they carry one (computed here, so the range is assigned once)
            let old_extended = self.extended_attrs.dense(pos, len);
            let fill_extended =
                (fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0).then_some(fill_cell_data.extended);
            let flagged =
                |line: &BufferLine, c: usize| line.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
            let mut updates = Vec::with_capacity(len - pos);
            // the smallest flagged source at or after the window's low end (`len - 1`: none)
            let mut held_at = pos;
            for k in pos..len {
                let offset = k - pos;
                let steps = if k == pos {
                    0
                } else {
                    times.min((offset - 1) / stride + 1)
                };
                let low = k - steps;
                held_at = held_at.max(low);
                while held_at < len - 1 && !flagged(self, held_at) {
                    held_at += 1;
                }
                let held = (steps > 0 && held_at < k).then(|| {
                    old_extended
                        .get(held_at - pos)
                        .copied()
                        .flatten()
                        .unwrap_or_default()
                });
                let written = overlay
                    .as_ref()
                    .filter(|_| offset < span)
                    .map(|overlay| match offset % stride {
                        0 => overlay.head,
                        _ => overlay.stub,
                    })
                    .filter(|cell| cell.bg & BgFlags::HAS_EXTENDED != 0);
                let is_fill = offset % stride == 0 && offset / stride < times;
                let value = match written {
                    Some(cell) => Some(cell.extended),
                    None if is_fill && fill_extended.is_some() => fill_extended,
                    None => held,
                };
                if let Some(value) = value {
                    if old_extended.get(offset).copied().flatten() != Some(value) {
                        updates.push((k, value));
                    }
                }
            }
            self.move_cells_strided(pos, stride, times, fill_cell_data, moved_from);
            self.extended_attrs.assign_ascending(&updates);
            set_after(self, overlay);
            return;
        }
        // (with the cells written after and no string on the line, in a source or the fill, the
        // strings are the written cells' only: shared with a line filled alike)
        let share = overlay.is_some()
            && self.combined.is_empty()
            && fill_cell_data.content & Content::IS_COMBINED_MASK == 0
            && (pos..len - 1).all(|c| self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK == 0);
        // the strings of an earlier line written alike, if any
        let reuse = overlay
            .as_ref()
            .filter(|_| share)
            .and_then(|overlay| {
                overlay.template.as_ref().filter(|t| {
                    (t.pos, t.w, t.times) == (pos, stride, times)
                        && t.head == *overlay.head
                        && t.stub == *overlay.stub
                })
            })
            .map(|t| t.combined.clone());

        // the sources `pos + i` (`pos..len - 1`, what a shift reads) as `load_cell` reads them,
        // and as keys right of `edge` read them (the wide ones in `edge - times ..= edge - 1`
        // rewritten by the edge fix-up)
        #[derive(Clone)]
        struct Source {
            slot: [u32; CELL_SIZE],
            // (M7 fix round 3: an index into `strings`, so that no source copies a string)
            combined: Option<usize>,
            extended: Option<ExtendedAttrs>,
            wide: bool,
        }
        let edge_extended =
            (edge_attrs.bg & BgFlags::HAS_EXTENDED != 0).then_some(edge_attrs.extended);
        let rewritten_source = Source {
            slot: [
                NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
                edge_attrs.fg,
                edge_attrs.bg,
            ],
            combined: None,
            extended: edge_extended,
            wide: false,
        };
        // (every key's entries read in one walk, for the sources and to skip equal writes)
        let (mut strings, old_combined) = match self.combined.is_empty() {
            true => (Vec::new(), Vec::new()),
            false => self.combined.dense_indexed(pos, len),
        };
        // (a flagged source without an entry reads as the empty string)
        let empty = (pos..len - 1).any(|c| {
            self.read(c, CELL_CONTENT) & Content::IS_COMBINED_MASK != 0
                && old_combined.get(c - pos).copied().flatten().is_none()
        });
        let empty = empty.then(|| {
            strings.push(shared_str(""));
            strings.len() - 1
        });
        let strings = strings;
        let old_string = |offset: usize| {
            old_combined
                .get(offset)
                .copied()
                .flatten()
                .and_then(|id| strings.get(id))
        };
        let old_extended = self.extended_attrs.dense(pos, len);
        let sources: Vec<Source> = (pos..len - 1)
            .zip(&old_extended)
            .map(|(c, attrs)| {
                let chars = old_combined.get(c - pos).copied().flatten();
                let slot = [
                    self.read(c, CELL_CONTENT),
                    self.read(c, CELL_FG),
                    self.read(c, CELL_BG),
                ];
                Source {
                    combined: (slot[0] & Content::IS_COMBINED_MASK != 0)
                        .then(|| chars.or(empty))
                        .flatten(),
                    extended: (slot[2] & BgFlags::HAS_EXTENDED != 0)
                        .then(|| attrs.unwrap_or_default()),
                    wide: slot[0] >> Content::WIDTH_SHIFT == 2,
                    slot,
                }
            })
            .collect();
        let rewritten = |source: &Source, c: usize| {
            edge_fix && source.wide && c < edge && c >= edge.saturating_sub(times)
        };
        // (only where the edge fix-up rewrites a source)
        let right_sources: Option<Vec<Source>> = sources
            .iter()
            .zip(pos..)
            .any(|(source, c)| rewritten(source, c))
            .then(|| {
                sources
                    .iter()
                    .zip(pos..)
                    .map(|(source, c)| match rewritten(source, c) {
                        true => rewritten_source.clone(),
                        false => source.clone(),
                    })
                    .collect()
            });
        // per source: the first source at or after it that holds an entry (`usize::MAX`: none;
        // empty where no source holds one, which every lookup then reads as none)
        let next_held = |sources: &[Source], held: fn(&Source) -> bool| {
            if !sources.iter().any(held) {
                return Vec::new();
            }
            let mut next = vec![usize::MAX; sources.len()];
            let mut found = usize::MAX;
            for (i, (source, slot)) in sources.iter().zip(next.iter_mut()).enumerate().rev() {
                if held(source) {
                    found = i;
                }
                *slot = found;
            }
            next
        };
        let has_combined: fn(&Source) -> bool = |source| source.combined.is_some();
        let has_extended: fn(&Source) -> bool = |source| source.extended.is_some();
        let next = (
            next_held(&sources, has_combined),
            next_held(&sources, has_extended),
        );
        let right_next = right_sources.as_ref().map(|right_sources| {
            (
                next_held(right_sources, has_combined),
                next_held(right_sources, has_extended),
            )
        });

        let fill_slot = [fill_cell_data.content, fill_cell_data.fg, fill_cell_data.bg];
        let fill_combined = (fill_cell_data.content & Content::IS_COMBINED_MASK != 0)
            .then(|| shared_str(&fill_cell_data.combined_data));
        let fill_extended =
            (fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0).then_some(fill_cell_data.extended);
        let is_wide = |slot: &[u32; CELL_SIZE]| slot[0] >> Content::WIDTH_SHIFT == 2;

        // (M7 fix round 3) the strings of the cells written after, one each per call: a string
        // the line already holds at `pos` when equal, else a new one
        let written_strings = {
            let string = |cell: &CellData| {
                (cell.is_combined() != 0).then(|| match old_string(0) {
                    Some(held) if **held == *cell.combined_data => held.clone(),
                    _ => shared_str(&cell.combined_data),
                })
            };
            match &overlay {
                Some(overlay) if reuse.is_none() => (string(overlay.head), string(overlay.stub)),
                _ => (None, None),
            }
        };
        let mut new_slots: Vec<[u32; CELL_SIZE]> = Vec::with_capacity(len - pos);
        let mut combined_updates = Vec::new();
        let mut extended_updates = Vec::new();
        for k in pos..len {
            let offset = k - pos;
            let steps = if k == pos {
                0
            } else {
                times.min((k - 1 - pos) / stride + 1)
            };
            let is_fill = offset % stride == 0 && offset / stride < times;
            // the step `j >= 1` whose `pos - 1` fix-up lands here
            let gap_fix =
                (offset + 1) % stride == 0 && (1..times).contains(&((offset + 1) / stride));
            let (sources, (next_combined, next_extended)) = match (&right_sources, &right_next) {
                (Some(right_sources), Some(right_next)) if edge_fix && k > edge => {
                    (right_sources, right_next)
                }
                _ => (&sources, &next),
            };
            // the window of sources, as indexes: `k - steps ..= k - 1`, the smallest written last
            let (low, high) = (offset - steps, offset.saturating_sub(1));
            let window = || sources.get(low..=high).unwrap_or_default();
            let last_held = |next: &[usize]| {
                (steps > 0)
                    .then(|| next.get(low).copied())
                    .flatten()
                    .filter(|&i| i <= high)
                    .and_then(|i| sources.get(i))
            };
            // the cell written after the inserts, if any
            let written =
                overlay
                    .as_ref()
                    .filter(|_| offset < span)
                    .map(|overlay| match offset % stride {
                        0 => overlay.head,
                        _ => overlay.stub,
                    });
            // cells: the fill, or the last source, as a fix-up after it may leave it
            let slot = match (is_fill, sources.get(low)) {
                (false, Some(source)) if steps > 0 => {
                    let fix = if gap_fix || k == len - 1 {
                        Some((fill_cell_data.fg, fill_cell_data.bg))
                    } else if edge_fix && k == edge {
                        Some((edge_attrs.fg, edge_attrs.bg))
                    } else {
                        None
                    };
                    match fix.filter(|_| is_wide(&source.slot)) {
                        Some((fg, bg)) => [1 << Content::WIDTH_SHIFT, fg, bg],
                        None => source.slot,
                    }
                }
                _ => fill_slot,
            };
            new_slots.push(match written {
                Some(cell) => [cell.content, cell.fg, cell.bg],
                None => slot,
            });
            // combined
            let written_combined = written.filter(|cell| cell.is_combined() != 0);
            let held_here = old_string(offset);
            if written_combined.is_some() {
                if reuse.is_none() {
                    // (M7 fix round 3) the head's or the stub's one string
                    let chars = match offset % stride {
                        0 => &written_strings.0,
                        _ => &written_strings.1,
                    };
                    if let Some(chars) = chars.as_ref().filter(|chars| !same_str(held_here, chars))
                    {
                        combined_updates.push((k, chars.clone()));
                    }
                }
            } else if let Some(chars) = fill_combined.as_ref().filter(|_| is_fill) {
                combined_updates.push((k, chars.clone()));
            } else if let Some(chars) = last_held(next_combined)
                .and_then(|s| s.combined)
                .and_then(|id| strings.get(id))
            {
                if !same_str(held_here, chars) {
                    combined_updates.push((k, chars.clone()));
                }
            }
            // extended
            let held = last_held(next_extended).and_then(|source| source.extended);
            // the window's last entry where a fix-up after each step writes `fix` over a wide
            // source (no fix-up entry: the shifts' last entry)
            let fixed_up = |fix: Option<ExtendedAttrs>| match fix {
                Some(fix) if steps > 0 => window().iter().find_map(|source| {
                    if source.wide {
                        Some(fix)
                    } else {
                        source.extended
                    }
                }),
                _ => held,
            };
            let last_wide = steps > 0 && sources.get(low).is_some_and(|source| source.wide);
            let written_extended = written
                .filter(|cell| cell.bg & BgFlags::HAS_EXTENDED != 0)
                .map(|cell| cell.extended);
            let value = if written_extended.is_some() {
                written_extended
            } else if is_fill && fill_extended.is_some() {
                fill_extended
            } else if k == len - 1 {
                fixed_up(fill_extended)
            } else if edge_fix && k == edge {
                fixed_up(edge_extended)
            } else if gap_fix && fill_extended.is_some() && last_wide {
                fill_extended
            } else {
                held
            };
            if let Some(value) = value {
                if old_extended.get(offset).copied().flatten() != Some(value) {
                    extended_updates.push((k, value));
                }
            }
        }
        for (k, slot) in (pos..).zip(new_slots) {
            self.write(k, CELL_CONTENT, slot[0]);
            self.write(k, CELL_FG, slot[1]);
            self.write(k, CELL_BG, slot[2]);
        }
        self.extended_attrs.assign_ascending(&extended_updates);
        match (reuse, overlay) {
            (Some(shared), _) => self.combined = shared,
            (
                None,
                Some(Overlay {
                    head,
                    stub,
                    template,
                }),
            ) if share => {
                self.combined.insert_ascending(combined_updates);
                if !self.combined.is_empty() {
                    *template = Some(CombinedTemplate {
                        pos,
                        w: stride,
                        times,
                        head: head.clone(),
                        stub: stub.clone(),
                        combined: self.combined.clone(),
                    });
                }
            }
            _ => self.combined.insert_ascending(combined_updates),
        }
    }

    /// LUNETERM: (plan M7 step 13, S7 residual 5) `InputHandler::print`'s inserts of a narrow
    /// unit stuck at the last column (wraparound off, insert mode on, `pos` = its `cols - 1`)
    /// repeated `times` times, in one pass:
    ///
    /// ```text
    /// for _ in 0..times {
    ///     self.insert_cells(pos, 1, fill);
    ///     if self.get_width(pos) == 2 {
    ///         self.set_cell_from_codepoint(pos, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
    ///     }
    ///     self.set_cell(pos, head);
    /// }
    /// ```
    ///
    /// `set_cell(pos, head)` is what `print` writes there (`head` loaded from a cell `print`
    /// wrote with the unit: the same cells and entries, as the unit's writes do not depend on
    /// the cell's old value). It leaves the same cells and `combined` and `extended_attrs`
    /// entries, stale ones included, at O(len + entries written), where the loop costs
    /// O(times × len).
    ///
    /// Why it is the loop, for `fill` and `head` not wide and `pos < len` (otherwise this runs
    /// the loop). Let `m = len - pos` and `c_i` be the old cell at `pos + i`.
    ///  - Fix-ups: `insert_cells`' fix-up at `pos - 1` can fire at the first step only (it
    ///    leaves a narrow cell, and nothing else writes `pos - 1`), so it runs once up front.
    ///    The edge fix-up never fires: after the insert `pos` holds the fill. The line-end
    ///    fix-up rewrites the cell shifted onto `len - 1`, which the next step shifts out
    ///    unread, so it is seen only in that key's entries and in the final last cell.
    ///  - Cells: step `t` (1-based) shifts `[pos, len - 1)` right by one and writes the head
    ///    at `pos`, so at its start `pos + j` holds the head for `j < t - 1` and `c_{j-t+1}`
    ///    otherwise. After `k = min(times, m)` steps `pos + j` holds the head for `j < k`
    ///    and `c_{j-k}` otherwise (one shift by `k`; steps past `m` only rewrite heads), the
    ///    last cell as the line-end fix-up leaves it.
    ///  - Entries: a shifted cell is loaded where its last write put it, so a flagged cell
    ///    carries its entry as first loaded (`c_i`: its old entry, or the empty string and
    ///    default attributes when it had none) and an unflagged one writes none. Key
    ///    `pos + j` (`j >= 1`) is written at step `t` by `c_{j-t}` for `t <= min(j, k)` and
    ///    by the head for `t > j`: it ends with the head's entry if `k > j` and the head has
    ///    one, else with the flagged source of smallest index in `j - min(j, k) ..= j - 1`
    ///    (written latest), else unchanged. Key `len - 1` also takes the line-end fix-up's
    ///    extended entry (`fill`'s, if flagged) after each wide source. Key `pos` takes the
    ///    fill's and then the head's entries at every step: the head's, else the fill's,
    ///    else unchanged.
    pub fn stuck_insert_repeatedly(
        &mut self,
        pos: usize,
        times: usize,
        fill_cell_data: &CellData,
        head: &CellData,
        edge_attrs: &AttributeData,
    ) {
        let len = self.length;
        if times == 0 {
            return;
        }
        if pos >= len
            || fill_cell_data.get_width() == 2
            || head.get_width() == 2
            || self.data.len() < len.saturating_mul(CELL_SIZE)
        {
            for _ in 0..times {
                self.insert_cells(pos, 1, fill_cell_data);
                if self.get_width(pos) == 2 {
                    self.set_cell_from_codepoint(pos, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
                }
                self.set_cell(pos, head);
            }
            return;
        }
        #[cfg(test)]
        count_insert_pass();
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
        }
        let m = len - pos;
        let k = times.min(m);

        // the sources a step can shift, `c_0 ..= c_{m-2}`, as `load_cell` reads them
        let sources = m - 1;
        let mut combined: Vec<Option<Arc<str>>> = Vec::with_capacity(sources);
        let mut extended: Vec<Option<ExtendedAttrs>> = Vec::with_capacity(sources);
        let mut wide = Vec::with_capacity(sources);
        for c in pos..len - 1 {
            let content = self.read(c, CELL_CONTENT);
            let bg = self.read(c, CELL_BG);
            combined.push((content & Content::IS_COMBINED_MASK != 0).then(|| {
                self.combined
                    .get(&c)
                    .cloned()
                    .unwrap_or_else(|| shared_str(""))
            }));
            extended.push(
                (bg & BgFlags::HAS_EXTENDED != 0)
                    .then(|| self.extended_attrs.get(&c).copied().unwrap_or_default()),
            );
            wide.push(content >> Content::WIDTH_SHIFT == 2);
        }
        // per source index: the first index at or after it that holds an entry
        // (`usize::MAX`: none)
        let next_held = |held: &dyn Fn(usize) -> bool| {
            let mut next = vec![usize::MAX; sources + 1];
            for i in (0..sources).rev() {
                let found = if held(i) {
                    i
                } else {
                    next.get(i + 1).copied().unwrap_or(usize::MAX)
                };
                if let Some(slot) = next.get_mut(i) {
                    *slot = found;
                }
            }
            next
        };
        let next_combined = next_held(&|i| combined.get(i).is_some_and(Option::is_some));
        let next_extended = next_held(&|i| extended.get(i).is_some_and(Option::is_some));

        let head_combined = (head.content & Content::IS_COMBINED_MASK != 0).then(|| {
            // (the string `print` left at `pos` is shared when it is the head's)
            self.combined
                .get(&pos)
                .filter(|held| ***held == *head.combined_data)
                .cloned()
                .unwrap_or_else(|| shared_str(&head.combined_data))
        });
        let fill_combined = (fill_cell_data.content & Content::IS_COMBINED_MASK != 0)
            .then(|| shared_str(&fill_cell_data.combined_data));
        let head_extended = (head.bg & BgFlags::HAS_EXTENDED != 0).then_some(head.extended);
        let fill_extended =
            (fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0).then_some(fill_cell_data.extended);

        let mut combined_updates = Vec::new();
        let mut extended_updates = Vec::new();
        if let Some(chars) = head_combined.clone().or(fill_combined) {
            combined_updates.push((pos, chars));
        }
        if let Some(attrs) = head_extended.or(fill_extended) {
            extended_updates.push((pos, attrs));
        }
        for j in 1..m {
            let key = pos + j;
            // the sources that pass key `pos + j` (indexes `low..=high`), and whether heads
            // follow them
            let (low, high) = (j - j.min(k), j - 1);
            let heads = k > j;
            let in_window = |next: &[usize]| next.get(low).copied().filter(|&i| i <= high);
            let chars = match &head_combined {
                Some(chars) if heads => Some(chars.clone()),
                _ => in_window(&next_combined).and_then(|i| combined.get(i).cloned().flatten()),
            };
            if let Some(chars) = chars {
                if self.combined.get(&key) != Some(&chars) {
                    combined_updates.push((key, chars));
                }
            }
            let attrs = match head_extended {
                Some(attrs) if heads => Some(attrs),
                // the line-end fix-up writes after each wide source's shift
                _ if j == m - 1 => (low..=high).find_map(|i| match fill_extended {
                    Some(fix) if wide.get(i) == Some(&true) => Some(fix),
                    _ => extended.get(i).copied().flatten(),
                }),
                _ => in_window(&next_extended).and_then(|i| extended.get(i).copied().flatten()),
            };
            if let Some(attrs) = attrs {
                if self.extended_attrs.get(&key) != Some(&attrs) {
                    extended_updates.push((key, attrs));
                }
            }
        }

        // the cells: `c_0 ..= c_{m-1-k}` shifted by `k`, the heads, the line-end fix-up
        self.data.copy_within(
            pos * CELL_SIZE..(len - k) * CELL_SIZE,
            (pos + k) * CELL_SIZE,
        );
        let head_slots = [head.content, head.fg, head.bg];
        if let Some(slots) = self.data.get_mut(pos * CELL_SIZE..(pos + k) * CELL_SIZE) {
            for cell in slots.chunks_exact_mut(CELL_SIZE) {
                cell.copy_from_slice(&head_slots);
            }
        }
        if k < m && wide.get(m - 1 - k) == Some(&true) {
            // (its entry is among the updates)
            self.write(len - 1, CELL_CONTENT, 1 << Content::WIDTH_SHIFT);
            self.write(len - 1, CELL_FG, fill_cell_data.fg);
            self.write(len - 1, CELL_BG, fill_cell_data.bg);
        }
        self.combined.insert_ascending(combined_updates);
        self.extended_attrs.insert_ascending(extended_updates);
    }

    /// LUNETERM: (M7 fix round 3, review S2) what `InputHandler::print`'s inserts do right of
    /// `start` (= its `cols`) over `periods` passes of REP's line fill in insert mode, each
    /// pass pushing the same `block` cells over `start` `step` cells at a time, in one pass:
    ///
    /// ```text
    /// let pushed: Vec<CellData> = (start..start + block).map(|c| load_cell(c)).collect();
    /// for _ in 0..periods {
    ///     for j in 0..block / step {
    ///         for c in (start..len - step).rev() {
    ///             self.set_cell(c + step, &load_cell(c));
    ///         }
    ///         for i in 0..step {
    ///             self.set_cell(start + i, &pushed[block - (j + 1) * step + i]);
    ///         }
    ///         if self.get_width(len - 1) == 2 {
    ///             self.set_cell_from_codepoint(len - 1, NULL_CELL_CODE, NULL_CELL_WIDTH, fill);
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// Each step is what `insert_cells(p, step, fill)` at a `p < start` does at `start` and
    /// right of it: the cells shift right by `step`, the cells left of `start` that cross it
    /// arrive at `start..start + step` (here the next ones of the block, last first), and the
    /// line-end fix-up rewrites a wide last cell. The block is the cells a pass left at
    /// `start..start + block` (the caller's precondition: the passes push it again, see
    /// `InputHandler::print_repeated`). The same cells and `combined` and `extended_attrs`
    /// entries, stale ones included, at O(len + block) where the loop costs
    /// O(periods × block × len).
    ///
    /// Why it is the loop, for `step >= 1` dividing `block` and `start + block < len`
    /// (otherwise this runs the loop). Let `m = len - start`, `T = periods × block` the cells
    /// pushed in all, and `o_u` the old cell at `start + u` as `load_cell` reads it (the block
    /// is `o_0 ..= o_{block-1}`).
    ///  - Cells: the cell pushed at step `s` (counting from 0 over all passes) at offset `i`
    ///    moves `step` per later step, so it ends at `(S - 1 - s) × step + i` (`S = T / step`
    ///    steps), which is `≡ block - (j + 1) × step + i (mod block)` for `s`'s step `j` in
    ///    its pass: the final cell at offset `t` is `V(t) = o_{t mod block}` for `t < T`; an
    ///    old cell moves `T`, so `V(t) = o_{t-T}` for `t >= T`. Only `len - 1` differs: the
    ///    line-end fix-up rewrites `V(m - 1)` when it is wide. A rewritten last cell is never
    ///    read again (no shift reads `len - 1`).
    ///  - Entries: a flagged cell writes its entries wherever it lands and carries them as
    ///    first loaded (`set_cell` of what `load_cell` read); an unflagged one writes none.
    ///    Key `start + t` is written by every step, by the cell that ends at `t + q × step`
    ///    at the step `S - 1 - q`, for `q = 0..S`, the latest first: it ends with the entry of
    ///    the first flagged `V(t + q × step)`, else unchanged. Key `len - 1` also takes the
    ///    line-end fix-up's extended entry (`fill`'s, if flagged) after each step that leaves a
    ///    wide cell there: there the first `q` whose cell is wide (with a flagged `fill`) or
    ///    flagged decides.
    ///  - Search: the progression `t, t + step, ...` meets `V(t') = o_{t' mod block}` while
    ///    `t' < T`, a cycle through one residue class of the block (`step` divides `block`
    ///    and `T`), so its first flagged cell is within one cycle when the class holds one;
    ///    then, from `t' = T` on, `o_{t'-T}` for `t' - T` from `t mod step` (or `t - T`) up to
    ///    `t`. One table per class of the block and one per class of the old cells answer
    ///    each key in O(1).
    ///  - Periods past `m / block + 2` change nothing: then `T >= m + block`, every final cell
    ///    is `o_{t mod block}` and every key's progression holds a whole cycle before the old
    ///    cells, so the result does not depend on `T`. They are capped there.
    pub(crate) fn insert_block_repeatedly(
        &mut self,
        start: usize,
        block: usize,
        step: usize,
        periods: usize,
        fill_cell_data: &CellData,
    ) {
        let len = self.length;
        if periods == 0 || step == 0 || block == 0 || block % step != 0 {
            debug_assert!(
                periods == 0,
                "insert_block_repeatedly: bad block {block} / {step}"
            );
            return;
        }
        let fast = start.checked_add(block).is_some_and(|end| end < len)
            && self.data.len() >= len.saturating_mul(CELL_SIZE);
        if !fast {
            let mut pushed = Vec::with_capacity(block);
            for c in start..start.saturating_add(block) {
                let mut cell = CellData::new();
                self.load_cell(c, &mut cell);
                pushed.push(cell);
            }
            for _ in 0..periods.min(len / block + 2) {
                for j in 0..block / step {
                    let mut cell = CellData::new();
                    for c in (start..len.saturating_sub(step)).rev() {
                        self.load_cell(c, &mut cell);
                        self.set_cell(c + step, &cell);
                    }
                    for i in 0..step {
                        if let Some(cell) = pushed.get(block - (j + 1) * step + i) {
                            self.set_cell(start + i, cell);
                        }
                    }
                    if len > 0 && self.get_width(len - 1) == 2 {
                        self.set_cell_from_codepoint(
                            len - 1,
                            NULL_CELL_CODE,
                            NULL_CELL_WIDTH,
                            fill_cell_data,
                        );
                    }
                }
            }
            return;
        }
        #[cfg(test)]
        count_insert_pass();
        let m = len - start;
        let periods = periods.min(m / block + 2);
        // (at most m + 2 × block, below 3 × len)
        let total = periods * block;

        // the old cells `o_u`, as `load_cell` reads them
        let old_combined = self.combined.dense(start, len);
        let old_extended = self.extended_attrs.dense(start, len);
        let mut slots = Vec::with_capacity(m);
        let mut combined = Vec::with_capacity(m);
        let mut extended = Vec::with_capacity(m);
        let mut wide = Vec::with_capacity(m);
        for (c, (chars, attrs)) in (start..len).zip(old_combined.iter().zip(&old_extended)) {
            let slot = [
                self.read(c, CELL_CONTENT),
                self.read(c, CELL_FG),
                self.read(c, CELL_BG),
            ];
            combined.push(
                (slot[0] & Content::IS_COMBINED_MASK != 0)
                    .then(|| chars.clone().unwrap_or_else(|| shared_str(""))),
            );
            extended
                .push((slot[2] & BgFlags::HAS_EXTENDED != 0).then(|| attrs.unwrap_or_default()));
            wide.push(slot[0] >> Content::WIDTH_SHIFT == 2);
            slots.push(slot);
        }
        // the old cell each final offset holds: `V(t)`
        let source = |t: usize| if t < total { t % block } else { t - total };

        // per residue `r` of the block: the distance (a multiple of `step`, below `block`) to
        // the first flagged block cell on the cycle `r, r + step, ...` (mod block)
        let first_in_block = |flagged: &dyn Fn(usize) -> bool| -> Vec<Option<usize>> {
            let mut first = vec![None; block];
            let cycle = block / step;
            for class in 0..step {
                let mut next = None;
                for idx in (0..2 * cycle).rev() {
                    let r = class + (idx % cycle) * step;
                    if flagged(r) {
                        next = Some(idx);
                    }
                    if idx < cycle {
                        if let Some(slot) = first.get_mut(r) {
                            *slot = next.map(|found| (found - idx) * step);
                        }
                    }
                }
            }
            first
        };
        // per old offset `u`: the first flagged old cell at or after it on its class (mod
        // step), `usize::MAX` for none
        let next_old = |flagged: &dyn Fn(usize) -> bool| -> Vec<usize> {
            let mut next = vec![usize::MAX; m];
            for u in (0..m).rev() {
                let found = if flagged(u) {
                    u
                } else {
                    next.get(u + step).copied().unwrap_or(usize::MAX)
                };
                if let Some(slot) = next.get_mut(u) {
                    *slot = found;
                }
            }
            next
        };
        // the old cell whose entry key `start + t` ends with: the first flagged `V(t')` on
        // `t' = t, t + step, ... < t + T`
        let latest = |t: usize, in_block: &[Option<usize>], old: &[usize]| -> Option<usize> {
            let from_old = if t < total {
                let delta = in_block.get(t % block).copied().flatten();
                if let Some(delta) = delta.filter(|&delta| delta < total - t) {
                    return Some((t + delta) % block);
                }
                t % step
            } else {
                t - total
            };
            old.get(from_old).copied().filter(|&u| u < t)
        };
        let has_combined = |u: usize| combined.get(u).is_some_and(Option::is_some);
        let has_extended = |u: usize| extended.get(u).is_some_and(Option::is_some);
        let (combined_in_block, combined_old) =
            (first_in_block(&has_combined), next_old(&has_combined));
        let (extended_in_block, extended_old) =
            (first_in_block(&has_extended), next_old(&has_extended));
        let fill_extended =
            (fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0).then_some(fill_cell_data.extended);

        let mut combined_updates = Vec::new();
        let mut extended_updates = Vec::new();
        for t in 0..m {
            let key = start + t;
            let chars = latest(t, &combined_in_block, &combined_old)
                .and_then(|u| combined.get(u).cloned().flatten());
            if let Some(chars) = chars {
                if old_combined.get(t).and_then(Option::as_ref) != Some(&chars) {
                    combined_updates.push((key, chars));
                }
            }
            let attrs = if t == m - 1 {
                // the line-end fix-up writes after each wide cell's shift: a direct walk of
                // the progression, one cycle of the block then the old cells
                let pick = |u: usize| match fill_extended {
                    Some(fix) if wide.get(u) == Some(&true) => Some(fix),
                    _ => extended.get(u).copied().flatten(),
                };
                let mut found = None;
                let mut t2 = t;
                while found.is_none() && t2 < total.min(t.saturating_add(block)) {
                    found = pick(t2 % block);
                    t2 += step;
                }
                let mut u = if t < total { t % step } else { t - total };
                while found.is_none() && u < t {
                    found = pick(u);
                    u += step;
                }
                found
            } else {
                latest(t, &extended_in_block, &extended_old)
                    .and_then(|u| extended.get(u).copied().flatten())
            };
            if let Some(attrs) = attrs {
                if old_extended.get(t).copied().flatten() != Some(attrs) {
                    extended_updates.push((key, attrs));
                }
            }
        }

        // the cells: `V(t)`, the last one as the line-end fix-up leaves it
        let mut new_slots: Vec<[u32; CELL_SIZE]> = (0..m)
            .map(|t| slots.get(source(t)).copied().unwrap_or_default())
            .collect();
        if wide.get(source(m - 1)) == Some(&true) {
            if let Some(last) = new_slots.last_mut() {
                *last = [
                    NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
                    fill_cell_data.fg,
                    fill_cell_data.bg,
                ];
            }
        }
        for (c, slot) in (start..).zip(new_slots) {
            self.write(c, CELL_CONTENT, slot[0]);
            self.write(c, CELL_FG, slot[1]);
            self.write(c, CELL_BG, slot[2]);
        }
        self.combined.insert_ascending(combined_updates);
        self.extended_attrs.insert_ascending(extended_updates);
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

    /// The cells `insert_cell_strided`'s inserts leave at the keys `from..len` (`from >= pos`)
    /// where they move no entry and fire no fix-up: each key's last source, `k - J(k)`, or the
    /// fill. From the right, so a source (a lower key) is read before it is written.
    fn move_cells_strided(
        &mut self,
        pos: usize,
        stride: usize,
        times: usize,
        fill_cell_data: &CellData,
        from: usize,
    ) {
        for k in (from.max(pos)..self.length).rev() {
            let offset = k - pos;
            let slot = if offset % stride == 0 && offset / stride < times {
                [fill_cell_data.content, fill_cell_data.fg, fill_cell_data.bg]
            } else {
                let source = k - times.min((offset - 1) / stride + 1);
                [
                    self.read(source, CELL_CONTENT),
                    self.read(source, CELL_FG),
                    self.read(source, CELL_BG),
                ]
            };
            self.write(k, CELL_CONTENT, slot[0]);
            self.write(k, CELL_FG, slot[1]);
            self.write(k, CELL_BG, slot[2]);
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
            self.combined.truncate(cols);
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
        self.combined
            .copy_shifted_from(&src.combined, src_col, dest_col);
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
        let keys = self.combined.keys_from(src_col);
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
                let chars = self.combined.get(&start_col).map_or("", |chars| &**chars);
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
