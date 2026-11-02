//! Tests for `marker.rs`. Upstream has no marker test file; these pin the three listeners of
//! `Buffer.addMarker` (`Buffer.ts:627-655`), `clearMarkers`/`clearAllMarkers`, the disposal
//! log that replaces `Marker.onDispose`, and the `CircularList` integration (plan M5 D1).

#![allow(clippy::unwrap_used)]

use super::*;
use crate::circular_list::CircularList;

fn set_with(lines: &[i64]) -> (MarkerSet, Vec<MarkerId>) {
    let mut set = MarkerSet::new();
    let ids = lines.iter().map(|l| set.add(*l)).collect();
    (set, ids)
}

fn lines_of(set: &MarkerSet, ids: &[MarkerId]) -> Vec<Option<i64>> {
    ids.iter().map(|id| set.get(*id).map(|m| m.line)).collect()
}

#[test]
fn ids_are_unique_and_increase_in_creation_order() {
    let (set, ids) = set_with(&[0, 0, 0]);
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    assert!(ids[0].0 >= 1);
    assert_eq!(set.len(), 3);
    assert_eq!(set.iter().map(|m| m.id).collect::<Vec<_>>(), ids);
    let (_other, other_ids) = set_with(&[0]);
    assert!(other_ids[0] > ids[2], "the counter is process-wide");
}

#[test]
fn trim_subtracts_and_disposes_markers_pushed_off_the_top() {
    let (mut set, ids) = set_with(&[0, 1, 2, 5]);
    set.on_trim(2);
    assert_eq!(
        lines_of(&set, &ids),
        vec![None, None, Some(0), Some(3)],
        "line 2 - 2 == 0 survives; -1 and -2 are disposed"
    );
    assert_eq!(set.take_disposed(), vec![ids[0], ids[1]]);
    set.on_trim(0);
    assert_eq!(lines_of(&set, &ids[2..]), vec![Some(0), Some(3)]);
    assert!(set.take_disposed().is_empty());
}

#[test]
fn insert_shifts_markers_at_or_after_the_index() {
    let (mut set, ids) = set_with(&[1, 2, 3]);
    set.on_insert(InsertEvent {
        index: 2,
        amount: 2,
    });
    assert_eq!(lines_of(&set, &ids), vec![Some(1), Some(4), Some(5)]);
    assert!(set.take_disposed().is_empty());
}

#[test]
fn delete_disposes_markers_in_range_and_shifts_those_after_it() {
    let (mut set, ids) = set_with(&[0, 1, 2, 3, 4]);
    set.on_delete(DeleteEvent {
        index: 1,
        amount: 2,
    });
    assert_eq!(
        lines_of(&set, &ids),
        vec![Some(0), None, None, Some(1), Some(2)]
    );
    assert_eq!(set.take_disposed(), vec![ids[1], ids[2]]);
}

#[test]
fn delete_range_end_is_exclusive_and_index_itself_is_inside() {
    let (mut set, ids) = set_with(&[3, 4]);
    set.on_delete(DeleteEvent {
        index: 3,
        amount: 1,
    });
    assert_eq!(lines_of(&set, &ids), vec![None, Some(3)]);
}

#[test]
fn dispose_is_idempotent_and_ignores_unknown_ids() {
    let (mut set, ids) = set_with(&[1, 2]);
    set.dispose(ids[0]);
    set.dispose(ids[0]);
    set.dispose(MarkerId(u64::MAX));
    assert_eq!(set.get(ids[0]), None);
    assert_eq!(set.get(ids[1]).map(|m| m.line), Some(2));
    assert_eq!(set.take_disposed(), vec![ids[0]], "logged once");
    assert!(set.take_disposed().is_empty(), "the log is drained");
}

#[test]
fn a_disposed_marker_no_longer_hears_events() {
    let (mut set, ids) = set_with(&[3]);
    set.dispose(ids[0]);
    set.on_insert(InsertEvent {
        index: 0,
        amount: 5,
    });
    assert_eq!(set.get(ids[0]), None);
    assert!(set.is_empty());
}

#[test]
fn clear_line_disposes_only_that_line_in_creation_order() {
    let (mut set, ids) = set_with(&[2, 1, 2, 3, 2]);
    set.clear_line(2);
    assert_eq!(
        lines_of(&set, &ids),
        vec![None, Some(1), None, Some(3), None]
    );
    assert_eq!(set.take_disposed(), vec![ids[0], ids[2], ids[4]]);
    set.clear_line(9);
    assert!(set.take_disposed().is_empty());
    assert_eq!(set.len(), 2);
}

