//! `workspace.json` (plan M10 step 5, D17, ledger 12): the tabs to open again at the next start.
//!
//! `{"version": 1, "active": 0, "tabs": [{"cwd": ..., "live_cwd": ..., "title": ..., "group": ...},
//! ...], "groups": [{"name": ..., "color": ..., "collapsed": ...}, ...]}`, the shape of Lunegit's
//! `TerminalWorkspaceState` plus the tab groups: per tab the directory it was started in, the last
//! directory its shell reported, the name the user gave it and the name of its group. At most
//! [`MAX_TABS`] tabs are read. Directories are checked ([`crate::cwd::syntax`] on read,
//! [`crate::cwd::validated`] at restore, which stats and so runs off the UI thread); titles go
//! through [`title::sanitize_name`], group names through [`label_ok`]. Nothing in the file is
//! executed.
//!
//! The groups are additions inside version 1: an older build drops a tab's `group` and keeps the
//! top-level `groups` it does not know. A file without a group is written without `groups`. What a
//! file gets wrong about its groups is repaired on read, never refused ([`parse_workspace`]).
//!
//! A tab that runs a program given to `luneterm ctl` is not saved.
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
use crate::ctl::{MAX_GROUP_CHARS, label_ok};
use crate::cwd;
use crate::groups::{Group, GroupColor, default_color};
use crate::tabs::{TabId, TabSet};
use crate::title;

/// The version this build writes and reads.
pub const WORKSPACE_VERSION: i64 = 1;
/// Tabs read from a file; the rest are ignored.
pub const MAX_TABS: usize = 32;
/// Characters of a title read (it is then sanitized, and at most [`title::MAX_NAME_CHARS`]).
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
    /// The name of its group, one of [`Workspace::groups`].
    pub group: Option<String>,
}

/// The saved tabs and the active one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workspace {
    pub tabs: Vec<TabState>,
    pub active: usize,
    /// The groups the tabs are in. As read: one per group, in the order of the tabs, each with at
    /// least one tab and the tabs of each next to one another.
    pub groups: Vec<Group>,
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
    title::sanitize_name(&capped)
}

/// A tab's group name, if it passes the label rule `ctl` names a group by.
fn group_text(members: &[(String, Value)]) -> Option<String> {
    let raw = string_member(members, "group")?;
    label_ok(raw, MAX_GROUP_CHARS).then(|| raw.to_owned())
}

/// The groups `tabs` name, in the order of their first tab. Colour and collapsed state come from
/// the first object of `entries` with that name; a group without one, or with a colour this build
/// does not know, gets the colour a new group would get. An entry no tab names is ignored.
fn read_groups(tabs: &[TabState], entries: &[Value]) -> Vec<Group> {
    let mut found: Vec<(&str, Option<GroupColor>, bool)> = Vec::new();
    for name in tabs.iter().filter_map(|t| t.group.as_deref()) {
        if found.iter().any(|(n, ..)| *n == name) {
            continue;
        }
        let entry = entries.iter().find_map(|e| match e {
            Value::Object(group) if string_member(group, "name") == Some(name) => Some(group),
            _ => None,
        });
        let color = entry
            .and_then(|g| string_member(g, "color"))
            .and_then(GroupColor::from_name);
        let collapsed = entry.is_some_and(|g| {
            g.iter()
                .find(|(k, _)| k == "collapsed")
                .is_some_and(|(_, v)| matches!(v, Value::Bool(true)))
        });
        found.push((name, color, collapsed));
    }
    // The colours the file does name are taken before any is given out.
    let mut in_use: Vec<GroupColor> = found.iter().filter_map(|(_, c, _)| *c).collect();
    found
        .into_iter()
        .map(|(name, color, collapsed)| {
            let color = color.unwrap_or_else(|| {
                let new = default_color(in_use.iter().copied());
                in_use.push(new);
                new
            });
            Group {
                name: name.to_owned(),
                color,
                collapsed,
            }
        })
        .collect()
}

