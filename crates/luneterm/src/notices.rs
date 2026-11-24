//! The notices model (bell-notifications plan step 3, D2, D6, D7, D10, D13, D14): what the app
//! remembers about bells and program messages, and the pure policy around it. No UI, no luneui
//! type, no clock of its own: every time is a [`Duration`] since the app started (the toast
//! clock), passed in.
//!
//!  - [`Inbox`] is the only store. A tab's dot is "has an unread row and is not the active tab",
//!    the header badge is [`Inbox::unread`], the list is [`Inbox::entries`]; nothing else keeps a
//!    copy. [`Inbox::settle`] keeps "the tab you look at has nothing unread" and "a closed tab has
//!    no rows" true from one place.
//!  - [`route`] is the D6 table: what a bell or a message does by window focus and by whether its
//!    tab is the active one.
//!  - [`ToastSlots`] is the toast bookkeeping (D10, D14): one live toast per tab, three in all, a
//!    tab's toast changes at most once a second, and a toast ignores a press for 400 ms after it
//!    appears or its text is replaced. Toast ids are opaque.
//!  - [`age`] and [`next_change`] are the row labels and when they next change.
//!
//! The text a row holds was sanitised in `Tab::apply` and is capped there; this module stores the
//! two strings and nothing else of a message, never logs, and its `Debug` impls show lengths and
//! counts, never the text (text from a program stays out of every debug output).

use std::fmt;
use std::time::Duration;

use crate::tabs::TabId;

/// Most rows of one tab (D10): the oldest of the tab goes first.
pub const TAB_CAP: usize = 20;
/// Most rows in all (D10): the oldest read row goes first, or the oldest row when none is read.
pub const INBOX_CAP: usize = 50;

/// How long a notification toast lives (luneui runs the clock; the app asks for this).
pub const TOAST_LIFE: Duration = Duration::from_secs(6);
/// Most notification toasts alive at once.
pub const TOAST_LIVE_CAP: usize = 3;
/// A tab's toast changes at most once in this time (D10).
pub const TOAST_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// A toast ignores its Go to tab press for this long after it appeared or was replaced (D14).
pub const TOAST_ARM: Duration = Duration::from_millis(400);

// --------------------------------------------------------------------------------------- routing

/// What one bell or message does (D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// Keep a row (and so a dot and a badge count).
    pub record: bool,
    /// Show a toast (messages only).
    pub toast: bool,
    /// A candidate for the OS attention request; the caller still asks its rate limiter.
    pub os_attention: bool,
}

/// The D6 table. `focused` is the window focus; `active` is whether the tab it came from is the
/// active one.
///
/// | | focused, active | focused, other | unfocused, active | unfocused, other |
/// |---|---|---|---|---|
/// | bell | nothing | row | attention | row + attention |
/// | message | nothing | row + toast | attention | row + attention |
pub fn route(is_bell: bool, focused: bool, active: bool) -> Route {
    match (focused, active) {
        (true, true) => Route {
            record: false,
            toast: false,
            os_attention: false,
        },
        (true, false) => Route {
            record: true,
            toast: !is_bell,
            os_attention: false,
        },
        (false, true) => Route {
            record: false,
            toast: false,
            os_attention: true,
        },
        (false, false) => Route {
            record: true,
            toast: false,
            os_attention: true,
        },
    }
}

// ----------------------------------------------------------------------------------------- inbox

/// What a row says.
#[derive(Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// A bell; `Notice::count` is how many.
    Bell,
    /// A program message, sanitised: either field may be empty, not both.
    Message { title: String, body: String },
}

/// One row of the inbox.
#[derive(Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub tab: TabId,
    pub kind: NoticeKind,
    /// Bells folded into this row, or identical messages (always at least 1).
    pub count: u32,
    /// When it last happened: time since the app started.
    pub at: Duration,
    pub unread: bool,
}

impl fmt::Debug for NoticeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoticeKind::Bell => f.write_str("Bell"),
            NoticeKind::Message { title, body } => f
                .debug_struct("Message")
                .field("title_chars", &title.chars().count())
                .field("body_chars", &body.chars().count())
                .finish(),
        }
    }
}

impl fmt::Debug for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Notice")
            .field("id", &self.id)
            .field("tab", &self.tab)
            .field("kind", &self.kind)
            .field("count", &self.count)
            .field("at", &self.at)
            .field("unread", &self.unread)
            .finish()
    }
}