#[test]
fn clear_all_disposes_everything_in_creation_order() {
    let (mut set, ids) = set_with(&[4, 0, 7]);
    set.dispose(ids[1]);
    set.clear_all();
    assert!(set.is_empty());
    assert_eq!(set.take_disposed(), vec![ids[1], ids[0], ids[2]]);
}

#[test]
fn disposal_log_keeps_disposal_order_across_mixed_operations() {
    let (mut set, ids) = set_with(&[0, 1, 2, 3]);
    set.dispose(ids[3]);
    set.on_trim(2); // disposes 0 and 1
    set.clear_line(0); // ids[2] now at line 0
    assert_eq!(set.take_disposed(), vec![ids[3], ids[0], ids[1], ids[2]]);
}

#[test]
fn detach_all_stops_following_but_keeps_the_markers() {
    let (mut set, ids) = set_with(&[5, 6]);
    set.detach_all();
    let late = set.add(5);
    set.on_trim(10);
    assert_eq!(lines_of(&set, &ids), vec![Some(5), Some(6)]);
    assert_eq!(
        set.get(late),
        None,
        "markers added after detach follow events"
    );
    set.on_insert(InsertEvent {
        index: 0,
        amount: 1,
    });
    assert_eq!(lines_of(&set, &ids), vec![Some(5), Some(6)]);
    set.clear_line(5);
    assert_eq!(lines_of(&set, &ids), vec![None, Some(6)]);
    set.clear_all();
    assert!(set.is_empty());
}

#[test]
fn lines_with_a_marker_set_observer_move_markers_synchronously() {
    let mut lines: CircularList<char, MarkerSet> = CircularList::with_observer(4, MarkerSet::new());
    let mut ids = vec![];
    for (i, c) in "abcd".chars().enumerate() {
        lines.push(c);
        ids.push(lines.observer_mut().add(i as i64));
    }
    let at = |lines: &CircularList<char, MarkerSet>| -> Vec<Option<i64>> {
        lines_of(lines.observer(), &ids)
    };
    // push on a full list trims one: 'a' is gone, the rest move up
    lines.push('e');
    assert_eq!(at(&lines), vec![None, Some(0), Some(1), Some(2)]);
    // splice(1, 1, [x]) on [b c d e]: 'c' deleted (marker disposed), 'd' and 'e' keep order
    lines.splice(1, 1, vec!['x']);
    assert_eq!(at(&lines), vec![None, Some(0), None, Some(2)]);
    // the list is full: the insert shifts the markers down (b to 1, e to 3) and the overflow
    // trims one line off the top (b to 0, e to 2)
    lines.splice(0, 0, vec!['y']);
    assert_eq!(at(&lines), vec![None, Some(0), None, Some(2)]);
    assert_eq!(
        lines.observer_mut().take_disposed(),
        vec![ids[0], ids[2]],
        "disposal order across the three events"
    );
    // trim_start
    lines.trim_start(2);
    assert_eq!(at(&lines), vec![None, None, None, Some(0)]);
}

// ---- on_rotate, `// LUNETERM:` (review S6) ----

/// A small deterministic generator (no dependencies in lune-vt).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

/// Two sets with the same markers (`detached` of them created before `detach_all`); returns
/// the sets and their ids, index for index.
fn twin_sets(lines: &[i64], detached: usize) -> [(MarkerSet, Vec<MarkerId>); 2] {
    [0, 1].map(|_| {
        let mut set = MarkerSet::new();
        let mut ids: Vec<MarkerId> = lines[..detached].iter().map(|&l| set.add(l)).collect();
        set.detach_all();
        ids.extend(lines[detached..].iter().map(|&l| set.add(l)));
        (set, ids)
    })
}

fn positions(log: &[MarkerId], ids: &[MarkerId]) -> Vec<usize> {
    log.iter()
        .map(|id| ids.iter().position(|x| x == id).unwrap())
        .collect()
}

