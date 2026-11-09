//! Opening a link (plan M10 step 4, D4): the check, and the launch.
//!
//! The text of a link comes from the PTY (an OSC 8 URI, or text printed on the screen), so any
//! program running in the session picks it. Opening it hands a string to the OS (`open`,
//! `xdg-open`, `ShellExecuteW`), whose parsers this program cannot read. So the check here is a
//! **closed grammar**: it accepts only forms every parser reads the same way, rejects everything
//! else, and never repairs (no trimming, no decoding, no re-encoding). It runs at open, whatever
//! produced the string ([`check_url`]), and the launch takes exactly the checked string, as one
//! argument, with no shell and no format string.
//!
//! The grammar:
//!  - the scheme is `http://` or `https://`, ASCII case-insensitive;
//!  - at most [`MAX_URL_BYTES`] bytes;
//!  - no ASCII control character, no space, no `\` anywhere (`http:\\evil` is one parser's
//!    `http://evil`);
//!  - no `"`, `<`, `>`, `` ` ``, `^`, `|`, `{` or `}` (not URL characters; `"` also breaks out of the
//!    quotes a Windows protocol handler puts around `%1`);
//!  - no Unicode control or format character (C1 controls, bidi controls, zero-width marks,
//!    line separators, tags) anywhere: they spoof what the user thinks they open;
//!  - the authority (between `//` and the first `/`, `?` or `#`) is ASCII only, has no userinfo
//!    (`@`: `http://trusted.example@evil.example` reads two ways), and is a host with an optional
//!    port: a registered name of letters, digits, `.`, `-` and `_` that does not begin with `-`,
//!    or an IPv6 literal in brackets; the port is 1 to 5 digits;
//!  - the host is not empty.
//!
//! Stricter than Lunegit's rule (.NET `Uri.TryCreate`, absolute, http or https), on purpose: a
//! URL this rejects is one the user can copy and open by hand.

use std::io;
use std::process::{Command, Stdio};
use std::thread;

use crate::spawn::Os;

/// The longest URL opened, in bytes (the engine publishes no OSC 8 URI over the same limit).
pub const MAX_URL_BYTES: usize = 2048;

/// Why a URL is not opened. Never carries the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    Empty,
    TooLong,
    /// Not `http://` or `https://`.
    Scheme,
    /// An ASCII control character, a space, or a backslash.
    Forbidden,
    /// A character no URL has: `"`, `<`, `>`, `` ` ``, `^`, `|`, `{`, `}`.
    NotUrl,
    /// A Unicode control or format character.
    Format,
    /// The authority has non-ASCII text, userinfo, a bad host or a bad port.
    Authority,
    EmptyHost,
}

/// Why opening failed. Never carries the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    Rejected(Reject),
    /// The OS handler could not be started.
    Failed(io::ErrorKind),
}

impl OpenError {
    /// What the user is told (no URL).
    pub fn message(self) -> &'static str {
        match self {
            OpenError::Rejected(_) => {
                "That link cannot be opened: only plain http and https addresses are."
            }
            OpenError::Failed(_) => "The link could not be opened.",
        }
    }
}

/// Unicode characters that change how text reads or hide what it says.
fn is_format_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x80..=0x9F
            | 0xAD
            | 0x61C
            | 0x200B..=0x200F
            | 0x2028..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0xE0000..=0xE007F
    )
}

fn host_ok(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty()
            && inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
    }
    !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Splits an authority into host and port text: a bracketed literal ends at `]`, any other host
/// at the first `:`.
fn split_authority(authority: &str) -> (&str, Option<&str>) {
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => match authority[end + 1..].strip_prefix(':') {
                Some(port) => (&authority[..=end], Some(port)),
                None if authority.len() == end + 1 => (authority, None),
                // Something other than a port after the bracket.
                None => (authority, Some("x")),
            },
            None => (authority, Some("x")),
        };
    }
    match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