/// The inbox: newest first by `at`, bounded, in memory only (D15).
#[derive(Clone, Default)]
pub struct Inbox {
    rows: Vec<Notice>,
    next_id: u64,
}

impl fmt::Debug for Inbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inbox")
            .field("rows", &self.rows.len())
            .field("unread", &self.unread())
            .finish()
    }
}

impl Inbox {
    pub fn new() -> Self {
        Inbox::default()
    }

    /// `n` bells from `tab` at `now`. Folds into the tab's unread bell row (it moves to the front),
    /// else a new unread row. `n == 0` does nothing.
    pub fn bell(&mut self, tab: TabId, n: u32, now: Duration) {
        if n == 0 {
            return;
        }
        let existing = self
            .rows
            .iter()
            .position(|r| r.tab == tab && r.unread && r.kind == NoticeKind::Bell);
        match existing {
            Some(index) => {
                let mut row = self.rows.remove(index);
                row.count = row.count.saturating_add(n);
                row.at = now;
                self.place(row);
            }
            None => self.add(tab, NoticeKind::Bell, n, now),
        }
        self.trim(tab);
    }

    /// A message from `tab` at `now` (both strings already sanitised). Folds into the tab's newest
    /// row when that is an unread message with the same title and body, else a new unread row.
    pub fn message(&mut self, tab: TabId, title: String, body: String, now: Duration) {
        let same = self.rows.iter().position(|r| r.tab == tab).filter(|&i| {
            let r = &self.rows[i];
            r.unread
                && matches!(&r.kind, NoticeKind::Message { title: t, body: b }
                    if *t == title && *b == body)
        });
        match same {
            Some(index) => {
                let mut row = self.rows.remove(index);
                row.count = row.count.saturating_add(1);
                row.at = now;
                self.place(row);
            }
            None => self.add(tab, NoticeKind::Message { title, body }, 1, now),
        }
        self.trim(tab);
    }

    fn add(&mut self, tab: TabId, kind: NoticeKind, count: u32, now: Duration) {
        let id = self.next_id;
        self.next_id += 1;
        self.place(Notice {
            id,
            tab,
            kind,
            count,
            at: now,
            unread: true,
        });
    }

    /// Insert keeping `at` descending; among equal times the newest call goes first.
    fn place(&mut self, row: Notice) {
        let at = self.rows.iter().position(|r| r.at <= row.at);
        self.rows.insert(at.unwrap_or(self.rows.len()), row);
    }

    /// The caps, after an insert for `tab`.
    fn trim(&mut self, tab: TabId) {
        let mut of_tab = self.rows.iter().filter(|r| r.tab == tab).count();
        while of_tab > TAB_CAP {
            if let Some(index) = self.rows.iter().rposition(|r| r.tab == tab) {
                self.rows.remove(index);
            }
            of_tab -= 1;
        }
        while self.rows.len() > INBOX_CAP {
            let index = self
                .rows
                .iter()
                .rposition(|r| !r.unread)
                .unwrap_or(self.rows.len() - 1);
            self.rows.remove(index);
        }
    }

    /// The rows, newest first.
    pub fn entries(&self) -> impl Iterator<Item = &Notice> {
        self.rows.iter()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Unread rows: the header badge.
    pub fn unread(&self) -> usize {
        self.rows.iter().filter(|r| r.unread).count()
    }

    /// Unread rows of `tab`: nonzero means the tab shows its dot while it is not the active one.
    pub fn tab_unread(&self, tab: TabId) -> usize {
        self.rows
            .iter()
            .filter(|r| r.tab == tab && r.unread)
            .count()
    }

    /// Remove everything ("Clear all").
    pub fn clear(&mut self) {
        self.rows.clear();
    }

    /// Keeps the inbox true (D13): rows of tabs that are not `alive` are removed; when `focused`,
    /// every row of `active` is marked read. Returns whether anything changed. After it: no row of
    /// a dead tab, and while focused the active tab has nothing unread.
    pub fn settle(
        &mut self,
        active: Option<TabId>,
        focused: bool,
        alive: impl Fn(TabId) -> bool,
    ) -> bool {
        let before = self.rows.len();
        self.rows.retain(|r| alive(r.tab));
        let mut changed = self.rows.len() != before;
        if focused && let Some(active) = active {
            for row in self.rows.iter_mut().filter(|r| r.tab == active && r.unread) {
                row.unread = false;
                changed = true;
            }
        }
        changed
    }
}

// ------------------------------------------------------------------------------------ toast slots

/// What to do with a message's toast (D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit<Id> {
    /// Push a toast; first dismiss `evict` (the oldest) when three are live.
    New { evict: Option<Id> },
    /// Update this toast in place.
    Replace(Id),
    /// Show nothing: the tab's toast changed less than a second ago.
    Skip,
}

