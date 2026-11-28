//! The JSON object of each successful command, with exactly Protocol 1's keys, `null` for
//! nothing. Written by the app's own writer, so every string is escaped and a line that would not
//! read back is refused ([`WriteError`]: a value over the reader's limits). The line has no
//! newline: the client prints it.
//!
//! Strings go out as given. Bounding a title or a directory and removing control characters from
//! them is the caller's work.

use super::PROTOCOL;
use crate::persist::json_write::{Json, WriteError};
use crate::tabs::TabId;

/// One tab in `tab list`.
#[derive(Debug, Clone, Copy)]
pub struct TabEntry<'a> {
    pub id: TabId,
    /// Whether the tab was created through `ctl` (`origin` "control", else "user").
    pub by_control: bool,
    pub group: Option<&'a str>,
    pub title: &'a str,
    pub cwd: Option<&'a str>,
    pub active: bool,
    pub pid: Option<u32>,
    pub input_idle_ms: Option<u64>,
}

/// `status`.
pub fn status(version: &str, instance: &str, control_on: bool) -> Result<String, WriteError> {
    line(vec![
        ("protocol", Json::Int(PROTOCOL)),
        ("version", string(version)),
        ("instance", string(instance)),
        ("control", string(if control_on { "on" } else { "off" })),
    ])
}

/// `tab list`, the tabs in the order given.
pub fn tab_list(instance: &str, tabs: &[TabEntry<'_>]) -> Result<String, WriteError> {
    line(vec![
        ("instance", string(instance)),
        ("tabs", Json::Array(tabs.iter().map(entry).collect())),
    ])
}

/// `tab new`.
pub fn tab_new(id: TabId) -> Result<String, WriteError> {
    line(vec![("id", number(id))])
}

/// `tab close`.
pub fn tab_closed(id: TabId) -> Result<String, WriteError> {
    line(vec![("closed", number(id))])
}

/// `tab focus`.
pub fn tab_focused(id: TabId) -> Result<String, WriteError> {
    line(vec![("focused", number(id))])
}

/// `tab group` (the name) and `tab ungroup` (`None`).
pub fn tab_group(id: TabId, group: Option<&str>) -> Result<String, WriteError> {
    line(vec![("id", number(id)), ("group", or_null(group, string))])
}

fn entry(tab: &TabEntry<'_>) -> Json {
    object(vec![
        ("id", number(tab.id)),
        (
            "origin",
            string(if tab.by_control { "control" } else { "user" }),
        ),
        ("group", or_null(tab.group, string)),
        ("title", string(tab.title)),
        ("cwd", or_null(tab.cwd, string)),
        ("active", Json::Bool(tab.active)),
        ("pid", or_null(tab.pid, |pid| number(u64::from(pid)))),
        ("input_idle_ms", or_null(tab.input_idle_ms, number)),
    ])
}

fn string(s: &str) -> Json {
    Json::Str(s.to_owned())
}

/// A number past the writer's exact range (2^53) makes it refuse the line.
fn number(n: u64) -> Json {
    Json::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn or_null<T>(value: Option<T>, json: impl FnOnce(T) -> Json) -> Json {
    value.map_or(Json::Null, json)
}

fn object(members: Vec<(&str, Json)>) -> Json {
    Json::Object(
        members
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn line(members: Vec<(&str, Json)>) -> Result<String, WriteError> {
    let mut text = object(members).to_text()?;
    // The writer ends its text with one newline.
    text.truncate(text.trim_end_matches('\n').len());
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_has_protocol_1s_keys() {
        assert_eq!(
            status("0.8.0", "abcDEF12_-", true).as_deref(),
            Ok(r#"{"protocol":1,"version":"0.8.0","instance":"abcDEF12_-","control":"on"}"#)
        );
        assert_eq!(
            status("0.8.0", "abcDEF12_-", false).as_deref(),
            Ok(r#"{"protocol":1,"version":"0.8.0","instance":"abcDEF12_-","control":"off"}"#)
        );
    }

    #[test]
    fn a_tab_list_has_every_key_and_null_for_nothing() {
        let tabs = [
            TabEntry {
                id: 7,
                by_control: true,
                group: Some("orch: x"),
                title: "repo",
                cwd: Some("/tmp/repo"),
                active: true,
                pid: Some(123),
                input_idle_ms: Some(4200),
            },
            TabEntry {
                id: 8,
                by_control: false,
                group: None,
                title: "",
                cwd: None,
                active: false,
                pid: None,
                input_idle_ms: None,
            },
        ];
        assert_eq!(
            tab_list("abcdefgh", &tabs).as_deref(),
            Ok(concat!(
                r#"{"instance":"abcdefgh","tabs":["#,
                r#"{"id":7,"origin":"control","group":"orch: x","title":"repo","#,
                r#""cwd":"/tmp/repo","active":true,"pid":123,"input_idle_ms":4200},"#,
                r#"{"id":8,"origin":"user","group":null,"title":"","#,
                r#""cwd":null,"active":false,"pid":null,"input_idle_ms":null}]}"#
            ))
        );
        assert_eq!(
            tab_list("abcdefgh", &[]).as_deref(),
            Ok(r#"{"instance":"abcdefgh","tabs":[]}"#)
        );
    }

    #[test]
    fn the_short_replies() {
        assert_eq!(tab_new(7).as_deref(), Ok(r#"{"id":7}"#));
        assert_eq!(tab_closed(7).as_deref(), Ok(r#"{"closed":7}"#));
        assert_eq!(tab_focused(0).as_deref(), Ok(r#"{"focused":0}"#));
        assert_eq!(
            tab_group(7, Some("a b")).as_deref(),
            Ok(r#"{"id":7,"group":"a b"}"#)
        );
        assert_eq!(
            tab_group(7, None).as_deref(),
            Ok(r#"{"id":7,"group":null}"#)
        );
    }

    #[test]
    fn a_reply_is_one_line_whatever_the_strings_hold() {
        let nasty = "a\n\r\"}\u{1b}\u{2028}\u{2029}\u{85}b";
        let line = tab_group(1, Some(nasty)).expect("writes");
        assert_eq!(
            line,
            r#"{"id":1,"group":"a\n\r\"}\u001b\u2028\u2029\u0085b"}"#
        );
    }

    #[test]
    fn a_value_the_reader_could_not_hold_is_refused() {
        assert!(tab_new(1 << 53).is_ok());
        assert_eq!(tab_new((1 << 53) + 1), Err(WriteError::Number));
        assert_eq!(tab_new(u64::MAX), Err(WriteError::Number));
        let long = "a".repeat(4097);
        assert_eq!(tab_group(1, Some(&long)), Err(WriteError::OverLimit));
    }
}
