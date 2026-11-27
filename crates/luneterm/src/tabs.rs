//! Tabs (plan M10 step 2, D1): one [`Tab`] per [`Session`], the pure list logic ([`TabSet`], tab
//! groups included: [`crate::groups`]), the rules that decide what closing asks, the tab's name,
//! the paste routing and the check of a directory for a new tab.
//!
//! The UI thread owns all of it. What needs the buffer or the PTY stays on the session's engine
//! thread and arrives in an [`Update`] (M9 I1); nothing here touches a `Terminal`.

use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use lune_termview::{Cursor, Grid, TermTheme};
use lune_ui::input::Event;
use lune_ui::widgets::tone::Tone;
use lune_vt::ProgressState;

use crate::app::{cursor_of, theme_with_palette};
use crate::cwd;
use crate::groups::{Group, GroupColor, GroupId, default_color};
use crate::links::{self, Hover, LinkSpan, MAX_URI_TABLE, UriTable};
use crate::outline::CommandState;
use crate::session::{
    Command, CopyResult, ForegroundState, Geometry, SelectionView, Session, Update, UpdateModes,
    apply_update,
};
use crate::shell::ScrollView;
use crate::spawn::SpawnFailure;
use crate::title::{self, DEFAULT_TITLE, NOTICE_BODY_CHARS, NOTICE_TITLE_CHARS};

/// The app's identity of a tab: stable across reorders and closes, never reused.
pub type TabId = u64;

/// What a tab with no manual name and no title from its program is called (Lunegit's
/// `terminal.tab.defaultTitle`).
pub const DEFAULT_TAB_NAME: &str = "Terminal";

/// How long a new tab waits for the directory check before it starts in the default directory.
pub const CWD_CHECK_TIMEOUT: Duration = Duration::from_millis(300);

// ----------------------------------------------------------------------------------- the list

/// The tabs in order, their groups, which tab is active, and the next ids. Pure list logic: no
/// window, no session.
///
/// Every public method keeps this true: the members of a group are next to each other (the
/// group's run), a group has at least one member, group names are unique (compared exactly), and a
/// tab or group id is never given twice.
#[derive(Debug)]
pub struct TabSet<T> {
    items: Vec<Entry<T>>,
    /// The open groups. Their order is that of their runs, not of this list.
    groups: Vec<(GroupId, Group)>,
    active: Option<TabId>,
    next: TabId,
    next_group: u64,
}

#[derive(Debug)]
struct Entry<T> {
    id: TabId,
    group: Option<GroupId>,
    item: T,
}

impl<T> Default for TabSet<T> {
    fn default() -> Self {
        TabSet {
            items: Vec::new(),
            groups: Vec::new(),
            active: None,
            next: 1,
            next_group: GroupId::NONE.0 + 1,
        }
    }
}