#[derive(Debug, Clone, Copy)]
struct Slot<Id> {
    id: Id,
    tab: TabId,
    /// When the toast appeared or last had its text replaced.
    changed: Duration,
}

/// The live notification toasts: at most one per tab, at most [`TOAST_LIVE_CAP`] in all. The
/// caller pushes what [`ToastSlots::admit`] says and then reports it with
/// [`ToastSlots::shown`] (for a replacement too); a toast that left is reported with
/// [`ToastSlots::gone`]. In creation order, oldest first.
#[derive(Debug, Clone)]
pub struct ToastSlots<Id> {
    slots: Vec<Slot<Id>>,
}

impl<Id> Default for ToastSlots<Id> {
    fn default() -> Self {
        ToastSlots { slots: Vec::new() }
    }
}

impl<Id: Copy + PartialEq> ToastSlots<Id> {
    pub fn new() -> Self {
        ToastSlots::default()
    }

    /// What a message from `tab` at `now` does.
    pub fn admit(&self, tab: TabId, now: Duration) -> Admit<Id> {
        if let Some(slot) = self.slots.iter().find(|s| s.tab == tab) {
            return if now.saturating_sub(slot.changed) >= TOAST_MIN_INTERVAL {
                Admit::Replace(slot.id)
            } else {
                Admit::Skip
            };
        }
        Admit::New {
            evict: (self.slots.len() >= TOAST_LIVE_CAP)
                .then(|| self.slots.first().map(|s| s.id))
                .flatten(),
        }
    }

    /// Toast `id` of `tab` appeared at `now`, or (when `id` is already known) had its text
    /// replaced: either way its interval and arm delay restart.
    pub fn shown(&mut self, id: Id, tab: TabId, now: Duration) {
        match self.slots.iter_mut().find(|s| s.id == id) {
            Some(slot) => {
                slot.tab = tab;
                slot.changed = now;
            }
            None => self.slots.push(Slot {
                id,
                tab,
                changed: now,
            }),
        }
    }

    /// The toast left (it timed out, was closed, or was dismissed): free its slot.
    pub fn gone(&mut self, id: Id) {
        self.slots.retain(|s| s.id != id);
    }

    /// The tab a live toast belongs to.
    pub fn tab_of(&self, id: Id) -> Option<TabId> {
        self.slots.iter().find(|s| s.id == id).map(|s| s.tab)
    }

    /// Whether a press on toast `id` may act yet: false within [`TOAST_ARM`] of its appearing or
    /// of its text being replaced, and for a toast this table does not know.
    pub fn armed(&self, id: Id, now: Duration) -> bool {
        self.slots
            .iter()
            .find(|s| s.id == id)
            .is_some_and(|s| now.saturating_sub(s.changed) >= TOAST_ARM)
    }

