//! Port of xterm.js src/common/buffer/Marker.ts @ f447274, with the marker list of
//! `Buffer.ts` (`markers`, `addMarker`, `clearMarkers`, `clearAllMarkers`, `_removeMarker`,
//! `:600-662`). There is no upstream test file for markers; `marker_tests.rs` pins the listener
//! rules of `addMarker`.
//!
//! Rust notes (plan M5 D1, D4):
//!  - A `Marker` object is a [`MarkerId`] into a [`MarkerSet`]; the set owns the markers and
//!    is the `ListObserver` of its buffer's lines, applying the three listeners `addMarker`
//!    registers, to every marker in creation order. A disposed marker is gone from the set
//!    ([`MarkerSet::get`] is `None`), where upstream keeps an `isDisposed` object with
//!    `line == -1`.
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

use std::collections::HashSet;
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

#[derive(Debug)]
struct Entry {
    marker: Marker,
    /// Follows list events. `Buffer::clear` replaces the line list, so the markers stay but no
    /// longer hear the new list (`detach_all`).
    attached: bool,
}

/// The markers of one buffer (upstream `Buffer.markers`) and their listeners.
#[derive(Debug, Default)]
pub struct MarkerSet {
    /// Creation order; ids strictly increase.
    entries: Vec<Entry>,
    disposed: Vec<MarkerId>,
    /// Entries visited or moved by walks and removals (tests pin bulk operations to one walk).
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
        self.entries.push(Entry {
            marker: Marker { id, line },
            attached: true,
        });
        id
    }

    /// The live marker, `None` once disposed.
    pub fn get(&self, id: MarkerId) -> Option<&Marker> {
        self.index_of(id)
            .and_then(|i| self.entries.get(i))
            .map(|e| &e.marker)
    }

    /// The live markers in creation order.
    pub fn iter(&self) -> impl Iterator<Item = &Marker> {
        self.entries.iter().map(|e| &e.marker)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Upstream `Marker.dispose`: idempotent; the marker leaves the set and is logged.
    pub fn dispose(&mut self, id: MarkerId) {
        if let Some(i) = self.index_of(id) {
            #[cfg(test)]
            {
                self.visits += (self.entries.len() - i) as u64;
            }
            self.entries.remove(i);
            self.disposed.push(id);
        }
    }

    /// LUNETERM: (review F6) disposes every marker in `ids` (unknown ones are ignored) in one
    /// walk, logged in creation order; `dispose` per id costs a removal each, quadratic for a
    /// link that marks every line of a long buffer.
    pub fn dispose_all(&mut self, ids: &HashSet<MarkerId>) {
        if !ids.is_empty() {
            self.dispose_where(|m| ids.contains(&m.id));
        }
    }

    /// Upstream `Buffer.clearMarkers`: disposes the markers on line `y`.
    pub fn clear_line(&mut self, y: i64) {
        self.dispose_where(|m| m.line == y);
    }

    /// Upstream `Buffer.clearAllMarkers`.
    pub fn clear_all(&mut self) {
        self.dispose_where(|_| true);
    }

    /// The ids disposed since the last call, in disposal order.
    pub fn take_disposed(&mut self) -> Vec<MarkerId> {
        std::mem::take(&mut self.disposed)
    }

    /// The current markers stop following list events but stay in the set (`Buffer.clear`
    /// replaces `lines`; markers added afterwards follow the new list).
    pub fn detach_all(&mut self) {
        for entry in &mut self.entries {
            entry.attached = false;
        }
    }

    fn index_of(&self, id: MarkerId) -> Option<usize> {
        self.entries.binary_search_by_key(&id, |e| e.marker.id).ok()
    }

    /// Disposes every marker matching `pred` in creation order, as one walk.
    fn dispose_where(&mut self, mut pred: impl FnMut(&Marker) -> bool) {
        #[cfg(test)]
        {
            self.visits += self.entries.len() as u64;
        }
        let disposed = &mut self.disposed;
        self.entries.retain(|e| {
            if pred(&e.marker) {
                disposed.push(e.marker.id);
                false
            } else {
                true
            }
        });
    }

    /// Runs one listener over every attached marker in creation order; the markers it returns
    /// `true` for are disposed (removed and logged, in that order).
    fn apply(&mut self, mut listener: impl FnMut(&mut Marker) -> bool) {
        #[cfg(test)]
        {
            self.visits += self.entries.len() as u64;
        }
        let disposed = &mut self.disposed;
        self.entries.retain_mut(|e| {
            if e.attached && listener(&mut e.marker) {
                disposed.push(e.marker.id);
                false
            } else {
                true
            }
        });
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl ListObserver for MarkerSet {
    /// `marker.line -= amount`; a marker trimmed off the buffer is disposed.
    fn on_trim(&mut self, amount: usize) {
        let amount = to_i64(amount);
        self.apply(|m| {
            m.line = m.line.saturating_sub(amount);
            m.line < 0
        });
    }

    /// Markers at or after the insertion point shift down.
    fn on_insert(&mut self, e: InsertEvent) {
        let (index, amount) = (to_i64(e.index), to_i64(e.amount));
        self.apply(|m| {
            if m.line >= index {
                m.line = m.line.saturating_add(amount);
            }
            false
        });
    }

    /// Markers inside the deleted range are disposed; markers after it shift up.
    fn on_delete(&mut self, e: DeleteEvent) {
        let (index, amount) = (to_i64(e.index), to_i64(e.amount));
        self.apply(|m| {
            if m.line >= index && m.line < index.saturating_add(amount) {
                return true;
            }
            if m.line > index {
                m.line = m.line.saturating_sub(amount);
            }
            false
        });
    }

    /// LUNETERM: (review S6) `n` times `on_delete(delete_at, 1)` then `on_insert(insert_at, 1)`
    /// in one walk (IL, DL, SU and SD make one such pair per line, and each event walks every
    /// marker). Per pair, a marker outside the lines between `delete_at` and `insert_at` (both
    /// included) does not move: below both it is untouched, above both the delete moves it up
    /// and the insert down again. Inside, it moves one line towards `delete_at` per pair
    /// (down when `insert_at` is above, up when below) and is disposed by the pair that finds
    /// it on `delete_at`: pair `k = |delete_at - line|` (0-based). So it is disposed by pair
    /// `k` when `k < n`, else it moves `n` lines. Disposals are logged in pair order, creation
    /// order within a pair, as the per-event calls log them; detached markers are skipped.
    fn on_rotate(&mut self, delete_at: usize, insert_at: usize, n: usize) {
        #[cfg(test)]
        {
            self.visits += self.entries.len() as u64;
        }
        let (d, i, n) = (to_i64(delete_at), to_i64(insert_at), to_i64(n));
        let (low, high) = (d.min(i), d.max(i));
        let mut gone: Vec<(i64, MarkerId)> = Vec::new();
        self.entries.retain_mut(|e| {
            let m = e.marker.line;
            if !e.attached || m < low || m > high {
                return true;
            }
            let k = (d - m).abs();
            if k < n {
                gone.push((k, e.marker.id));
                return false;
            }
            e.marker.line = if d >= i {
                m.saturating_add(n)
            } else {
                m.saturating_sub(n)
            };
            true
        });
        // stable: creation order within a pair
        gone.sort_by_key(|&(k, _)| k);
        self.disposed.extend(gone.into_iter().map(|(_, id)| id));
    }
}

#[cfg(test)]
#[path = "marker_tests.rs"]
mod tests;
