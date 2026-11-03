//! Port of xterm.js src/common/buffer/Marker.ts @ f447274, with the marker list of
//! `Buffer.ts` (`markers`, `addMarker`, `clearMarkers`, `clearAllMarkers`, `_removeMarker`,
//! `:600-662`). There is no upstream test file for markers; `marker_tests.rs` pins the listener
//! rules of `addMarker`.
//!
//! Rust notes (plan M5 D1, D4):
//!  - A `Marker` object is a [`MarkerId`] into a [`MarkerSet`]; the set owns the markers and
//!    is the `ListObserver` of its buffer's lines, applying the three listeners `addMarker`
//!    registers, with the effect of applying them to every marker in creation order. A
//!    disposed marker is gone from the set ([`MarkerSet::get`] is `None`), where upstream keeps
//!    an `isDisposed` object with `line == -1`.
//!  - `// LUNETERM:` (review S7 residual 1, plan M7 D14) the set is indexed by line with a trim
//!    offset instead of a list every event walks (see [`MarkerSet`]); a marker's line is
//!    computed, so [`MarkerSet::get`] and [`MarkerSet::iter`] return [`Marker`] by value.
//!  - `Marker.onDispose` becomes the disposal log: [`MarkerSet::take_disposed`] drains the ids
//!    disposed since the last call, each once, in disposal order. The host forwards them to
//!    whoever listened upstream (`OscLinkService`). The log holds at most one id per marker ever
//!    created.
//!  - `_isClearing` guards `_removeMarker` against removing from the list `clearMarkers` is
//!    walking. Here removal and disposal are one step in every path, so the flag is not needed;
//!    the observable order (disposal order, the survivors' order) is the same.
//!  - Ids come from one process-wide counter (upstream's static `_nextId`, starting at 1), so
//!    markers of the normal and alternate buffers never collide.
//!  - Marker lines are `i64`: a trimmed marker goes below 0 before it is disposed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::{Bound, RangeBounds};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::circular_list::{DeleteEvent, InsertEvent, ListObserver};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Upstream `Marker.id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarkerId(pub u64);

/// A line number that follows its line as the buffer scrolls (upstream `Marker`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub id: MarkerId,
    pub line: i64,
}

/// The live markers of one state (attached or detached) by line: `(line key, id)` pairs and the
/// number of markers per key, so the markers on a line are a range and their count one lookup.
#[derive(Debug)]
struct LineIndex<K> {
    set: BTreeSet<(K, MarkerId)>,
    counts: BTreeMap<K, usize>,
}

impl<K> Default for LineIndex<K> {
    fn default() -> Self {
        Self {
            set: BTreeSet::new(),
            counts: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Copy> LineIndex<K> {
    fn insert(&mut self, key: K, id: MarkerId) {
        if self.set.insert((key, id)) {
            *self.counts.entry(key).or_insert(0) += 1;
        }
    }

    fn remove(&mut self, key: K, id: MarkerId) {
        if self.set.remove(&(key, id)) {
            if let Some(count) = self.counts.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&key);
                }
            }
        }
    }

    fn count(&self, key: K) -> usize {
        self.counts.get(&key).copied().unwrap_or(0)
    }

    /// The ids on `key`, in id order.
    fn on(&self, key: K) -> impl Iterator<Item = MarkerId> + '_ {
        self.set
            .range((key, MarkerId(0))..=(key, MarkerId(u64::MAX)))
            .map(|&(_, id)| id)
    }

    /// The pairs with a key in `range`, collected (callers re-key them).
    fn collect(&self, range: impl RangeBounds<K>) -> Vec<(K, MarkerId)> {
        let lo = match range.start_bound() {
            Bound::Included(&k) => Bound::Included((k, MarkerId(0))),
            Bound::Excluded(&k) => Bound::Excluded((k, MarkerId(u64::MAX))),
            Bound::Unbounded => Bound::Unbounded,
        };
        let hi = match range.end_bound() {
            Bound::Included(&k) => Bound::Included((k, MarkerId(u64::MAX))),
            Bound::Excluded(&k) => Bound::Excluded((k, MarkerId(0))),
            Bound::Unbounded => Bound::Unbounded,
        };
        self.set.range((lo, hi)).copied().collect()
    }
}

/// Where a live marker is.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// Follows list events: `line = key - offset` (see [`MarkerSet::offset`]).
    Attached(i128),
    /// `Buffer::clear` replaced the line list, so the marker stays on `line` and no longer
    /// hears the new list (`detach_all`).
    Detached(i64),
}