impl<T> TabSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The id the next new tab gets ([`push`](Self::push), [`insert`](Self::insert),
    /// [`insert_by_hand`](Self::insert_by_hand)). Nothing else uses ids up, so a child can be
    /// told it before its tab is in the list.
    pub fn next_id(&self) -> TabId {
        self.next
    }

    /// Appends `item` at the end, in no group, and makes it active.
    pub fn push(&mut self, item: T) -> TabId {
        self.insert(item, self.items.len(), true)
    }

    /// Adds `item`, in no group, at index `at`; past the end means the end, and an index inside a
    /// group's run means just after that run. It becomes active only with `activate`.
    pub fn insert(&mut self, item: T, at: usize, activate: bool) -> TabId {
        let at = at.min(self.items.len());
        let at = self.run_split_at(at).map_or(at, |run| run.end);
        let id = self.next;
        self.next += 1;
        let entry = Entry {
            id,
            group: None,
            item,
        };
        self.items.insert(at, entry);
        if activate {
            self.active = Some(id);
        }
        id
    }

    /// A tab the user asked for, opened from `reference` (the active tab, or the one a menu is
    /// about), and made active. From a grouped tab it joins that group after the last member;
    /// else it goes to the end, in no group.
    pub fn insert_by_hand(&mut self, item: T, reference: Option<TabId>) -> TabId {
        let group = reference.and_then(|id| self.group_of(id));
        let at = group
            .and_then(|g| self.run(g))
            .map_or(self.items.len(), |run| run.end);
        let id = self.insert(item, at, true);
        if let Some(entry) = self.items.iter_mut().find(|e| e.id == id) {
            entry.group = group;
        }
        id
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn ids(&self) -> Vec<TabId> {
        self.items.iter().map(|e| e.id).collect()
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.items.iter().position(|e| e.id == id)
    }

    pub fn get(&self, id: TabId) -> Option<&T> {
        self.items.iter().find(|e| e.id == id).map(|e| &e.item)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut T> {
        self.items
            .iter_mut()
            .find(|e| e.id == id)
            .map(|e| &mut e.item)
    }

    pub fn active_item(&self) -> Option<&T> {
        self.active.and_then(|id| self.get(id))
    }

    pub fn active_item_mut(&mut self) -> Option<&mut T> {
        let id = self.active?;
        self.get_mut(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TabId, &T)> {
        self.items.iter().map(|e| (e.id, &e.item))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (TabId, &mut T)> {
        self.items.iter_mut().map(|e| (e.id, &mut e.item))
    }

    /// Makes `id` active. `false` when there is no such tab. A collapsed group stays collapsed
    /// ([`expand_for`](Self::expand_for)).
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.index_of(id).is_some() {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    /// Activates the tab `delta` places after the active one, wrapping around: over all tabs,
    /// those of collapsed groups included.
    pub fn activate_step(&mut self, delta: isize) {
        let (Some(at), len) = (self.active.and_then(|a| self.index_of(a)), self.items.len()) else {
            return;
        };
        let len = isize::try_from(len).unwrap_or(isize::MAX);
        let at = isize::try_from(at).unwrap_or(0);
        let to = (at + delta).rem_euclid(len);
        self.active = usize::try_from(to)
            .ok()
            .and_then(|i| self.items.get(i))
            .map(|e| e.id);
    }

    /// Activates the tab at `index` (0-based, over all tabs); `false` when there is none.
    pub fn activate_index(&mut self, index: usize) -> bool {
        match self.items.get(index) {
            Some(entry) => {
                self.active = Some(entry.id);
                true
            }
            None => false,
        }
    }

    /// Removes `id`. Closing the active tab activates its right neighbour, else its left one;
    /// closing another tab leaves the active one. A group left without members is gone.
    pub fn close(&mut self, id: TabId) -> Option<T> {
        let at = self.index_of(id)?;
        let entry = self.items.remove(at);
        if let Some(group) = entry.group {
            self.drop_if_empty(group);
        }
        if self.active == Some(id) {
            self.active = self
                .items
                .get(at)
                .or_else(|| self.items.last())
                .map(|e| e.id);
        }
        Some(entry.item)
    }

    // ------------------------------------------------------------------------------- groups

    /// The group of tab `id`; `None` when it has none or there is no such tab.
    pub fn group_of(&self, id: TabId) -> Option<GroupId> {
        self.items.iter().find(|e| e.id == id).and_then(|e| e.group)
    }

    /// The tabs of group `g`, in order; empty when there is no such group.
    pub fn members(&self, g: GroupId) -> Vec<TabId> {
        self.items
            .iter()
            .filter(|e| e.group == Some(g))
            .map(|e| e.id)
            .collect()
    }

    pub fn group(&self, g: GroupId) -> Option<&Group> {
        self.groups.iter().find(|(i, _)| *i == g).map(|(_, g)| g)
    }

    /// The open groups, in the order of their runs.
    pub fn groups(&self) -> impl Iterator<Item = (GroupId, &Group)> {
        let mut last = None;
        self.items.iter().filter_map(move |e| {
            let first = e.group != last;
            last = e.group;
            let id = e.group.filter(|_| first)?;
            self.group(id).map(|g| (id, g))
        })
    }

    /// The group with exactly this name.
    pub fn group_named(&self, name: &str) -> Option<GroupId> {
        self.groups
            .iter()
            .find(|(_, g)| g.name == name)
            .map(|(id, _)| *id)
    }

    /// Puts tab `id` into the group called `name`, making it (expanded, with
    /// [`default_color`]) when there is none, and returns that group. The caller has checked
    /// that `name` is a usable name.
    ///
    /// A tab already in it stays where it is. Joining an existing group moves the tab to just
    /// after its last member. A new group leaves the tab in place, unless the tab sits between
    /// members of its old group: then it moves to just after that run. With no such tab nothing
    /// happens and the result is [`GroupId::NONE`].
    pub fn set_group(&mut self, id: TabId, name: &str) -> GroupId {
        let Some(at) = self.index_of(id) else {
            return GroupId::NONE;
        };
        if let Some(g) = self.group_named(name) {
            if self.group_of(id) != Some(g) {
                let mut entry = self.items.remove(at);
                if let Some(old) = entry.group.replace(g) {
                    self.drop_if_empty(old);
                }
                let end = self.run(g).map_or(self.items.len(), |run| run.end);
                self.items.insert(end, entry);
            }
            return g;
        }
        self.ungroup(id);
        let color = default_color(self.groups.iter().map(|(_, g)| g.color));
        let g = GroupId(self.next_group);
        self.next_group += 1;
        let group = Group {
            name: name.to_owned(),
            color,
            collapsed: false,
        };
        self.groups.push((g, group));
        if let Some(entry) = self.items.iter_mut().find(|e| e.id == id) {
            entry.group = Some(g);
        }
        g
    }

    /// Takes tab `id` out of its group. A tab between other members moves to just after the run;
    /// one at either end stays where it is. A group left without members is gone.
    pub fn ungroup(&mut self, id: TabId) {
        let (Some(at), Some(g)) = (self.index_of(id), self.group_of(id)) else {
            return;
        };
        let Some(run) = self.run(g) else {
            return;
        };
        let mut entry = self.items.remove(at);
        entry.group = None;
        // The run is one shorter now; between two members means after what is left of it.
        let to = if run.start < at && at + 1 < run.end {
            run.end - 1
        } else {
            at
        };
        self.items.insert(to, entry);
        self.drop_if_empty(g);
    }

    /// Drops group `g` and keeps its tabs, in place.
    pub fn ungroup_all(&mut self, g: GroupId) {
        for entry in &mut self.items {
            if entry.group == Some(g) {
                entry.group = None;
            }
        }
        self.drop_if_empty(g);
    }

    /// Renames group `g`. `false`, and nothing changes, when there is no such group or another
    /// group has that name. The caller has checked that `name` is a usable name.
    pub fn rename_group(&mut self, g: GroupId, name: &str) -> bool {
        if self.group_named(name).is_some_and(|other| other != g) {
            return false;
        }
        match self.group_mut(g) {
            Some(group) => {
                name.clone_into(&mut group.name);
                true
            }
            None => false,
        }
    }

    /// Changes the colour of group `g`; `false` when there is no such group.
    pub fn recolor_group(&mut self, g: GroupId, color: GroupColor) -> bool {
        match self.group_mut(g) {
            Some(group) => {
                group.color = color;
                true
            }
            None => false,
        }
    }

    /// Collapses or expands group `g` in the side pane. The active tab does not change, even when
    /// it is a member.
    pub fn set_collapsed(&mut self, g: GroupId, collapsed: bool) {
        if let Some(group) = self.group_mut(g) {
            group.collapsed = collapsed;
        }
    }

    /// Expands the group of tab `id`, if it has one: for the actions that send the user to a tab.
    pub fn expand_for(&mut self, id: TabId) {
        if let Some(g) = self.group_of(id) {
            self.set_collapsed(g, false);
        }
    }

    /// The tabs that closing group `g` closes.
    pub fn close_group_ids(&self, g: GroupId) -> Vec<TabId> {
        self.members(g)
    }

    // -------------------------------------------------------------------------------- moves

    /// The side pane's drop of one tab: moves `id` to index `slot` of the list without it, into
    /// `group`. Refused (`false`, nothing changes) unless every run stays whole: into a group,
    /// the slot is inside or at either end of the run of its other members (a group's only member
    /// can only stay where it is); out of any group, the slot is not inside a run. A group left
    /// without members is gone. The active tab stays the same tab.
    pub fn move_tab(&mut self, id: TabId, slot: usize, group: Option<GroupId>) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        if group.is_some_and(|g| self.group(g).is_none()) {
            return false;
        }
        let mut entry = self.items.remove(from);
        let fits = slot <= self.items.len()
            && match group {
                Some(g) => match self.run(g) {
                    Some(run) => (run.start..=run.end).contains(&slot),
                    // The moved tab is the whole group.
                    None => slot == from,
                },
                None => self.run_split_at(slot).is_none(),
            };
        if !fits {
            self.items.insert(from, entry);
            return false;
        }
        let old = std::mem::replace(&mut entry.group, group);
        self.items.insert(slot, entry);
        if let Some(old) = old {
            self.drop_if_empty(old);
        }
        true
    }

    /// The side pane's drop of a whole group: moves the run of `g` to index `slot` of the list
    /// without it, members in the same order. Refused (`false`, nothing changes) when the slot is
    /// inside another run or past the end, or there is no such group.
    pub fn move_group(&mut self, g: GroupId, slot: usize) -> bool {
        let Some(run) = self.run(g) else {
            return false;
        };
        let from = run.start;
        let moved: Vec<Entry<T>> = self.items.drain(run).collect();
        let fits = slot <= self.items.len() && self.run_split_at(slot).is_none();
        let to = if fits { slot } else { from };
        self.items.splice(to..to, moved);
        fits
    }

    /// The top bar's drop: moves the tab at `from` to index `to` (remove, then insert: the
    /// toolkit's `Reorder`) without changing any tab's group, because the bar does not show
    /// groups. A grouped tab stays inside its run; a tab in no group dropped inside a run goes to
    /// the nearer end of it (a tie: after). Out of range indices do nothing. The active tab stays
    /// the same tab.
    pub fn reorder_bar(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() {
            return;
        }
        let entry = self.items.remove(from);
        let to = match entry.group {
            // With no other member there is nowhere to go.
            Some(g) => self.run(g).map_or(from, |run| to.clamp(run.start, run.end)),
            None => match self.run_split_at(to) {
                Some(run) if to - run.start < run.end - to => run.start,
                Some(run) => run.end,
                None => to,
            },
        };
        self.items.insert(to, entry);
    }

    // ------------------------------------------------------------------------------ helpers

    fn group_mut(&mut self, g: GroupId) -> Option<&mut Group> {
        self.groups
            .iter_mut()
            .find(|(i, _)| *i == g)
            .map(|(_, g)| g)
    }

    /// The indices from the first to the last member of `g`; `None` when no tab is in it.
    fn run(&self, g: GroupId) -> Option<Range<usize>> {
        let first = self.items.iter().position(|e| e.group == Some(g))?;
        let last = self.items.iter().rposition(|e| e.group == Some(g))?;
        Some(first..last + 1)
    }

    /// The run that inserting at index `at` would cut in two.
    fn run_split_at(&self, at: usize) -> Option<Range<usize>> {
        let before = self.items.get(at.checked_sub(1)?)?.group?;
        let after = self.items.get(at)?.group?;
        if before == after {
            self.run(after)
        } else {
            None
        }
    }

    fn drop_if_empty(&mut self, g: GroupId) {
        if self.run(g).is_none() {
            self.groups.retain(|(i, _)| *i != g);
        }
    }

    /// Panics unless what every public method promises holds.
    #[cfg(test)]
    pub(crate) fn assert_invariants(&self) {
        use std::collections::HashSet;

        let mut tabs = HashSet::new();
        for e in &self.items {
            assert!(e.id < self.next && tabs.insert(e.id), "tab id {}", e.id);
            if let Some(g) = e.group {
                assert!(self.group(g).is_some(), "{g:?} has a member and no entry");
            }
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for (id, group) in &self.groups {
            assert!(*id != GroupId::NONE && id.0 < self.next_group, "{id:?}");
            assert!(ids.insert(*id), "{id:?} twice");
            assert!(names.insert(group.name.as_str()), "a name twice: {id:?}");
            let run = self.run(*id);
            let run = run.unwrap_or_else(|| panic!("{id:?} has no member"));
            assert!(
                self.items[run].iter().all(|e| e.group == Some(*id)),
                "{id:?} is split"
            );
        }
        if let Some(active) = self.active {
            assert!(tabs.contains(&active), "the active tab {active} is gone");
        }
    }
}

// ------------------------------------------------------------------------------------ one tab

/// A program's notification (OSC 9 or OSC 777 `notify`) after [`Tab::apply`] sanitised it: both
/// fields are clean display text within [`NOTICE_TITLE_CHARS`] and [`NOTICE_BODY_CHARS`] chars,
/// and at least one is not empty. `title` is empty for OSC 9.
#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
}

impl std::fmt::Debug for Message {
    /// Lengths only, as every program text is kept out of a `Debug` (I16).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message")
            .field("title_chars", &self.title.chars().count())
            .field("body_chars", &self.body.chars().count())
            .finish()
    }
}

/// What one Update changed that the app acts on beyond the tab's own mirror.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub bells: u32,
    /// The Update's notifications, sanitised, in order, the empty ones dropped. They do not set
    /// `chrome`: they change nothing the strip draws by themselves.
    pub messages: Vec<Message>,
    pub clipboard_write: Option<String>,
    /// The answer to a copy: the text, or that it was too large.
    pub copy: Option<CopyResult>,
    pub exited: bool,
    /// The session palette changed: the terminal theme is new.
    pub palette: bool,
    /// The shell reported another directory (the workspace has something new to save).
    pub cwd: bool,
    /// Something the strip or the title band shows may have changed (title, command, exit).
    pub chrome: bool,
    /// The Update parsed PTY bytes (the tab is busy): the efficiency controller's input.
    pub output: bool,
}

/// Who asked for a tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Origin {
    /// The user: a key, a menu, or the saved workspace.
    #[default]
    User,
    /// A program, through `luneterm ctl`.
    Control,
}

/// One terminal tab: its session (or why there is none) and the mirror of what the session
/// last published.
pub struct Tab {
    /// `None` after a failed start.
    pub session: Option<Session>,
    /// Why no shell started: the grid tells, keys are ignored.
    pub failure: Option<SpawnFailure>,
    pub grid: Grid,
    pub cursor: Cursor,
    pub scroll: Option<ScrollView>,
    pub modes: UpdateModes,
    pub alt_screen: bool,
    /// The user's name for the tab; wins over the program's title.
    name: Option<String>,
    /// The program's title (OSC 0/2), sanitized; `None` while it set none.
    osc_title: Option<String>,
    /// The selection as the engine last published it (buffer coordinates).
    pub selection: Option<SelectionView>,
    /// The URIs of the OSC 8 link ids the engine published (untrusted text: every use checks it).
    pub link_uris: UriTable,
    /// The link shown as hovered, and the id the termview underlines for it (0: none).
    hover: Option<Hover>,
    pub hover_id: u32,
    /// An update hid a shown hover: the app looks again at what is under the pointer (and at
    /// nothing otherwise, so output costs no scan).
    rehover: bool,
    /// The terminal colours in force (the session palette on the base theme).
    pub theme: TermTheme,
    /// The geometry the session was last told.
    pub sent_geometry: Option<Geometry>,
    /// The last directory the shell reported (OSC 7), unchecked: untrusted text.
    pub cwd: Option<String>,
    /// The directory the tab was started in (saved with the workspace).
    pub launch_dir: Option<String>,
    /// When the tab was made: a child that exits within moments of it failed to start.
    pub started: Instant,
    pub foreground: ForegroundState,
    pub command: CommandState,
    /// An OSC 9;4 progress report is active (any state but "removed").
    pub progress_state: ProgressState,
    /// The focus the session was last told (focus reports, mode 1004).
    pub focused: bool,
    /// Who asked for the tab.
    pub origin: Origin,
    /// The tab runs a program given to `luneterm ctl`, not the user's shell.
    pub program: bool,
    /// When the user last typed, pasted or composed into the tab; `None` for never.
    pub last_input: Option<Instant>,
}

impl std::fmt::Debug for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab").finish_non_exhaustive()
    }
}

