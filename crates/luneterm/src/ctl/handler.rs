//! What the app does with a control request: [`handle`], a pure function over the tab list, the
//! setting and the clock that gives the reply and the [`Effect`]s the app performs (open, close,
//! raise the window, reveal a tab, hold the keyboard). And the [`Bridge`] that carries a request
//! from the server's handler thread to the UI thread and its reply back, under a deadline.
//!
//! N1 and N2 hold by construction: this module sees a tab only through [`ControlTab`], which has
//! no session, no grid, no scrollback, no selection and no clipboard, so no request can write to a
//! tab's input or read what it shows.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use super::reply::{self, TabEntry};
use super::server::{Handler, Reply};
use super::{Code, Request};
use crate::cwd;
use crate::tabs::{CWD_CHECK_TIMEOUT, Origin, TabId, TabSet};

/// The most tabs of origin `control` open at once (D18): the next `tab new` is refused (1).
pub const MAX_CONTROL_TABS: usize = 64;

/// How long keyboard, paste and IME input is dropped after a control request moved the active
/// tab (D9).
pub const INPUT_HOLD: Duration = Duration::from_millis(400);

/// A request the UI thread has not started on by this long after the handler got it is dropped:
/// what is left of the server's 1.5 s is for the reply's way back.
const ACT_BY: Duration = Duration::from_millis(1200);
/// The longest the handler waits for a request the UI thread did start on.
const ANSWER_BY: Duration = Duration::from_millis(1450);

const NO_ANSWER: &str = "Luneterm did not answer";
const CLOSING: &str = "Luneterm is closing";
const STARTING: &str = "Luneterm is starting";
const NOT_A_DIR: &str = "--cwd is not an existing directory";
const NO_SUCH_TAB: &str = "no such tab";
const USER_TAB: &str = "the tab was not opened through ctl; --any-origin closes it";
const TOO_MANY: &str = "64 tabs opened through ctl are open already";
const CONTROL_OFF: &str = "tab control is turned off in settings";
const UNWRITABLE: &str = "the answer could not be written";

/// What a request may know of a tab. Nothing here reaches the tab's session (N1) or what the tab
/// shows (N2).
pub trait ControlTab {
    fn origin(&self) -> Origin;
    /// What the strip shows.
    fn title(&self) -> &str;
    /// The tab's directory as its shell last reported it (untrusted text), else where it started.
    fn cwd(&self) -> Option<&str>;
    fn pid(&self) -> Option<u32>;
    /// When the user last typed, pasted or composed into the tab.
    fn last_input(&self) -> Option<Instant>;
}

/// What the app knows that a request reads.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// The setting "Tab control from programs", read for this request.
    pub control_on: bool,
    /// The window exists (a tab can be opened).
    pub window: bool,
    pub version: &'a str,
    pub instance: &'a str,
    pub now: Instant,
}

/// The tab a `tab new` opens: its words, after the checks.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenSpec {
    /// An existing directory (checked off the UI thread).
    pub cwd: PathBuf,
    pub group: Option<String>,
    pub title: Option<String>,
    pub focus: bool,
    /// The program and its arguments; empty for the user's shell.
    pub argv: Vec<String>,
}

impl fmt::Debug for OpenSpec {
    /// No caller text: sizes and flags only.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSpec")
            .field("group", &self.group.is_some())
            .field("title", &self.title.is_some())
            .field("focus", &self.focus)
            .field("argv_words", &self.argv.len())
            .finish_non_exhaustive()
    }
}

/// What the app does after a request was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Close these tabs now, as a child exit does: no confirmation (the last one quits the app).
    Close(Vec<TabId>),
    /// Restore the window from the Dock or taskbar and ask the OS to bring it to the front.
    Raise,
    /// Show this tab: the side pane scrolls it into view (its group is expanded already).
    Reveal(TabId),
    /// Drop keyboard, paste and IME input for [`INPUT_HOLD`]: the active tab changed under the
    /// user's hands.
    Hold,
    /// The tab's group changed: the workspace has news.
    Regrouped(TabId),
}

/// The outcome of [`handle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The reply, and what to do.
    Answer(Reply, Vec<Effect>),
    /// Start this tab, then [`place`] it: that gives the reply.
    Spawn(OpenSpec),
}

