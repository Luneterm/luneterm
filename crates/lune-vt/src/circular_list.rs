//! Port of xterm.js src/common/CircularList.ts @ f447274 (tests: CircularList.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1, D2):
//!  - Upstream is a ring of `maxLength` slots with a start index. This port keeps only the
//!    logical sequence, a lazily grown `VecDeque<Option<T>>` of exactly `len()` slots, so nothing
//!    is allocated in proportion to `max_length` (scrollback can be 2^32-1). A slot at or past
//!    `len()` holds nothing: [`get`](CircularList::get) there is `None`, where upstream returns a
//!    stale reference or `undefined`. Upstream's `get`/`set` on the raw ring (any index, wrapped
//!    modulo `maxLength`) has no meaning here; `set` outside `0..len()` is a no-op.
//!  - The three emitters (`onTrim`, `onInsert`, `onDelete`) become the [`ListObserver`] type
//!    parameter, called synchronously at exactly the upstream `fire` points with upstream's
//!    payloads. `()` is the no-op observer. `notify_*` are the places `Buffer`/`BufferReflow`
//!    fire an emitter directly.
//!  - JS aliasing (a slot read into another slot) becomes a move wherever the source slot is
//!    overwritten afterwards, and a clone otherwise, so [`shift_elements`](CircularList::shift_elements)
//!    clones at most `min(|offset|, count)` elements and still leaves upstream's duplicated-slot
//!    values (pinned by the upstream tests).
//!  - `throw` becomes [`ListError`].
//!
//! `// LUNETERM:` deviations, each in a case where upstream's ring corrupts or goes negative:
//!  - `splice`: upstream moves elements through ring slots, and when the inserted items would
//!    trim more elements than precede `start` (`len + items.len() - max_length > start`), the
//!    wrapped writes clobber elements that have not yet been moved. Here the result is the
//!    logical one (insert, then drop the oldest). A `delete_count` past the end truncates the
//!    last `delete_count` elements like upstream, but the length stops at 0 instead of going
//!    negative.
//!  - `shift_elements`: the same clobbering exists for forward shifts whose `offset` is within
//!    `count` of `max_length` (`offset > max_length - count`); the logical result is produced.
//!    A shift that would grow the list by more than `max_length` (every old element trimmed,
//!    ring wrapped more than once) is [`ListError::ShiftTooFar`], which also bounds the work.
//!  - `set_len` clamps to `max_length` (a longer list wraps onto itself upstream).
//!  - A `max_length` of 0 drops pushed values (upstream indexes the ring modulo 0).
//!  - `recycle` on a full list whose oldest slot is empty is [`ListError::EmptySlot`] (upstream
//!    returns `undefined`).

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::string_slice,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use std::collections::VecDeque;
use std::fmt;

/// Upstream `IInsertEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertEvent {
    pub index: usize,
    pub amount: usize,
}

/// Upstream `IDeleteEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteEvent {
    pub index: usize,
    pub amount: usize,
}

/// Receiver of the events upstream's `CircularList` emits (`onTrim`, `onInsert`, `onDelete`).
/// Called synchronously, at the point upstream fires.
pub trait ListObserver {
    /// `amount` items were removed from the start of the list.
    fn on_trim(&mut self, amount: usize);
    /// `e.amount` items were inserted at `e.index`.
    fn on_insert(&mut self, e: InsertEvent);
    /// `e.amount` items were deleted starting at `e.index`.
    fn on_delete(&mut self, e: DeleteEvent);

    /// LUNETERM: (review S6) the events of [`CircularList::splice_rotate`]: `n` times a delete
    /// of one item at `delete_at`, then an insert of one item at `insert_at`. The default makes
    /// those `2 * n` calls; an observer whose per-event work is large overrides it with the
    /// composed effect, which must leave the state the calls would.
    fn on_rotate(&mut self, delete_at: usize, insert_at: usize, n: usize) {
        for _ in 0..n {
            self.on_delete(DeleteEvent {
                index: delete_at,
                amount: 1,
            });
            self.on_insert(InsertEvent {
                index: insert_at,
                amount: 1,
            });
        }
    }
}