/// Moves each group's tabs to just after its first one, in the order they came; returns them with
/// the new index of the tab that was at `active`.
fn gather(tabs: Vec<TabState>, active: usize) -> (Vec<TabState>, usize) {
    let firsts: Vec<usize> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| match &t.group {
            Some(_) => tabs.iter().position(|o| o.group == t.group).unwrap_or(i),
            None => i,
        })
        .collect();
    let mut keyed: Vec<((usize, usize), TabState)> = tabs
        .into_iter()
        .enumerate()
        .map(|(i, t)| ((firsts[i], i), t))
        .collect();
    keyed.sort_by_key(|(key, _)| *key);
    let active = keyed
        .iter()
        .position(|((_, i), _)| *i == active)
        .unwrap_or(0);
    (keyed.into_iter().map(|(_, t)| t).collect(), active)
}

/// Reads a workspace file's bytes. A file that is not a valid workspace document is [`Reject`]ed;
/// inside a valid one, a tab that is not an object is skipped and a field of the wrong type is
/// none.
///
/// Groups are repaired, never a reason to refuse: a name that fails [`label_ok`] is no group; of
/// two entries with one name the first counts; a tab's group with no entry, or with an unknown
/// colour, gets a new group's colour; an entry without a tab is dropped; and tabs of one group
/// that are apart are gathered after its first one (`active` follows its tab).
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
                    group: group_text(tab),
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
    let (tabs, active) = gather(tabs, active);
    let entries: &[Value] = match members.iter().find(|(k, _)| k == "groups") {
        Some((_, Value::Array(entries))) => entries,
        _ => &[],
    };
    let groups = read_groups(&tabs, entries);
    let unknown = members
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "version" | "active" | "tabs" | "groups"))
        .map(|(k, v)| (k.clone(), Json::from_value(v)))
        .collect();
    Ok(Workspace {
        tabs,
        active,
        groups,
        unknown,
    })
}

/// The text of a workspace file. A directory or a title the writer cannot write (over the
/// reader's string limit) is left out of its tab, not the tab. Only the groups of the written tabs
/// are written.
pub fn render_workspace(workspace: &Workspace) -> Result<String, WriteError> {
    let fit = |s: &Option<String>| -> Option<Json> {
        s.as_ref()
            .filter(|s| s.len() <= json::MAX_STRING)
            .map(|s| Json::Str(s.clone()))
    };
    let written = &workspace.tabs[..workspace.tabs.len().min(MAX_TABS)];
    let tabs: Vec<Json> = written
        .iter()
        .map(|t| {
            let mut members = Vec::new();
            for (key, value) in [
                ("cwd", fit(&t.working_directory)),
                ("live_cwd", fit(&t.current_directory)),
                ("title", fit(&t.manual_title)),
                ("group", fit(&t.group)),
            ] {
                if let Some(v) = value {
                    members.push((key.to_owned(), v));
                }
            }
            Json::Object(members)
        })
        .collect();
    let groups: Vec<Json> = workspace
        .groups
        .iter()
        .filter(|g| written.iter().any(|t| t.group.as_ref() == Some(&g.name)))
        .map(|g| {
            Json::Object(vec![
                ("name".to_owned(), Json::Str(g.name.clone())),
                ("color".to_owned(), Json::Str(g.color.as_str().to_owned())),
                ("collapsed".to_owned(), Json::Bool(g.collapsed)),
            ])
        })
        .collect();
    let active = i64::try_from(workspace.active.min(tabs.len().saturating_sub(1))).unwrap_or(0);
    let mut members = vec![
        ("version".to_owned(), Json::Int(WORKSPACE_VERSION)),
        ("active".to_owned(), Json::Int(active)),
        ("tabs".to_owned(), Json::Array(tabs)),
    ];
    if !groups.is_empty() {
        members.push(("groups".to_owned(), Json::Array(groups)));
    }
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

/// Puts the tabs a start has just restored back as `saved` has them: each into its group, the
/// groups with their colour and collapsed state, and the saved active tab active (which leaves a
/// collapsed group collapsed). `ids[i]` is the tab made for `saved.tabs[i]`, `None` if none was.
pub fn restore_layout<T>(tabs: &mut TabSet<T>, ids: &[Option<TabId>], saved: &Workspace) {
    for (state, id) in saved.tabs.iter().zip(ids) {
        let (Some(name), Some(id)) = (&state.group, id) else {
            continue;
        };
        let made = tabs.set_group(*id, name);
        if let Some(group) = saved.groups.iter().find(|g| &g.name == name) {
            tabs.recolor_group(made, group.color);
            tabs.set_collapsed(made, group.collapsed);
        }
    }
    if let Some(Some(id)) = ids.get(saved.active) {
        tabs.activate(*id);
    }
}

/// What the app knows of one tab when it saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    /// The shell really started (a failed-spawn tab is not saved).
    pub started: bool,
    pub launch_dir: Option<String>,
    pub live_dir: Option<String>,
    pub title: Option<String>,
    /// It runs a program given to `luneterm ctl`: such a tab is not saved.
    pub program: bool,
    /// The group it is in, as it is now.
    pub group: Option<Group>,
}

