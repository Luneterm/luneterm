//! Tabs (plan M10 step 2, D1): one [`Tab`] per [`Session`], the pure list logic ([`TabSet`]), the
//! rules that decide what closing asks, the tab's name, the paste routing and the check of a
//! directory for a new tab.
//!
//! The UI thread owns all of it. What needs the buffer or the PTY stays on the session's engine
//! thread and arrives in an [`Update`] (M9 I1); nothing here touches a `Terminal`.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use lune_termview::{Cursor, Grid, TermTheme};
use lune_ui::input::Event;

use crate::app::{cursor_of, theme_with_palette};
use crate::cwd;
use crate::outline::CommandState;
use crate::session::{
    Command, CopyResult, ForegroundState, Geometry, SelectionView, Session, Update, UpdateModes,
    apply_update,
};
use crate::shell::ScrollView;
use crate::spawn::SpawnFailure;
use crate::title::{self, DEFAULT_TITLE};

/// The app's identity of a tab: stable across reorders and closes, never reused.
pub type TabId = u64;

/// What a tab with no manual name and no title from its program is called (Lunegit's
/// `terminal.tab.defaultTitle`).
pub const DEFAULT_TAB_NAME: &str = "Terminal";

/// How long a new tab waits for the directory check before it starts in the default directory.
pub const CWD_CHECK_TIMEOUT: Duration = Duration::from_millis(300);

// ----------------------------------------------------------------------------------- the list

/// The tabs in order, which one is active, and the next id. Pure list logic: no window, no
/// session.
#[derive(Debug)]
pub struct TabSet<T> {
    items: Vec<(TabId, T)>,
    active: Option<TabId>,
    next: TabId,
}

impl<T> Default for TabSet<T> {
    fn default() -> Self {
        TabSet {
            items: Vec::new(),
            active: None,
            next: 1,
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

    /// Appends `item` at the end and makes it active.
    pub fn push(&mut self, item: T) -> TabId {
        let id = self.next;
        self.next += 1;
        self.items.push((id, item));
        self.active = Some(id);
        id
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn ids(&self) -> Vec<TabId> {
        self.items.iter().map(|(id, _)| *id).collect()
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.items.iter().position(|(i, _)| *i == id)
    }

    pub fn get(&self, id: TabId) -> Option<&T> {
        self.items.iter().find(|(i, _)| *i == id).map(|(_, t)| t)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut T> {
        self.items
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, t)| t)
    }

    pub fn active_item(&self) -> Option<&T> {
        self.active.and_then(|id| self.get(id))
    }

    pub fn active_item_mut(&mut self) -> Option<&mut T> {
        let id = self.active?;
        self.get_mut(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TabId, &T)> {
        self.items.iter().map(|(id, t)| (*id, t))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (TabId, &mut T)> {
        self.items.iter_mut().map(|(id, t)| (*id, t))
    }

    /// Makes `id` active. `false` when there is no such tab.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.index_of(id).is_some() {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    /// Activates the tab `delta` places after the active one, wrapping around.
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
            .map(|(id, _)| *id);
    }

    /// Activates the tab at `index` (0-based); `false` when there is none.
    pub fn activate_index(&mut self, index: usize) -> bool {
        match self.items.get(index) {
            Some((id, _)) => {
                self.active = Some(*id);
                true
            }
            None => false,
        }
    }

    /// Removes `id`. Closing the active tab activates its right neighbour, else its left one;
    /// closing another tab leaves the active one.
    pub fn close(&mut self, id: TabId) -> Option<T> {
        let at = self.index_of(id)?;
        let (_, item) = self.items.remove(at);
        if self.active == Some(id) {
            self.active = self
                .items
                .get(at)
                .or_else(|| self.items.last())
                .map(|(i, _)| *i);
        }
        Some(item)
    }

    /// Moves the tab at `from` to index `to` (remove, then insert: the toolkit's `Reorder`). Out
    /// of range indices do nothing. The active tab stays the same tab.
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() {
            return;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
    }
}

// ------------------------------------------------------------------------------------ one tab

/// What one Update changed that the app acts on beyond the tab's own mirror.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub bells: u32,
    pub clipboard_write: Option<String>,
    /// The answer to a copy: the text, or that it was too large.
    pub copy: Option<CopyResult>,
    pub exited: bool,
    /// The session palette changed: the terminal theme is new.
    pub palette: bool,
    /// Something the strip or the title band shows may have changed (title, command, exit).
    pub chrome: bool,
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
    /// The terminal colours in force (the session palette on the base theme).
    pub theme: TermTheme,
    /// The geometry the session was last told.
    pub sent_geometry: Option<Geometry>,
    /// The last directory the shell reported (OSC 7), unchecked: untrusted text.
    pub cwd: Option<String>,
    pub foreground: ForegroundState,
    pub command: CommandState,
    /// The focus the session was last told (focus reports, mode 1004).
    pub focused: bool,
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
            theme,
            sent_geometry: None,
            cwd: None,
            foreground: ForegroundState::default(),
            command: CommandState::default(),
            focused: false,
        }
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

    /// The user renames the tab. The name is sanitized like a title; an empty result clears the
    /// manual name, so the program's title shows again.
    pub fn rename(&mut self, raw: &str) {
        self.name = named(raw);
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

    /// Applies one Update to the mirror and tells what the app must act on.
    pub fn apply(&mut self, update: Update) -> Effects {
        let mut fx = Effects::default();
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
            self.cwd = Some(raw);
        }
        if let Some(f) = update.foreground {
            self.foreground = f;
        }
        if let Some(c) = update.command {
            self.command = c;
            fx.chrome = true;
        }
        if let Some(selection) = update.selection {
            self.selection = selection;
        }
        fx.copy = update.selection_text;
        fx.bells = update.bells;
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

/// A name from untrusted or typed text: sanitized, `None` when nothing printable is left (the
/// sanitizer's fallback, [`DEFAULT_TITLE`], counts as none).
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
    fn reorder_is_remove_then_insert_and_the_active_tab_stays_the_same_tab() {
        let (mut s, ids) = set(&["a", "b", "c", "d"]);
        s.activate(ids[0]);
        s.reorder(0, 2);
        assert_eq!(s.ids(), vec![ids[1], ids[2], ids[0], ids[3]]);
        assert_eq!(s.active(), Some(ids[0]));
        s.reorder(3, 0);
        assert_eq!(s.ids(), vec![ids[3], ids[1], ids[2], ids[0]]);
        // Out of range is a no-op.
        s.reorder(9, 0);
        s.reorder(0, 9);
        assert_eq!(s.ids(), vec![ids[3], ids[1], ids[2], ids[0]]);
        s.reorder(1, 1);
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
}