    /// Frees the slots whose tab satisfies `pred` and returns their ids (the caller dismisses the
    /// toasts): used by settle for closed tabs and for the focused active tab.
    pub fn drop_where(&mut self, mut pred: impl FnMut(TabId) -> bool) -> Vec<Id> {
        let mut dropped = Vec::new();
        self.slots.retain(|s| {
            let drop = pred(s.tab);
            if drop {
                dropped.push(s.id);
            }
            !drop
        });
        dropped
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

// ------------------------------------------------------------------------------------------- age

/// A row's age label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    /// Under a minute.
    Now,
    Minutes(u32),
    Hours(u32),
    Days(u32),
}

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(86_400);

/// The label of something `elapsed` old: under 60 s `Now`; under 60 min minutes; under 24 h hours;
/// else days.
pub fn age(elapsed: Duration) -> Age {
    let count =
        |unit: Duration| u32::try_from(elapsed.as_secs() / unit.as_secs()).unwrap_or(u32::MAX);
    if elapsed < MINUTE {
        Age::Now
    } else if elapsed < HOUR {
        Age::Minutes(count(MINUTE))
    } else if elapsed < DAY {
        Age::Hours(count(HOUR))
    } else {
        Age::Days(count(DAY))
    }
}

/// How long after an age of `elapsed` the label next changes (always more than zero): the repaint
/// request for an open list.
pub fn next_change(elapsed: Duration) -> Duration {
    let unit = if elapsed < HOUR {
        MINUTE
    } else if elapsed < DAY {
        HOUR
    } else {
        DAY
    };
    let unit_nanos = unit.as_nanos();
    let next = (elapsed.as_nanos() / unit_nanos + 1) * unit_nanos;
    Duration::from_nanos(u64::try_from(next - elapsed.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn msg(inbox: &mut Inbox, tab: TabId, title: &str, body: &str, at: Duration) {
        inbox.message(tab, title.to_owned(), body.to_owned(), at);
    }

    fn describe(n: &Notice) -> String {
        match &n.kind {
            NoticeKind::Bell => format!("bell x{} tab{}", n.count, n.tab),
            NoticeKind::Message { title, body } => {
                format!("msg {title}|{body} x{} tab{}", n.count, n.tab)
            }
        }
    }

    // ---- route

    #[test]
    fn the_route_table_has_all_eight_cells_of_d6() {
        // (is_bell, focused, active) -> (record, toast, os_attention)
        let table = [
            (true, true, true, (false, false, false)),
            (true, true, false, (true, false, false)),
            (true, false, true, (false, false, true)),
            (true, false, false, (true, false, true)),
            (false, true, true, (false, false, false)),
            (false, true, false, (true, true, false)),
            (false, false, true, (false, false, true)),
            (false, false, false, (true, false, true)),
        ];
        for (bell, focused, active, (record, toast, os)) in table {
            assert_eq!(
                route(bell, focused, active),
                Route {
                    record,
                    toast,
                    os_attention: os
                },
                "bell {bell} focused {focused} active {active}"
            );
        }
    }

    // ---- inbox

    #[test]
    fn a_bell_folds_into_the_tabs_unread_bell_row_and_moves_to_the_front() {
        let mut inbox = Inbox::new();
        inbox.bell(1, 0, s(1));
        assert!(inbox.is_empty(), "n == 0 does nothing");
        inbox.bell(1, 2, s(1));
        msg(&mut inbox, 2, "", "other tab", s(2));
        inbox.bell(1, 3, s(3));
        let rows: Vec<String> = inbox.entries().map(describe).collect();
        assert_eq!(rows, ["bell x5 tab1", "msg |other tab x1 tab2"]);
        assert_eq!(inbox.entries().next().map(|r| r.at), Some(s(3)));
        // saturating
        inbox.bell(1, u32::MAX, s(4));
        assert_eq!(inbox.entries().next().map(|r| r.count), Some(u32::MAX));
        inbox.bell(1, 1, s(5));
        assert_eq!(inbox.entries().next().map(|r| r.count), Some(u32::MAX));
    }

    #[test]
    fn a_bell_after_the_row_was_read_makes_a_new_unread_row() {
        let mut inbox = Inbox::new();
        inbox.bell(1, 1, s(1));
        assert!(inbox.settle(Some(1), true, |_| true));
        assert_eq!(inbox.unread(), 0);
        inbox.bell(1, 1, s(2));
        assert_eq!(inbox.len(), 2, "the read one stays as history");
        assert_eq!(inbox.unread(), 1);
        let rows: Vec<(bool, u32)> = inbox.entries().map(|r| (r.unread, r.count)).collect();
        assert_eq!(rows, [(true, 1), (false, 1)]);
    }

    #[test]
    fn an_identical_message_folds_only_into_the_tabs_newest_unread_message_row() {
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "T", "B", s(1));
        msg(&mut inbox, 1, "T", "B", s(2));
        assert_eq!(inbox.len(), 1);
        assert_eq!(
            inbox.entries().next().map(|r| (r.count, r.at)),
            Some((2, s(2)))
        );
        // another message in between: the old one is no longer the newest of the tab
        msg(&mut inbox, 1, "T", "C", s(3));
        msg(&mut inbox, 1, "T", "B", s(4));
        let rows: Vec<String> = inbox.entries().map(describe).collect();
        assert_eq!(
            rows,
            ["msg T|B x1 tab1", "msg T|C x1 tab1", "msg T|B x2 tab1"]
        );
        // the same text from another tab does not fold
        msg(&mut inbox, 2, "T", "B", s(5));
        assert_eq!(inbox.len(), 4);
        // a bell between breaks the run too
        inbox.bell(2, 1, s(6));
        msg(&mut inbox, 2, "T", "B", s(7));
        assert_eq!(inbox.len(), 6);
        // title and body are both compared
        msg(&mut inbox, 3, "", "x", s(8));
        msg(&mut inbox, 3, "x", "", s(9));
        assert_eq!(inbox.len(), 8);
        // a read row is not folded into
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "T", "B", s(1));
        inbox.settle(Some(1), true, |_| true);
        msg(&mut inbox, 1, "T", "B", s(2));
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox.unread(), 1);
    }