impl Tab {
    /// A tab with no session yet and an empty grid.
    pub fn empty(theme: TermTheme) -> Tab {
        Tab {
            session: None,
            failure: None,
            grid: Grid::new(0, 0),
            cursor: Cursor::default(),
            scroll: None,
            modes: UpdateModes::default(),
            alt_screen: false,
            name: None,
            osc_title: None,
            selection: None,
            link_uris: UriTable::new(),
            hover: None,
            hover_id: 0,
            rehover: false,
            theme,
            sent_geometry: None,
            cwd: None,
            launch_dir: None,
            started: Instant::now(),
            foreground: ForegroundState::default(),
            command: CommandState::default(),
            progress_state: ProgressState::None,
            focused: false,
            origin: Origin::User,
            program: false,
            last_input: None,
        }
    }

    /// The user typed, pasted or composed into this tab at `now`.
    pub fn note_input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    /// Tells the session its focus changed; nothing is sent when it did not.
    pub fn set_focus(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            if let Some(session) = &self.session {
                session.send(Command::Focus(focused));
            }
        }
    }

    /// The tab's own label: the manual name, else the program's title; `None` when neither.
    pub fn label(&self) -> Option<&str> {
        self.name.as_deref().or(self.osc_title.as_deref())
    }

    /// What the strip shows.
    pub fn strip_title(&self) -> &str {
        self.label().unwrap_or(DEFAULT_TAB_NAME)
    }

    /// Names the tab by hand (the user, the saved workspace, `ctl --title`). The name goes
    /// through [`title::sanitize_name`]; an empty result clears the manual name, so the
    /// program's title shows again.
    pub fn rename(&mut self, raw: &str) {
        self.name = title::sanitize_name(raw);
    }

    /// The manual name, for the rename field.
    pub fn manual_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether closing this tab should ask (D7): a shell that is not the foreground process ends
    /// a running program; when the platform cannot say, only if the shell-integration marks say
    /// a command runs. A tab with no session never asks.
    pub fn needs_confirm(&self) -> bool {
        asks_on_close(self.session.is_some(), self.foreground, self.command)
    }

    /// The tone an active OSC 9;4 report gives the tab (its title shines in it, and the outline
    /// takes it): an error is red, a warning amber, normal and indeterminate neutral. `None`
    /// when no report is active.
    pub fn progress_tone(&self) -> Option<Tone> {
        match self.progress_state {
            ProgressState::None => None,
            ProgressState::Normal | ProgressState::Indeterminate => Some(Tone::Neutral),
            ProgressState::Error => Some(Tone::Err),
            ProgressState::Warning => Some(Tone::Warn),
        }
    }

    /// Whether the running-command outline should show for this tab: the gate of
    /// `outline::should_show` over what the tab last published. `animate_full_screen` is the
    /// setting that lets it run in full-screen and interactive programs.
    pub fn busy(&self, animate_full_screen: bool) -> bool {
        crate::outline::should_show(
            self.progress_state != ProgressState::None,
            self.command.running,
            self.alt_screen,
            animate_full_screen,
            self.command.interactive,
        )
    }

    /// The link under cell (`row`, `col`) of the mirror, if any.
    pub fn link_at(&self, row: usize, col: usize) -> Option<LinkSpan> {
        links::link_at(&self.grid, row, col, &self.link_uris)
    }

    /// Stops showing a link as hovered.
    pub fn hide_link_hover(&mut self) {
        if let Some(hover) = self.hover.take() {
            hover.hide(&mut self.grid);
        }
        self.hover_id = 0;
    }

    /// The link shown as hovered.
    pub fn hover_span(&self) -> Option<&LinkSpan> {
        self.hover.as_ref().map(|h| &h.span)
    }

    /// Whether an update hid a shown hover since the last call (and so the pointer's cell is to
    /// be read again).
    pub fn take_rehover(&mut self) -> bool {
        std::mem::take(&mut self.rehover)
    }

    /// Shows the link at cell (`row`, `col`) as hovered, or none when there is none there. A span
    /// is only ever a URI that passed [`links::open::check_url`] (the scanner's rule), so what is
    /// underlined is what would open. Returns whether what is shown changed.
    pub fn set_link_hover(&mut self, at: Option<(usize, usize)>) -> bool {
        let span = at.and_then(|(row, col)| self.link_at(row, col));
        if span.as_ref() == self.hover.as_ref().map(|h| &h.span) {
            return false;
        }
        self.hide_link_hover();
        if let Some(span) = span {
            let (hover, id) = Hover::show(span, &mut self.grid);
            self.hover = Some(hover);
            self.hover_id = id;
        }
        true
    }

    /// Applies one Update to the mirror and tells what the app must act on.
    pub fn apply(&mut self, update: Update) -> Effects {
        let mut fx = Effects::default();
        // The hovered link's cells are put back first: the update may rewrite them. The app shows
        // the hover again after the update.
        self.rehover |= self.hover.is_some();
        self.hide_link_hover();
        apply_update(&mut self.grid, &update);
        self.cursor = cursor_of(&update.cursor, self.grid.cols());
        self.scroll = Some(ScrollView {
            viewport_top: update.viewport_top,
            base_y: update.base_y,
            total_lines: update.total_lines,
        });
        self.modes = update.modes;
        self.alt_screen = update.alt_screen;
        if let Some(palette) = &update.palette {
            self.theme = theme_with_palette(&self.theme, palette);
            self.grid.mark_all_dirty();
            fx.palette = true;
        }
        if let Some(raw) = &update.title {
            self.osc_title = named(raw);
            fx.chrome = true;
        }
        if let Some(raw) = update.cwd {
            fx.cwd = self.cwd.as_deref() != Some(raw.as_str());
            self.cwd = Some(raw);
        }
        if let Some(f) = update.foreground {
            self.foreground = f;
        }
        if let Some(progress) = &update.progress {
            let state = if crate::outline::progress_active(progress) {
                progress.state
            } else {
                ProgressState::None
            };
            fx.chrome |= state != self.progress_state;
            self.progress_state = state;
        }
        if let Some(c) = update.command {
            self.command = c;
            fx.chrome = true;
        }
        if !update.link_uris.is_empty() {
            if self.link_uris.len() + update.link_uris.len() > MAX_URI_TABLE {
                self.link_uris.clear();
            }
            self.link_uris.extend(update.link_uris);
        }
        if let Some(selection) = update.selection {
            self.selection = selection;
        }
        fx.copy = update.selection_text;
        fx.output = update.output;
        fx.bells = update.bells;
        // The only place program text from a notification becomes app text: every string leaves
        // here cleaned and capped (D9).
        fx.messages = update
            .notifications
            .into_iter()
            .filter_map(|n| {
                let title = title::sanitize_text(&n.title, NOTICE_TITLE_CHARS);
                let body = title::sanitize_text(&n.body, NOTICE_BODY_CHARS);
                (!title.is_empty() || !body.is_empty()).then_some(Message { title, body })
            })
            .collect();
        fx.clipboard_write = update.clipboard_write;
        if update.exited.is_some() {
            fx.exited = true;
            fx.chrome = true;
        }
        fx
    }
}