/// The workspace to save for `tabs` (in order) with the tab at `active` active, or `None` when the
/// guard says nothing may be written. Only tabs that started and run a shell are in it, and only
/// the groups of those.
pub fn snapshot(
    tabs: &[TabInfo],
    active: usize,
    guard: &SaveGuard,
    unknown: &[(String, Json)],
) -> Option<Workspace> {
    if !guard.may_write() {
        return None;
    }
    let saved: Vec<(usize, &TabInfo)> = tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.started && !t.program)
        .collect();
    let active = saved.iter().position(|(i, _)| *i == active).unwrap_or(0);
    let mut groups: Vec<Group> = Vec::new();
    for group in saved.iter().filter_map(|(_, t)| t.group.as_ref()) {
        if !groups.iter().any(|g| g.name == group.name) {
            groups.push(group.clone());
        }
    }
    Some(Workspace {
        tabs: saved
            .into_iter()
            .map(|(_, t)| TabState {
                working_directory: t.launch_dir.clone(),
                current_directory: t.live_dir.clone(),
                manual_title: t.title.clone(),
                group: t.group.as_ref().map(|g| g.name.clone()),
            })
            .collect(),
        active,
        groups,
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

    /// An absolute directory on this OS: `/a/b`, or `C:\a\b` on Windows (where `/a/b` is not
    /// absolute, so the syntax check would refuse it).
    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:{}", p.replace('/', "\\"))
        } else {
            p.to_owned()
        }
    }

    /// [`abs`] as it is written inside a JSON string.
    fn json_abs(p: &str) -> String {
        abs(p).replace('\\', "\\\\")
    }

    fn tab(cwd: &str, live: Option<&str>, title: Option<&str>) -> TabState {
        TabState {
            working_directory: Some(cwd.into()),
            current_directory: live.map(Into::into),
            manual_title: title.map(Into::into),
            group: None,
        }
    }

    #[test]
    fn a_workspace_round_trips() {
        let ws = Workspace {
            tabs: vec![
                tab(&abs("/a"), Some(&abs("/a/b")), Some("build")),
                tab(&abs("/c"), None, None),
                TabState::default(),
            ],
            active: 1,
            ..Workspace::default()
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
        let tabs: Vec<String> = (0..50)
            .map(|i| {
                let dir = json_abs(&format!("/t{i}"));
                format!(r#"{{"cwd":"{dir}"}}"#)
            })
            .collect();
        let doc = format!(r#"{{"version":1,"active":45,"tabs":[{}]}}"#, tabs.join(","));
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert_eq!(ws.tabs.len(), MAX_TABS);
        assert_eq!(ws.active, MAX_TABS - 1);
        assert_eq!(
            ws.tabs[31].working_directory.as_deref(),
            Some(abs("/t31").as_str())
        );
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
            ..Workspace::default()
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
              {{"cwd":"relative/dir","live_cwd":"{ok}"}},
              {{"cwd":"\\\\evil\\share","live_cwd":"//evil/share"}},
              {{"cwd":"/has\nnewline"}},
              "not a tab",
              {{"cwd":["/a"]}}
            ]}}"#,
            ok = json_abs("/ok/dir")
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
        assert_eq!(ws.tabs[1].manual_title.as_deref().map(str::len), Some(128));
        assert_eq!(
            ws.tabs[2].manual_title, None,
            "nothing printable is no title"
        );
        assert_eq!(ws.tabs[3].manual_title, None, "a number is no title");
        assert_eq!(ws.tabs[4].working_directory, None, "relative");
        assert_eq!(
            ws.tabs[4].current_directory.as_deref(),
            Some(abs("/ok/dir").as_str())
        );
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

    /// What v0.8.0's writer wrote (its own `snapshot` and `render_workspace`, run at the tag over
    /// four tabs, the third not started, the second active, with a top-level member it did not
    /// know). The Unix directories are spelled for the OS the test runs on.
    fn v0_8_0_file() -> String {
        let [code, app, srv] = ["/home/u/code", "/home/u/code/app", "/srv"].map(json_abs);
        format!(
            "{{\"version\":1,\"active\":1,\"tabs\":[{{\"cwd\":\"{code}\",\"live_cwd\":\"{app}\",\
             \"title\":\"build \\\"dev\\\" é\"}},{{\"cwd\":\"{srv}\"}},{{}}],\
             \"layout\":{{\"split\":2}}}}\n"
        )
    }

    #[test]
    fn a_v0_8_0_file_reads_unchanged_and_is_written_back_the_same() {
        let golden = v0_8_0_file();
        let ws = parse_workspace(golden.as_bytes()).expect("read");
        assert_eq!(
            ws.tabs,
            [
                tab(
                    &abs("/home/u/code"),
                    Some(&abs("/home/u/code/app")),
                    Some("build \"dev\" é")
                ),
                tab(&abs("/srv"), None, None),
                TabState::default(),
            ]
        );
        assert_eq!((ws.active, ws.groups.len()), (1, 0));
        assert_eq!(render_workspace(&ws).expect("writes"), golden);
    }

    /// The group name of each tab, `.` for none.
    fn membership(ws: &Workspace) -> Vec<&str> {
        ws.tabs
            .iter()
            .map(|t| t.group.as_deref().unwrap_or("."))
            .collect()
    }

    fn group(name: &str, color: GroupColor, collapsed: bool) -> Group {
        Group {
            name: name.into(),
            color,
            collapsed,
        }
    }

    /// A document whose tab `i` is titled `i` and is in group `groups_of[i]` (`None`: none).
    fn grouped_doc(active: usize, groups_of: &[Option<&str>], groups: &str) -> String {
        let tabs: Vec<String> = groups_of
            .iter()
            .enumerate()
            .map(|(i, g)| match g {
                Some(g) => format!(r#"{{"title":"{i}","group":{g}}}"#),
                None => format!(r#"{{"title":"{i}"}}"#),
            })
            .collect();
        format!(
            r#"{{"version":1,"active":{active},"tabs":[{}],"groups":{groups}}}"#,
            tabs.join(",")
        )
    }

    fn titles(ws: &Workspace) -> Vec<&str> {
        ws.tabs
            .iter()
            .map(|t| t.manual_title.as_deref().unwrap_or(""))
            .collect()
    }

    #[test]
    fn a_file_with_groups_is_read_and_written_back_identical() {
        let text = "{\"version\":1,\"active\":2,\"tabs\":[{\"title\":\"a\"},\
                    {\"group\":\"web é \\\"x\\\"\"},{\"group\":\"web é \\\"x\\\"\"},\
                    {\"title\":\"b\",\"group\":\"db\"},{}],\
                    \"groups\":[{\"name\":\"web é \\\"x\\\"\",\"color\":\"cyan\",\
                    \"collapsed\":true},{\"name\":\"db\",\"color\":\"grey\",\
                    \"collapsed\":false}],\"later\":[1]}\n";
        let ws = parse_workspace(text.as_bytes()).expect("read");
        assert_eq!(
            membership(&ws),
            [".", "web é \"x\"", "web é \"x\"", "db", "."]
        );
        assert_eq!(
            ws.groups,
            [
                group("web é \"x\"", GroupColor::Cyan, true),
                group("db", GroupColor::Grey, false)
            ]
        );
        assert_eq!(ws.active, 2);
        assert_eq!(render_workspace(&ws).expect("writes"), text);
    }

    #[test]
    fn a_name_that_fails_the_label_rule_is_no_group() {
        let long = format!("\"{}\"", "é".repeat(MAX_GROUP_CHARS + 1));
        let longest = format!("\"{}\"", "é".repeat(MAX_GROUP_CHARS));
        let bad = [
            "\"\"",
            "\" a\"",
            "\"a \"",
            "\"a\\u0007b\"",
            "\"a\\u202eb\"",
            "\"a\\u2028b\"",
            long.as_str(),
            "7",
            "null",
            "[\"a\"]",
        ];
        for name in bad {
            let entry = format!(r#"[{{"name":{name},"color":"red","collapsed":true}}]"#);
            let doc = grouped_doc(0, &[Some(name)], &entry);
            let ws = parse_workspace(doc.as_bytes()).expect("never refused");
            assert_eq!(ws.tabs.len(), 1, "{name}");
            assert_eq!((membership(&ws), ws.groups.len()), (vec!["."], 0), "{name}");
        }
        let doc = grouped_doc(0, &[Some(&longest)], "[]");
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert_eq!(ws.groups.len(), 1, "the longest name is one");
    }

    #[test]
    fn what_a_file_gets_wrong_about_its_groups_is_repaired() {
        use GroupColor::{Blue, Grey, Red, Yellow};
        // Twice the same name: the first entry. An entry no tab names, and things that are no
        // entry, are dropped.
        let doc = grouped_doc(
            0,
            &[Some("\"a\"")],
            r#"[7,"a",{"color":"pink"},{"name":"unused","color":"pink"},
                {"name":"a","color":"red","collapsed":true},
                {"name":"a","color":"blue","collapsed":false}]"#,
        );
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert_eq!(ws.groups, [group("a", Red, true)]);

        // An unknown, missing or wrong-typed colour, and no entry at all: the colour a new
        // group would get, past the ones the file does name (here `d`'s grey, read later).
        let doc = grouped_doc(
            0,
            &[Some("\"a\""), Some("\"b\""), Some("\"c\""), Some("\"d\"")],
            r#"[{"name":"a","color":"mauve","collapsed":"yes"},{"name":"b","color":3,
                "collapsed":1},{"name":"d","color":"grey"}]"#,
        );
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert_eq!(
            ws.groups,
            [
                group("a", Blue, false),
                group("b", Red, false),
                group("c", Yellow, false),
                group("d", Grey, false),
            ]
        );

        // `groups` of another type, or none: the tabs still say who is grouped. It is not kept
        // as an unknown member, which would be written twice.
        for groups in ["\"x\"", "{}", "null"] {
            let doc = grouped_doc(0, &[Some("\"a\"")], groups);
            let ws = parse_workspace(doc.as_bytes()).expect("read");
            assert_eq!(ws.groups, [group("a", Grey, false)], "{groups}");
            assert!(ws.unknown.is_empty(), "{groups}");
            let text = render_workspace(&ws).expect("writes");
            assert_eq!(text.matches("\"groups\"").count(), 1, "{text}");
        }

        // A group in the list and in no tab leaves a file with no `groups`.
        let doc = grouped_doc(0, &[None], r#"[{"name":"a","color":"red"}]"#);
        let ws = parse_workspace(doc.as_bytes()).expect("read");
        assert!(ws.groups.is_empty());
        assert!(!render_workspace(&ws).expect("writes").contains("groups"));
    }

    #[test]
    fn scattered_members_are_gathered_after_the_first_and_the_active_tab_follows() {
        let tabs = [
            None,
            Some("\"a\""),
            Some("\"b\""),
            None,
            Some("\"a\""),
            Some("\"b\""),
            Some("\"a\""),
        ];
        // The new place of each old index.
        for (active, moved) in [0, 1, 4, 6, 2, 5, 3].into_iter().enumerate() {
            let doc = grouped_doc(active, &tabs, "[]");
            let ws = parse_workspace(doc.as_bytes()).expect("read");
            assert_eq!(titles(&ws), ["0", "1", "4", "6", "2", "5", "3"]);
            assert_eq!(membership(&ws), [".", "a", "a", "a", "b", "b", "."]);
            assert_eq!(ws.active, moved, "old {active}");
            assert_eq!(
                ws.groups.iter().map(|g| &g.name).collect::<Vec<_>>(),
                ["a", "b"]
            );
            // Repaired once: reading what is written changes nothing.
            let text = render_workspace(&ws).expect("writes");
            assert_eq!(parse_workspace(text.as_bytes()), Ok(ws));
        }
    }

    #[test]
    fn at_most_32_grouped_tabs_are_read_and_written_with_only_their_groups() {
        let names: Vec<String> = (0..40).map(|i| format!("g{i}")).collect();
        let quoted: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
        let of: Vec<Option<&str>> = quoted.iter().map(|n| Some(n.as_str())).collect();
        let ws = parse_workspace(grouped_doc(39, &of, "[]").as_bytes()).expect("read");
        assert_eq!((ws.tabs.len(), ws.groups.len()), (MAX_TABS, MAX_TABS));
        assert_eq!(ws.active, MAX_TABS - 1);

        let many = Workspace {
            tabs: names
                .iter()
                .map(|n| TabState {
                    group: Some(n.clone()),
                    ..TabState::default()
                })
                .collect(),
            active: 39,
            groups: names
                .iter()
                .map(|n| group(n, GroupColor::Green, false))
                .collect(),
            unknown: Vec::new(),
        };
        let text = render_workspace(&many).expect("writes");
        let back = parse_workspace(text.as_bytes()).expect("read");
        assert_eq!((back.tabs.len(), back.groups.len()), (MAX_TABS, MAX_TABS));
        assert!(!text.contains("\"g32\""), "no group of a tab left out");
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
                group: None,
            }],
            active: 0,
            ..Workspace::default()
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
        let in_g = |dir: &str| TabState {
            group: Some("g".into()),
            ..tab(dir, None, None)
        };
        let valid = render_workspace(&Workspace {
            tabs: vec![
                tab("/a", Some("/b"), Some("t")),
                in_g("/c"),
                tab("/d", None, None),
                in_g("/e"),
            ],
            active: 1,
            groups: vec![group("g", GroupColor::Pink, true)],
            ..Workspace::default()
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
                    // One group per name the tabs have, in their order, each in one piece.
                    let mut runs: Vec<&str> = ws
                        .tabs
                        .chunk_by(|a, b| a.group == b.group)
                        .filter_map(|run| run[0].group.as_deref())
                        .collect();
                    let listed: Vec<&str> = ws.groups.iter().map(|g| g.name.as_str()).collect();
                    assert_eq!(runs, listed);
                    runs.sort_unstable();
                    runs.dedup();
                    assert_eq!(runs.len(), listed.len(), "a group in two pieces");
                    assert!(listed.iter().all(|n| label_ok(n, MAX_GROUP_CHARS)));
                }
                Err(_) => rejected += 1,
            }
        }
        assert!(ok > 0 && rejected > 0, "ok {ok}, rejected {rejected}");
    }
}