    #[test]
    fn the_mockups_list_is_four_rows_in_order_with_three_unread() {
        let t = s(7200);
        let mut inbox = Inbox::new();
        msg(&mut inbox, 3, "", "Build finished", t - s(3600));
        // the user looks at the tab: its row is read
        inbox.settle(Some(3), true, |_| true);
        msg(
            &mut inbox,
            1,
            "Claude Code",
            "Claude needs your permission",
            t - s(540),
        );
        inbox.bell(2, 1, t - s(180));
        inbox.bell(2, 1, t - s(120));
        msg(&mut inbox, 1, "Claude Code", "Claude finished", t);
        let rows: Vec<String> = inbox.entries().map(describe).collect();
        assert_eq!(
            rows,
            [
                "msg Claude Code|Claude finished x1 tab1",
                "bell x2 tab2",
                "msg Claude Code|Claude needs your permission x1 tab1",
                "msg |Build finished x1 tab3",
            ]
        );
        assert_eq!(inbox.unread(), 3);
        assert_eq!(
            inbox.entries().map(|r| r.unread).collect::<Vec<_>>(),
            [true, true, true, false]
        );
        assert_eq!(
            (
                inbox.tab_unread(1),
                inbox.tab_unread(2),
                inbox.tab_unread(3)
            ),
            (2, 1, 0)
        );
        // ages as the mockup labels them
        let labels: Vec<Age> = inbox.entries().map(|r| age(t - r.at)).collect();
        assert_eq!(
            labels,
            [Age::Now, Age::Minutes(2), Age::Minutes(9), Age::Hours(1)]
        );
    }