#[test]
fn on_rotate_equals_the_per_event_calls() {
    let mut rng = Lcg(0x5e6);
    for case in 0..3000 {
        let count = rng.below(40) as usize;
        let span = 1 + rng.below(30);
        let lines: Vec<i64> = (0..count).map(|_| rng.below(span + 4) as i64).collect();
        let detached = rng.below(count as u64 + 1) as usize;
        let [(mut fast, fast_ids), (mut slow, slow_ids)] = twin_sets(&lines, detached);
        let d = rng.below(span) as usize;
        let i = rng.below(span) as usize;
        let n = rng.below(span + 5) as usize;
        fast.on_rotate(d, i, n);
        for _ in 0..n {
            slow.on_delete(DeleteEvent {
                index: d,
                amount: 1,
            });
            slow.on_insert(InsertEvent {
                index: i,
                amount: 1,
            });
        }
        let ctx = format!("case {case}: lines {lines:?} detached {detached} d {d} i {i} n {n}");
        assert_eq!(
            lines_of(&fast, &fast_ids),
            lines_of(&slow, &slow_ids),
            "{ctx}"
        );
        assert_eq!(
            positions(&fast.take_disposed(), &fast_ids),
            positions(&slow.take_disposed(), &slow_ids),
            "disposal order, {ctx}"
        );
        // survivors keep creation order
        let order = |s: &MarkerSet, ids: &[MarkerId]| {
            positions(&s.iter().map(|m| m.id).collect::<Vec<_>>(), ids)
        };
        assert_eq!(order(&fast, &fast_ids), order(&slow, &slow_ids), "{ctx}");
    }
}

/// Review S6 pinned one walk of all markers (1000) instead of `2 * n`; S7 residual 1 tightens
/// it to the markers between the two lines (both included), each visited once.
#[test]
fn on_rotate_visits_only_the_markers_between_the_lines() {
    let (mut set, _) = set_with(&(0..1000).map(|l| l % 50).collect::<Vec<_>>());
    set.visits = 0;
    set.on_rotate(40, 2, 30);
    let between = (0..1000).filter(|l| (2..=40).contains(&(l % 50))).count() as u64;
    assert_eq!(set.visits, between, "not 2 * 30 walks, and not all 1000");
}

// ---- cost bounds, `// LUNETERM:` (review S7 residual 1); `visits` counts, not time ----

#[test]
fn on_trim_visits_only_the_disposed_markers() {
    let (mut set, ids) = set_with(&(0..10_000).collect::<Vec<_>>());
    set.visits = 0;
    set.on_trim(1);
    assert_eq!(
        set.visits, 1,
        "one marker trimmed off; the other 9,999 not touched"
    );
    assert_eq!(set.take_disposed(), vec![ids[0]]);
    assert_eq!(set.get(ids[9_999]).map(|m| m.line), Some(9_998));
    set.visits = 0;
    set.on_trim(0);
    assert_eq!(set.visits, 0);
    set.on_trim(5);
    assert_eq!(set.visits, 5);
    assert_eq!(set.take_disposed(), ids[1..6].to_vec());
}

#[test]
fn insert_and_delete_visit_only_the_markers_from_the_index_on() {
    let lines: Vec<i64> = (0..10_000).map(|l| l % 1000).collect();
    let from = |index: i64| lines.iter().filter(|&&l| l >= index).count() as u64;
    let (mut set, _) = set_with(&lines);
    set.visits = 0;
    set.on_insert(InsertEvent {
        index: 990,
        amount: 3,
    });
    assert_eq!(set.visits, from(990));
    let (mut set, _) = set_with(&lines);
    set.visits = 0;
    set.on_delete(DeleteEvent {
        index: 995,
        amount: 2,
    });
    // the disposed (lines 995, 996) and the shifted (997..=999); line 995 is in the range
    assert_eq!(set.visits, from(995));
    assert_eq!(set.take_disposed().len(), 20);
    // a delete with nothing at or after the index touches nothing
    set.visits = 0;
    set.on_delete(DeleteEvent {
        index: 5000,
        amount: 7,
    });
    assert_eq!(set.visits, 0);
}

