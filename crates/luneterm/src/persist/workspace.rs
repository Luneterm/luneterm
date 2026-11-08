//! `workspace.json` (plan M10 step 5, D17, ledger 12): the tabs to open again at the next start.
//!
//! `{"version": 1, "active": 0, "tabs": [{"cwd": ..., "live_cwd": ..., "title": ...}, ...]}`, the
//! shape of Lunegit's `TerminalWorkspaceState`: per tab the directory it was started in, the last
//! directory its shell reported, and the name the user gave it. At most [`MAX_TABS`] tabs are
//! read. Directories are checked ([`crate::cwd::syntax`] on read, [`crate::cwd::validated`] at
//! restore, which stats and so runs off the UI thread); titles go through [`title::sanitize`].
//! Nothing in the file is executed.
//!
//! **Ledger 12: the file only ever describes tabs that really started.** [`SaveGuard`] suppresses
//! every write while any tab is in the failed-spawn state, and after a tab that died within moments
//! of its start (a shell that cannot run) until the user does something with the tabs, so a
//! failure never shrinks the saved list as a side effect; the previous file stands.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use lune_ui::json::{self, Value};

use super::json_write::{Json, WriteError};
use super::{Reject, read_version};
use crate::cwd;
use crate::title::{self, DEFAULT_TITLE};

/// The version this build writes and reads.
pub const WORKSPACE_VERSION: i64 = 1;
/// Tabs read from a file; the rest are ignored.
pub const MAX_TABS: usize = 32;
/// Characters of a title read (it is then sanitized, and at most 80).
pub const MAX_TITLE_CHARS: usize = 256;
/// A tab whose child exits within this long of its start counts as a failed start.
pub const STARTUP_GRACE: Duration = Duration::from_secs(3);

/// One saved tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabState {
    /// Where the tab was started.
    pub working_directory: Option<String>,
    /// The last directory its shell reported.
    pub current_directory: Option<String>,
    /// The name the user gave it.
    pub manual_title: Option<String>,
}

/// The saved tabs and the active one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workspace {
    pub tabs: Vec<TabState>,
    pub active: usize,
    /// Top-level members this build does not know, kept through a rewrite (per-tab unknown
    /// members are dropped).
    pub unknown: Vec<(String, Json)>,
}

fn string_member<'a>(members: &'a [(String, Value)], key: &str) -> Option<&'a str> {
    match members.iter().find(|(k, _)| k == key) {
        Some((_, Value::String(s))) => Some(s),
        _ => None,
    }
}

/// A directory text that passes the syntax check (absolute, no control, no network or device
/// form): nothing is touched on disk here.
fn dir_text(members: &[(String, Value)], key: &str) -> Option<String> {
    let raw = string_member(members, key)?;
    cwd::syntax(raw).map(|_| raw.to_owned())
}

fn title_text(members: &[(String, Value)]) -> Option<String> {
    let raw = string_member(members, "title")?;
    let capped: String = raw.chars().take(MAX_TITLE_CHARS).collect();
    let clean = title::sanitize(&capped);
    (clean != DEFAULT_TITLE).then_some(clean)
}

/// Reads a workspace file's bytes. A file that is not a valid workspace document is [`Reject`]ed;
/// inside a valid one, a tab that is not an object is skipped and a field of the wrong type is
/// none.
pub fn parse_workspace(bytes: &[u8]) -> Result<Workspace, Reject> {
    let value = json::parse(bytes).map_err(|_| Reject::Unreadable)?;
    let Value::Object(members) = &value else {
        return Err(Reject::NotAnObject);
    };
    read_version(members, WORKSPACE_VERSION)?;
    let tabs: Vec<TabState> = match members.iter().find(|(k, _)| k == "tabs") {
        Some((_, Value::Array(items))) => items
            .iter()
            .filter_map(|item| match item {
                Value::Object(tab) => Some(TabState {
                    working_directory: dir_text(tab, "cwd"),
                    current_directory: dir_text(tab, "live_cwd"),
                    manual_title: title_text(tab),
                }),
                _ => None,
            })
            .take(MAX_TABS)
            .collect(),
        _ => Vec::new(),
    };
    let active = members
        .iter()
        .find(|(k, _)| k == "active")
        .and_then(|(_, v)| v.as_int().ok())
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
        .min(tabs.len().saturating_sub(1));
    let unknown = members
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "version" | "active" | "tabs"))
        .map(|(k, v)| (k.clone(), Json::from_value(v)))
        .collect();
    Ok(Workspace {
        tabs,
        active,
        unknown,
    })
}