/// The markers of one buffer (upstream `Buffer.markers`) and their listeners.
///
/// LUNETERM: (review S7 residual 1) upstream walks every marker on every list event, so a
/// trim, insert or delete costs O(markers) however few move; a link open across a long
/// scrollback holds one marker per line and made every scrolled line cost that walk. Here an
/// attached marker is stored as `key = line + offset`: a trim bumps `offset` (every line goes
/// up by the amount at once) and disposes only the keys below it, and inserts, deletes and
/// rotations re-key only the markers in the lines they affect (found by a range of
/// `attached`). Keys are `i128` so `line + offset` never saturates: every line reads back as
/// the `i64` the per-marker walk computes (which saturates at the `i64` ends; the events
/// that can saturate are applied per marker, in `i64`, as the walk does). Disposal order and
/// creation order are the walk's (`marker_tests.rs` pins it against a copy of the walk).
#[derive(Debug, Default)]
pub struct MarkerSet {
    /// Sum of every trim amount so far.
    offset: i128,
    /// Every live marker; id order is creation order.
    by_id: BTreeMap<MarkerId, Slot>,
    attached: LineIndex<i128>,
    detached: LineIndex<i64>,
    disposed: Vec<MarkerId>,
    /// Markers visited, moved or disposed by events and removals (tests pin each event to the
    /// markers it affects).
    #[cfg(test)]
    pub(crate) visits: u64,
}