#[test]
fn ids_on_line_and_count_on_line_see_attached_and_detached_markers() {
    let (mut set, ids) = set_with(&[3, 4, 3]);
    set.detach_all();
    let late = set.add(3);
    set.on_trim(1); // the detached stay on 3 and 4; `late` moves to 2
    assert_eq!(set.count_on_line(3), 2);
    let mut on3: Vec<MarkerId> = set.ids_on_line(3).collect();
    on3.sort_unstable();
    assert_eq!(on3, vec![ids[0], ids[2]]);
    assert_eq!(set.ids_on_line(2).collect::<Vec<_>>(), vec![late]);
    assert_eq!(set.count_on_line(2), 1);
    assert_eq!(set.count_on_line(9), 0);
    set.clear_line(3);
    assert_eq!(set.count_on_line(3), 0);
    assert_eq!(set.take_disposed(), vec![ids[0], ids[2]]);
}

// ---- ReferenceMarkerSet: the `Vec`-based set as shipped before plan M7 step 11 ----
//
// Moved verbatim from `marker.rs` (only `MarkerSet` renamed to `ReferenceMarkerSet`), so the
// differential below compares the indexed set against the code it replaces (S7 residual 1).

#[allow(dead_code, unreachable_pub)]
mod reference {
    use super::*;

    #[derive(Debug)]
    struct Entry {
        marker: Marker,
        /// Follows list events. `Buffer::clear` replaces the line list, so the markers stay but no
        /// longer hear the new list (`detach_all`).
        attached: bool,
    }

    /// The markers of one buffer (upstream `Buffer.markers`) and their listeners.
    #[derive(Debug, Default)]
    pub struct ReferenceMarkerSet {
        /// Creation order; ids strictly increase.
        entries: Vec<Entry>,
        disposed: Vec<MarkerId>,
        /// Entries visited or moved by walks and removals (tests pin bulk operations to one walk).
        #[cfg(test)]
        pub(crate) visits: u64,
    }

    impl ReferenceMarkerSet {
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

    impl ListObserver for ReferenceMarkerSet {
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
}

// ---- differential: MarkerSet against ReferenceMarkerSet (plan M7 step 11 contract) ----

use reference::ReferenceMarkerSet;

/// The two sets fed the same operations; ids differ (one process-wide counter), so markers are
/// compared by creation index.
struct Twin {
    new: MarkerSet,
    old: ReferenceMarkerSet,
    /// `(new id, reference id)` per creation index.
    ids: Vec<(MarkerId, MarkerId)>,
}

impl Twin {
    fn index_new(&self, id: MarkerId) -> usize {
        self.ids.iter().position(|p| p.0 == id).unwrap()
    }

    fn index_old(&self, id: MarkerId) -> usize {
        self.ids.iter().position(|p| p.1 == id).unwrap()
    }

    /// The current line of a live marker (reference view), to aim events at it.
    fn some_line(&self, rng: &mut Lcg) -> Option<i64> {
        let live: Vec<i64> = self.old.iter().map(|m| m.line).collect();
        (!live.is_empty()).then(|| live[rng.below(live.len() as u64) as usize])
    }

    fn check(&mut self, ctx: &str) {
        assert_eq!(self.new.len(), self.old.len(), "len, {ctx}");
        for (i, &(n, o)) in self.ids.iter().enumerate() {
            assert_eq!(
                self.new.get(n).map(|m| m.line),
                self.old.get(o).map(|m| m.line),
                "marker #{i}, {ctx}"
            );
        }
        let order_new: Vec<(usize, i64)> = self
            .new
            .iter()
            .map(|m| (self.index_new(m.id), m.line))
            .collect();
        let order_old: Vec<(usize, i64)> = self
            .old
            .iter()
            .map(|m| (self.index_old(m.id), m.line))
            .collect();
        assert_eq!(order_new, order_old, "iter order and values, {ctx}");
    }

