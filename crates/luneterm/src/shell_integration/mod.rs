//! Shell integration (plan M10 step 7, D12): small scripts the user's own shell sources, so it
//! reports its working directory (OSC 7, or OSC 9;9 in PowerShell) and when a command starts and
//! ends (OSC 133). Tabs restore at the right directory, the close-confirm knows a command runs
//! when the platform cannot say, and the outline can show "busy".
//!
//! Ported from Lunegit's `TerminalShellIntegrator.cs` (integration version 5; the scripts in
//! `scripts/` are its scripts with the identity changed, `LUNEGIT_*` to `LUNETERM_*`). What is
//! ours is the boundary around them, because the shell **sources** these files:
//!
//!  - the directory is `<config dir>/shell-integration` (D12), created 0700. On Unix, before
//!    every use it is `lstat`-checked, and so is each subdirectory and file: a real directory or
//!    file (never a symlink), owned by the effective user, writable by no one else
//!    ([`check_dir`], [`check_file`]). Anything else skips integration for that spawn and the
//!    shell starts as it would without it (as Lunegit does on any error);
//!  - files are written 0600 through [`crate::persist::write_atomic`] (a new file under a
//!    temporary name, renamed over the target), only when absent or different, so a symlink at a
//!    script's path is replaced and never written through;
//!  - script text is a constant: nothing user-controlled is interpolated when it is written.
//!    What reaches a command line is a path: PowerShell gets it as a single-quoted literal with
//!    every quote doubled ([`pwsh_command`]); zsh and bash get it through env and argv only;
//!  - on Windows the directory is under `dirs::config_dir()` (the roaming app data folder), whose
//!    default ACL is user-only. That is assumption A3 of the plan, not probed here, and the
//!    security review's scope; no ACL check is made there.
//!
//! The user's own rc files run exactly as without integration: zsh gets `ZDOTDIR` pointed at our
//! wrappers and `LUNETERM_USER_ZDOTDIR` at the user's directory (the wrappers source the user's
//! files and then restore `ZDOTDIR`); bash runs our rc file, which sources `/etc/profile`, the
//! first of `.bash_profile`, `.bash_login`, `.profile`, then `.bashrc` (the login sequence the
//! dropped `-l` would have run); PowerShell starts without `-NoProfile`, so the profile loads
//! before our script wraps the prompt.
//!
//! Which shell: the program of the spawn, or for the login shell (`program: None`) the `SHELL`
//! variable, the source portable-pty reads first. A shell this module does not know, a `SHELL`
//! that is not set (portable-pty then asks the password database; we do not), a path or value
//! that is not UTF-8: no integration. On Windows only PowerShell is integrated.

use std::io;
use std::path::{Path, PathBuf};

use crate::spawn::Os;

const DIR_NAME: &str = "shell-integration";

const ZSHENV: &str = include_str!("scripts/zshenv");
const ZPROFILE: &str = include_str!("scripts/zprofile");
const ZSHRC: &str = include_str!("scripts/zshrc");
const BASHRC: &str = include_str!("scripts/bashrc.bash");
const PWSH: &str = include_str!("scripts/integration.ps1");

/// `(subdirectory, file name, text)` of every script.
const FILES: [(&str, &str, &str); 5] = [
    ("zsh", ".zshenv", ZSHENV),
    ("zsh", ".zprofile", ZPROFILE),
    ("zsh", ".zshrc", ZSHRC),
    ("bash", "luneterm-rc.bash", BASHRC),
    ("pwsh", "luneterm-integration.ps1", PWSH),
];

const SUBDIRS: [&str; 3] = ["zsh", "bash", "pwsh"];

/// The kinds of shell this module tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
    Cmd,
    Unknown,
}

/// The kind of the program `program` (a path or a bare name): its file name without a `.exe`
/// extension, case-insensitive. Windows paths split on both separators; Unix paths on `/` only.
pub fn detect_kind(program: &str, os: Os) -> ShellKind {
    let name = if os == Os::Windows {
        program.rsplit(['/', '\\']).next().unwrap_or("")
    } else {
        program.rsplit('/').next().unwrap_or("")
    };
    let mut name = name.to_ascii_lowercase();
    if os == Os::Windows
        && let Some(stem) = name.strip_suffix(".exe")
    {
        name = stem.to_owned();
    }
    match name.as_str() {
        "zsh" => ShellKind::Zsh,
        "bash" => ShellKind::Bash,
        "fish" => ShellKind::Fish,
        "pwsh" | "powershell" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Unknown,
    }
}

// ------------------------------------------------------------------------- the safety checks

/// What a path is, from `lstat` (a symlink is a symlink, not what it points at).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// What the checks need to know of a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub kind: FileKind,
    pub uid: u32,
    /// The permission bits (`st_mode & 0o7777`).
    pub mode: u32,
    pub len: u64,
}

/// Why integration is not used for a spawn. Never carries a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The app has no config directory.
    NoDir,
    /// A path is not UTF-8, or has a control character in it.
    Path,
    /// A directory or file is not what it must be (a symlink, not ours, writable by others).
    Unsafe(Unsafe),
    Io(io::ErrorKind),
}

