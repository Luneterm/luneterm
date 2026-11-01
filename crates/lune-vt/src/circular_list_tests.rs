//! Port of src/common/CircularList.test.ts @ f447274 (all 19 cases: 2 `push`, 2 `maxLength`,
//! 1 `length`, 4 `splice`, 2 `trimStart`, 8 `shiftElements`), plus Rust-only cases: event order
//! per mutating method (D1), lazy storage (D2), the clone bound of `shift_elements`, the
//! `splice` edge cases, and a seeded comparison against a literal port of upstream's ring.
//!
//! `adapted:` `assert.throws(.., 'msg')` becomes `Err(..)` with the same message through
//! `Display`; `list.length = n` is `set_len`; `list.maxLength = n` is `set_max_length`.

#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::rc::Rc;

use super::*;

fn str_list(max: usize, items: &[&'static str]) -> CircularList<&'static str> {
    let mut list = CircularList::new(max);
    for item in items {
        list.push(*item);
    }
    list
}

fn num_list(max: usize, items: &[i32]) -> CircularList<i32> {
    let mut list = CircularList::new(max);
    for item in items {
        list.push(*item);
    }
    list
}

fn nums<O: ListObserver>(list: &CircularList<i32, O>) -> Vec<Option<i32>> {
    (0..list.len()).map(|i| list.get(i).copied()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev {
    Trim(usize),
    Insert(usize, usize),
    Delete(usize, usize),
}

#[derive(Debug, Default)]
struct Rec(Vec<Ev>);

impl ListObserver for Rec {
    fn on_trim(&mut self, amount: usize) {
        self.0.push(Ev::Trim(amount));
    }
    fn on_insert(&mut self, e: InsertEvent) {
        self.0.push(Ev::Insert(e.index, e.amount));
    }
    fn on_delete(&mut self, e: DeleteEvent) {
        self.0.push(Ev::Delete(e.index, e.amount));
    }
}

fn rec_list(max: usize, items: &[i32]) -> CircularList<i32, Rec> {
    let mut list = CircularList::with_observer(max, Rec::default());
    for item in items {
        list.push(*item);
    }
    list.observer_mut().0.clear();
    list
}

// ---- upstream: push ----

#[test]
fn push_should_push_values_onto_the_array() {
    let list = str_list(5, &["1", "2", "3", "4", "5"]);
    assert_eq!(list.get(0), Some(&"1"));
    assert_eq!(list.get(1), Some(&"2"));
    assert_eq!(list.get(2), Some(&"3"));
    assert_eq!(list.get(3), Some(&"4"));
    assert_eq!(list.get(4), Some(&"5"));
}

#[test]
fn push_should_push_old_values_from_the_start_out_of_the_array_when_max_length_is_reached() {
    let mut list = str_list(2, &["1", "2"]);
    assert_eq!(list.get(0), Some(&"1"));
    assert_eq!(list.get(1), Some(&"2"));
    list.push("3");
    assert_eq!(list.get(0), Some(&"2"));
    assert_eq!(list.get(1), Some(&"3"));
    list.push("4");
    assert_eq!(list.get(0), Some(&"3"));
    assert_eq!(list.get(1), Some(&"4"));
}

// ---- upstream: maxLength ----

#[test]
fn max_length_should_increase_the_size_of_the_list() {
    let mut list = str_list(2, &["1", "2"]);
    assert_eq!(list.get(0), Some(&"1"));
    assert_eq!(list.get(1), Some(&"2"));
    list.set_max_length(4);
    list.push("3");
    list.push("4");
    assert_eq!(list.get(0), Some(&"1"));
    assert_eq!(list.get(1), Some(&"2"));
    assert_eq!(list.get(2), Some(&"3"));
    assert_eq!(list.get(3), Some(&"4"));
    list.push("wrapped");
    assert_eq!(list.get(0), Some(&"2"));
    assert_eq!(list.get(1), Some(&"3"));
    assert_eq!(list.get(2), Some(&"4"));
    assert_eq!(list.get(3), Some(&"wrapped"));
}

#[test]
fn max_length_should_return_the_maximum_length_of_the_list() {
    let mut list = str_list(2, &[]);
    assert_eq!(list.max_length(), 2);
    list.push("1");
    list.push("2");
    assert_eq!(list.max_length(), 2);
    list.push("3");
    assert_eq!(list.max_length(), 2);
    list.set_max_length(4);
    assert_eq!(list.max_length(), 4);
}

// ---- upstream: length ----

#[test]
fn length_should_return_the_current_length_of_the_list_capped_at_the_maximum_length() {
    let mut list = str_list(2, &[]);
    assert_eq!(list.len(), 0);
    list.push("1");
    assert_eq!(list.len(), 1);
    list.push("2");
    assert_eq!(list.len(), 2);
    list.push("3");
    assert_eq!(list.len(), 2);
}

// ---- upstream: splice ----

#[test]
fn splice_should_delete_items() {
    let mut list = str_list(2, &["1", "2"]);
    list.splice(0, 1, vec![]);
    assert_eq!(list.len(), 1);
    assert_eq!(list.get(0), Some(&"2"));
    list.push("3");
    list.splice(1, 1, vec![]);
    assert_eq!(list.len(), 1);
    assert_eq!(list.get(0), Some(&"2"));
}

#[test]
fn splice_should_insert_items() {
    let mut list = str_list(2, &["1"]);
    list.splice(0, 0, vec!["2"]);
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&"2"));
    assert_eq!(list.get(1), Some(&"1"));
    list.splice(1, 0, vec!["3"]);
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&"3"));
    assert_eq!(list.get(1), Some(&"1"));
}

