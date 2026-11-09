//! Checking a directory the shell reported (plan M10 step 1; OSC 7 and OSC 9;9 text).
//!
//! The text comes from the PTY, so anything that runs in the session can choose it. The engine
//! guarantees only: no control characters, at most 4096 bytes, not empty (`lune-vt`
//! `ext/osc_cwd.rs`). [`validated`] is the host's check before a tab starts in the directory.
//!
//! Two rules matter beyond "is a directory":
//!  - **No filesystem call for a network form.** On Windows, a `stat` of `\\host\share` starts an
//!    SMB handshake that sends the user's credentials to `host`, which the program inside the
//!    session picked. A path that begins with two separators (`\\`, `//`, `\/`, `/\`; this
//!    covers `\\?\` and `\\.\` device forms) is refused by [`syntax`], which touches nothing.
//!    The rule is the same on every OS (POSIX leaves a leading `//` to the implementation, and a
//!    cwd there is not worth an exception), so the code has no per-OS branch.
//!  - **Not on the UI thread.** [`validated`] stats the path, and a hung network mount can block
//!    a stat for a long time. Call it from a worker or the engine thread, never the UI thread.

use std::path::{Path, PathBuf};

/// The longest text accepted, in bytes (the engine's own cap).
pub const MAX_CWD_BYTES: usize = 4096;

/// The checks that need no filesystem access: not empty, at most [`MAX_CWD_BYTES`] bytes, no
/// control character (C0, DEL, C1), no network or device form, and absolute.
pub fn syntax(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() > MAX_CWD_BYTES || raw.chars().any(char::is_control) {
        return None;
    }
    if starts_with_two_separators(raw) {
        return None;
    }
    let path = Path::new(raw);
    path.is_absolute().then(|| path.to_path_buf())
}

/// `raw` as a path if it is an absolute path to an existing directory; `None` otherwise. See the
/// module docs: this stats the path, so not on the UI thread.
pub fn validated(raw: &str) -> Option<PathBuf> {
    let path = syntax(raw)?;
    path.is_dir().then_some(path)
}

fn starts_with_two_separators(raw: &str) -> bool {
    let mut chars = raw.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('/' | '\\'), Some('/' | '\\'))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luneterm-cwd-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the scratch dir");
        dir
    }

    #[test]
    fn an_existing_absolute_directory_is_accepted() {
        let dir = temp_dir("ok");
        let text = dir.to_str().expect("utf-8 temp dir");
        assert_eq!(validated(text), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_missing_and_file_paths_are_refused() {
        let dir = temp_dir("refuse");
        let file = dir.join("a-file");
        std::fs::write(&file, b"x").expect("write the file");
        let missing = dir.join("nope");
        for (name, raw) in [
            ("relative", "some/dir".to_owned()),
            ("dot", ".".to_owned()),
            ("home shorthand", "~".to_owned()),
            ("empty", String::new()),
            ("missing", missing.to_str().expect("utf-8").to_owned()),
            ("file", file.to_str().expect("utf-8").to_owned()),
        ] {
            assert_eq!(validated(&raw), None, "{name}: {raw:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn control_characters_are_refused_before_anything_else() {
        let dir = temp_dir("ctl");
        let base = dir.to_str().expect("utf-8").to_owned();
        for c in ['\0', '\n', '\x1b', '\x7f', '\u{85}'] {
            assert_eq!(validated(&format!("{base}{c}")), None, "{c:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_and_device_forms_are_refused_without_a_filesystem_call() {
        // `syntax` touches nothing, so a refusal here proves no stat can have happened.
        for raw in [
            r"\\attacker\share",
            r"\\attacker\share\dir",
            "//attacker/share",
            r"\\?\C:\Users",
            r"\\.\pipe\x",
            "//?/C:/Users",
            "//./pipe/x",
            r"\/attacker/share",
            r"/\attacker\share",
        ] {
            assert_eq!(syntax(raw), None, "{raw}");
            assert_eq!(validated(raw), None, "{raw}");
        }
    }

    #[test]
    fn syntax_accepts_a_path_a_stat_would_then_judge() {
        // Absolute and clean: `syntax` passes it, `validated` is where a missing one is refused.
        if cfg!(unix) {
            assert!(syntax("/definitely/not/a/real/dir").is_some());
        }
        assert_eq!(validated("/definitely/not/a/real/dir"), None);
    }

    #[test]
    fn the_length_cap_is_4096_bytes() {
        let at_cap = format!("/{}", "a".repeat(MAX_CWD_BYTES - 1));
        assert_eq!(at_cap.len(), MAX_CWD_BYTES);
        let over = format!("/{}", "a".repeat(MAX_CWD_BYTES));
        assert_eq!(over.len(), MAX_CWD_BYTES + 1);
        // On Unix the 4096-byte text is absolute and clean, so `syntax` lets it through to the
        // stat (which then finds no such directory); one more byte never gets that far.
        if cfg!(unix) {
            assert!(syntax(&at_cap).is_some());
        }
        assert_eq!(syntax(&over), None);
        assert_eq!(validated(&at_cap), None, "no such directory");
        assert_eq!(validated(&over), None);
    }

    #[test]
    fn multibyte_text_is_counted_in_bytes() {
        // 2049 two-byte characters = 4098 bytes, 2049 chars: over the cap by bytes only.
        let over = format!("/{}", "é".repeat(2049));
        assert!(over.chars().count() < MAX_CWD_BYTES);
        assert_eq!(syntax(&over), None);
    }
}