fn answer(reply: Result<String, impl Sized>, effects: Vec<Effect>) -> Outcome {
    match reply {
        Ok(line) => Outcome::Answer(Reply::new(Code::Done, line), effects),
        Err(_) => refuse(Code::Failed, UNWRITABLE),
    }
}

fn refuse(code: Code, reason: &str) -> Outcome {
    Outcome::Answer(Reply::new(code, reason), Vec::new())
}

/// Answers `request`. `cwd_ok` is whether a `tab new`'s directory exists (checked off the UI
/// thread before; ignored for the other commands). With control off every request but `status`
/// is refused (3) before anything is looked at, and nothing changes.
pub fn handle<T: ControlTab>(
    tabs: &mut TabSet<T>,
    request: Request,
    cwd_ok: bool,
    ctx: &Ctx<'_>,
) -> Outcome {
    if !ctx.control_on && request != Request::Status {
        return refuse(Code::ControlOff, CONTROL_OFF);
    }
    match request {
        Request::Status => answer(
            reply::status(ctx.version, ctx.instance, ctx.control_on),
            Vec::new(),
        ),
        Request::TabList => answer(list(tabs, ctx), Vec::new()),
        Request::TabNew {
            cwd,
            group,
            title,
            focus,
            argv,
        } => {
            if !ctx.window {
                return refuse(Code::Failed, STARTING);
            }
            if !cwd_ok {
                return refuse(Code::Usage, NOT_A_DIR);
            }
            let control = tabs
                .iter()
                .filter(|(_, t)| t.origin() == Origin::Control)
                .count();
            if control >= MAX_CONTROL_TABS {
                return refuse(Code::Failed, TOO_MANY);
            }
            Outcome::Spawn(OpenSpec {
                cwd,
                group,
                title,
                focus,
                argv,
            })
        }
        Request::TabClose { id, any_origin } => match tabs.get(id).map(ControlTab::origin) {
            None => refuse(Code::NoSuchTab, NO_SUCH_TAB),
            Some(Origin::User) if !any_origin => refuse(Code::Refused, USER_TAB),
            Some(_) => answer(reply::tab_closed(id), vec![Effect::Close(vec![id])]),
        },
        Request::TabFocus(id) => {
            if tabs.get(id).is_none() {
                return refuse(Code::NoSuchTab, NO_SUCH_TAB);
            }
            let before = tabs.active();
            tabs.expand_for(id);
            tabs.activate(id);
            let mut effects = vec![Effect::Reveal(id), Effect::Raise];
            if before != Some(id) {
                effects.push(Effect::Hold);
            }
            answer(reply::tab_focused(id), effects)
        }
        Request::TabGroup(id, name) => {
            if tabs.get(id).is_none() {
                return refuse(Code::NoSuchTab, NO_SUCH_TAB);
            }
            let changed = tabs
                .group_of(id)
                .and_then(|g| tabs.group(g))
                .map(|g| &g.name)
                != Some(&name);
            tabs.set_group(id, &name);
            let effects = if changed {
                vec![Effect::Regrouped(id)]
            } else {
                Vec::new()
            };
            answer(reply::tab_group(id, Some(&name)), effects)
        }
        Request::TabUngroup(id) => {
            if tabs.get(id).is_none() {
                return refuse(Code::NoSuchTab, NO_SUCH_TAB);
            }
            let changed = tabs.group_of(id).is_some();
            tabs.ungroup(id);
            let effects = if changed {
                vec![Effect::Regrouped(id)]
            } else {
                Vec::new()
            };
            answer(reply::tab_group(id, None), effects)
        }
    }
}

/// Adds the tab a `tab new` started (`item`, origin control) where Protocol 1 puts it: at the end,
/// or after the last member of `spec.group` (made when there is none), and active only with
/// `--focus`. Returns the reply and what the app does next. The tab's id is
/// [`TabSet::next_id`] as it was before the call: the one its environment was given.
pub fn place<T: ControlTab>(
    tabs: &mut TabSet<T>,
    item: T,
    spec: &OpenSpec,
) -> (Reply, Vec<Effect>) {
    let before = tabs.active();
    let id = tabs.insert(item, tabs.len(), spec.focus);
    if let Some(name) = &spec.group {
        tabs.set_group(id, name);
    }
    let mut effects = Vec::new();
    if spec.focus {
        tabs.expand_for(id);
        effects.extend([Effect::Reveal(id), Effect::Raise]);
        if before != Some(id) {
            effects.push(Effect::Hold);
        }
    }
    let reply = match reply::tab_new(id) {
        Ok(line) => Reply::new(Code::Done, line),
        Err(_) => Reply::new(Code::Failed, UNWRITABLE),
    };
    (reply, effects)
}