#[test]
fn splice_should_delete_items_then_insert_items() {
    let mut list = str_list(3, &["1", "2"]);
    list.splice(0, 1, vec!["3", "4"]);
    assert_eq!(list.len(), 3);
    assert_eq!(list.get(0), Some(&"3"));
    assert_eq!(list.get(1), Some(&"4"));
    assert_eq!(list.get(2), Some(&"2"));
}

#[test]
fn splice_should_wrap_the_array_correctly_when_more_items_are_inserted_than_deleted() {
    let mut list = str_list(3, &["1", "2"]);
    list.splice(1, 0, vec!["3", "4"]);
    assert_eq!(list.len(), 3);
    assert_eq!(list.get(0), Some(&"3"));
    assert_eq!(list.get(1), Some(&"4"));
    assert_eq!(list.get(2), Some(&"2"));
}

// ---- upstream: trimStart ----

#[test]
fn trim_start_should_remove_items_from_the_beginning_of_the_list() {
    let mut list = str_list(5, &["1", "2", "3", "4", "5"]);
    list.trim_start(1);
    assert_eq!(list.len(), 4);
    assert_eq!(list.get(0), Some(&"2"));
    assert_eq!(list.get(1), Some(&"3"));
    assert_eq!(list.get(2), Some(&"4"));
    assert_eq!(list.get(3), Some(&"5"));
    list.trim_start(2);
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&"4"));
    assert_eq!(list.get(1), Some(&"5"));
}

#[test]
fn trim_start_should_remove_all_items_if_the_requested_trim_amount_is_larger_than_the_lists_length()
{
    let mut list = str_list(5, &["1"]);
    list.trim_start(2);
    assert_eq!(list.len(), 0);
}

// ---- upstream: shiftElements ----

#[test]
fn shift_elements_should_not_mutate_the_list_when_count_is_0() {
    let mut list = num_list(5, &[1, 2]);
    list.shift_elements(0, 0, 1).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&1));
    assert_eq!(list.get(1), Some(&2));
}