/// The text of a workspace file. A directory or a title the writer cannot write (over the
/// reader's string limit) is left out of its tab, not the tab.
pub fn render_workspace(workspace: &Workspace) -> Result<String, WriteError> {
    let fit = |s: &Option<String>| -> Option<Json> {
        s.as_ref()
            .filter(|s| s.len() <= json::MAX_STRING)
            .map(|s| Json::Str(s.clone()))
    };
    let tabs: Vec<Json> = workspace
        .tabs
        .iter()
        .take(MAX_TABS)
        .map(|t| {
            let mut members = Vec::new();
            for (key, value) in [
                ("cwd", fit(&t.working_directory)),
                ("live_cwd", fit(&t.current_directory)),
                ("title", fit(&t.manual_title)),
            ] {
                if let Some(v) = value {
                    members.push((key.to_owned(), v));
                }
            }
            Json::Object(members)
        })
        .collect();
    let active = i64::try_from(workspace.active.min(tabs.len().saturating_sub(1))).unwrap_or(0);
    let mut members = vec![
        ("version".to_owned(), Json::Int(WORKSPACE_VERSION)),
        ("active".to_owned(), Json::Int(active)),
        ("tabs".to_owned(), Json::Array(tabs)),
    ];
    members.extend(workspace.unknown.iter().cloned());
    Json::Object(members).to_text()
}

/// Which directory each saved tab starts in: its live directory, else the one it was started in,
/// else `None` (the default directory). `valid` is [`cwd::validated`]: it stats, so this runs off
/// the UI thread.
pub fn restore_dirs(
    tabs: &[TabState],
    valid: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<Option<PathBuf>> {
    tabs.iter()
        .map(|t| {
            [&t.current_directory, &t.working_directory]
                .into_iter()
                .flatten()
                .find_map(|dir| valid(dir))
        })
        .collect()
}

/// What the app knows of one tab when it saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    /// The shell really started (a failed-spawn tab is not saved).
    pub started: bool,
    pub launch_dir: Option<String>,
    pub live_dir: Option<String>,
    pub title: Option<String>,
}

/// The workspace to save for `tabs` (in order) with the tab at `active` active, or `None` when the
/// guard says nothing may be written. Only tabs that started are in it.
pub fn snapshot(
    tabs: &[TabInfo],
    active: usize,
    guard: &SaveGuard,
    unknown: &[(String, Json)],
) -> Option<Workspace> {
    if !guard.may_write() {
        return None;
    }
    let started: Vec<(usize, &TabInfo)> =
        tabs.iter().enumerate().filter(|(_, t)| t.started).collect();
    let active = started.iter().position(|(i, _)| *i == active).unwrap_or(0);
    Some(Workspace {
        tabs: started
            .into_iter()
            .map(|(_, t)| TabState {
                working_directory: t.launch_dir.clone(),
                current_directory: t.live_dir.clone(),
                manual_title: t.title.clone(),
            })
            .collect(),
        active,
        unknown: unknown.to_vec(),
    })
}

// -------------------------------------------------------------------------------- ledger 12

/// When the workspace may be written (ledger 12). The app tells it what happened to the tabs.
#[derive(Debug, Default)]
pub struct SaveGuard {
    /// Tabs whose shell never started: while there is one, nothing is written.
    failed: HashSet<u64>,
    /// A tab died at its start and closed itself: nothing is written until the user acts.
    quick_exit: bool,
}

impl SaveGuard {
    pub fn new() -> SaveGuard {
        SaveGuard::default()
    }

    /// Tab `id` is showing why its shell did not start.
    pub fn spawn_failed(&mut self, id: u64) {
        self.failed.insert(id);
    }

    /// Tab `id` is gone (the user closed it).
    pub fn tab_closed(&mut self, id: u64) {
        self.failed.remove(&id);
    }

    /// Tab `id`'s child exited `after` its start and the tab closed on its own. A child that dies
    /// at once is a failed start, not a decision of the user.
    pub fn child_exited(&mut self, id: u64, after: Duration) {
        self.failed.remove(&id);
        if after < STARTUP_GRACE {
            self.quick_exit = true;
        }
    }

    /// The user did something with the tabs (opened, closed, renamed, moved or chose one): what
    /// they see now is what they want kept, so a past quick exit no longer blocks.
    pub fn user_acted(&mut self) {
        self.quick_exit = false;
    }

