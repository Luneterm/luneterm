//! The words after `ctl`: Protocol 1's grammar and limits. The client parses them before it
//! connects and the app parses them again, with this same code.

use std::ffi::OsString;
use std::fmt;

use super::Request;
use crate::tabs::TabId;
use crate::title::is_bidi_control;

/// The most words after `--`, the program included.
pub const MAX_PROGRAM_WORDS: usize = 256;
/// The most bytes in the words after `--`, summed.
pub const MAX_PROGRAM_BYTES: usize = 64 * 1024;
/// The longest group name, in Unicode scalar values.
pub const MAX_GROUP_CHARS: usize = 64;
/// The longest `--title`, in Unicode scalar values.
pub const MAX_TITLE_CHARS: usize = 128;

/// The words `tab new` knows: none of them is taken as the value of an option.
const NEW_WORDS: [&str; 5] = ["--cwd", "--group", "--title", "--focus", "--"];

/// Why the words are no request (exit code 2). The text is fixed: it never repeats what the
/// caller sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage(pub &'static str);

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

const TWICE: Usage = Usage("an option is given twice");
const MISSING: Usage = Usage("an option has no value");

/// The OS's arguments as text; one that is not valid Unicode is a usage error.
pub fn words(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, Usage> {
    args.into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| Usage("an argument is not valid Unicode"))
        })
        .collect()
}

/// The request the words after `ctl` spell, or why they spell none. Nothing is touched on disk:
/// `--cwd` is checked for its form only ([`crate::cwd::syntax`]: absolute, no control character,
/// no network or device form, at most 4096 bytes).
pub fn parse(words: &[String]) -> Result<Request, Usage> {
    if words.iter().any(|w| w.contains('\0')) {
        return Err(Usage("an argument contains a NUL"));
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["status"] => Ok(Request::Status),
        ["tab", "list"] => Ok(Request::TabList),
        ["tab", "new", rest @ ..] => tab_new(rest),
        ["tab", "close", rest @ ..] => tab_close(rest),
        ["tab", "focus", id] => Ok(Request::TabFocus(tab_id(id)?)),
        ["tab", "group", id, name] => Ok(Request::TabGroup(
            tab_id(id)?,
            label(name, MAX_GROUP_CHARS)?,
        )),
        ["tab", "ungroup", id] => Ok(Request::TabUngroup(tab_id(id)?)),
        _ => Err(Usage("unknown command, or the wrong number of arguments")),
    }
}

/// The words after `tab new`.
fn tab_new(mut rest: &[&str]) -> Result<Request, Usage> {
    let (mut cwd, mut group, mut title) = (None, None, None);
    let mut focus = false;
    let mut argv = Vec::new();
    while let [option, tail @ ..] = rest {
        rest = tail;
        let slot = match *option {
            "--cwd" => &mut cwd,
            "--group" => &mut group,
            "--title" => &mut title,
            "--focus" if focus => return Err(TWICE),
            "--focus" => {
                focus = true;
                continue;
            }
            "--" => {
                argv = program(rest)?;
                break;
            }
            _ => return Err(Usage("unknown argument to `tab new`")),
        };
        if slot.is_some() {
            return Err(TWICE);
        }
        let [value, tail @ ..] = rest else {
            return Err(MISSING);
        };
        if NEW_WORDS.contains(value) {
            return Err(MISSING);
        }
        *slot = Some(*value);
        rest = tail;
    }
    let cwd = cwd.ok_or(Usage("`tab new` needs --cwd <dir>"))?;
    let cwd = crate::cwd::syntax(cwd).ok_or(Usage("--cwd is not an absolute local path"))?;
    Ok(Request::TabNew {
        cwd,
        group: group.map(|g| label(g, MAX_GROUP_CHARS)).transpose()?,
        title: title.map(|t| label(t, MAX_TITLE_CHARS)).transpose()?,
        focus,
        argv,
    })
}

/// The words after `--`.
fn program(words: &[&str]) -> Result<Vec<String>, Usage> {
    if words.first().is_none_or(|program| program.is_empty()) {
        return Err(Usage("`--` needs a program"));
    }
    if words.len() > MAX_PROGRAM_WORDS {
        return Err(Usage("more than 256 arguments after `--`"));
    }
    if words.iter().map(|w| w.len()).sum::<usize>() > MAX_PROGRAM_BYTES {
        return Err(Usage("more than 64 KiB of arguments after `--`"));
    }
    Ok(words.iter().map(|w| (*w).to_owned()).collect())
}