#[test]
fn shift_elements_should_throw_for_invalid_args() {
    let mut list = num_list(5, &[1]);
    let msg = |r: Result<(), ListError>| r.unwrap_err().to_string();
    assert_eq!(
        msg(list.shift_elements(-1, 1, 1)),
        "start argument out of range"
    );
    assert_eq!(
        msg(list.shift_elements(1, 1, 1)),
        "start argument out of range"
    );
    assert_eq!(
        msg(list.shift_elements(0, 1, -1)),
        "Cannot shift elements in list beyond index 0"
    );
    assert_eq!(
        nums(&list),
        vec![Some(1)],
        "failed shifts leave the list alone"
    );
}

#[test]
fn shift_elements_should_shift_an_element_forward() {
    let mut list = num_list(5, &[1, 2]);
    list.shift_elements(0, 1, 1).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&1));
    assert_eq!(list.get(1), Some(&1));
}

#[test]
fn shift_elements_should_shift_elements_forward() {
    let mut list = num_list(5, &[1, 2, 3, 4]);
    list.shift_elements(0, 2, 2).unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(nums(&list), vec![Some(1), Some(2), Some(1), Some(2)]);
}

#[test]
fn shift_elements_should_shift_elements_forward_expanding_the_list_if_needed() {
    let mut list = num_list(5, &[1, 2]);
    list.shift_elements(0, 2, 2).unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(nums(&list), vec![Some(1), Some(2), Some(1), Some(2)]);
}

#[test]
fn shift_elements_should_shift_elements_forward_wrapping_the_list_if_needed() {
    let mut list = num_list(5, &[1, 2, 3, 4, 5]);
    list.shift_elements(2, 2, 3).unwrap();
    assert_eq!(list.len(), 5);
    assert_eq!(
        nums(&list),
        vec![Some(3), Some(4), Some(5), Some(3), Some(4)]
    );
}

#[test]
fn shift_elements_should_shift_an_element_backwards() {
    let mut list = num_list(5, &[1, 2]);
    list.shift_elements(1, 1, -1).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0), Some(&2));
    assert_eq!(list.get(1), Some(&2));
}

#[test]
fn shift_elements_should_shift_elements_backwards() {
    let mut list = num_list(5, &[1, 2, 3, 4]);
    list.shift_elements(2, 2, -2).unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(nums(&list), vec![Some(3), Some(4), Some(3), Some(4)]);
}

// ---- Rust-only: storage ----

#[test]
fn storage_is_not_proportional_to_max_length() {
    let mut list = CircularList::<u8>::new(u32::MAX as usize);
    for i in 0..10u8 {
        list.push(i);
    }
    assert_eq!(list.len(), 10);
    assert_eq!(list.get(9), Some(&9));
    assert_eq!(list.get(10), None, "a slot at or past len holds nothing");
    list.set_max_length(4);
    assert_eq!(
        list.len(),
        4,
        "shrinking keeps the first min(new, len) items"
    );
    assert_eq!(list.get(3), Some(&3));
}

#[test]
fn set_len_grows_with_empty_slots_shrinks_and_clamps_to_max_length() {
    let mut list = num_list(4, &[1, 2]);
    list.set_len(4);
    assert_eq!(nums(&list), vec![Some(1), Some(2), None, None]);
    list.set(2, Some(3));
    list.set(9, Some(9));
    assert_eq!(nums(&list), vec![Some(1), Some(2), Some(3), None]);
    list.set_len(1);
    assert_eq!(nums(&list), vec![Some(1)]);
    list.set_len(100);
    assert_eq!(list.len(), 4);
    assert!(list.is_full());
}