/// Whether `url` may be opened. See the module docs for the grammar.
pub fn check_url(url: &str) -> Result<(), Reject> {
    if url.is_empty() {
        return Err(Reject::Empty);
    }
    if url.len() > MAX_URL_BYTES {
        return Err(Reject::TooLong);
    }
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            let head = url.get(..scheme.len())?;
            head.eq_ignore_ascii_case(scheme)
                .then(|| &url[scheme.len()..])
        })
        .ok_or(Reject::Scheme)?;
    for c in url.chars() {
        if c.is_ascii_control() || c == ' ' || c == '\\' {
            return Err(Reject::Forbidden);
        }
        if matches!(c, '"' | '<' | '>' | '`' | '^' | '|' | '{' | '}') {
            return Err(Reject::NotUrl);
        }
        if is_format_char(c) {
            return Err(Reject::Format);
        }
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if !authority.is_ascii() || authority.contains('@') {
        return Err(Reject::Authority);
    }
    let (host, port) = split_authority(authority);
    if host.is_empty() {
        return Err(Reject::EmptyHost);
    }
    if !host_ok(host) {
        return Err(Reject::Authority);
    }
    if let Some(port) = port
        && (port.is_empty()
            || port.len() > 5
            || !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u32>().map_or(true, |p| p > 65535))
    {
        return Err(Reject::Authority);
    }
    Ok(())
}

/// What starts a process: a program and its arguments, one argument per element.
pub trait Launch {
    fn launch(&self, program: &str, args: &[&str]) -> io::Result<()>;
}

/// The real launcher: no shell, the standard streams closed, the child reaped on a thread so it
/// never lingers as a zombie and the UI never waits for it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Process;

impl Launch for Process {
    fn launch(&self, program: &str, args: &[&str]) -> io::Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // A failure to start the reaper only delays the reap to process exit.
        let _ = thread::Builder::new()
            .name("luneterm-open".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }
}

/// The program that opens a URL on `os`, `None` where the OS has an API for it instead
/// (Windows: `ShellExecuteW`).
pub fn opener_program(os: Os) -> Option<&'static str> {
    match os {
        // The absolute path: not looked up on `PATH`.
        Os::Macos => Some("/usr/bin/open"),
        Os::Linux => Some("xdg-open"),
        Os::Windows => None,
    }
}

/// Opens `url` on `os` through `launch`: checked first, then handed over as one argument.
/// Windows goes through [`windows::shell_open`], so `launch` is not used there.
pub fn open_url_on(os: Os, launch: &dyn Launch, url: &str) -> Result<(), OpenError> {
    check_url(url).map_err(OpenError::Rejected)?;
    match opener_program(os) {
        Some(program) => launch
            .launch(program, &[url])
            .map_err(|e| OpenError::Failed(e.kind())),
        None => shell_open(url).map_err(|e| OpenError::Failed(e.kind())),
    }
}

/// Opens `url` on this OS with the real launcher.
pub fn open_url(url: &str) -> Result<(), OpenError> {
    open_url_on(Os::HOST, &Process, url)
}

#[cfg(windows)]
fn shell_open(url: &str) -> io::Result<()> {
    windows::shell_open(url)
}

#[cfg(not(windows))]
fn shell_open(_url: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "ShellExecuteW is Windows only",
    ))
}