/// What is wrong with a path ([`check_dir`], [`check_file`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsafe {
    NotADirectory,
    NotAFile,
    NotOwned,
    Writable,
}

const GROUP_OTHER_WRITE: u32 = 0o022;

/// A directory the shell may source from: a real directory (not a symlink), owned by `euid`,
/// with no group or other write bit.
pub fn check_dir(f: &Facts, euid: u32) -> Result<(), Unsafe> {
    if f.kind != FileKind::Dir {
        return Err(Unsafe::NotADirectory);
    }
    check_owner_and_mode(f, euid)
}

/// A file the shell may source: a regular file (not a symlink), owned by `euid`, with no group
/// or other write bit.
pub fn check_file(f: &Facts, euid: u32) -> Result<(), Unsafe> {
    if f.kind != FileKind::File {
        return Err(Unsafe::NotAFile);
    }
    check_owner_and_mode(f, euid)
}

fn check_owner_and_mode(f: &Facts, euid: u32) -> Result<(), Unsafe> {
    if f.uid != euid {
        return Err(Unsafe::NotOwned);
    }
    if f.mode & GROUP_OTHER_WRITE != 0 {
        return Err(Unsafe::Writable);
    }
    Ok(())
}

#[cfg(unix)]
mod platform {
    //! The two things only the OS knows: who we are, and what a path is without following it.

    #![allow(unsafe_code)]

    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use super::{Facts, FileKind};

    /// The effective user id.
    pub(super) fn euid() -> u32 {
        // SAFETY: `geteuid` takes no argument, reads no memory and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// `lstat` of `path`.
    pub(super) fn facts(path: &Path) -> io::Result<Facts> {
        let m = std::fs::symlink_metadata(path)?;
        let t = m.file_type();
        let kind = if t.is_symlink() {
            FileKind::Symlink
        } else if t.is_dir() {
            FileKind::Dir
        } else if t.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        Ok(Facts {
            kind,
            uid: m.uid(),
            mode: m.mode() & 0o7777,
            len: m.len(),
        })
    }
}

// ------------------------------------------------------------------------------- installing

/// The scripts are in place: their directory, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    root: String,
}

impl Install {
    pub fn root(&self) -> &str {
        &self.root
    }

    fn zsh_dir(&self) -> String {
        format!("{}{}zsh", self.root, sep())
    }

    fn bash_rc(&self) -> String {
        format!("{}{}bash{}luneterm-rc.bash", self.root, sep(), sep())
    }

    fn pwsh_script(&self) -> String {
        format!(
            "{}{}pwsh{}luneterm-integration.ps1",
            self.root,
            sep(),
            sep()
        )
    }
}

/// An [`Install`] over `root` for tests of the callers: nothing is checked or written.
#[cfg(test)]
pub fn install_for_tests(root: &str) -> Install {
    Install {
        root: root.to_owned(),
    }
}

fn sep() -> char {
    std::path::MAIN_SEPARATOR
}

/// A path text safe to hand to a shell as an argument or an env value: UTF-8, no control
/// character.
fn path_text(p: &Path) -> Result<String, Skip> {
    let s = p.to_str().ok_or(Skip::Path)?;
    if s.is_empty() || s.chars().any(char::is_control) {
        return Err(Skip::Path);
    }
    Ok(s.to_owned())
}

/// Checks the directory `<config_dir>/shell-integration` and its subdirectories (creating them
/// 0700), and writes the scripts that are absent or different. On any problem: `Err`, and the
/// shell starts without integration.
pub fn install(config_dir: &Path) -> Result<Install, Skip> {
    let root_path = config_dir.join(DIR_NAME);
    let root = path_text(&root_path)?;
    #[cfg(unix)]
    let euid = platform::euid();
    #[cfg(unix)]
    {
        ensure_dir(&root_path, euid)?;
        for sub in SUBDIRS {
            ensure_dir(&root_path.join(sub), euid)?;
        }
    }
    #[cfg(not(unix))]
    for sub in SUBDIRS {
        crate::persist::create_dir(&root_path.join(sub)).map_err(|e| Skip::Io(e.kind()))?;
    }
    for (sub, name, text) in FILES {
        let dir = root_path.join(sub);
        #[cfg(unix)]
        ensure_file(&dir, name, text, euid)?;
        #[cfg(not(unix))]
        crate::persist::write_atomic(&dir, name, text.as_bytes())
            .map_err(|e| Skip::Io(e.kind()))?;
    }
    Ok(Install { root })
}

#[cfg(unix)]
fn ensure_dir(path: &Path, euid: u32) -> Result<(), Skip> {
    match platform::facts(path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            crate::persist::create_dir(path).map_err(|e| Skip::Io(e.kind()))?;
        }
        Err(e) => return Err(Skip::Io(e.kind())),
    }
    let facts = platform::facts(path).map_err(|e| Skip::Io(e.kind()))?;
    check_dir(&facts, euid).map_err(Skip::Unsafe)
}