#[test]
fn pop_get_mut_and_max_length_zero() {
    let mut list = num_list(3, &[1, 2, 3]);
    *list.get_mut(1).unwrap() = 20;
    assert_eq!(list.pop(), Some(3));
    assert_eq!(nums(&list), vec![Some(1), Some(20)]);
    assert_eq!(list.get(2), None);
    let mut zero = CircularList::<i32, Rec>::with_observer(0, Rec::default());
    zero.push(1);
    assert_eq!(zero.len(), 0);
    assert_eq!(zero.observer().0, vec![Ev::Trim(1)]);
    assert_eq!(zero.recycle().unwrap_err(), ListError::EmptySlot);
}

#[test]
fn recycle_rotates_and_returns_the_reused_slot() {
    let mut list = num_list(3, &[1, 2]);
    assert_eq!(list.recycle().unwrap_err(), ListError::NotFull);
    assert_eq!(
        ListError::NotFull.to_string(),
        "Can only recycle when the buffer is full"
    );
    list.push(3);
    *list.recycle().unwrap() = 4;
    assert_eq!(nums(&list), vec![Some(2), Some(3), Some(4)]);
    list.set(0, None);
    assert_eq!(list.recycle().unwrap_err(), ListError::EmptySlot);
    assert_eq!(
        nums(&list),
        vec![None, Some(3), Some(4)],
        "unchanged on error"
    );
}

// ---- Rust-only: events (D1) ----

#[test]
fn events_push_fires_trim_only_when_full() {
    let mut list = rec_list(2, &[1]);
    list.push(2);
    assert!(list.observer().0.is_empty());
    list.push(3);
    assert_eq!(list.observer().0, vec![Ev::Trim(1)]);
}

#[test]
fn events_recycle_fires_trim_one() {
    let mut list = rec_list(2, &[1, 2]);
    list.recycle().unwrap();
    assert_eq!(list.observer().0, vec![Ev::Trim(1)]);
}

#[test]
fn events_splice_fires_delete_then_insert_then_trim() {
    let mut list = rec_list(4, &[1, 2, 3, 4]);
    list.splice(1, 1, vec![10, 11, 12]);
    assert_eq!(
        list.observer().0,
        vec![Ev::Delete(1, 1), Ev::Insert(1, 3), Ev::Trim(2)]
    );
    assert_eq!(nums(&list), vec![Some(11), Some(12), Some(3), Some(4)]);
}

#[test]
fn events_splice_without_delete_insert_or_trim_fires_nothing() {
    let mut list = rec_list(4, &[1, 2, 3]);
    list.splice(1, 0, vec![]);
    assert!(list.observer().0.is_empty());
    list.splice(1, 0, vec![9]);
    assert_eq!(list.observer().0, vec![Ev::Insert(1, 1)]);
    list.observer_mut().0.clear();
    list.splice(0, 2, vec![]);
    assert_eq!(list.observer().0, vec![Ev::Delete(0, 2)]);
}

#[test]
fn events_trim_start_fires_even_for_zero() {
    let mut list = rec_list(5, &[1, 2, 3]);
    list.trim_start(0);
    list.trim_start(2);
    list.trim_start(9);
    assert_eq!(
        list.observer().0,
        vec![Ev::Trim(0), Ev::Trim(2), Ev::Trim(1)]
    );
}

#[test]
fn events_shift_elements_fires_trim_one_per_dropped_item_and_only_when_growing() {
    let mut list = rec_list(5, &[1, 2, 3, 4, 5]);
    list.shift_elements(2, 2, 3).unwrap();
    assert_eq!(list.observer().0, vec![Ev::Trim(1), Ev::Trim(1)]);
    list.observer_mut().0.clear();
    list.shift_elements(3, 1, -1).unwrap();
    list.shift_elements(0, 1, 1).unwrap();
    assert!(list.observer().0.is_empty());
}

#[test]
fn events_pop_set_max_length_and_set_len_fire_nothing() {
    let mut list = rec_list(5, &[1, 2, 3]);
    list.pop();
    list.set_max_length(2);
    list.set_len(1);
    assert!(list.observer().0.is_empty());
}