    #[test]
    fn the_order_is_by_time_descending_at_all_times() {
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "", "a", s(10));
        msg(&mut inbox, 2, "", "b", s(20));
        // a late fold of the older tab's bell goes in front of nothing older than itself
        inbox.bell(1, 1, s(30));
        inbox.bell(3, 1, s(15)); // an out-of-order time still lands in its place
        let times: Vec<Duration> = inbox.entries().map(|r| r.at).collect();
        assert_eq!(times, [s(30), s(20), s(15), s(10)]);
        let mut sorted = times.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(times, sorted);
    }

    #[test]
    fn a_flood_from_one_tab_keeps_twenty_rows_of_it() {
        let mut inbox = Inbox::new();
        for i in 0..10_000u64 {
            msg(&mut inbox, 1, "", &format!("m{i}"), ms(i));
        }
        assert_eq!(inbox.entries().filter(|r| r.tab == 1).count(), TAB_CAP);
        // the newest survived, the oldest went
        let first = inbox.entries().next().map(describe);
        assert_eq!(first.as_deref(), Some("msg |m9999 x1 tab1"));
        assert_eq!(inbox.len(), TAB_CAP);
    }

    #[test]
    fn the_inbox_cap_drops_the_oldest_read_row_before_any_unread_one() {
        let mut inbox = Inbox::new();
        // tab 2: twenty unread rows; tab 3: twenty read rows (older)
        for i in 0..20u64 {
            msg(&mut inbox, 3, "", &format!("old{i}"), ms(i));
        }
        inbox.settle(Some(3), true, |_| true);
        for i in 0..20u64 {
            msg(&mut inbox, 2, "", &format!("keep{i}"), ms(100 + i));
        }
        assert_eq!((inbox.len(), inbox.tab_unread(2)), (40, 20));
        // a flood from tab 1 (cap 20 of its own): 10 more rows fit, then read rows go
        for i in 0..10_000u64 {
            msg(&mut inbox, 1, "", &format!("flood{i}"), ms(1000 + i));
        }
        assert_eq!(inbox.len(), INBOX_CAP);
        assert_eq!(
            inbox.tab_unread(2),
            20,
            "another tab's unread rows are kept"
        );
        assert_eq!(inbox.tab_unread(1), 20);
        // the read rows were the ones that went (50 = 20 + 20 + 10 read left)
        assert_eq!(inbox.entries().filter(|r| r.tab == 3).count(), 10);
        // the oldest read rows went first: old0..old9 are gone
        assert!(
            inbox
                .entries()
                .filter(|r| r.tab == 3)
                .all(|r| r.at >= ms(10))
        );
    }

    #[test]
    fn with_no_read_row_the_oldest_row_goes() {
        let mut inbox = Inbox::new();
        for tab in 1..=3u64 {
            for i in 0..20u64 {
                msg(
                    &mut inbox,
                    tab,
                    "",
                    &format!("t{tab}m{i}"),
                    ms(tab * 1000 + i),
                );
            }
        }
        assert_eq!(inbox.len(), INBOX_CAP);
        assert_eq!(inbox.entries().filter(|r| r.tab == 1).count(), 10);
        let oldest = inbox.entries().last().map(describe);
        assert_eq!(oldest.as_deref(), Some("msg |t1m10 x1 tab1"));
    }

    #[test]
    fn settle_removes_dead_tabs_and_reads_the_active_one_only_while_focused() {
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "", "a", s(1));
        msg(&mut inbox, 2, "", "b", s(2));
        inbox.bell(3, 1, s(3));
        // unfocused: nothing is read; a closed tab's rows go
        assert!(inbox.settle(Some(1), false, |t| t != 3));
        assert_eq!(inbox.len(), 2);
        assert_eq!(
            (
                inbox.tab_unread(1),
                inbox.tab_unread(2),
                inbox.tab_unread(3)
            ),
            (1, 1, 0)
        );
        // nothing changed: false
        assert!(!inbox.settle(Some(1), false, |t| t != 3));
        // focused: the active tab's rows are read
        assert!(inbox.settle(Some(1), true, |t| t != 3));
        assert_eq!((inbox.tab_unread(1), inbox.tab_unread(2)), (0, 1));
        assert!(!inbox.settle(Some(1), true, |t| t != 3));
        // no active tab: nothing read, and the dead are still removed
        assert!(inbox.settle(None, true, |t| t == 2));
        assert_eq!((inbox.len(), inbox.unread()), (1, 1));
        // the invariants hold whatever the arguments
        for focused in [false, true] {
            for active in [None, Some(1), Some(2)] {
                inbox.settle(active, focused, |t| t == 2);
                assert!(inbox.entries().all(|r| r.tab == 2));
                if focused && let Some(a) = active {
                    assert_eq!(inbox.tab_unread(a), 0);
                }
            }
        }
    }

    #[test]
    fn clear_removes_everything() {
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "", "a", s(1));
        inbox.bell(2, 3, s(2));
        inbox.clear();
        assert!(inbox.is_empty());
        assert_eq!(
            (inbox.unread(), inbox.tab_unread(1), inbox.tab_unread(2)),
            (0, 0, 0)
        );
        // ids are not reused after a clear
        inbox.bell(1, 1, s(3));
        assert_eq!(inbox.entries().next().map(|r| r.id), Some(2));
    }

    // ---- toast slots

    #[test]
    fn one_tab_flooding_gets_one_toast_then_skips_for_a_second_then_replaces() {
        let mut slots: ToastSlots<u32> = ToastSlots::new();
        assert_eq!(slots.admit(1, s(10)), Admit::New { evict: None });
        slots.shown(100, 1, s(10));
        assert_eq!(slots.admit(1, s(10)), Admit::Skip);
        assert_eq!(slots.admit(1, ms(10_999)), Admit::Skip);
        assert_eq!(slots.admit(1, ms(11_000)), Admit::Replace(100));
        // a replacement restarts the interval
        slots.shown(100, 1, ms(11_000));
        assert_eq!(slots.len(), 1, "the same id is the same slot");
        assert_eq!(slots.admit(1, ms(11_500)), Admit::Skip);
        assert_eq!(slots.admit(1, ms(12_000)), Admit::Replace(100));
        // the toast leaves: the tab's next message gets a new toast at once
        slots.gone(100);
        assert_eq!(slots.admit(1, ms(12_001)), Admit::New { evict: None });
        assert!(slots.is_empty());
    }

    #[test]
    fn four_tabs_the_fourth_evicts_the_oldest() {
        let mut slots: ToastSlots<u32> = ToastSlots::new();
        for (i, tab) in [1u64, 2, 3].into_iter().enumerate() {
            assert_eq!(
                slots.admit(tab, s(i as u64)),
                Admit::New { evict: None },
                "tab {tab}"
            );
            slots.shown(100 + i as u32, tab, s(i as u64));
        }
        assert_eq!(slots.admit(4, s(5)), Admit::New { evict: Some(100) });
        // replacing the oldest one's text does not make it the newest to evict
        slots.shown(100, 1, s(5));
        assert_eq!(slots.admit(4, s(6)), Admit::New { evict: Some(100) });
        slots.gone(100);
        slots.shown(103, 4, s(6));
        assert_eq!(slots.len(), 3);
        assert_eq!(slots.admit(5, s(7)), Admit::New { evict: Some(101) });
        // a tab with a live toast never needs an eviction
        assert_eq!(slots.admit(2, s(9)), Admit::Replace(101));
        assert_eq!(slots.tab_of(103), Some(4));
        assert_eq!(slots.tab_of(100), None);
    }

    #[test]
    fn a_toast_is_armed_after_400_ms_and_a_replacement_restarts_it() {
        let mut slots: ToastSlots<u32> = ToastSlots::new();
        slots.shown(7, 1, s(10));
        assert!(!slots.armed(7, s(10)));
        assert!(!slots.armed(7, ms(10_399)));
        assert!(slots.armed(7, ms(10_400)));
        slots.shown(7, 1, s(20));
        assert!(!slots.armed(7, ms(20_399)));
        assert!(slots.armed(7, ms(20_400)));
        // a toast the table does not know is never armed; a clock before `shown` is not armed
        assert!(!slots.armed(8, s(99)));
        assert!(!slots.armed(7, s(5)));
    }

    #[test]
    fn drop_where_frees_the_slots_of_closed_or_watched_tabs() {
        let mut slots: ToastSlots<u32> = ToastSlots::new();
        slots.shown(1, 10, s(0));
        slots.shown(2, 20, s(0));
        slots.shown(3, 30, s(0));
        let dropped = slots.drop_where(|tab| tab == 10 || tab == 30);
        assert_eq!(dropped, [1, 3]);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots.tab_of(2), Some(20));
        assert!(slots.drop_where(|_| false).is_empty());
    }

    #[test]
    fn debug_output_never_carries_message_text() {
        let mut inbox = Inbox::new();
        msg(&mut inbox, 1, "hunter2", "hunter2", s(1));
        let shown = format!("{inbox:?} {:?}", inbox.entries().collect::<Vec<_>>());
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("rows"), "{shown}");
    }

    // ---- age

    #[test]
    fn age_labels_by_threshold() {
        assert_eq!(age(Duration::ZERO), Age::Now);
        assert_eq!(age(ms(59_999)), Age::Now);
        assert_eq!(age(s(60)), Age::Minutes(1));
        assert_eq!(age(s(119)), Age::Minutes(1));
        assert_eq!(age(s(120)), Age::Minutes(2));
        assert_eq!(age(s(3599)), Age::Minutes(59));
        assert_eq!(age(s(3600)), Age::Hours(1));
        assert_eq!(age(s(86_399)), Age::Hours(23));
        assert_eq!(age(s(86_400)), Age::Days(1));
        assert_eq!(age(s(86_400 * 40)), Age::Days(40));
        assert_eq!(age(Duration::MAX), Age::Days(u32::MAX));
    }

    #[test]
    fn next_change_is_when_the_label_next_changes() {
        for (elapsed, want) in [
            (ms(0), s(60)),
            (s(59), s(1)),
            (ms(59_500), ms(500)),
            (s(60), s(60)),
            (s(90), s(30)),
            (s(3599), s(1)),
            (s(3600), s(3600)),
            (s(5400), s(1800)),
            (s(86_399), s(1)),
            (s(86_400), s(86_400)),
            (s(90_000), s(82_800)),
        ] {
            let d = next_change(elapsed);
            assert_eq!(d, want, "{elapsed:?}");
            assert!(d > Duration::ZERO);
            // exactly at the change the label differs; just before it, it does not
            assert_ne!(age(elapsed + d), age(elapsed), "{elapsed:?}");
            assert_eq!(age(elapsed + d - ms(1)), age(elapsed), "{elapsed:?}");
        }
    }
}