/// The file `dir/name` holds `text`, and is a file the shell may source.
#[cfg(unix)]
fn ensure_file(dir: &Path, name: &str, text: &str, euid: u32) -> Result<(), Skip> {
    let path = dir.join(name);
    let in_place = match platform::facts(&path) {
        Ok(f) => {
            check_file(&f, euid).is_ok()
                && f.len == text.len() as u64
                && std::fs::read(&path).is_ok_and(|b| b == text.as_bytes())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(Skip::Io(e.kind())),
    };
    if !in_place {
        // A new file under a temporary name, renamed over the target: a symlink (or anything
        // else) at `path` is replaced, never written through.
        crate::persist::write_atomic(dir, name, text.as_bytes()).map_err(|e| Skip::Io(e.kind()))?;
    }
    let facts = platform::facts(&path).map_err(|e| Skip::Io(e.kind()))?;
    check_file(&facts, euid).map_err(Skip::Unsafe)
}

// -------------------------------------------------------------------------------- the spawn

/// What the app's environment says about a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Unset,
    Set(String),
    /// Set to something that is not UTF-8: it cannot be passed on faithfully.
    Invalid,
}

impl Var {
    /// The value: `Some(None)` when unset or empty, `None` when it is not UTF-8 (the spawn is
    /// then left alone).
    fn text(self) -> Option<Option<String>> {
        match self {
            Var::Unset => Some(None),
            Var::Set(s) if s.is_empty() => Some(None),
            Var::Set(s) => Some(Some(s)),
            Var::Invalid => None,
        }
    }
}

/// The variable `name` of this process's environment.
pub fn process_var(name: &str) -> Var {
    match std::env::var_os(name) {
        None => Var::Unset,
        Some(v) => v.into_string().map_or(Var::Invalid, Var::Set),
    }
}

/// What integration changes about one spawn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Augment {
    /// The program to run instead of the spawn's (the login shell needs a name when arguments
    /// are given).
    pub program: Option<PathBuf>,
    /// Replaces the spawn's arguments.
    pub args: Vec<String>,
    /// Added to the spawn's environment.
    pub env: Vec<(String, String)>,
}

/// A PowerShell single-quoted literal of `s`. PowerShell reads the four Unicode single quotes
/// (U+2018, U+2019, U+201A, U+201B) as `'` too, so each of them is doubled with its own kind, as
/// the ASCII one is.
pub fn pwsh_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// The `-Command` text that evaluates the script's content (not the file: the execution policy
/// gates files, not strings, and `Restricted` is the Windows client default), read as UTF-8 and
/// by literal path.
pub fn pwsh_command(script: &str) -> String {
    format!(
        "Invoke-Expression (Get-Content -Raw -Encoding UTF8 -LiteralPath {})",
        pwsh_literal(script)
    )
}

/// Whether `args` are the default of bash: none, or exactly `-l`.
fn default_bash_args(args: &[String]) -> bool {
    args.is_empty() || (args.len() == 1 && args[0] == "-l")
}

/// What integration does to the spawn of `program` (`None`: the login shell) with `args`, once
/// the scripts are installed; `None` when the shell is not one it integrates or the spawn is not
/// the default form (then the spawn is untouched). `var` reads the app's environment.
pub fn augment(
    install: &Install,
    program: Option<&str>,
    args: &[String],
    var: &dyn Fn(&str) -> Var,
    os: Os,
) -> Option<Augment> {
    // The shell: the spawn's program, or `$SHELL` for the login shell (never on Windows, where
    // the default is `%ComSpec%`, cmd).
    let (name, login) = match program {
        Some(p) => (p.to_owned(), false),
        None if os == Os::Windows => return None,
        None => (var("SHELL").text()??, true),
    };
    match detect_kind(&name, os) {
        ShellKind::Zsh if os != Os::Windows => {
            // The user's own directory, which the wrappers source from: ZDOTDIR, else HOME.
            let user = match var("ZDOTDIR").text()? {
                Some(z) => z,
                None => var("HOME").text()??,
            };
            if !Path::new(&user).is_absolute() {
                return None;
            }
            Some(Augment {
                program: None,
                args: args.to_vec(),
                env: vec![
                    ("ZDOTDIR".to_owned(), install.zsh_dir()),
                    ("LUNETERM_USER_ZDOTDIR".to_owned(), user),
                ],
            })
        }
        ShellKind::Bash if os != Os::Windows && default_bash_args(args) => Some(Augment {
            // `--rcfile` needs a program to give arguments to.
            program: login.then(|| PathBuf::from(&name)),
            args: vec!["--rcfile".to_owned(), install.bash_rc(), "-i".to_owned()],
            env: Vec::new(),
        }),
        ShellKind::PowerShell if args.is_empty() => Some(Augment {
            program: None,
            args: vec![
                "-NoExit".to_owned(),
                "-Command".to_owned(),
                pwsh_command(&install.pwsh_script()),
            ],
            env: Vec::new(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