#[test]
fn events_notify_fires_the_observer() {
    let mut list = rec_list(5, &[]);
    list.notify_insert(1, 2);
    list.notify_delete(3, 4);
    list.notify_trim(5);
    assert_eq!(
        list.observer().0,
        vec![Ev::Insert(1, 2), Ev::Delete(3, 4), Ev::Trim(5)]
    );
}

#[test]
fn take_take_all_and_replace_all_fire_nothing() {
    let mut list = rec_list(4, &[1, 2, 3]);
    assert_eq!(list.take(1), Some(2));
    assert_eq!(nums(&list), vec![Some(1), None, Some(3)]);
    assert_eq!(list.take(7), None);
    let all = list.take_all();
    assert_eq!(all, vec![Some(1), None, Some(3)]);
    assert_eq!(list.len(), 0);
    list.replace_all(vec![Some(7), Some(8), Some(9), Some(10), Some(11)]);
    assert_eq!(nums(&list), vec![Some(7), Some(8), Some(9), Some(10)]);
    assert!(list.observer().0.is_empty());
}

// ---- Rust-only: splice edge cases ----

#[test]
fn splice_start_past_the_end_leaves_empty_slots_in_the_gap() {
    let mut list = num_list(10, &[1, 2]);
    list.splice(5, 0, vec![7, 8]);
    assert_eq!(nums(&list), vec![Some(1), Some(2), None, None]);

    let mut list = num_list(10, &[1, 2]);
    list.splice(3, 0, vec![7, 8]);
    assert_eq!(nums(&list), vec![Some(1), Some(2), None, Some(7)]);

    let mut list = rec_list(10, &[1, 2]);
    list.splice(usize::MAX, usize::MAX, vec![7]);
    assert_eq!(
        list.len(),
        1,
        "delete count past the end truncates the tail"
    );
    assert_eq!(
        list.observer().0,
        vec![
            Ev::Delete(usize::MAX, usize::MAX),
            Ev::Insert(usize::MAX, 1)
        ]
    );
}

#[test]
fn splice_delete_count_past_the_end_shrinks_the_length_like_upstream() {
    // Upstream's copy loop does not run when start + deleteCount > length; only `_length -= n`.
    let mut list = num_list(5, &[1, 2, 3]);
    list.splice(2, 2, vec![]);
    assert_eq!(nums(&list), vec![Some(1)]);
    let mut list = num_list(5, &[1, 2, 3]);
    list.splice(0, 9, vec![]);
    assert_eq!(list.len(), 0);
}

#[test]
fn splice_insert_that_trims_past_start_keeps_the_logical_result() {
    // deviation from upstream (module doc): upstream's ring clobbers [1, 2, 3] here.
    let mut list = rec_list(3, &[1, 2, 3]);
    list.splice(0, 0, vec![10, 11]);
    assert_eq!(nums(&list), vec![Some(1), Some(2), Some(3)]);
    assert_eq!(list.observer().0, vec![Ev::Insert(0, 2), Ev::Trim(2)]);
}

// ---- Rust-only: shift_elements ----

#[derive(Debug)]
struct Counted(i32, Rc<Cell<usize>>);

impl Clone for Counted {
    fn clone(&self) -> Self {
        self.1.set(self.1.get() + 1);
        Self(self.0, Rc::clone(&self.1))
    }
}