/// The rule behind [`Tab::needs_confirm`].
pub fn asks_on_close(
    has_session: bool,
    foreground: ForegroundState,
    command: CommandState,
) -> bool {
    has_session
        && match foreground.shell_is_foreground {
            Some(shell) => !shell,
            None => command.running,
        }
}

/// A program's title: sanitized, `None` when nothing printable is left (the sanitizer's
/// fallback, [`DEFAULT_TITLE`], counts as none).
fn named(raw: &str) -> Option<String> {
    let s = title::sanitize(raw);
    (s != DEFAULT_TITLE).then_some(s)
}

// ----------------------------------------------------------------------------------- closing

/// What a request to close a set of tabs does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosePlan {
    /// Close them now.
    Now(Vec<TabId>),
    /// Ask once; `n` of them would each have asked.
    Ask { ids: Vec<TabId>, n: usize },
}

/// Decides whether closing `ids` asks: not when `confirm_close` is off or none of them would.
pub fn plan_close(ids: Vec<TabId>, confirm_close: bool, asks: impl Fn(TabId) -> bool) -> ClosePlan {
    let n = ids.iter().filter(|id| asks(**id)).count();
    if confirm_close && n > 0 {
        ClosePlan::Ask { ids, n }
    } else {
        ClosePlan::Now(ids)
    }
}