impl MarkerSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream `Buffer.addMarker`.
    pub fn add(&mut self, line: i64) -> MarkerId {
        let id = MarkerId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let key = self.key(line);
        self.by_id.insert(id, Slot::Attached(key));
        self.attached.insert(key, id);
        id
    }

    /// The live marker, `None` once disposed.
    pub fn get(&self, id: MarkerId) -> Option<Marker> {
        self.by_id.get(&id).map(|&slot| Marker {
            id,
            line: self.line_of(slot),
        })
    }

    /// The live markers in creation order.
    pub fn iter(&self) -> impl Iterator<Item = Marker> + '_ {
        self.by_id.iter().map(|(&id, &slot)| Marker {
            id,
            line: self.line_of(slot),
        })
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// LUNETERM: (review S7 residual 1) the ids of the live markers on line `y` (attached ones
    /// in id order, then detached ones in id order), without walking the others.
    pub fn ids_on_line(&self, y: i64) -> impl Iterator<Item = MarkerId> + '_ {
        self.attached.on(self.key(y)).chain(self.detached.on(y))
    }

    /// LUNETERM: (review S7 residual 1) how many live markers are on line `y`, in O(log n).
    pub fn count_on_line(&self, y: i64) -> usize {
        self.attached.count(self.key(y)) + self.detached.count(y)
    }

    /// Upstream `Marker.dispose`: idempotent; the marker leaves the set and is logged.
    pub fn dispose(&mut self, id: MarkerId) {
        if self.remove(id) {
            self.disposed.push(id);
        }
    }

    /// LUNETERM: (review F6) disposes every marker in `ids` (unknown ones are ignored), logged
    /// in creation order; `dispose` per id would log them in the set's iteration order.
    pub fn dispose_all(&mut self, ids: &HashSet<MarkerId>) {
        let mut ids: Vec<MarkerId> = ids.iter().copied().collect();
        ids.sort_unstable();
        self.dispose_sorted(ids);
    }

    /// Upstream `Buffer.clearMarkers`: disposes the markers on line `y`.
    pub fn clear_line(&mut self, y: i64) {
        let mut ids: Vec<MarkerId> = self.ids_on_line(y).collect();
        ids.sort_unstable();
        self.dispose_sorted(ids);
    }

    /// Upstream `Buffer.clearAllMarkers`.
    pub fn clear_all(&mut self) {
        #[cfg(test)]
        {
            self.visits += self.by_id.len() as u64;
        }
        self.disposed.extend(self.by_id.keys().copied());
        self.by_id.clear();
        self.attached = LineIndex::default();
        self.detached = LineIndex::default();
    }

    /// The ids disposed since the last call, in disposal order.
    pub fn take_disposed(&mut self) -> Vec<MarkerId> {
        std::mem::take(&mut self.disposed)
    }

    /// The current markers stop following list events but stay in the set (`Buffer.clear`
    /// replaces `lines`; markers added afterwards follow the new list).
    pub fn detach_all(&mut self) {
        let attached = std::mem::take(&mut self.attached);
        #[cfg(test)]
        {
            self.visits += attached.set.len() as u64;
        }
        for (key, id) in attached.set {
            let line = self.line_at(key);
            self.by_id.insert(id, Slot::Detached(line));
            self.detached.insert(line, id);
        }
    }

    /// The key of an attached marker on `line`.
    fn key(&self, line: i64) -> i128 {
        i128::from(line) + self.offset
    }

    /// The line of an attached marker's key. Always in `i64` range: keys are written as
    /// `line + offset` with `line` an `i64`, and a trim disposes every key that would go below
    /// 0 (so the line only shrinks towards 0 as `offset` grows).
    fn line_at(&self, key: i128) -> i64 {
        i64::try_from(key - self.offset).unwrap_or(i64::MAX)
    }

    fn line_of(&self, slot: Slot) -> i64 {
        match slot {
            Slot::Attached(key) => self.line_at(key),
            Slot::Detached(line) => line,
        }
    }

    /// Removes a live marker from every index; `false` if unknown.
    fn remove(&mut self, id: MarkerId) -> bool {
        #[cfg(test)]
        {
            self.visits += 1;
        }
        match self.by_id.remove(&id) {
            Some(Slot::Attached(key)) => self.attached.remove(key, id),
            Some(Slot::Detached(line)) => self.detached.remove(line, id),
            None => return false,
        }
        true
    }

    /// Disposes the known ids of `ids` (in that order; unknown ones are skipped).
    fn dispose_sorted(&mut self, ids: Vec<MarkerId>) {
        for id in ids {
            self.dispose(id);
        }
    }

    /// Moves an attached marker to `line`.
    fn rekey(&mut self, key: i128, id: MarkerId, line: i64) {
        #[cfg(test)]
        {
            self.visits += 1;
        }
        let new = self.key(line);
        if new != key {
            self.attached.remove(key, id);
            self.attached.insert(new, id);
            self.by_id.insert(id, Slot::Attached(new));
        }
    }

    /// The attached markers with a line of at least `line`, as `(key, id, line)`.
    fn attached_from(&self, line: i64) -> Vec<(i128, MarkerId, i64)> {
        self.attached
            .collect(self.key(line)..)
            .into_iter()
            .map(|(key, id)| (key, id, self.line_at(key)))
            .collect()
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl ListObserver for MarkerSet {
    /// `marker.line -= amount`; a marker trimmed off the buffer is disposed.
    ///
    /// LUNETERM: (review S7 residual 1) one `offset` bump; only the markers that go below line
    /// 0 are visited (removed and logged in creation order, as the walk logs them). A surviving
    /// line is at least `amount`, so `line - amount` never saturates and the key need not move.
    fn on_trim(&mut self, amount: usize) {
        self.offset += i128::from(to_i64(amount));
        let mut gone: Vec<MarkerId> = self
            .attached
            .collect(..self.offset)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        gone.sort_unstable();
        self.dispose_sorted(gone);
    }

    /// Markers at or after the insertion point shift down.
    fn on_insert(&mut self, e: InsertEvent) {
        let (index, amount) = (to_i64(e.index), to_i64(e.amount));
        for (key, id, line) in self.attached_from(index) {
            self.rekey(key, id, line.saturating_add(amount));
        }
    }

    /// Markers inside the deleted range are disposed; markers after it shift up.
    fn on_delete(&mut self, e: DeleteEvent) {
        let (index, amount) = (to_i64(e.index), to_i64(e.amount));
        let mut gone = Vec::new();
        for (key, id, line) in self.attached_from(index) {
            if line < index.saturating_add(amount) {
                gone.push(id);
            } else if line > index {
                self.rekey(key, id, line.saturating_sub(amount));
            }
        }
        gone.sort_unstable();
        self.dispose_sorted(gone);
    }

    /// LUNETERM: (review S6) `n` times `on_delete(delete_at, 1)` then `on_insert(insert_at, 1)`
    /// in one pass (IL, DL, SU and SD make one such pair per line). Per pair, a marker outside
    /// the lines between `delete_at` and `insert_at` (both included) does not move: below both
    /// it is untouched, above both the delete moves it up and the insert down again. Inside, it
    /// moves one line towards `delete_at` per pair (down when `insert_at` is above, up when
    /// below) and is disposed by the pair that finds it on `delete_at`: pair
    /// `k = |delete_at - line|` (0-based). So it is disposed by pair `k` when `k < n`, else it
    /// moves `n` lines. Disposals are logged in pair order, creation order within a pair, as
    /// the per-event calls log them; detached markers are skipped. (review S7 residual 1) Only
    /// the markers between the two lines are visited.
    fn on_rotate(&mut self, delete_at: usize, insert_at: usize, n: usize) {
        let (d, i, n) = (to_i64(delete_at), to_i64(insert_at), to_i64(n));
        let (low, high) = (d.min(i), d.max(i));
        let span = self.attached.collect(self.key(low)..=self.key(high));
        let mut gone: Vec<(i64, MarkerId)> = Vec::new();
        for (key, id) in span {
            let m = self.line_at(key);
            let k = (d - m).abs();
            if k < n {
                gone.push((k, id));
                continue;
            }
            let line = if d >= i {
                m.saturating_add(n)
            } else {
                m.saturating_sub(n)
            };
            self.rekey(key, id, line);
        }
        // pair order, creation order within a pair
        gone.sort_unstable();
        self.dispose_sorted(gone.into_iter().map(|(_, id)| id).collect());
    }
}

#[cfg(test)]
#[path = "marker_tests.rs"]
mod tests;