    fn check_disposed(&mut self, ctx: &str) {
        let new: Vec<usize> = self
            .new
            .take_disposed()
            .iter()
            .map(|&id| self.index_new(id))
            .collect();
        let old: Vec<usize> = self
            .old
            .take_disposed()
            .iter()
            .map(|&id| self.index_old(id))
            .collect();
        assert_eq!(new, old, "take_disposed, {ctx}");
    }
}

/// A line or index aimed at the event boundaries: at, one above or one below a live marker's
/// line, a small line, or a huge one near `i64::MAX`.
fn aimed(twin: &Twin, rng: &mut Lcg) -> i64 {
    match rng.below(10) {
        0..=4 => match twin.some_line(rng) {
            Some(l) => l.saturating_add(rng.below(3) as i64 - 1),
            None => rng.below(12) as i64,
        },
        5..=7 => rng.below(12) as i64,
        8 => i64::MAX - rng.below(4) as i64,
        _ => -(rng.below(3) as i64),
    }
}

fn as_usize(l: i64) -> usize {
    usize::try_from(l).unwrap_or(0)
}

/// An amount: small, large enough to cover every marker, or huge.
fn amount(rng: &mut Lcg) -> usize {
    match rng.below(10) {
        0..=5 => rng.below(4) as usize,
        6..=7 => rng.below(40) as usize,
        8 => usize::MAX - rng.below(2) as usize,
        _ => (i64::MAX as usize).wrapping_sub(rng.below(3) as usize),
    }
}

fn run_marker_differential(seed: u64, ops: usize) {
    let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x51ed);
    let mut twin = Twin {
        new: MarkerSet::new(),
        old: ReferenceMarkerSet::new(),
        ids: Vec::new(),
    };
    for op in 0..ops {
        let ctx = format!("seed {seed} op {op}");
        match rng.below(20) {
            0..=5 => {
                let line = aimed(&twin, &mut rng);
                let n = twin.new.add(line);
                let o = twin.old.add(line);
                twin.ids.push((n, o));
            }
            6 => {
                if !twin.ids.is_empty() {
                    let (n, o) = twin.ids[rng.below(twin.ids.len() as u64) as usize];
                    twin.new.dispose(n);
                    twin.old.dispose(o);
                }
            }
            7 => {
                let (mut n, mut o) = (HashSet::new(), HashSet::new());
                for _ in 0..rng.below(6) {
                    if !twin.ids.is_empty() {
                        let p = twin.ids[rng.below(twin.ids.len() as u64) as usize];
                        n.insert(p.0);
                        o.insert(p.1);
                    }
                }
                if rng.below(4) == 0 {
                    n.insert(MarkerId(u64::MAX));
                    o.insert(MarkerId(u64::MAX));
                }
                twin.new.dispose_all(&n);
                twin.old.dispose_all(&o);
            }
            8 => {
                let y = aimed(&twin, &mut rng);
                twin.new.clear_line(y);
                twin.old.clear_line(y);
            }
            9 => {
                if rng.below(4) == 0 {
                    twin.new.clear_all();
                    twin.old.clear_all();
                } else {
                    twin.new.detach_all();
                    twin.old.detach_all();
                }
            }
            10..=11 => {
                let a = if rng.below(5) == 0 {
                    // trims that dispose everything
                    as_usize(twin.some_line(&mut rng).unwrap_or(0)).saturating_add(1)
                } else {
                    amount(&mut rng)
                };
                twin.new.on_trim(a);
                twin.old.on_trim(a);
            }
            12..=13 => {
                let e = InsertEvent {
                    index: as_usize(aimed(&twin, &mut rng)),
                    amount: amount(&mut rng),
                };
                twin.new.on_insert(e);
                twin.old.on_insert(e);
            }
            14..=15 => {
                let e = DeleteEvent {
                    index: as_usize(aimed(&twin, &mut rng)),
                    amount: amount(&mut rng),
                };
                twin.new.on_delete(e);
                twin.old.on_delete(e);
            }
            16..=17 => {
                let d = as_usize(aimed(&twin, &mut rng));
                let i = as_usize(aimed(&twin, &mut rng));
                let n = if rng.below(3) == 0 {
                    amount(&mut rng)
                } else {
                    rng.below(d.abs_diff(i).min(1 << 20) as u64 + 3) as usize
                };
                twin.new.on_rotate(d, i, n);
                twin.old.on_rotate(d, i, n);
            }
            _ => twin.check_disposed(&ctx),
        }
        twin.check(&ctx);
    }
    twin.check_disposed(&format!("seed {seed} end"));
}

/// Plan M7 step 11: the indexed set equals the `Vec` walk it replaces for every operation
/// sequence. `LUNE_VT_MARKER_DIFF_SEEDS=<n>` runs more seeds (2,000 in release for the step's
/// gate); `LUNE_VT_MARKER_DIFF_SEED=<n>` replays one.
#[test]
fn marker_set_equals_the_reference_walk() {
    if let Some(seed) = std::env::var("LUNE_VT_MARKER_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        run_marker_differential(seed, 200);
        return;
    }
    let seeds: u64 = std::env::var("LUNE_VT_MARKER_DIFF_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    for seed in 0..seeds {
        run_marker_differential(seed, 200);
    }
}