/// The close dialog's message (Lunegit `en.json`); `n` counts the tabs that would each have
/// asked.
pub fn confirm_message(n: usize) -> String {
    if n <= 1 {
        "A terminal session is still running. Closing it will end the session.".to_owned()
    } else {
        format!("{n} terminal sessions are still running. Closing them will end those sessions.")
    }
}

// ------------------------------------------------------------------------------------- paste

/// A paste in flight (D8): Cmd+V or Ctrl+Shift+V asks the toolkit for the clipboard's text, which
/// arrives in a later frame's input. The answer goes to the tab that asked, as `Command::Paste`,
/// and to nowhere else: if that tab is gone by then, the text is dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PasteState {
    /// The tab that asked and was not answered yet.
    pending: Option<TabId>,
    /// The next frame must carry the read request.
    read: bool,
}

impl PasteState {
    /// A paste shortcut was pressed in `tab`. A second press while one is pending does nothing.
    pub fn request(&mut self, tab: TabId) {
        if self.pending.is_none() {
            self.pending = Some(tab);
            self.read = true;
        }
    }

    /// Whether the frame being built carries the read request (once per paste).
    pub fn take_read(&mut self) -> bool {
        std::mem::take(&mut self.read)
    }

    /// An event of a frame's input. Returns the tab and the text when it answers a pending paste;
    /// clipboard text nobody asked for is ignored.
    pub fn answer(&mut self, event: &Event) -> Option<(TabId, String)> {
        match event {
            Event::ClipboardText(text) if self.pending.is_some() => {
                let tab = self.pending.take()?;
                (!text.is_empty()).then(|| (tab, text.clone()))
            }
            Event::ClipboardUnavailable => {
                self.pending = None;
                None
            }
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------- the new tab's cwd

/// The directory check of a new tab, off the UI thread (`cwd::validated` stats the path, and a
/// hung mount can block a stat for a long time). Poll it each frame; after
/// [`CWD_CHECK_TIMEOUT`] it counts as unusable and the tab starts in the default directory.
/// A worker stuck in a stat stays stuck, which is why the app starts at most one at a time.
#[derive(Debug)]
pub struct CwdCheck {
    rx: Receiver<Option<PathBuf>>,
    started: Instant,
}

/// Where a [`CwdCheck`] stands.
#[derive(Debug, PartialEq, Eq)]
pub enum CwdState {
    Pending,
    /// The checked directory, or `None` for "use the default".
    Done(Option<PathBuf>),
}

impl CwdCheck {
    /// Starts checking `raw` (already past [`cwd::syntax`]). `wake` runs on the worker when it
    /// has an answer.
    pub fn start(
        raw: String,
        now: Instant,
        wake: impl FnOnce() + Send + 'static,
    ) -> io::Result<CwdCheck> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("luneterm-cwd".into())
            .spawn(move || {
                let _ = tx.send(cwd::validated(&raw));
                wake();
            })?;
        Ok(CwdCheck { rx, started: now })
    }

    /// A check that already has its answer (tests, and a path the syntax check refused).
    pub fn answered(answer: Option<PathBuf>, now: Instant) -> CwdCheck {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(answer);
        CwdCheck { rx, started: now }
    }

    pub fn poll(&self, now: Instant) -> CwdState {
        match self.rx.try_recv() {
            Ok(answer) => CwdState::Done(answer),
            Err(TryRecvError::Disconnected) => CwdState::Done(None),
            Err(TryRecvError::Empty) => {
                if now.saturating_duration_since(self.started) >= CWD_CHECK_TIMEOUT {
                    CwdState::Done(None)
                } else {
                    CwdState::Pending
                }
            }
        }
    }

    /// When a still-pending check gives up.
    pub fn deadline(&self) -> Instant {
        self.started + CWD_CHECK_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&'static str]) -> (TabSet<&'static str>, Vec<TabId>) {
        let mut s = TabSet::new();
        let ids = names.iter().map(|n| s.push(*n)).collect();
        (s, ids)
    }

    #[test]
    fn a_new_tab_is_appended_and_active_and_ids_are_not_reused() {
        let (mut s, ids) = set(&["a", "b"]);
        assert_eq!(s.active(), Some(ids[1]));
        let c = s.push("c");
        assert_eq!((s.active(), s.index_of(c)), (Some(c), Some(2)));
        s.close(c);
        assert!(s.push("d") > c);
    }

    #[test]
    fn closing_the_active_tab_picks_the_right_neighbour_else_the_left_else_none() {
        // Middle: the right neighbour takes its place.
        let (mut s, ids) = set(&["a", "b", "c"]);
        s.activate(ids[1]);
        assert_eq!(s.close(ids[1]), Some("b"));
        assert_eq!(s.active(), Some(ids[2]));
        // Last: the left neighbour.
        assert_eq!(s.close(ids[2]), Some("c"));
        assert_eq!(s.active(), Some(ids[0]));
        // Only: nothing is left.
        assert_eq!(s.close(ids[0]), Some("a"));
        assert_eq!(s.active(), None);
        assert!(s.is_empty());
        // First of several: the right neighbour.
        let (mut s, ids) = set(&["a", "b", "c"]);
        s.activate(ids[0]);
        s.close(ids[0]);
        assert_eq!(s.active(), Some(ids[1]));
    }

    #[test]
    fn closing_another_tab_keeps_the_active_one_and_an_unknown_id_does_nothing() {
        let (mut s, ids) = set(&["a", "b", "c"]);
        s.activate(ids[2]);
        s.close(ids[0]);
        assert_eq!(s.active(), Some(ids[2]));
        assert_eq!(s.close(999), None);
        assert_eq!(s.len(), 2);
        assert!(!s.activate(999));
        assert_eq!(s.active(), Some(ids[2]));
    }

    #[test]
    fn a_bar_reorder_is_remove_then_insert_and_the_active_tab_stays_the_same_tab() {
        let (mut s, ids) = set(&["a", "b", "c", "d"]);
        s.activate(ids[0]);
        s.reorder_bar(0, 2);
        assert_eq!(s.ids(), vec![ids[1], ids[2], ids[0], ids[3]]);
        assert_eq!(s.active(), Some(ids[0]));
        s.reorder_bar(3, 0);
        assert_eq!(s.ids(), vec![ids[3], ids[1], ids[2], ids[0]]);
        // Out of range is a no-op.
        s.reorder_bar(9, 0);
        s.reorder_bar(0, 9);
        assert_eq!(s.ids(), vec![ids[3], ids[1], ids[2], ids[0]]);
        s.reorder_bar(1, 1);
        assert_eq!(s.ids(), vec![ids[3], ids[1], ids[2], ids[0]]);
    }

    #[test]
    fn stepping_wraps_and_an_index_beyond_the_list_does_nothing() {
        let (mut s, ids) = set(&["a", "b", "c"]);
        s.activate(ids[0]);
        s.activate_step(-1);
        assert_eq!(s.active(), Some(ids[2]));
        s.activate_step(1);
        assert_eq!(s.active(), Some(ids[0]));
        s.activate_step(1);
        assert_eq!(s.active(), Some(ids[1]));
        assert!(s.activate_index(2));
        assert_eq!(s.active(), Some(ids[2]));
        assert!(!s.activate_index(3));
        assert_eq!(s.active(), Some(ids[2]));
        let mut empty: TabSet<()> = TabSet::new();
        empty.activate_step(1);
        assert_eq!(empty.active(), None);
    }

    #[test]
    fn a_tab_is_named_by_hand_then_by_its_program_then_terminal() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        assert_eq!(t.label(), None);
        assert_eq!(t.strip_title(), "Terminal");
        t.osc_title = named("vim main.rs");
        assert_eq!(t.strip_title(), "vim main.rs");
        t.rename("  build  ");
        assert_eq!(t.strip_title(), "build");
        assert_eq!(t.manual_name(), Some("build"));
        // The manual name wins over a later program title.
        t.osc_title = named("zsh");
        assert_eq!(t.strip_title(), "build");
        // An empty name clears it; the program's title shows again.
        t.rename("   ");
        assert_eq!(t.strip_title(), "zsh");
        // A title that sanitizes to nothing counts as none, and so does a control-only one.
        assert_eq!(named("\u{1b}\u{7}"), None);
        assert_eq!(named("\u{202e}"), None);
        // Untrusted text is bounded and cleaned.
        let long = named(&"x".repeat(500)).expect("name");
        assert_eq!(long.chars().count(), 80);
        assert_eq!(named("a\u{202e}b\nc").as_deref(), Some("abc"));
    }

    #[test]
    fn a_name_by_hand_keeps_128_chars_and_a_program_title_80() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        for (typed, kept) in [(80, 80), (81, 81), (128, 128), (129, 128), (500, 128)] {
            t.rename(&"é".repeat(typed));
            assert_eq!(
                t.manual_name().map(|n| n.chars().count()),
                Some(kept),
                "{typed}"
            );
        }
        // The app's own name is a name like any other when the user (or `ctl`) gives it.
        t.rename("Luneterm");
        assert_eq!(t.manual_name(), Some("Luneterm"));
        t.rename("\u{202e}");
        assert_eq!(t.manual_name(), None);

        let mut update = empty_update();
        update.title = Some("x".repeat(500).into());
        t.apply(update);
        assert_eq!(t.strip_title().chars().count(), 80);
        // A program that sets the app's name sets no title.
        let mut update = empty_update();
        update.title = Some("Luneterm".into());
        t.apply(update);
        assert_eq!(t.label(), None);
    }