/// `ShellExecuteW` with the "open" verb, the platform module of the app (`unsafe` lives here and
/// in the other platform modules only).
#[cfg(windows)]
mod windows {
    #![allow(unsafe_code)]

    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    pub(super) fn shell_open(url: &str) -> io::Result<()> {
        let wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };
        let verb = wide("open");
        let file = wide(url);
        // SAFETY: `verb` and `file` are NUL-terminated UTF-16 buffers that live until the call
        // returns; the other pointer arguments are documented as optional and are null; the
        // window handle is null (no owner). The URL is already checked (`check_url`): no control
        // character, no quote, no backslash.
        let result = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                ptr::null(),
                ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // The documented success is a value greater than 32.
        if result as usize > 32 {
            Ok(())
        } else {
            Err(io::Error::other("ShellExecuteW failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        fail: bool,
    }

    impl Launch for Recorder {
        fn launch(&self, program: &str, args: &[&str]) -> io::Result<()> {
            self.calls.borrow_mut().push((
                program.to_owned(),
                args.iter().map(|a| (*a).to_owned()).collect(),
            ));
            if self.fail {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                Ok(())
            }
        }
    }

    fn accepted(url: &str) {
        assert_eq!(check_url(url), Ok(()), "{url:?} should open");
    }

    fn rejected(url: &str, why: Reject) {
        assert_eq!(check_url(url), Err(why), "{url:?}");
    }

    #[test]
    fn plain_http_and_https_addresses_open() {
        for url in [
            "http://x",
            "https://example.com",
            "HTTP://x",
            "HtTpS://Example.COM/Path",
            "https://example.com/a/b?c=d&e=f#g",
            "https://example.com:8080/",
            "http://localhost:3000",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://[2001:db8::1]:8443/x",
            "https://en.wikipedia.org/wiki/Foo_(bar)",
            "https://medium.com/@user/post",
            "https://example.com/a%20b",
            "https://example.com/?q=it's",
            "https://my_host.example/",
            // A non-ASCII path is the user's to read: only controls and format characters out.
            "https://ja.wikipedia.org/wiki/日本",
        ] {
            accepted(url);
        }
    }

    #[test]
    fn the_scheme_is_http_or_https_and_nothing_else() {
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "mailto:a@b.com",
            "ftp://x/",
            "ssh://x/",
            "x-apple.systempreferences:",
            "/relative",
            "example.com",
            "//example.com",
            "http:/example.com",
            "http:example.com",
            "https//example.com",
            "xhttp://example.com",
            " http://x",
        ] {
            assert!(check_url(url).is_err(), "{url:?}");
        }
        rejected("javascript:alert(1)", Reject::Scheme);
        rejected("file:///etc/passwd", Reject::Scheme);
        rejected("", Reject::Empty);
        // A leading dash is an option to the opener, not a URL.
        rejected("-a Calculator", Reject::Scheme);
        rejected("--help", Reject::Scheme);
        rejected("-https://x", Reject::Scheme);
    }

    #[test]
    fn a_backslash_makes_it_a_different_url_to_a_different_parser() {
        rejected(r"http:\\evil", Reject::Scheme);
        rejected(r"http://evil\@good.example", Reject::Forbidden);
        rejected(r"http://good.example\.evil.example", Reject::Forbidden);
        rejected(r"https://example.com/a\b", Reject::Forbidden);
        rejected(r"http:\/evil", Reject::Scheme);
    }

    #[test]
    fn controls_spaces_and_quotes_are_out_wherever_they_are() {
        for bad in ["\t", "\n", "\r", "\0", "\x1b", "\x7f", " "] {
            rejected(&format!("http://x{bad}y"), Reject::Forbidden);
            rejected(&format!("http://x/{bad}"), Reject::Forbidden);
            rejected(&format!("http://x/?{bad}"), Reject::Forbidden);
            rejected(&format!("http://{bad}x"), Reject::Forbidden);
        }
        for bad in ['"', '<', '>', '`', '^', '|', '{', '}'] {
            rejected(&format!("http://x/{bad}"), Reject::NotUrl);
            rejected(&format!("http://x/a?b={bad}c"), Reject::NotUrl);
        }
        // A Windows handler runs `"browser" "%1"`: a quote in the URL would close the quoting.
        rejected("https://x/\" --new-window \"y", Reject::NotUrl);
    }

    #[test]
    fn unicode_that_spoofs_or_hides_is_out_everywhere() {
        // Right-to-left override, isolates, zero-width space and joiner, soft hyphen, BOM, a C1
        // control, a line separator, a tag character.
        for bad in [
            "\u{202e}",
            "\u{2066}",
            "\u{2069}",
            "\u{200b}",
            "\u{200d}",
            "\u{ad}",
            "\u{feff}",
            "\u{85}",
            "\u{2028}",
            "\u{e0041}",
        ] {
            rejected(&format!("https://example.com/{bad}gpj.exe"), Reject::Format);
            rejected(&format!("https://example.com/?{bad}"), Reject::Format);
            rejected(&format!("https://ex{bad}ample.com/"), Reject::Format);
        }
    }

    #[test]
    fn the_authority_is_ascii_without_userinfo_and_a_real_host_and_port() {
        // Userinfo: the host a person reads is not the host a client uses.
        rejected("http://user@host", Reject::Authority);
        rejected("http://trusted.example@evil.example/", Reject::Authority);
        rejected("http://user:pass@host/", Reject::Authority);
        rejected("http://@host/", Reject::Authority);
        // The `@` of a path is only a path.
        accepted("http://host/@user");
        accepted("http://host/?a=b@c");
        accepted("http://host/#@x");
        // Non-ASCII hosts (homoglyphs) and encoded hosts.
        rejected("http://例え.jp/", Reject::Authority);
        rejected("http://exаmple.com/", Reject::Authority); // Cyrillic a
        rejected("http://%65vil.example/", Reject::Authority);
        rejected("http://evil.example%2f@good/", Reject::Authority);
        // No host.
        rejected("http://", Reject::EmptyHost);
        rejected("https://", Reject::EmptyHost);
        rejected("http:///path", Reject::EmptyHost);
        rejected("http://?q", Reject::EmptyHost);
        rejected("http://#f", Reject::EmptyHost);
        rejected("http://:80/", Reject::EmptyHost);
        // Bad hosts and ports.
        rejected("http://-host/", Reject::Authority);
        rejected("http://ho st/", Reject::Forbidden);
        rejected("http://host:/", Reject::Authority);
        rejected("http://host:abc/", Reject::Authority);
        rejected("http://host:123456/", Reject::Authority);
        rejected("http://host:65536/", Reject::Authority);
        rejected("http://host:80:80/", Reject::Authority);
        rejected("http://[::1/", Reject::Authority);
        rejected("http://[::1]x/", Reject::Authority);
        rejected("http://[]/", Reject::Authority);
        rejected("http://[evil.example]/", Reject::Authority);
        accepted("http://host:65535/");
        accepted("http://host:1/");
    }

    #[test]
    fn the_length_limit_is_2048_bytes() {
        let at = format!("http://x/{}", "a".repeat(MAX_URL_BYTES - "http://x/".len()));
        assert_eq!(at.len(), MAX_URL_BYTES);
        accepted(&at);
        rejected(&format!("{at}a"), Reject::TooLong);
        rejected(&format!("http://x/{}", "é".repeat(1100)), Reject::TooLong);
    }

    #[test]
    fn a_url_is_not_repaired() {
        // No trimming, no decoding, no scheme guessing.
        rejected("http://x/ ", Reject::Forbidden);
        rejected("\thttp://x/", Reject::Scheme);
        rejected("http://x/\n", Reject::Forbidden);
        accepted("http://x/%0a%0d");
    }

    #[test]
    fn the_opener_program_is_the_oss_and_never_looked_up_on_macos() {
        assert_eq!(opener_program(Os::Macos), Some("/usr/bin/open"));
        assert_eq!(opener_program(Os::Linux), Some("xdg-open"));
        assert_eq!(opener_program(Os::Windows), None);
    }

    #[test]
    fn a_checked_url_is_one_argument_and_nothing_else() {
        for os in [Os::Macos, Os::Linux] {
            let rec = Recorder::default();
            let url = "https://example.com/a?b=c&d=$(touch%20pwn);x";
            assert_eq!(open_url_on(os, &rec, url), Ok(()));
            let calls = rec.calls.borrow();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].1,
                vec![url.to_owned()],
                "one argv element, verbatim"
            );
            assert_eq!(Some(calls[0].0.as_str()), opener_program(os));
        }
    }

    #[test]
    fn a_rejected_url_launches_nothing() {
        let rec = Recorder::default();
        for url in [
            "",
            "-a x",
            "javascript:alert(1)",
            "file:///etc/passwd",
            r"http:\\evil",
            "http://x/\n--help",
            "http://user@host",
            "https://x/\u{202e}",
        ] {
            assert!(
                matches!(
                    open_url_on(Os::Linux, &rec, url),
                    Err(OpenError::Rejected(_))
                ),
                "{url:?}"
            );
        }
        assert!(rec.calls.borrow().is_empty());
    }

    #[test]
    fn a_launch_failure_is_reported_without_the_url() {
        let rec = Recorder {
            fail: true,
            ..Recorder::default()
        };
        let err =
            open_url_on(Os::Linux, &rec, "https://secret.example/?token=abc").expect_err("fails");
        assert_eq!(err, OpenError::Failed(io::ErrorKind::NotFound));
        assert!(!format!("{err:?} {}", err.message()).contains("secret"));
        let rejected = OpenError::Rejected(Reject::Scheme);
        assert!(!rejected.message().is_empty());
    }

    #[test]
    fn the_real_launcher_reaps_and_reports_a_missing_program() {
        // No browser: a harmless program, and one that does not exist.
        #[cfg(unix)]
        {
            assert!(Process.launch("/usr/bin/true", &[]).is_ok());
            let err = Process
                .launch("/definitely/not/a/program", &[])
                .expect_err("missing");
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }
    }
}