    /// Whether the workspace may be written now.
    pub fn may_write(&self) -> bool {
        self.failed.is_empty() && !self.quick_exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(cwd: &str, live: Option<&str>, title: Option<&str>) -> TabState {
        TabState {
            working_directory: Some(cwd.into()),
            current_directory: live.map(Into::into),
            manual_title: title.map(Into::into),
        }
    }

    #[test]
    fn a_workspace_round_trips() {
        let ws = Workspace {
            tabs: vec![
                tab("/a", Some("/a/b"), Some("build")),
                tab("/c", None, None),
                TabState::default(),
            ],
            active: 1,
            unknown: Vec::new(),
        };
        let text = render_workspace(&ws).expect("writes");
        assert!(
            text.starts_with("{\"version\":1,\"active\":1,\"tabs\":["),
            "{text}"
        );
        assert_eq!(parse_workspace(text.as_bytes()), Ok(ws));
    }

    #[test]
    fn at_most_32_tabs_are_read_and_active_is_clamped() {
        let tabs: Vec<String> = (0..50).map(|i| format!(r#"{{"cwd":"/t{i}"}}"#)).collect();
        let doc = format!(r#"{{"version":1,"active":45,"tabs":[{}]}}"#, tabs.join(","));
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert_eq!(ws.tabs.len(), MAX_TABS);
        assert_eq!(ws.active, MAX_TABS - 1);
        assert_eq!(ws.tabs[31].working_directory.as_deref(), Some("/t31"));
        for active in ["-1", "1.5", "\"two\"", "null", "9007199254740992"] {
            let doc = format!(r#"{{"version":1,"active":{active},"tabs":[{{}},{{}}]}}"#);
            let ws = parse_workspace(doc.as_bytes()).expect("read");
            assert!(ws.active <= 1, "{active}: {}", ws.active);
        }
        let none = parse_workspace(br#"{"version":1,"active":3,"tabs":[]}"#).expect("read");
        assert_eq!((none.tabs.len(), none.active), (0, 0));
        // Writing clamps too.
        let ws = Workspace {
            tabs: vec![TabState::default()],
            active: 9,
            unknown: Vec::new(),
        };
        assert!(
            render_workspace(&ws)
                .expect("writes")
                .contains("\"active\":0")
        );
    }

    #[test]
    fn titles_are_sanitized_and_capped_and_directories_are_syntax_checked() {
        let long = "x".repeat(3000);
        let doc = format!(
            r#"{{"version":1,"tabs":[
              {{"title":"  build \u001b[31m\u202e  "}},
              {{"title":"{long}"}},
              {{"title":"   "}},
              {{"title":7}},
              {{"cwd":"relative/dir","live_cwd":"/ok/dir"}},
              {{"cwd":"\\\\evil\\share","live_cwd":"//evil/share"}},
              {{"cwd":"/has\nnewline"}},
              "not a tab",
              {{"cwd":["/a"]}}
            ]}}"#
        );
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        let titles: Vec<_> = ws.tabs.iter().map(|t| t.manual_title.as_deref()).collect();
        assert_eq!(
            titles[0],
            Some("build [31m"),
            "controls and bidi marks are dropped"
        );
        assert_eq!(
            ws.tabs.len(),
            8,
            "the string is skipped, every object is a tab"
        );
        assert_eq!(ws.tabs[1].manual_title.as_deref().map(str::len), Some(80));
        assert_eq!(
            ws.tabs[2].manual_title, None,
            "nothing printable is no title"
        );
        assert_eq!(ws.tabs[3].manual_title, None, "a number is no title");
        assert_eq!(ws.tabs[4].working_directory, None, "relative");
        assert_eq!(ws.tabs[4].current_directory.as_deref(), Some("/ok/dir"));
        assert_eq!(
            ws.tabs[5],
            TabState::default(),
            "network and device forms never get in"
        );
        assert_eq!(ws.tabs[6], TabState::default(), "a control character");
        assert_eq!(ws.tabs[7], TabState::default(), "an array is no string");
    }

    #[test]
    fn a_file_that_is_not_a_workspace_is_refused_whole() {
        for (bytes, why) in [
            (&b""[..], Reject::Unreadable),
            (b"\x00\x01", Reject::Unreadable),
            (b"[]", Reject::NotAnObject),
            (b"{}", Reject::MissingVersion),
            (br#"{"version":3,"tabs":[]}"#, Reject::NewerVersion(3)),
            (br#"{"version":0}"#, Reject::OldVersion(0)),
        ] {
            assert_eq!(parse_workspace(bytes), Err(why));
        }
        // Wrong-typed tabs inside a valid document is no tabs, not a refusal.
        let ws = parse_workspace(br#"{"version":1,"tabs":"nope","x":1}"#).expect("read");
        assert!(ws.tabs.is_empty());
        assert_eq!(ws.unknown.len(), 1);
    }

    #[test]
    fn unknown_top_level_members_survive_a_rewrite_and_tab_members_do_not() {
        let ws = parse_workspace(
            br#"{"version":1,"layout":{"split":2},"tabs":[{"cwd":"/a","future":true}]}"#,
        )
        .expect("read");
        let text = render_workspace(&ws).expect("writes");
        assert!(text.contains(r#""layout":{"split":2}"#), "{text}");
        assert!(
            !text.contains("future"),
            "per-tab unknown fields are dropped"
        );
    }

    #[test]
    fn a_hostile_string_that_grows_under_escaping_round_trips_or_is_left_out() {
        // A directory of 4096 bytes made of quotes and backslashes (12 KiB once written): the
        // reader's limit is on the decoded text, so it reads back.
        let hostile = format!("/{}", "\"\\".repeat(2047));
        assert_eq!(hostile.len(), 4095);
        let ws = Workspace {
            tabs: vec![TabState {
                working_directory: Some(hostile.clone()),
                current_directory: Some(format!("{hostile}x{hostile}")),
                manual_title: Some("t".into()),
            }],
            active: 0,
            unknown: Vec::new(),
        };
        let text = render_workspace(&ws).expect("writes");
        let back = parse_workspace(text.as_bytes()).expect("reads");
        // `cwd::syntax` allows quotes and backslashes in the middle of an absolute path on Unix.
        assert_eq!(back.tabs[0].manual_title.as_deref(), Some("t"));
        assert_eq!(back.tabs[0].working_directory.is_some(), cfg!(unix));
        assert_eq!(
            back.tabs[0].current_directory, None,
            "over the limit: left out, not saved"
        );
    }

    #[test]
    fn restore_prefers_the_live_directory_then_the_launch_one_then_none() {
        let tabs = vec![
            tab("/launch", Some("/live"), None),
            tab("/launch", Some("/gone"), None),
            tab("/gone", Some("/gone"), None),
            TabState::default(),
        ];
        let valid = |p: &str| matches!(p, "/launch" | "/live").then(|| PathBuf::from(p));
        assert_eq!(
            restore_dirs(&tabs, &valid),
            vec![
                Some(PathBuf::from("/live")),
                Some(PathBuf::from("/launch")),
                None,
                None
            ]
        );
    }

    #[test]
    fn nothing_is_written_while_a_tab_failed_to_start_and_the_previous_file_stands() {
        let mut g = SaveGuard::new();
        assert!(g.may_write());
        g.spawn_failed(2);
        assert!(!g.may_write());
        g.spawn_failed(3);
        g.tab_closed(2);
        assert!(!g.may_write(), "another failed tab remains");
        g.user_acted();
        assert!(!g.may_write(), "acting does not hide a failed tab");
        g.tab_closed(3);
        assert!(
            g.may_write(),
            "the failed tabs are gone: what remains started"
        );
        // Closing an unknown tab is nothing.
        g.tab_closed(99);
        assert!(g.may_write());
    }

    #[test]
    fn a_child_that_dies_at_its_start_blocks_writes_until_the_user_acts() {
        let mut g = SaveGuard::new();
        g.child_exited(1, Duration::from_millis(50));
        assert!(!g.may_write());
        g.user_acted();
        assert!(g.may_write());
        // A shell the user exited after a while is a decision, not a failure.
        g.child_exited(2, Duration::from_secs(60));
        assert!(g.may_write());
        g.child_exited(3, STARTUP_GRACE);
        assert!(g.may_write(), "at the grace the start is over");
        // A failed tab that then dies: the failure is the exit now.
        g.spawn_failed(4);
        g.child_exited(4, Duration::from_secs(10));
        assert!(g.may_write());
    }

    #[test]
    fn a_seeded_fuzz_of_bytes_never_panics_and_what_it_reads_is_within_bounds() {
        let valid = render_workspace(&Workspace {
            tabs: vec![tab("/a", Some("/b"), Some("t")), tab("/c", None, None)],
            active: 1,
            unknown: Vec::new(),
        })
        .expect("writes")
        .into_bytes();
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut ok, mut rejected) = (0, 0);
        for round in 0..3000 {
            let mut bytes: Vec<u8> = if round % 3 == 0 {
                (0..(next() % 200)).map(|_| next() as u8).collect()
            } else {
                valid.clone()
            };
            if round % 3 != 0 {
                for _ in 0..=(next() % 4) {
                    if bytes.is_empty() {
                        break;
                    }
                    let at = (next() as usize) % bytes.len();
                    match next() % 3 {
                        0 => bytes[at] = next() as u8,
                        1 => bytes.truncate(at),
                        _ => bytes.insert(at, next() as u8),
                    }
                }
            }
            match parse_workspace(&bytes) {
                Ok(ws) => {
                    ok += 1;
                    assert!(ws.tabs.len() <= MAX_TABS);
                    assert!(ws.active < ws.tabs.len().max(1));
                    for t in &ws.tabs {
                        for dir in [&t.working_directory, &t.current_directory]
                            .into_iter()
                            .flatten()
                        {
                            assert!(cwd::syntax(dir).is_some(), "an unchecked directory got in");
                        }
                        assert!(
                            t.manual_title
                                .as_deref()
                                .is_none_or(|t| t.chars().count() <= 80)
                        );
                    }
                }
                Err(_) => rejected += 1,
            }
        }
        assert!(ok > 0 && rejected > 0, "ok {ok}, rejected {rejected}");
    }
}