    #[test]
    fn a_new_tab_is_the_users_and_has_had_no_input() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        assert_eq!(
            (t.origin, t.program, t.last_input),
            (Origin::User, false, None)
        );
        let first = Instant::now();
        t.note_input(first);
        assert_eq!(t.last_input, Some(first));
        let later = first + Duration::from_millis(5);
        t.note_input(later);
        assert_eq!(t.last_input, Some(later));
    }

    /// The tab's gate is `outline::should_show` over its own state; the full-screen setting is the
    /// opt-in that bypasses the alternate-screen and interactive suppression.
    #[test]
    fn a_tab_is_busy_per_the_gate_and_the_full_screen_setting() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        assert!(!t.busy(false) && !t.busy(true), "nothing runs");
        t.command = CommandState {
            running: true,
            interactive: false,
        };
        assert!(t.busy(false));
        // A full-screen program: static unless the setting is on.
        t.alt_screen = true;
        assert!(!t.busy(false));
        assert!(t.busy(true));
        t.alt_screen = false;
        t.command.interactive = true;
        assert!(!t.busy(false));
        assert!(t.busy(true));
        // Progress is an independent trigger.
        t.command.running = false;
        assert!(!t.busy(true));
        t.progress_state = ProgressState::Normal;
        assert!(t.busy(false));
    }

    /// The tone an OSC 9;4 report gives the tab's title and the outline.
    #[test]
    fn a_progress_report_tints_the_tab_by_its_state() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        assert_eq!(t.progress_tone(), None);
        for (state, want) in [
            (ProgressState::Normal, Some(Tone::Neutral)),
            (ProgressState::Indeterminate, Some(Tone::Neutral)),
            (ProgressState::Error, Some(Tone::Err)),
            (ProgressState::Warning, Some(Tone::Warn)),
            (ProgressState::None, None),
        ] {
            t.progress_state = state;
            assert_eq!(t.progress_tone(), want, "{state:?}");
            assert_eq!(t.busy(false), want.is_some(), "{state:?}");
        }
    }

    #[test]
    fn closing_asks_per_the_foreground_or_the_marks_when_it_is_unknown() {
        let fg = |shell| ForegroundState {
            shell_is_foreground: shell,
            ..ForegroundState::default()
        };
        let cmd = |running| CommandState {
            running,
            interactive: false,
        };
        // Without a session nothing asks.
        assert!(!asks_on_close(false, fg(Some(false)), cmd(true)));
        // A child in the foreground asks; the shell in the foreground has nothing to lose.
        assert!(asks_on_close(true, fg(Some(false)), cmd(false)));
        assert!(!asks_on_close(true, fg(Some(true)), cmd(true)));
        // D7: unknown foreground: the marks decide, and no marks means no question.
        assert!(asks_on_close(true, fg(None), cmd(true)));
        assert!(!asks_on_close(true, fg(None), cmd(false)));
        // The tab applies the same rule.
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        t.foreground = fg(Some(false));
        assert!(!t.needs_confirm(), "no session");
    }

    #[test]
    fn close_plans_honour_the_setting_and_count_the_tabs_that_would_ask() {
        let asks = |id: TabId| id == 2 || id == 3;
        assert_eq!(
            plan_close(vec![1, 2, 3], true, asks),
            ClosePlan::Ask {
                ids: vec![1, 2, 3],
                n: 2
            }
        );
        assert_eq!(
            plan_close(vec![1, 2, 3], false, asks),
            ClosePlan::Now(vec![1, 2, 3])
        );
        assert_eq!(plan_close(vec![1], true, asks), ClosePlan::Now(vec![1]));
        assert_eq!(plan_close(Vec::new(), true, asks), ClosePlan::Now(vec![]));
    }

    #[test]
    fn the_confirm_text_names_the_count_and_one_uses_the_single_text() {
        assert_eq!(
            confirm_message(1),
            "A terminal session is still running. Closing it will end the session."
        );
        assert_eq!(
            confirm_message(3),
            "3 terminal sessions are still running. Closing them will end those sessions."
        );
        assert_eq!(confirm_message(0), confirm_message(1));
    }

    #[test]
    fn a_paste_is_one_read_one_answer_and_unasked_text_is_ignored() {
        let text = |t: &str| Event::ClipboardText(t.to_owned());
        let mut p = PasteState::default();
        // Text nobody asked for goes nowhere.
        assert_eq!(p.answer(&text("surprise")), None);
        assert!(!p.take_read());
        // A request is one read, however many presses.
        p.request(1);
        p.request(1);
        assert!(p.take_read());
        assert!(!p.take_read());
        p.request(1);
        assert!(!p.take_read(), "still pending: no second read");
        // Other events do not answer it; the text does, once.
        assert_eq!(p.answer(&Event::Focus(true)), None);
        assert_eq!(p.answer(&text("hello")), Some((1, "hello".to_owned())));
        assert_eq!(p.answer(&text("again")), None);
        // Unavailable clears a pending paste; empty text pastes nothing.
        p.request(1);
        assert!(p.take_read());
        assert_eq!(p.answer(&Event::ClipboardUnavailable), None);
        p.request(1);
        assert!(p.take_read(), "a new paste can be asked for");
        assert_eq!(p.answer(&text("")), None);
        p.request(1);
        assert!(p.take_read());
    }

    /// The answer belongs to the tab that asked, whichever is active when it arrives; if that tab
    /// is gone, nobody gets it.
    #[test]
    fn a_paste_answer_goes_to_the_tab_that_asked_and_is_dropped_if_it_is_gone() {
        let (mut tabs, ids) = set(&["a", "b"]);
        let mut p = PasteState::default();
        p.request(ids[0]);
        assert!(p.take_read());
        // The user switched tabs before the clipboard answered.
        tabs.activate(ids[1]);
        let (target, text) = p
            .answer(&Event::ClipboardText("secret".into()))
            .expect("answered");
        assert_eq!((target, text.as_str()), (ids[0], "secret"));
        assert_eq!(
            tabs.get(target),
            Some(&"a"),
            "delivered to the asker, not the active tab"
        );

        // The asker closed first: the lookup finds nothing and the text goes nowhere.
        p.request(ids[1]);
        assert!(p.take_read());
        tabs.close(ids[1]);
        let (target, _) = p
            .answer(&Event::ClipboardText("lost".into()))
            .expect("answered");
        assert_eq!(tabs.get(target), None);
    }

    #[test]
    fn a_cwd_check_gives_its_answer_or_gives_up_after_the_timeout() {
        let t0 = Instant::now();
        let dir = std::env::temp_dir();
        let done = CwdCheck::answered(Some(dir.clone()), t0);
        assert_eq!(done.poll(t0), CwdState::Done(Some(dir)));
        // No answer yet: pending, then given up at the deadline.
        let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
        let hung = CwdCheck { rx, started: t0 };
        assert_eq!(
            hung.poll(t0 + Duration::from_millis(299)),
            CwdState::Pending
        );
        assert_eq!(hung.deadline(), t0 + CWD_CHECK_TIMEOUT);
        assert_eq!(hung.poll(hung.deadline()), CwdState::Done(None));
        drop(tx);
        // A worker that died answers "default".
        assert_eq!(hung.poll(t0), CwdState::Done(None));
    }

    #[test]
    fn a_real_worker_validates_off_thread_and_wakes() {
        let (tx, rx) = mpsc::channel();
        let dir = std::env::temp_dir();
        let raw = dir.to_str().expect("utf-8 temp dir").to_owned();
        let check = CwdCheck::start(raw, Instant::now(), move || {
            let _ = tx.send(());
        })
        .expect("start");
        rx.recv_timeout(Duration::from_secs(5)).expect("woken");
        assert_eq!(check.poll(Instant::now()), CwdState::Done(Some(dir)));
        let bad = CwdCheck::start("/definitely/not/a/dir".to_owned(), Instant::now(), || {})
            .expect("start");
        let t0 = Instant::now();
        let state = loop {
            match bad.poll(Instant::now()) {
                CwdState::Pending if t0.elapsed() < Duration::from_secs(5) => {
                    thread::sleep(Duration::from_millis(5));
                }
                other => break other,
            }
        };
        assert_eq!(state, CwdState::Done(None));
    }

    /// A tab whose mirror shows `text` on row 0 (40 columns, 2 rows).
    fn tab_showing(text: &str) -> Tab {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        t.grid = Grid::new(40, 2);
        for (x, ch) in text.chars().enumerate() {
            let mut cell = lune_termview::Cell::default();
            cell.set_char(ch);
            t.grid.set(0, x, cell);
        }
        t
    }

    /// An Update that changes nothing but carries the grid's size.
    fn empty_update() -> Update {
        Update {
            seq: 1,
            cols: 40,
            rows: 2,
            full: false,
            rows_changed: Vec::new(),
            cursor: lune_vt::ViewportCursor::default(),
            viewport_top: 0,
            base_y: 0,
            total_lines: 2,
            is_at_bottom: true,
            alt_screen: false,
            modes: UpdateModes::default(),
            title: None,
            bells: 0,
            clipboard_write: None,
            palette: None,
            cwd: None,
            progress: None,
            marks: Vec::new(),
            notifications: Vec::new(),
            sync_update_in_command: false,
            foreground: None,
            command: None,
            link_uris: Vec::new(),
            selection: None,
            selection_text: None,
            output: false,
            exited: None,
        }
    }

    #[test]
    fn only_a_link_that_would_open_is_hovered() {
        // A good bare URL is underlined; userinfo and a file: OSC 8 URI are not links the user
        // can open, so they get no underline and no tooltip.
        let mut t = tab_showing("see https://example.com/a ok");
        assert!(t.set_link_hover(Some((0, 6))));
        assert_eq!(
            t.hover_span().map(|s| s.uri.as_str()),
            Some("https://example.com/a")
        );
        assert_ne!(t.hover_id, 0);
        assert!(
            !t.set_link_hover(Some((0, 7))),
            "the same link: nothing changed"
        );
        assert!(t.set_link_hover(Some((0, 0))), "off the link: hidden");
        assert!(t.hover_span().is_none() && t.hover_id == 0);

        let mut bad = tab_showing("http://trusted.example@evil.example/x");
        assert!(!bad.set_link_hover(Some((0, 3))), "userinfo: not hovered");
        assert!(bad.hover_span().is_none() && bad.hover_id == 0);
        let mut file = tab_showing("evil");
        for cell in file.grid.row_mut(0).expect("row").iter_mut().take(4) {
            cell.link = 5;
        }
        file.link_uris.insert(5, "file:///etc/passwd".to_owned());
        assert!(
            file.link_at(0, 1).is_none(),
            "the scanner makes no span of a refused URI"
        );
        assert!(!file.set_link_hover(Some((0, 1))));
        assert!(file.hover_span().is_none());
    }

    #[test]
    fn an_update_over_a_shown_hover_asks_for_one_look_and_a_quiet_one_asks_for_none() {
        let mut t = tab_showing("https://example.com/a");
        assert!(!t.take_rehover());
        // No hover shown: output costs no rehover.
        t.apply(empty_update());
        assert!(!t.take_rehover());
        assert!(t.set_link_hover(Some((0, 2))));
        t.apply(empty_update());
        assert!(t.hover_span().is_none(), "the update put the cells back");
        // A second update before the app looked must not lose the request.
        t.apply(empty_update());
        assert!(t.take_rehover());
        assert!(!t.take_rehover(), "asked once");
    }

    fn note(title: &str, body: &str) -> lune_vt::Notification {
        lune_vt::Notification {
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn notifications_leave_apply_cleaned_capped_in_order_and_without_chrome() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        let mut update = empty_update();
        update.notifications = vec![
            // a bidi override, a newline and a control character
            note("\u{202E}Build\nfinished\u{1b}", &"x".repeat(500)),
            note("", "  plain \t message  "),
            // nothing printable: dropped
            note("\u{202E}\n ", " \u{2028}\u{1} "),
            note("only title", ""),
        ];
        let fx = t.apply(update);
        assert!(!fx.chrome, "messages are not chrome");
        assert_eq!(fx.messages.len(), 3);
        assert_eq!(fx.messages[0].title, "Build finished");
        assert_eq!(fx.messages[0].body, "x".repeat(240));
        assert_eq!(
            fx.messages[1],
            Message {
                title: String::new(),
                body: "plain message".to_owned()
            }
        );
        assert_eq!(fx.messages[2].title, "only title");
        // a long title is capped in chars, not bytes
        let mut update = empty_update();
        update.notifications = vec![note(&"é".repeat(200), "b")];
        let fx = t.apply(update);
        assert_eq!(fx.messages[0].title.chars().count(), 80);
    }

    #[test]
    fn a_whitespace_only_notification_is_dropped_and_a_quiet_update_has_none() {
        let mut t = Tab::empty(TermTheme::lunegit_one_dark());
        let mut update = empty_update();
        update.notifications = vec![note(" \t\n", "\u{2028}  ")];
        assert!(t.apply(update).messages.is_empty());
        assert!(t.apply(empty_update()).messages.is_empty());
    }

    #[test]
    fn the_effects_debug_shows_message_sizes_never_text() {
        let fx = Effects {
            messages: vec![Message {
                title: "hunter2".to_owned(),
                body: "hunter2".to_owned(),
            }],
            ..Effects::default()
        };
        let shown = format!("{fx:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
    }
}