/// `tab list`: every tab in order (the bar's and the side pane's, collapsed groups' tabs
/// included).
fn list<T: ControlTab>(
    tabs: &TabSet<T>,
    ctx: &Ctx<'_>,
) -> Result<String, crate::persist::json_write::WriteError> {
    let active = tabs.active();
    let cleaned: Vec<(TabId, String, Option<String>)> = tabs
        .iter()
        .map(|(id, t)| (id, clean(t.title()), t.cwd().and_then(listed_cwd)))
        .collect();
    let entries: Vec<TabEntry<'_>> = cleaned
        .iter()
        .map(|(id, title, cwd)| {
            let tab = tabs.get(*id);
            TabEntry {
                id: *id,
                by_control: tab.is_some_and(|t| t.origin() == Origin::Control),
                group: tabs
                    .group_of(*id)
                    .and_then(|g| tabs.group(g))
                    .map(|g| g.name.as_str()),
                title,
                cwd: cwd.as_deref(),
                active: active == Some(*id),
                pid: tab.and_then(ControlTab::pid),
                input_idle_ms: tab
                    .and_then(ControlTab::last_input)
                    .map(|at| idle_ms(at, ctx.now)),
            }
        })
        .collect();
    reply::tab_list(ctx.instance, &entries)
}

/// Program text without control characters.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// A tab's directory as `tab list` gives it: without control characters, absolute, at most
/// [`cwd::MAX_CWD_BYTES`] bytes (the reply's own string limit); else none.
fn listed_cwd(raw: &str) -> Option<String> {
    let s = clean(raw);
    (s.len() <= cwd::MAX_CWD_BYTES && Path::new(&s).is_absolute()).then_some(s)
}

/// Milliseconds from `at` to `now`, within the reply's exact number range.
fn idle_ms(at: Instant, now: Instant) -> u64 {
    let ms = now.saturating_duration_since(at).as_millis();
    u64::try_from(ms).unwrap_or(u64::MAX).min(1 << 53)
}

// ----------------------------------------------------------------------------------- the bridge

const PENDING: u8 = 0;
const TAKEN: u8 = 1;
const GIVEN_UP: u8 = 2;

/// A request on its way to the UI thread. It is acted on only if [`Call::take`] says so: once the
/// handler thread has given up waiting (its client was told "did not answer"), it is dropped
/// unanswered, so nothing happens after a client exited 1.
pub struct Call {
    pub request: Request,
    /// For `tab new`: its directory exists.
    pub cwd_ok: bool,
    deadline: Instant,
    state: Arc<AtomicU8>,
    reply: SyncSender<Reply>,
}

impl fmt::Debug for Call {
    /// No caller text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call").finish_non_exhaustive()
    }
}

impl Call {
    /// Whether the UI thread may act on the request now: it is not overdue and the handler thread
    /// has not given up on it. Once this says yes the handler waits for [`Call::answer`].
    pub fn take(&self, now: Instant) -> bool {
        now < self.deadline
            && self
                .state
                .compare_exchange(PENDING, TAKEN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    pub fn answer(self, reply: Reply) {
        // A handler that is gone has nobody to tell.
        let _ = self.reply.send(reply);
    }
}

/// The server's [`Handler`]: checks a `tab new`'s directory on its own thread, hands the request
/// to `post` (which delivers it to the UI thread; `false` when it cannot) and waits for the reply.
pub struct Bridge<P> {
    post: P,
}

impl<P> fmt::Debug for Bridge<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bridge").finish_non_exhaustive()
    }
}

impl<P> Bridge<P> {
    pub fn new(post: P) -> Bridge<P> {
        Bridge { post }
    }
}