impl ListObserver for () {
    fn on_trim(&mut self, _amount: usize) {}
    fn on_insert(&mut self, _e: InsertEvent) {}
    fn on_delete(&mut self, _e: DeleteEvent) {}
}

/// Failures upstream reports by throwing (plus the two guards added here named in the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListError {
    /// `recycle` on a list that is not at `max_length`.
    NotFull,
    /// `recycle` where the slot to reuse is empty.
    EmptySlot,
    /// `shift_elements` with `start` outside `0..len()`.
    StartOutOfRange,
    /// `shift_elements` that would move an element below index 0.
    BeyondZero,
    /// `shift_elements` that would grow the list by more than `max_length`.
    ShiftTooFar,
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFull => "Can only recycle when the buffer is full",
            Self::EmptySlot => "Cannot recycle an empty slot",
            Self::StartOutOfRange => "start argument out of range",
            Self::BeyondZero => "Cannot shift elements in list beyond index 0",
            Self::ShiftTooFar => "Cannot shift elements further than the maximum length",
        })
    }
}

impl std::error::Error for ListError {}

/// A list with a maximum size that drops its oldest values when pushed past it.
#[derive(Debug)]
pub struct CircularList<T, O: ListObserver = ()> {
    slots: VecDeque<Option<T>>,
    max_length: usize,
    observer: O,
}

impl<T> CircularList<T, ()> {
    pub fn new(max_length: usize) -> Self {
        Self::with_observer(max_length, ())
    }
}

impl<T, O: ListObserver> CircularList<T, O> {
    pub fn with_observer(max_length: usize, observer: O) -> Self {
        Self {
            slots: VecDeque::new(),
            max_length,
            observer,
        }
    }