/// The words after `tab close`: the id, with `--any-origin` before or after it, or without.
fn tab_close(rest: &[&str]) -> Result<Request, Usage> {
    let (id, any_origin) = match rest {
        [id] => (id, false),
        ["--any-origin", id] | [id, "--any-origin"] => (id, true),
        _ => return Err(Usage("`tab close` takes <id> [--any-origin]")),
    };
    Ok(Request::TabClose {
        id: tab_id(id)?,
        any_origin,
    })
}

/// `[0-9]+` with no leading zero (but "0" itself), at most `u64::MAX`.
fn tab_id(word: &str) -> Result<TabId, Usage> {
    let canonical =
        word.bytes().all(|b| b.is_ascii_digit()) && (word == "0" || !word.starts_with('0'));
    canonical
        .then(|| word.parse().ok())
        .flatten()
        .ok_or(Usage("a tab id is a plain decimal number"))
}

fn label(word: &str, max: usize) -> Result<String, Usage> {
    label_ok(word, max).then(|| word.to_owned()).ok_or(Usage(
        "a name is empty, too long, has an edge space or has a control character",
    ))
}

/// Whether `s` may name a group or a tab: 1 to `max` Unicode scalar values; no control character
/// (Cc), no bidi control, no line or paragraph separator (U+2028, U+2029); no whitespace at
/// either end.
pub fn label_ok(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.chars().nth(max).is_none()
        && s.trim() == s
        && !s
            .chars()
            .any(|c| c.is_control() || is_bidi_control(c) || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// Whether `s` has the form of an instance token: 8 to 64 of `A-Z a-z 0-9 _ -`.
pub fn token_ok(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// An absolute path on the OS the test runs on.
    fn absolute() -> &'static str {
        if cfg!(windows) { r"C:\tmp" } else { "/tmp" }
    }

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    fn parsed(words: &[&str]) -> Result<Request, Usage> {
        parse(&owned(words))
    }

    /// `tab new --cwd <abs>` and then `more`.
    fn new_with(more: &[&str]) -> Result<Request, Usage> {
        let mut words = vec!["tab", "new", "--cwd", absolute()];
        words.extend_from_slice(more);
        parsed(&words)
    }

    fn plain_new() -> Request {
        Request::TabNew {
            cwd: PathBuf::from(absolute()),
            group: None,
            title: None,
            focus: false,
            argv: Vec::new(),
        }
    }

    #[test]
    fn every_command_parses() {
        for (words, request) in [
            (&["status"][..], Request::Status),
            (&["tab", "list"], Request::TabList),
            (&["tab", "focus", "7"], Request::TabFocus(7)),
            (&["tab", "focus", "0"], Request::TabFocus(0)),
            (&["tab", "ungroup", "10"], Request::TabUngroup(10)),
            (
                &["tab", "group", "3", "orch: a b"],
                Request::TabGroup(3, "orch: a b".to_owned()),
            ),
            // A name is free text, whatever it looks like.
            (
                &["tab", "group", "3", "--any-origin"],
                Request::TabGroup(3, "--any-origin".to_owned()),
            ),
            (
                &["tab", "close", "3"],
                Request::TabClose {
                    id: 3,
                    any_origin: false,
                },
            ),
            (
                &["tab", "close", "3", "--any-origin"],
                Request::TabClose {
                    id: 3,
                    any_origin: true,
                },
            ),
            (
                &["tab", "close", "--any-origin", "3"],
                Request::TabClose {
                    id: 3,
                    any_origin: true,
                },
            ),
        ] {
            assert_eq!(parsed(words), Ok(request), "{words:?}");
        }
        assert_eq!(new_with(&[]), Ok(plain_new()));
    }

    #[test]
    fn tab_new_takes_its_options_in_any_order() {
        let options: [&[&str]; 4] = [
            &["--cwd", absolute()],
            &["--group", "orch: luneterm"],
            &["--title", "--my title"],
            &["--focus"],
        ];
        let want = Request::TabNew {
            cwd: PathBuf::from(absolute()),
            group: Some("orch: luneterm".to_owned()),
            title: Some("--my title".to_owned()),
            focus: true,
            argv: owned(&["claude", "--focus", "--", "", "two words", "--cwd"]),
        };
        let mut orders = 0;
        for a in 0..4 {
            for b in (0..4).filter(|&b| b != a) {
                for c in (0..4).filter(|&c| c != a && c != b) {
                    let d = 6 - a - b - c;
                    let mut words = vec!["tab", "new"];
                    for i in [a, b, c, d] {
                        words.extend_from_slice(options[i]);
                    }
                    // Everything after `--` is the program's, option look-alikes included.
                    words.extend_from_slice(&[
                        "--",
                        "claude",
                        "--focus",
                        "--",
                        "",
                        "two words",
                        "--cwd",
                    ]);
                    assert_eq!(parsed(&words), Ok(want.clone()), "{words:?}");
                    orders += 1;
                }
            }
        }
        assert_eq!(orders, 24);
    }

    #[test]
    fn every_grammar_error_is_a_usage_error() {
        let cwd = absolute();
        let errors: &[&[&str]] = &[
            &[],
            &[""],
            &["ctl", "status"],
            &["Status"],
            &["status", "extra"],
            &["status", "--json"],
            &["status", "--"],
            &["tab"],
            &["tab", "lists"],
            &["tab", "list", "extra"],
            &["tab", "list", "--all"],
            &["window", "list"],
            &["--", "status"],
            // tab new
            &["tab", "new"],
            &["tab", "new", "--focus"],
            &["tab", "new", "--", "sh"],
            &["tab", "new", "--cwd"],
            &["tab", "new", "--cwd", "--focus"],
            &["tab", "new", "--cwd", "--", "sh"],
            &["tab", "new", "--cwd=/tmp"],
            &["tab", "new", cwd],
            &["tab", "new", "--cwd", cwd, "extra"],
            &["tab", "new", "--cwd", cwd, "--cwd", cwd],
            &["tab", "new", "--cwd", cwd, "--group", "a", "--group", "a"],
            &["tab", "new", "--cwd", cwd, "--title", "a", "--title", "b"],
            &["tab", "new", "--cwd", cwd, "--focus", "--focus"],
            &["tab", "new", "--cwd", cwd, "--focus", "yes"],
            &["tab", "new", "--cwd", cwd, "--focus=true"],
            &["tab", "new", "--cwd", cwd, "--group"],
            &["tab", "new", "--cwd", cwd, "--title"],
            &["tab", "new", "--cwd", cwd, "--group", "--focus"],
            &["tab", "new", "--cwd", cwd, "--title", "--cwd"],
            &["tab", "new", "--cwd", cwd, "--title", "--group", "a"],
            &["tab", "new", "--cwd", cwd, "--group", "--", "sh"],
            &["tab", "new", "--cwd", cwd, "--group=a"],
            &["tab", "new", "--cwd", cwd, "--env", "A=1"],
            &["tab", "new", "--cwd", cwd, "--any-origin"],
            &["tab", "new", "--cwd", cwd, "--"],
            &["tab", "new", "--cwd", cwd, "--", ""],
            &["tab", "new", "--cwd", cwd, "--group", ""],
            &["tab", "new", "--cwd", cwd, "--title", " a"],
            // tab close
            &["tab", "close"],
            &["tab", "close", "--any-origin"],
            &["tab", "close", "--any-origin", "--any-origin"],
            &["tab", "close", "3", "--any-origin", "--any-origin"],
            &["tab", "close", "--any-origin", "3", "--any-origin"],
            &["tab", "close", "--any-origin", "--any-origin", "3"],
            &["tab", "close", "3", "--force"],
            &["tab", "close", "--force", "3"],
            &["tab", "close", "3", "--any-origin", "--force"],
            &["tab", "close", "3", "--any-origin=true"],
            &["tab", "close", "3", "4"],
            &["tab", "close", "3", "--"],
            // tab focus, group, ungroup
            &["tab", "focus"],
            &["tab", "focus", "1", "2"],
            &["tab", "focus", "1", "--any-origin"],
            &["tab", "focus", "--any-origin"],
            &["tab", "group"],
            &["tab", "group", "1"],
            &["tab", "group", "1", "a", "b"],
            &["tab", "group", "a", "1"],
            &["tab", "group", "1", ""],
            &["tab", "ungroup"],
            &["tab", "ungroup", "1", "a"],
        ];
        for words in errors {
            assert!(parsed(words).is_err(), "{words:?}");
        }
    }

    #[test]
    fn a_nul_in_any_word_is_a_usage_error() {
        let cwd = absolute();
        let bad_cwd = format!("{cwd}\0x");
        for words in [
            &["status\0"][..],
            &["tab", "focus", "1\0"],
            &["tab", "group", "1", "a\0b"],
            &["tab", "new", "--cwd", &bad_cwd],
            &["tab", "new", "--cwd", cwd, "--title", "a\0b"],
            &["tab", "new", "--cwd", cwd, "--", "sh\0"],
            &["tab", "new", "--cwd", cwd, "--", "sh", "-c", "a\0b"],
        ] {
            assert_eq!(
                parsed(words),
                Err(Usage("an argument contains a NUL")),
                "{words:?}"
            );
        }
    }

    #[test]
    fn a_tab_id_is_a_plain_decimal_that_fits_u64() {
        let max = u64::MAX.to_string();
        assert_eq!(max, "18446744073709551615");
        assert_eq!(
            parsed(&["tab", "focus", &max]),
            Ok(Request::TabFocus(u64::MAX))
        );
        for id in [
            "18446744073709551616",
            "99999999999999999999999999999999",
            "00",
            "03",
            "+3",
            "-3",
            "-0",
            " 3",
            "3 ",
            "3.0",
            "0x3",
            "3e2",
            "",
            "three",
            // Digits to Unicode, not to the grammar.
            "\u{0663}",
            "\u{ff13}",
        ] {
            for words in [
                &["tab", "focus", id][..],
                &["tab", "close", id],
                &["tab", "close", id, "--any-origin"],
                &["tab", "close", "--any-origin", id],
                &["tab", "group", id, "a"],
                &["tab", "ungroup", id],
            ] {
                assert!(parsed(words).is_err(), "{words:?}");
            }
        }
    }

    #[test]
    fn cwd_must_be_an_absolute_local_path() {
        let longest = format!("{}{}", absolute(), "a".repeat(4096 - absolute().len()));
        assert_eq!(longest.len(), 4096);
        assert!(parsed(&["tab", "new", "--cwd", &longest]).is_ok());
        let over = format!("{longest}a");
        let control = format!("{}\n", absolute());
        for cwd in [
            "",
            ".",
            "..",
            "~",
            "tmp",
            "./tmp",
            "//host/share",
            r"\\host\share",
            r"\\?\C:\tmp",
            &over,
            &control,
        ] {
            assert!(parsed(&["tab", "new", "--cwd", cwd]).is_err(), "{cwd:?}");
        }
    }

    #[test]
    fn the_words_after_the_separator_are_capped_at_256_and_64_kib() {
        let with_program = |argv: &[String]| {
            let mut words = owned(&["tab", "new", "--cwd", absolute(), "--"]);
            words.extend_from_slice(argv);
            parse(&words)
        };
        let argv_of = |r: Result<Request, Usage>| match r {
            Ok(Request::TabNew { argv, .. }) => argv,
            other => panic!("not a new tab: {}", other.is_ok()),
        };
        // 256 words, the program included.
        let mut argv = vec!["a".to_owned(); 256];
        assert_eq!(argv_of(with_program(&argv)), argv);
        argv.push("a".to_owned());
        assert!(with_program(&argv).is_err());
        // 65,536 bytes, in one word.
        let argv = vec!["a".repeat(65_536)];
        assert_eq!(argv_of(with_program(&argv)), argv);
        assert!(with_program(&["a".repeat(65_537)]).is_err());
        // The same cap summed over words, in bytes and not in chars ("é" is two).
        let mut argv = vec!["sh".to_owned(), "é".repeat(32_000), "a".repeat(1_534)];
        assert_eq!(argv.iter().map(String::len).sum::<usize>(), 65_536);
        assert_eq!(argv_of(with_program(&argv)), argv);
        argv.push("a".to_owned());
        assert!(with_program(&argv).is_err());
        // Empty arguments count as words.
        let mut argv = vec![String::new(); 256];
        argv[0] = "sh".to_owned();
        assert!(with_program(&argv).is_ok());
        argv.push(String::new());
        assert!(with_program(&argv).is_err());
        // The options before `--` do not count.
        let mut words = owned(&[
            "tab",
            "new",
            "--cwd",
            absolute(),
            "--group",
            "g",
            "--title",
            "t",
            "--focus",
            "--",
        ]);
        words.extend(vec!["a".to_owned(); 256]);
        assert!(parse(&words).is_ok());
    }

    #[test]
    fn names_are_capped_in_scalar_values_and_never_cut() {
        // One, two, three and four bytes a char.
        for c in ["a", "é", "日", "🎉"] {
            let group = c.repeat(64);
            assert_eq!(
                parsed(&["tab", "group", "1", &group]),
                Ok(Request::TabGroup(1, group.clone()))
            );
            let title = c.repeat(128);
            assert_eq!(
                new_with(&["--group", &group, "--title", &title]),
                Ok(Request::TabNew {
                    cwd: PathBuf::from(absolute()),
                    group: Some(group),
                    title: Some(title),
                    focus: false,
                    argv: Vec::new(),
                })
            );
            assert!(parsed(&["tab", "group", "1", &c.repeat(65)]).is_err());
            assert!(new_with(&["--group", &c.repeat(65)]).is_err());
            assert!(new_with(&["--title", &c.repeat(129)]).is_err());
        }
        assert!(new_with(&["--group", "a", "--title", "b"]).is_ok());
    }

    #[test]
    fn label_ok_counts_from_one_to_max() {
        assert!(!label_ok("", 64));
        assert!(label_ok("a", 64));
        assert!(label_ok(&"é".repeat(64), 64));
        assert!(!label_ok(&"é".repeat(65), 64));
        assert!(!label_ok("a", 0));
    }

    #[test]
    fn label_ok_refuses_controls_bidi_controls_separators_and_edge_whitespace() {
        let controls = (0u32..=0x1f).chain(0x7f..=0x9f);
        let bidi = [0x061c, 0x200e, 0x200f]
            .into_iter()
            .chain(0x202a..=0x202e)
            .chain(0x2066..=0x2069);
        let mut refused = 0;
        for c in controls
            .chain(bidi)
            .chain([0x2028, 0x2029])
            .filter_map(char::from_u32)
        {
            for s in [
                format!("a{c}b"),
                format!("{c}ab"),
                format!("ab{c}"),
                c.into(),
            ] {
                assert!(!label_ok(&s, 64), "U+{:04X}", u32::from(c));
            }
            refused += 1;
        }
        assert_eq!(refused, 65 + 12 + 2);
        for c in [' ', '\u{a0}', '\u{2003}', '\u{202f}', '\u{3000}'] {
            assert!(!label_ok(&format!("{c}a"), 64), "leading {c:?}");
            assert!(!label_ok(&format!("a{c}"), 64), "trailing {c:?}");
            assert!(!label_ok(&c.to_string(), 64), "alone {c:?}");
        }
        for s in [
            "a b",
            "a  b",
            "a\u{a0}b",
            "orch: project",
            "日本語 🎉",
            "\u{1f468}\u{200d}\u{1f469}",
            "a\u{2065}b\u{206a}c",
            "\"quoted\" \\ $(x)",
        ] {
            assert!(label_ok(s, 64), "{s:?}");
        }
    }

    #[test]
    fn a_token_is_8_to_64_url_safe_chars() {
        for ok in ["abcdefgh".to_owned(), "AZaz09_-".to_owned(), "a".repeat(64)] {
            assert!(token_ok(&ok), "{ok:?}");
        }
        for bad in [
            String::new(),
            "abcdefg".to_owned(),
            "a".repeat(65),
            "abcdefg.".to_owned(),
            "abcdefg/".to_owned(),
            r"abcdefg\".to_owned(),
            "abcdefg ".to_owned(),
            "abcdefg=".to_owned(),
            "abcdefg+".to_owned(),
            "abcdefg\0".to_owned(),
            "../../abc".to_owned(),
            "abcdefgé".to_owned(),
        ] {
            assert!(!token_ok(&bad), "{bad:?}");
        }
    }

    #[test]
    fn an_argument_that_is_not_unicode_is_a_usage_error() {
        #[cfg(unix)]
        let broken: OsString = std::os::unix::ffi::OsStringExt::from_vec(vec![b'a', 0xff]);
        #[cfg(windows)]
        let broken: OsString = std::os::windows::ffi::OsStringExt::from_wide(&[0x61, 0xd800]);
        assert!(words([OsString::from("tab"), broken]).is_err());
        assert_eq!(
            words([OsString::from("tab"), OsString::from("日本")]),
            Ok(owned(&["tab", "日本"]))
        );
    }
}
