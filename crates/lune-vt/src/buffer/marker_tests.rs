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