    pub fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }

    pub fn max_length(&self) -> usize {
        self.max_length
    }

    /// Upstream `set maxLength`: keeps the first `min(new, len)` items.
    pub fn set_max_length(&mut self, new_max_length: usize) {
        if self.max_length == new_max_length {
            return;
        }
        let shrinking = new_max_length < self.slots.len();
        self.slots.truncate(new_max_length);
        if shrinking {
            self.slots.shrink_to_fit();
        }
        self.max_length = new_max_length;
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Upstream `set length`: growing adds empty slots, shrinking drops items. Clamped to
    /// `max_length` (module doc).
    pub fn set_len(&mut self, new_len: usize) {
        let new_len = new_len.min(self.max_length);
        if new_len > self.slots.len() {
            self.slots.resize_with(new_len, || None);
        } else {
            self.slots.truncate(new_len);
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.slots.get(index).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.slots.get_mut(index).and_then(Option::as_mut)
    }

    /// Sets the slot at `index`; a no-op when `index >= len()` (use [`set_len`](Self::set_len)
    /// first).
    pub fn set(&mut self, index: usize, value: Option<T>) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = value;
        }
    }

    /// Pushes a value, dropping index 0 (and firing `on_trim(1)`) when the list is full.
    pub fn push(&mut self, value: T) {
        if self.slots.len() >= self.max_length {
            self.slots.pop_front();
            if self.max_length > 0 {
                self.slots.push_back(Some(value));
            }
            self.observer.on_trim(1);
        } else {
            self.slots.push_back(Some(value));
        }
    }

    /// Advances the ring and returns the slot that now sits at the end, for reuse. Fires
    /// `on_trim(1)` as upstream does.
    ///
    /// # Errors
    /// [`ListError::NotFull`] when the list is not at `max_length`; [`ListError::EmptySlot`] when
    /// the oldest slot holds nothing.
    pub fn recycle(&mut self) -> Result<&mut T, ListError> {
        if self.slots.len() != self.max_length {
            return Err(ListError::NotFull);
        }
        if !self.slots.front().is_some_and(Option::is_some) {
            return Err(ListError::EmptySlot);
        }
        self.slots.rotate_left(1);
        self.observer.on_trim(1);
        self.slots
            .back_mut()
            .and_then(Option::as_mut)
            .ok_or(ListError::EmptySlot)
    }

    /// The list is at its maximum length.
    pub fn is_full(&self) -> bool {
        self.slots.len() == self.max_length
    }

    /// Removes and returns the last value. Fires nothing.
    pub fn pop(&mut self) -> Option<T> {
        self.slots.pop_back().flatten()
    }

    /// Deletes and/or inserts items at `start` (in that order), firing `on_delete`, `on_insert`
    /// and `on_trim` in upstream's order.
    ///
    /// A `start` past the end leaves empty slots in the gap: upstream writes the items at
    /// `start..` but the length only grows by `items.len()`, so only the part of the gap that
    /// falls inside the new length is visible (as empty slots, then the leading items).
    pub fn splice(&mut self, start: usize, delete_count: usize, items: Vec<T>) {
        self.splice_inner(start, delete_count, items, true);
    }

    /// LUNETERM: (review S6) `n` times `splice(delete_at, 1, [])` then
    /// `splice(insert_at, 0, [item()])`, the per-line loop of IL, DL, SU and SD, with the
    /// observer told once ([`ListObserver::on_rotate`]) instead of `2 * n` times. The items end
    /// up as the loop leaves them. The loop never trims when `max_length >= 1`: a delete of one
    /// item shrinks a non-empty list by one, which the insert restores, and on an empty list the
    /// insert makes one item. With `max_length == 0` it runs the plain loop.
    pub fn splice_rotate(
        &mut self,
        delete_at: usize,
        insert_at: usize,
        n: usize,
        mut item: impl FnMut() -> T,
    ) {
        if self.max_length == 0 {
            for _ in 0..n {
                self.splice(delete_at, 1, Vec::new());
                self.splice(insert_at, 0, vec![item()]);
            }
            return;
        }
        for _ in 0..n {
            self.splice_inner(delete_at, 1, Vec::new(), false);
            self.splice_inner(insert_at, 0, vec![item()], false);
        }
        if n != 0 {
            self.observer.on_rotate(delete_at, insert_at, n);
        }
    }

    /// `splice`, telling the observer only when `notify` is set.
    fn splice_inner(&mut self, start: usize, delete_count: usize, items: Vec<T>, notify: bool) {
        if delete_count != 0 {
            let len = self.slots.len();
            match start.checked_add(delete_count) {
                Some(end) if end <= len => {
                    self.slots.drain(start..end);
                }
                // Upstream's copy loop does not run, so only the length shrinks.
                _ => self.slots.truncate(len.saturating_sub(delete_count)),
            }
            if notify {
                self.observer.on_delete(DeleteEvent {
                    index: start,
                    amount: delete_count,
                });
            }
        }

        let amount = items.len();
        let len = self.slots.len();
        if start <= len {
            if amount == 1 {
                if let Some(item) = items.into_iter().next() {
                    self.slots.insert(start, Some(item));
                }
            } else if amount > 1 {
                let tail = self.slots.split_off(start);
                self.slots.extend(items.into_iter().map(Some));
                self.slots.extend(tail);
            }
        } else {
            let gap = start - len;
            let empty = gap.min(amount);
            self.slots
                .extend(std::iter::repeat_with(|| None).take(empty));
            self.slots
                .extend(items.into_iter().take(amount - empty).map(Some));
        }
        if amount != 0 && notify {
            self.observer.on_insert(InsertEvent {
                index: start,
                amount,
            });
        }

        if self.slots.len() > self.max_length {
            let count_to_trim = self.slots.len() - self.max_length;
            self.slots.drain(..count_to_trim);
            if notify {
                self.observer.on_trim(count_to_trim);
            }
        }
    }

    /// Removes `count` items (at most `len()`) from the start of the list and fires `on_trim`
    /// with the removed count, even when it is 0.
    pub fn trim_start(&mut self, count: usize) {
        let count = count.min(self.slots.len());
        self.slots.drain(..count);
        self.observer.on_trim(count);
    }

    /// Moves `count` items from `start` by `offset` places. The items at the destination are
    /// replaced; a forward shift past the end grows the list (trimming from the start, with
    /// `on_trim(1)` per trimmed item, if that exceeds `max_length`).
    ///
    /// The arguments are signed because upstream's guards reject negatives: `count <= 0` is a
    /// no-op.
    ///
    /// # Errors
    /// [`ListError::StartOutOfRange`] when `start` is not in `0..len()`,
    /// [`ListError::BeyondZero`] when `start + offset < 0`, [`ListError::ShiftTooFar`] (module
    /// doc).
    pub fn shift_elements(
        &mut self,
        start: isize,
        count: isize,
        offset: isize,
    ) -> Result<(), ListError>
    where
        T: Clone,
    {
        if count <= 0 {
            return Ok(());
        }
        let len = self.slots.len();
        let start = match usize::try_from(start) {
            Ok(s) if s < len => s,
            _ => return Err(ListError::StartOutOfRange),
        };
        let too_far = if offset < 0 {
            ListError::BeyondZero
        } else {
            ListError::ShiftTooFar
        };
        let dest_start = start.checked_add_signed(offset).ok_or(too_far)?;
        let count = usize::try_from(count).map_err(|_| ListError::ShiftTooFar)?;

        if offset > 0 {
            let total = dest_start
                .checked_add(count)
                .filter(|t| *t <= len.saturating_add(self.max_length))
                .ok_or(ListError::ShiftTooFar)?;
            if total > len {
                self.slots.resize_with(total, || None);
            }
            // Descending: a source below `dest_start` stays visible, so it is cloned; one at or
            // above it is overwritten later in this loop, so it moves.
            for k in (0..count).rev() {
                let q = start + k;
                let value = if q >= dest_start {
                    self.slots.get_mut(q).and_then(Option::take)
                } else {
                    self.slots.get(q).and_then(Option::clone)
                };
                if let Some(slot) = self.slots.get_mut(q + offset.unsigned_abs()) {
                    *slot = value;
                }
            }
            while self.slots.len() > self.max_length {
                self.slots.pop_front();
                self.observer.on_trim(1);
            }
        } else {
            let neg = offset.unsigned_abs();
            let src_end = start.saturating_add(count);
            // Sources past `len()` are empty; their destinations below `len()` are cleared.
            let q_end = src_end.min(len.saturating_add(neg));
            for q in start..q_end {
                let value = if q >= len {
                    None
                } else if q.saturating_add(neg) < src_end {
                    self.slots.get_mut(q).and_then(Option::take)
                } else {
                    self.slots.get(q).and_then(Option::clone)
                };
                if let Some(slot) = q.checked_sub(neg).and_then(|d| self.slots.get_mut(d)) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }

    /// Fires `on_trim` as `Buffer`/`BufferReflow` do upstream (`lines.onTrimEmitter.fire`).
    pub(crate) fn notify_trim(&mut self, amount: usize) {
        self.observer.on_trim(amount);
    }

    /// Fires `on_insert` (`lines.onInsertEmitter.fire`).
    pub(crate) fn notify_insert(&mut self, index: usize, amount: usize) {
        self.observer.on_insert(InsertEvent { index, amount });
    }

    /// Fires `on_delete` (`lines.onDeleteEmitter.fire`).
    pub(crate) fn notify_delete(&mut self, index: usize, amount: usize) {
        self.observer.on_delete(DeleteEvent { index, amount });
    }

    /// Takes the item out of slot `index`, leaving the slot empty.
    pub(crate) fn take(&mut self, index: usize) -> Option<T> {
        self.slots.get_mut(index).and_then(Option::take)
    }

    /// Takes every slot out, in order, leaving the list empty (`len() == 0`). No events.
    pub(crate) fn take_all(&mut self) -> Vec<Option<T>> {
        self.slots.drain(..).collect()
    }

    /// Replaces the contents with `slots` (`len()` becomes `slots.len()`, clamped to
    /// `max_length` by dropping the tail). No events.
    pub(crate) fn replace_all(&mut self, slots: Vec<Option<T>>) {
        self.slots = slots.into();
        self.slots.truncate(self.max_length);
    }
}

#[cfg(test)]
#[path = "circular_list_tests.rs"]
mod tests;