#[test]
fn shift_elements_clones_at_most_min_offset_count() {
    let mut cases = 0;
    for max in [3usize, 6, 9] {
        for len in 1..=max {
            for start in 0..len {
                for count in 1..=len as isize {
                    for offset in -(start as isize)..=(max as isize) {
                        if offset > 0 && offset + count > max as isize {
                            continue;
                        }
                        let clones = Rc::new(Cell::new(0));
                        let mut list = CircularList::new(max);
                        for i in 0..len {
                            list.push(Counted(i as i32, Rc::clone(&clones)));
                        }
                        let before = clones.get();
                        let r = list.shift_elements(start as isize, count, offset);
                        let bound = (offset.unsigned_abs()).min(count as usize);
                        if r.is_ok() {
                            assert!(
                                clones.get() - before <= bound,
                                "max {max} len {len} start {start} count {count} offset {offset}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cases > 1000, "{cases}");
}

#[test]
fn shift_elements_far_forward_shift_is_rejected_and_leaves_the_list_alone() {
    let mut list = num_list(5, &[1, 2, 3]);
    assert_eq!(
        list.shift_elements(0, 1, 100).unwrap_err(),
        ListError::ShiftTooFar
    );
    assert_eq!(
        list.shift_elements(0, isize::MAX, 1).unwrap_err(),
        ListError::ShiftTooFar
    );
    assert_eq!(
        list.shift_elements(0, 1, isize::MIN).unwrap_err(),
        ListError::BeyondZero
    );
    assert_eq!(nums(&list), vec![Some(1), Some(2), Some(3)]);
}

#[test]
fn shift_elements_backward_past_the_end_clears_the_destinations() {
    // Sources 3 and 4 are past len (empty), so their destinations 1 and 2 are cleared.
    let mut list = num_list(6, &[1, 2, 3]);
    list.shift_elements(1, 4, -1).unwrap();
    assert_eq!(nums(&list), vec![Some(2), Some(3), None]);
}

// ---- Rust-only: literal port of upstream's ring ----

/// `CircularList.ts` with its ring and cyclic indexing, for values. Elements are `i32`; empty
/// slots are `None`. Fires into `events` where upstream fires.
struct Ring {
    array: Vec<Option<i32>>,
    start: isize,
    length: isize,
    max: isize,
    events: Vec<Ev>,
}

impl Ring {
    fn new(max: usize) -> Self {
        Self {
            array: vec![None; max],
            start: 0,
            length: 0,
            max: max as isize,
            events: vec![],
        }
    }
    fn cyc(&self, i: isize) -> usize {
        (self.start + i).rem_euclid(self.max) as usize
    }
    fn get(&self, i: isize) -> Option<i32> {
        self.array[self.cyc(i)]
    }
    fn set(&mut self, i: isize, v: Option<i32>) {
        let c = self.cyc(i);
        self.array[c] = v;
    }
    fn push(&mut self, v: i32) {
        self.set(self.length, Some(v));
        if self.length == self.max {
            self.start = (self.start + 1) % self.max;
            self.events.push(Ev::Trim(1));
        } else {
            self.length += 1;
        }
    }
    fn recycle(&mut self) -> Option<i32> {
        self.start = (self.start + 1) % self.max;
        self.events.push(Ev::Trim(1));
        self.get(self.length - 1)
    }
    fn pop(&mut self) -> Option<i32> {
        let v = self.get(self.length - 1);
        self.length -= 1;
        v
    }
    fn splice(&mut self, start: isize, delete_count: isize, items: &[i32]) {
        if delete_count != 0 {
            for i in start..self.length - delete_count {
                let v = self.get(i + delete_count);
                self.set(i, v);
            }
            self.length -= delete_count;
            self.events
                .push(Ev::Delete(start as usize, delete_count as usize));
        }
        let n = items.len() as isize;
        for i in (start..self.length).rev() {
            let v = self.get(i);
            self.set(i + n, v);
        }
        for (i, item) in items.iter().enumerate() {
            self.set(start + i as isize, Some(*item));
        }
        if n != 0 {
            self.events.push(Ev::Insert(start as usize, n as usize));
        }
        if self.length + n > self.max {
            let trim = self.length + n - self.max;
            self.start += trim;
            self.length = self.max;
            self.events.push(Ev::Trim(trim as usize));
        } else {
            self.length += n;
        }
    }
    fn trim_start(&mut self, count: isize) {
        let count = count.min(self.length);
        self.start += count;
        self.length -= count;
        self.events.push(Ev::Trim(count as usize));
    }
    fn shift(&mut self, start: isize, count: isize, offset: isize) {
        if offset > 0 {
            for i in (0..count).rev() {
                let v = self.get(start + i);
                self.set(start + i + offset, v);
            }
            let expand = (start + count + offset) - self.length;
            if expand > 0 {
                self.length += expand;
                while self.length > self.max {
                    self.length -= 1;
                    self.start += 1;
                    self.events.push(Ev::Trim(1));
                }
            }
        } else {
            for i in 0..count {
                let v = self.get(start + i);
                self.set(start + i + offset, v);
            }
        }
    }
    fn set_max(&mut self, new_max: usize) {
        if self.max == new_max as isize {
            return;
        }
        let mut arr = vec![None; new_max];
        for i in 0..(new_max as isize).min(self.length) {
            arr[i as usize] = self.get(i);
        }
        self.array = arr;
        self.max = new_max as isize;
        self.start = 0;
        // upstream leaves `_length` above the new maximum (Buffer trims first); the port clamps
        self.length = self.length.min(self.max);
    }
    fn values(&self) -> Vec<Option<i32>> {
        (0..self.length).map(|i| self.get(i)).collect()
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[test]
fn random_operations_match_a_literal_port_of_the_upstream_ring() {
    let mut compared = 0usize;
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let max = 2 + rng.next(7);
        let mut ring = Ring::new(max);
        let mut list = CircularList::with_observer(max, Rec::default());
        let mut next = 100;
        for _ in 0..60 {
            let len = list.len();
            match rng.next(8) {
                0 | 1 => {
                    next += 1;
                    ring.push(next);
                    list.push(next);
                }
                2 => {
                    // keep `len + n - max <= start` (the ring only matches the logical result
                    // there, see the module doc)
                    let max = list.max_length();
                    let start = rng.next(len + 1);
                    let delete = rng.next(len - start + 1);
                    let n = rng.next(4);
                    if (len - delete + n).saturating_sub(max) > start {
                        continue;
                    }
                    let items: Vec<i32> = (0..n as i32).map(|i| next + i + 1).collect();
                    next += n as i32;
                    ring.splice(start as isize, delete as isize, &items);
                    list.splice(start, delete, items);
                }
                3 => {
                    let count = rng.next(len + 2);
                    ring.trim_start(count as isize);
                    list.trim_start(count);
                }
                4 | 5 if len > 0 => {
                    let start = rng.next(len);
                    let count = 1 + rng.next(len - start);
                    let max = list.max_length();
                    let offset = rng.next(max + 1) as isize - rng.next(start + 1) as isize;
                    // the ring only matches the logical result when the destinations do not wrap
                    // onto the sources (module doc)
                    if offset > 0 && offset + count as isize > max as isize {
                        continue;
                    }
                    if offset > 0 && start + offset as usize > len {
                        continue; // gap of empty slots; upstream leaves stale values there
                    }
                    ring.shift(start as isize, count as isize, offset);
                    list.shift_elements(start as isize, count as isize, offset)
                        .unwrap();
                }
                6 if len > 0 => {
                    let before = nums(&list);
                    assert_eq!(
                        ring.pop(),
                        list.pop(),
                        "seed {seed} {before:?} {:?}",
                        ring.values()
                    );
                }
                7 if len == list.max_length() && len > 0 => {
                    let v = ring.recycle();
                    assert_eq!(v, list.recycle().ok().copied(), "seed {seed}");
                }
                _ => {
                    let new_max = 2 + rng.next(7);
                    ring.set_max(new_max);
                    list.set_max_length(new_max);
                }
            }
            assert_eq!(nums(&list), ring.values(), "seed {seed}");
            assert_eq!(list.observer().0, ring.events, "seed {seed}");
            compared += 1;
        }
    }
    assert!(compared > 5000, "{compared}");
}