impl<P: Fn(Call) -> bool + Send + 'static> Handler for Bridge<P> {
    fn handle(&self, request: Request) -> Reply {
        let start = Instant::now();
        let cwd_ok = match &request {
            Request::TabNew { cwd, .. } => dir_exists(cwd),
            _ => true,
        };
        let (reply, answered) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(PENDING));
        let call = Call {
            request,
            cwd_ok,
            deadline: start + ACT_BY,
            state: Arc::clone(&state),
            reply,
        };
        if !(self.post)(call) {
            return Reply::new(Code::Failed, CLOSING);
        }
        let wait =
            |until: Instant| answered.recv_timeout(until.saturating_duration_since(Instant::now()));
        if let Ok(reply) = wait(start + ACT_BY) {
            return reply;
        }
        // Not taken by now: it never will be.
        if state
            .compare_exchange(PENDING, GIVEN_UP, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Reply::new(Code::Failed, NO_ANSWER);
        }
        wait(start + ANSWER_BY).unwrap_or_else(|_| Reply::new(Code::Failed, NO_ANSWER))
    }
}

/// Whether `dir` is an existing directory, asked on a thread of its own: a stat on a hung mount
/// can block for long. No answer within [`CWD_CHECK_TIMEOUT`] counts as no (the thread is left
/// to finish on its own).
fn dir_exists(dir: &Path) -> bool {
    let Some(raw) = dir.to_str().map(str::to_owned) else {
        return false;
    };
    let (tx, rx) = mpsc::channel();
    let started = thread::Builder::new()
        .name("luneterm-ctl-cwd".into())
        .spawn(move || {
            let _ = tx.send(cwd::validated(&raw).is_some());
        });
    started.is_ok() && rx.recv_timeout(CWD_CHECK_TIMEOUT).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::GroupColor;

    /// A tab as a request sees it, and nothing else: no session, no grid (N1, N2). That `handle`
    /// and `place` run on it is the proof that they need nothing more.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Fake {
        origin: Origin,
        title: String,
        cwd: Option<String>,
        pid: Option<u32>,
        last_input: Option<Instant>,
    }

    impl ControlTab for Fake {
        fn origin(&self) -> Origin {
            self.origin
        }
        fn title(&self) -> &str {
            &self.title
        }
        fn cwd(&self) -> Option<&str> {
            self.cwd.as_deref()
        }
        fn pid(&self) -> Option<u32> {
            self.pid
        }
        fn last_input(&self) -> Option<Instant> {
            self.last_input
        }
    }

    fn fake(origin: Origin) -> Fake {
        Fake {
            origin,
            title: "Terminal".to_owned(),
            cwd: None,
            pid: None,
            last_input: None,
        }
    }

    fn absolute() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\tmp" } else { "/tmp" })
    }

    fn ctx(control_on: bool, now: Instant) -> Ctx<'static> {
        Ctx {
            control_on,
            window: true,
            version: "0.9.0",
            instance: "abcdefgh",
            now,
        }
    }

    /// Tabs 1 (user, active), 2 (control), 3 (user) and 4 (control) in group "g" with 3.
    fn four() -> TabSet<Fake> {
        let mut tabs = TabSet::new();
        tabs.push(fake(Origin::User));
        tabs.insert(fake(Origin::Control), 9, false);
        tabs.insert(fake(Origin::User), 9, false);
        tabs.insert(fake(Origin::Control), 9, false);
        tabs.set_group(3, "g");
        tabs.set_group(4, "g");
        tabs.activate(1);
        tabs
    }

    fn snapshot(tabs: &TabSet<Fake>) -> String {
        format!("{tabs:?}")
    }

    fn new_request(focus: bool, group: Option<&str>) -> Request {
        Request::TabNew {
            cwd: absolute(),
            group: group.map(str::to_owned),
            title: None,
            focus,
            argv: Vec::new(),
        }
    }

    /// Every command but `status`, on tab `id`.
    fn every_command(id: TabId) -> Vec<Request> {
        vec![
            Request::TabList,
            new_request(false, None),
            new_request(true, Some("g")),
            Request::TabClose {
                id,
                any_origin: false,
            },
            Request::TabClose {
                id,
                any_origin: true,
            },
            Request::TabFocus(id),
            Request::TabGroup(id, "h".to_owned()),
            Request::TabUngroup(id),
        ]
    }

    fn code(outcome: &Outcome) -> Option<Code> {
        match outcome {
            Outcome::Answer(reply, _) => Some(reply.code),
            Outcome::Spawn(_) => None,
        }
    }

    #[test]
    fn with_control_off_every_request_but_status_is_3_and_changes_nothing() {
        let now = Instant::now();
        for id in [1, 2, 3, 4, 99] {
            for request in every_command(id) {
                for cwd_ok in [true, false] {
                    let mut tabs = four();
                    let before = snapshot(&tabs);
                    let outcome = handle(&mut tabs, request.clone(), cwd_ok, &ctx(false, now));
                    assert_eq!(
                        outcome,
                        Outcome::Answer(Reply::new(Code::ControlOff, CONTROL_OFF), Vec::new()),
                        "{id}"
                    );
                    assert_eq!(snapshot(&tabs), before);
                }
            }
        }
        let mut tabs = four();
        let before = snapshot(&tabs);
        assert_eq!(
            handle(&mut tabs, Request::Status, true, &ctx(false, now)),
            Outcome::Answer(
                Reply::new(
                    Code::Done,
                    r#"{"protocol":1,"version":"0.9.0","instance":"abcdefgh","control":"off"}"#
                ),
                Vec::new()
            )
        );
        assert_eq!(snapshot(&tabs), before);
        assert_eq!(
            code(&handle(&mut tabs, Request::Status, true, &ctx(true, now))),
            Some(Code::Done)
        );
    }

    #[test]
    fn an_unknown_id_is_5_for_every_command_on_a_tab_and_changes_nothing() {
        let now = Instant::now();
        for request in every_command(99) {
            if matches!(request, Request::TabList | Request::TabNew { .. }) {
                continue;
            }
            let mut tabs = four();
            let before = snapshot(&tabs);
            let outcome = handle(&mut tabs, request, true, &ctx(true, now));
            assert_eq!(
                outcome,
                Outcome::Answer(Reply::new(Code::NoSuchTab, NO_SUCH_TAB), Vec::new())
            );
            assert_eq!(snapshot(&tabs), before);
        }
    }

    #[test]
    fn close_takes_control_tabs_and_user_tabs_only_with_any_origin() {
        let now = Instant::now();
        for (id, any_origin, want) in [
            (1, false, Code::Refused),
            (3, false, Code::Refused),
            (2, false, Code::Done),
            (4, false, Code::Done),
            (1, true, Code::Done),
            (2, true, Code::Done),
            (3, true, Code::Done),
            (4, true, Code::Done),
        ] {
            let mut tabs = four();
            let before = snapshot(&tabs);
            let outcome = handle(
                &mut tabs,
                Request::TabClose { id, any_origin },
                true,
                &ctx(true, now),
            );
            // The handler only says what to close: the app closes it.
            assert_eq!(snapshot(&tabs), before);
            match want {
                Code::Done => assert_eq!(
                    outcome,
                    Outcome::Answer(
                        Reply::new(Code::Done, format!(r#"{{"closed":{id}}}"#)),
                        vec![Effect::Close(vec![id])]
                    ),
                    "{id} {any_origin}"
                ),
                _ => assert_eq!(
                    outcome,
                    Outcome::Answer(Reply::new(Code::Refused, USER_TAB), Vec::new()),
                    "{id} {any_origin}"
                ),
            }
        }
    }

    #[test]
    fn focus_activates_reveals_raises_and_holds_only_when_the_tab_changes() {
        let now = Instant::now();
        for origin_id in [1, 2, 3, 4] {
            let mut tabs = four();
            tabs.set_collapsed(tabs.group_named("g").expect("g"), true);
            let outcome = handle(
                &mut tabs,
                Request::TabFocus(origin_id),
                true,
                &ctx(true, now),
            );
            let mut effects = vec![Effect::Reveal(origin_id), Effect::Raise];
            if origin_id != 1 {
                effects.push(Effect::Hold);
            }
            assert_eq!(
                outcome,
                Outcome::Answer(
                    Reply::new(Code::Done, format!(r#"{{"focused":{origin_id}}}"#)),
                    effects
                )
            );
            assert_eq!(tabs.active(), Some(origin_id));
            let g = tabs.group_named("g").expect("g");
            // A collapsed member is revealed; the group of a tab outside it stays collapsed.
            let collapsed = tabs.group(g).expect("g").collapsed;
            assert_eq!(collapsed, !matches!(origin_id, 3 | 4), "{origin_id}");
            tabs.assert_invariants();
        }
    }

    #[test]
    fn group_and_ungroup_work_on_either_origin_and_give_a_colour_to_a_new_group() {
        let now = Instant::now();
        for id in [1, 2, 3, 4] {
            let mut tabs = four();
            let outcome = handle(
                &mut tabs,
                Request::TabGroup(id, "h".to_owned()),
                true,
                &ctx(true, now),
            );
            assert_eq!(
                outcome,
                Outcome::Answer(
                    Reply::new(Code::Done, format!(r#"{{"id":{id},"group":"h"}}"#)),
                    vec![Effect::Regrouped(id)]
                )
            );
            let h = tabs.group_named("h").expect("h");
            assert_eq!(tabs.group_of(id), Some(h));
            // "g" took the first colour; "h" the next free one.
            assert_eq!(tabs.group(h).map(|g| g.color), Some(GroupColor::ALL[1]));
            tabs.assert_invariants();

            let outcome = handle(&mut tabs, Request::TabUngroup(id), true, &ctx(true, now));
            assert_eq!(
                outcome,
                Outcome::Answer(
                    Reply::new(Code::Done, format!(r#"{{"id":{id},"group":null}}"#)),
                    vec![Effect::Regrouped(id)]
                )
            );
            assert_eq!(tabs.group_of(id), None);
            assert_eq!(tabs.group_named("h"), None);
            tabs.assert_invariants();
        }
        // Into the group it is in, or out of none: the same reply, nothing changed.
        let mut tabs = four();
        let before = snapshot(&tabs);
        assert_eq!(
            handle(
                &mut tabs,
                Request::TabGroup(3, "g".to_owned()),
                true,
                &ctx(true, now)
            ),
            Outcome::Answer(
                Reply::new(Code::Done, r#"{"id":3,"group":"g"}"#),
                Vec::new()
            )
        );
        assert_eq!(
            handle(&mut tabs, Request::TabUngroup(1), true, &ctx(true, now)),
            Outcome::Answer(
                Reply::new(Code::Done, r#"{"id":1,"group":null}"#),
                Vec::new()
            )
        );
        assert_eq!(snapshot(&tabs), before);
    }

    #[test]
    fn new_checks_the_window_the_directory_and_the_cap_in_that_order() {
        let now = Instant::now();
        let mut tabs = four();
        let mut no_window = ctx(true, now);
        no_window.window = false;
        assert_eq!(
            code(&handle(
                &mut tabs,
                new_request(false, None),
                false,
                &no_window
            )),
            Some(Code::Failed)
        );
        assert_eq!(
            handle(&mut tabs, new_request(false, None), false, &ctx(true, now)),
            Outcome::Answer(Reply::new(Code::Usage, NOT_A_DIR), Vec::new())
        );
        assert_eq!(
            handle(
                &mut tabs,
                new_request(true, Some("g")),
                true,
                &ctx(true, now)
            ),
            Outcome::Spawn(OpenSpec {
                cwd: absolute(),
                group: Some("g".to_owned()),
                title: None,
                focus: true,
                argv: Vec::new(),
            })
        );
        // 64 control tabs (two of them already): the 65th is refused; user tabs do not count.
        for _ in 0..MAX_CONTROL_TABS - 2 {
            tabs.insert(fake(Origin::Control), 99, false);
        }
        tabs.insert(fake(Origin::User), 99, false);
        let before = snapshot(&tabs);
        assert_eq!(
            handle(&mut tabs, new_request(false, None), true, &ctx(true, now)),
            Outcome::Answer(Reply::new(Code::Failed, TOO_MANY), Vec::new())
        );
        assert_eq!(snapshot(&tabs), before);
        let last = tabs
            .ids()
            .into_iter()
            .find(|id| tabs.get(*id).is_some_and(|t| t.origin == Origin::Control))
            .expect("one");
        tabs.close(last);
        assert!(matches!(
            handle(&mut tabs, new_request(false, None), true, &ctx(true, now)),
            Outcome::Spawn(_)
        ));
    }

    fn spec(focus: bool, group: Option<&str>) -> OpenSpec {
        OpenSpec {
            cwd: absolute(),
            group: group.map(str::to_owned),
            title: None,
            focus,
            argv: Vec::new(),
        }
    }

    #[test]
    fn a_background_tab_goes_to_the_end_or_its_group_and_keeps_the_active_tab() {
        let mut tabs = four();
        let id = tabs.next_id();
        let (reply, effects) = place(&mut tabs, fake(Origin::Control), &spec(false, None));
        assert_eq!(reply, Reply::new(Code::Done, format!(r#"{{"id":{id}}}"#)));
        assert!(effects.is_empty());
        assert_eq!(tabs.ids(), vec![1, 2, 3, 4, id]);
        assert_eq!(tabs.active(), Some(1));
        assert_eq!(tabs.group_of(id), None);

        // Into "g" (3, 4): after its last member, which is not the end any more.
        let id = tabs.next_id();
        let (_, effects) = place(&mut tabs, fake(Origin::Control), &spec(false, Some("g")));
        assert!(effects.is_empty());
        assert_eq!(tabs.ids(), vec![1, 2, 3, 4, id, id - 1]);
        assert_eq!(tabs.group_of(id), tabs.group_named("g"));
        assert_eq!(tabs.active(), Some(1));

        // A new group is made where the tab is: the end.
        let id = tabs.next_id();
        place(&mut tabs, fake(Origin::Control), &spec(false, Some("new")));
        assert_eq!(tabs.ids().last(), Some(&id));
        let g = tabs.group_named("new").expect("made");
        assert_eq!(tabs.members(g), vec![id]);
        assert_eq!(tabs.active(), Some(1));
        tabs.assert_invariants();
    }

    #[test]
    fn a_focused_new_tab_is_active_revealed_raised_and_holds_the_keys() {
        let mut tabs = four();
        let g = tabs.group_named("g").expect("g");
        tabs.set_collapsed(g, true);
        let id = tabs.next_id();
        let (_, effects) = place(&mut tabs, fake(Origin::Control), &spec(true, Some("g")));
        assert_eq!(
            effects,
            vec![Effect::Reveal(id), Effect::Raise, Effect::Hold]
        );
        assert_eq!(tabs.active(), Some(id));
        assert!(!tabs.group(g).expect("g").collapsed);
        assert_eq!(tabs.ids(), vec![1, 2, 3, 4, id]);
        // Into the very first window: there was no active tab.
        let mut empty = TabSet::new();
        let (_, effects) = place(&mut empty, fake(Origin::Control), &spec(true, None));
        assert_eq!(
            effects,
            vec![Effect::Reveal(1), Effect::Raise, Effect::Hold]
        );
    }

    #[test]
    fn the_list_has_every_tab_in_order_with_cleaned_text() {
        let now = Instant::now();
        // Absolute on the OS the test runs on, and nothing JSON escapes.
        let root = if cfg!(windows) { "C:/" } else { "/" };
        let long = |bytes: usize| format!("{root}{}", "a".repeat(bytes - root.len()));
        let mut tabs = four();
        let g = tabs.group_named("g").expect("g");
        tabs.set_collapsed(g, true);
        if let Some(t) = tabs.get_mut(1) {
            t.title = "a\u{1b}]0;x\u{7}b".to_owned();
            t.cwd = Some(format!("{root}tmp/a\nb"));
            t.pid = Some(42);
            t.last_input = Some(now - Duration::from_millis(4200));
        }
        if let Some(t) = tabs.get_mut(2) {
            t.cwd = Some("relative".to_owned());
        }
        if let Some(t) = tabs.get_mut(3) {
            t.cwd = Some(long(4097));
        }
        if let Some(t) = tabs.get_mut(4) {
            t.cwd = Some(long(4096));
        }
        let before = snapshot(&tabs);
        let Outcome::Answer(reply, effects) =
            handle(&mut tabs, Request::TabList, true, &ctx(true, now))
        else {
            panic!("a list is an answer");
        };
        assert!(effects.is_empty());
        assert_eq!(snapshot(&tabs), before);
        assert_eq!(reply.code, Code::Done);
        assert_eq!(
            reply.body,
            format!(
                concat!(
                    r#"{{"instance":"abcdefgh","tabs":["#,
                    r#"{{"id":1,"origin":"user","group":null,"title":"a]0;xb","cwd":"{}tmp/ab","#,
                    r#""active":true,"pid":42,"input_idle_ms":4200}},"#,
                    r#"{{"id":2,"origin":"control","group":null,"title":"Terminal","cwd":null,"#,
                    r#""active":false,"pid":null,"input_idle_ms":null}},"#,
                    r#"{{"id":3,"origin":"user","group":"g","title":"Terminal","cwd":null,"#,
                    r#""active":false,"pid":null,"input_idle_ms":null}},"#,
                    r#"{{"id":4,"origin":"control","group":"g","title":"Terminal","cwd":"{}","#,
                    r#""active":false,"pid":null,"input_idle_ms":null}}]}}"#
                ),
                root,
                long(4096)
            )
        );
    }

    /// The source of this module before its tests: what a request can reach.
    fn module_source() -> &'static str {
        let all = include_str!("handler.rs");
        all.split("#[cfg(test)]").next().unwrap_or(all)
    }

    #[test]
    fn the_handler_reaches_no_session_and_no_content() {
        let src = module_source();
        assert!(
            src.contains("pub fn handle<T: ControlTab>"),
            "the instrument reads this file"
        );
        // The doc comments name what is out of reach; code may not.
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for word in [
            "session",
            "Session",
            "Command",
            "grid",
            "Grid",
            "scrollback",
            "selection",
            "clipboard",
            "Clipboard",
            " Tab,",
            "{Tab,",
            " Tab}",
            "::Tab;",
            "::Tab}",
        ] {
            assert!(!code.contains(word), "{word}");
        }
    }

    // ------------------------------------------------------------------------------- bridge

    /// A bridge whose UI side runs `ui` on its own thread with each call.
    fn bridge(
        ui: impl Fn(Call) + Send + Sync + 'static,
    ) -> Bridge<impl Fn(Call) -> bool + Send + 'static> {
        let ui = Arc::new(ui);
        Bridge::new(move |call| {
            let ui = Arc::clone(&ui);
            thread::spawn(move || ui(call));
            true
        })
    }

    #[test]
    fn an_answered_call_is_the_reply() {
        let b = bridge(|call| {
            assert!(call.take(Instant::now()));
            assert!(call.cwd_ok);
            call.answer(Reply::new(Code::NoSuchTab, "x"));
        });
        assert_eq!(b.handle(Request::TabList), Reply::new(Code::NoSuchTab, "x"));
    }

    #[test]
    fn a_call_the_ui_does_not_take_in_time_is_1_and_cannot_be_taken_after() {
        let (late, got) = mpsc::channel();
        let b = bridge(move |call| {
            let _ = late.send(call);
        });
        let start = Instant::now();
        assert_eq!(
            b.handle(Request::TabList),
            Reply::new(Code::Failed, NO_ANSWER)
        );
        let took = start.elapsed();
        assert!(took >= ACT_BY && took < ANSWER_BY, "{took:?}");
        let call = got.recv().expect("posted");
        // Overdue by the clock, and given up by the handler: both refuse it.
        assert!(!call.take(Instant::now()));
        assert!(!call.take(start));
    }

    #[test]
    fn a_call_taken_in_time_is_waited_for_past_the_act_deadline() {
        let b = bridge(|call| {
            assert!(call.take(Instant::now()));
            thread::sleep(ACT_BY + Duration::from_millis(100));
            call.answer(Reply::new(Code::Done, "late"));
        });
        assert_eq!(b.handle(Request::TabList), Reply::new(Code::Done, "late"));
    }

    #[test]
    fn a_dropped_or_undeliverable_call_is_1_at_once() {
        let b = bridge(drop::<Call>);
        let start = Instant::now();
        assert_eq!(
            b.handle(Request::TabList),
            Reply::new(Code::Failed, NO_ANSWER)
        );
        assert!(start.elapsed() < ACT_BY);
        let closed = Bridge::new(|_call: Call| false);
        assert_eq!(
            closed.handle(Request::TabList),
            Reply::new(Code::Failed, CLOSING)
        );
    }

    #[test]
    fn a_new_tabs_directory_is_checked_before_the_ui_sees_it() {
        let dir = std::env::temp_dir();
        let missing = dir.join(format!("luneterm-ctl-no-such-dir-{}", std::process::id()));
        assert!(!missing.exists());
        for (cwd, want) in [(dir, true), (missing, false)] {
            let b = bridge(move |call| {
                assert!(call.take(Instant::now()));
                let ok = call.cwd_ok;
                call.answer(Reply::new(Code::Done, if ok { "yes" } else { "no" }));
            });
            let request = Request::TabNew {
                cwd,
                group: None,
                title: None,
                focus: false,
                argv: Vec::new(),
            };
            assert_eq!(b.handle(request).body, if want { "yes" } else { "no" });
        }
    }
}
