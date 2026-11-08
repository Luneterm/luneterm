//! Starting the shell (plan M9 D11, D12): which program, which directory, which environment, and
//! what the window shows when nothing starts. Ports Lunegit's `TerminalSessionDefaults`
//! (`Lunegit.Core/Application/Services/TerminalSessionDefaults.cs:108-153`) and `TerminalIdentity`
//! (`TerminalIdentity.cs:19-54`).

use std::io;
use std::path::{Path, PathBuf};

use lune_pty::{Pty, PtyBackend, PtySize, SpawnOptions};
use lune_vt::services::options_service::{PtyBackendKind, WindowsPty};

use crate::settings::Settings;

/// The version the app reports (`TERM_PROGRAM_VERSION`, XTVERSION).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The iTerm2 identity Lunegit reports when asked to (`TerminalIdentity.cs:19-25`).
const ITERM2_VERSION: &str = "3.6.11";

/// The OS the shell choice and the locale fallback depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

impl Os {
    #[cfg(target_os = "macos")]
    pub const HOST: Os = Os::Macos;
    #[cfg(target_os = "windows")]
    pub const HOST: Os = Os::Windows;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub const HOST: Os = Os::Linux;
}

/// What XTVERSION answers: `Luneterm(<version>)`, Lunegit's `Lunegit(<ver>)` form.
pub fn xtversion() -> String {
    format!("Luneterm({VERSION})")
}

/// What the engine's reflow heuristics need to know about the PTY: on Windows ConPTY and the OS
/// build; elsewhere nothing.
pub fn windows_pty() -> Option<WindowsPty> {
    cfg!(windows).then(|| WindowsPty {
        backend: Some(PtyBackendKind::Conpty),
        build_number: lune_pty::conpty_build(),
    })
}

/// The programs to try, in order, with the working directory set and no environment yet. The
/// first that spawns wins ([`first_spawned`]).
///
/// - A `shell` setting: that program, with `shell_args` split on whitespace.
/// - Otherwise Unix: the user's login shell (`program: None`, Lunegit's `$SHELL -l`); Windows:
///   `pwsh.exe`, then `powershell.exe`, then `%ComSpec%` (`None`).
///
/// The directory is `default_cwd` when it names an existing directory, else `None` (the home
/// directory).
pub fn candidates(settings: &Settings, os: Os) -> Vec<SpawnOptions> {
    let cwd = (!settings.default_cwd.is_empty())
        .then(|| PathBuf::from(&settings.default_cwd))
        .filter(|p| p.is_dir());
    let with = |program: Option<&str>, args: Vec<String>| SpawnOptions {
        program: program.map(PathBuf::from),
        args,
        cwd: cwd.clone(),
        env: Vec::new(),
    };
    let shell = settings.shell.trim();
    if !shell.is_empty() {
        let args = settings
            .shell_args
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        return vec![with(Some(shell), args)];
    }
    match os {
        Os::Windows => vec![
            with(Some("pwsh.exe"), Vec::new()),
            with(Some("powershell.exe"), Vec::new()),
            with(None, Vec::new()),
        ],
        Os::Macos | Os::Linux => vec![with(None, Vec::new())],
    }
}

/// The locale a shell started without one still needs: a Finder-launched or desktop-launched app
/// has none, and the shell would run in the C locale and mangle non-ASCII input.
fn fallback_ctype(os: Os) -> Option<&'static str> {
    match os {
        Os::Macos => Some("UTF-8"),
        Os::Linux => Some("C.UTF-8"),
        Os::Windows => None,
    }
}

/// The environment added on top of the inherited one, for `os`. `lookup` reads the app's own
/// environment (a variable set to the empty string counts as unset, as for POSIX locales).
pub fn env_for(
    settings: &Settings,
    lookup: &dyn Fn(&str) -> Option<String>,
    os: Os,
) -> Vec<(String, String)> {
    let (program, version) = if settings.report_iterm2 {
        ("iTerm.app", ITERM2_VERSION)
    } else {
        ("Luneterm", VERSION)
    };
    let mut env = vec![
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
        ("TERM_PROGRAM".to_owned(), program.to_owned()),
        ("TERM_PROGRAM_VERSION".to_owned(), version.to_owned()),
    ];
    let locale_set = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|k| lookup(k).is_some_and(|v| !v.is_empty()));
    if !locale_set && let Some(ctype) = fallback_ctype(os) {
        env.push(("LC_CTYPE".to_owned(), ctype.to_owned()));
    }
    env
}

/// [`env_for`] on this OS.
pub fn env(settings: &Settings, lookup: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    env_for(settings, lookup, Os::HOST)
}

/// Why no shell started: the error's kind and the last program tried. Not the error's text, which
/// can carry paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnFailure {
    pub kind: io::ErrorKind,
    pub program: String,
}

fn program_name(options: &SpawnOptions) -> String {
    options.program.as_deref().map_or_else(
        || "login shell".to_owned(),
        |p: &Path| p.display().to_string(),
    )
}

/// Tries `candidates` in order with `spawn`: the first success wins (with the name of the program
/// that started), `NotFound` moves to the next candidate, any other error stops.
pub fn first_spawned<T>(
    candidates: &[SpawnOptions],
    mut spawn: impl FnMut(&SpawnOptions) -> io::Result<T>,
) -> Result<(T, String), SpawnFailure> {
    let mut failure = SpawnFailure {
        kind: io::ErrorKind::NotFound,
        program: "no program".to_owned(),
    };
    for options in candidates {
        match spawn(options) {
            Ok(v) => return Ok((v, program_name(options))),
            Err(e) => {
                failure = SpawnFailure {
                    kind: e.kind(),
                    program: program_name(options),
                };
                if e.kind() != io::ErrorKind::NotFound {
                    break;
                }
            }
        }
    }
    Err(failure)
}

/// Spawns the shell the settings ask for on `backend`, at `size`, with the app's environment.
/// Returns the PTY and the name of the program that started.
pub fn launch(
    backend: &dyn PtyBackend,
    settings: &Settings,
    size: PtySize,
) -> Result<(Box<dyn Pty>, String), SpawnFailure> {
    let env = env(settings, &|k| std::env::var(k).ok());
    let mut list = candidates(settings, Os::HOST);
    for options in &mut list {
        options.env.clone_from(&env);
    }
    first_spawned(&list, |options| backend.spawn(options, size))
}

/// The lines the grid shows when nothing started.
pub fn failure_lines(failure: &SpawnFailure) -> [String; 2] {
    [
        "Failed to start terminal".to_owned(),
        format!("{:?}: {}", failure.kind, failure.program),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(shell: &str, args: &str, cwd: &str) -> Settings {
        Settings {
            shell: shell.into(),
            shell_args: args.into(),
            default_cwd: cwd.into(),
            ..Settings::default()
        }
    }

    fn programs(list: &[SpawnOptions]) -> Vec<Option<String>> {
        list.iter()
            .map(|o| o.program.as_ref().map(|p| p.display().to_string()))
            .collect()
    }

    #[test]
    fn a_shell_setting_is_the_only_candidate_with_its_args_split_on_whitespace() {
        for os in [Os::Macos, Os::Windows, Os::Linux] {
            let list = candidates(&settings("/bin/fish", " -l   --private ", ""), os);
            assert_eq!(programs(&list), vec![Some("/bin/fish".to_owned())]);
            assert_eq!(list[0].args, vec!["-l", "--private"]);
        }
        // A blank shell is no shell.
        let list = candidates(&settings("   ", "-x", ""), Os::Linux);
        assert_eq!(programs(&list), vec![None]);
        assert!(list[0].args.is_empty(), "args need a program");
    }

    #[test]
    fn the_default_is_the_login_shell_on_unix_and_the_powershells_on_windows() {
        for os in [Os::Macos, Os::Linux] {
            assert_eq!(programs(&candidates(&Settings::default(), os)), vec![None]);
        }
        assert_eq!(
            programs(&candidates(&Settings::default(), Os::Windows)),
            vec![
                Some("pwsh.exe".to_owned()),
                Some("powershell.exe".to_owned()),
                None
            ]
        );
    }

    #[test]
    fn the_cwd_is_the_setting_only_when_it_is_an_existing_directory() {
        let dir = std::env::temp_dir();
        let dir_str = dir.to_str().expect("utf-8 temp dir").to_owned();
        let list = candidates(&settings("", "", &dir_str), Os::Windows);
        assert_eq!(list.len(), 3);
        assert!(list.iter().all(|o| o.cwd.as_deref() == Some(dir.as_path())));
        let missing = dir.join("luneterm-no-such-directory-5f3a");
        assert!(!missing.exists());
        for cwd in [missing.to_str().expect("utf-8"), ""] {
            let list = candidates(&settings("", "", cwd), Os::Linux);
            assert_eq!(list[0].cwd, None, "cwd setting {cwd:?}");
        }
        // A file is not a directory.
        let file = std::env::current_exe().expect("exe");
        let list = candidates(&settings("", "", file.to_str().expect("utf-8")), Os::Linux);
        assert_eq!(list[0].cwd, None);
    }

    fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn lookup<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            set.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn the_environment_names_luneterm() {
        let env = env_for(
            &Settings::default(),
            &lookup(&[("LANG", "en_US.UTF-8")]),
            Os::Macos,
        );
        assert_eq!(value(&env, "TERM"), Some("xterm-256color"));
        assert_eq!(value(&env, "COLORTERM"), Some("truecolor"));
        assert_eq!(value(&env, "TERM_PROGRAM"), Some("Luneterm"));
        assert_eq!(value(&env, "TERM_PROGRAM_VERSION"), Some(VERSION));
        assert_eq!(env.len(), 4, "with a locale set nothing else is added");
        assert_eq!(xtversion(), format!("Luneterm({VERSION})"));
    }

    #[test]
    fn iterm2_reporting_changes_the_program_and_never_adds_iterm_variables() {
        let on = Settings {
            report_iterm2: true,
            ..Settings::default()
        };
        for os in [Os::Macos, Os::Windows, Os::Linux] {
            let env = env_for(&on, &lookup(&[]), os);
            assert_eq!(value(&env, "TERM_PROGRAM"), Some("iTerm.app"));
            assert_eq!(value(&env, "TERM_PROGRAM_VERSION"), Some("3.6.11"));
            for key in ["LC_TERMINAL", "ITERM_SESSION_ID", "LC_TERMINAL_VERSION"] {
                assert_eq!(value(&env, key), None, "{key}");
            }
        }
    }

    #[test]
    fn a_missing_locale_gets_a_utf8_ctype_on_macos_and_linux_only() {
        let s = Settings::default();
        let none = lookup(&[]);
        assert_eq!(
            value(&env_for(&s, &none, Os::Macos), "LC_CTYPE"),
            Some("UTF-8")
        );
        assert_eq!(
            value(&env_for(&s, &none, Os::Linux), "LC_CTYPE"),
            Some("C.UTF-8")
        );
        assert_eq!(value(&env_for(&s, &none, Os::Windows), "LC_CTYPE"), None);
        // Any of the three variables counts as a locale; the empty string does not.
        for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
            let set = [(var, "de_DE.UTF-8")];
            let env = env_for(&s, &lookup(&set), Os::Linux);
            assert_eq!(value(&env, "LC_CTYPE"), None, "{var} set");
        }
        let empty = [("LANG", ""), ("LC_ALL", "")];
        assert_eq!(
            value(&env_for(&s, &lookup(&empty), Os::Linux), "LC_CTYPE"),
            Some("C.UTF-8")
        );
    }

    fn opts(name: &str) -> SpawnOptions {
        SpawnOptions {
            program: Some(PathBuf::from(name)),
            ..SpawnOptions::default()
        }
    }

    #[test]
    fn not_found_moves_to_the_next_candidate_and_the_first_success_wins() {
        let list = [opts("a"), opts("b"), opts("c")];
        let mut tried = Vec::new();
        let got = first_spawned(&list, |o| {
            let name = program_name(o);
            tried.push(name.clone());
            if name == "c" {
                Ok(7)
            } else {
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
        });
        assert_eq!(got, Ok((7, "c".to_owned())));
        assert_eq!(tried, ["a", "b", "c"]);

        let mut tried = Vec::new();
        let got: Result<(i32, String), _> = first_spawned(&list, |o| {
            tried.push(program_name(o));
            Ok(1)
        });
        assert_eq!(got, Ok((1, "a".to_owned())));
        assert_eq!(tried, ["a"], "stops at the first success");
    }

    #[test]
    fn any_other_error_stops_and_all_not_found_reports_the_last_program() {
        let list = [opts("a"), opts("b"), opts("c")];
        let mut tried = Vec::new();
        let got: Result<((), String), _> = first_spawned(&list, |o| {
            tried.push(program_name(o));
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(
            got,
            Err(SpawnFailure {
                kind: io::ErrorKind::PermissionDenied,
                program: "a".into()
            })
        );
        assert_eq!(tried, ["a"]);

        let got: Result<((), String), _> =
            first_spawned(&list, |_| Err(io::Error::from(io::ErrorKind::NotFound)));
        assert_eq!(
            got,
            Err(SpawnFailure {
                kind: io::ErrorKind::NotFound,
                program: "c".into()
            })
        );
        let none: Result<((), String), _> = first_spawned(&[], |_| Ok(()));
        assert_eq!(none.map_err(|f| f.kind), Err(io::ErrorKind::NotFound));
        // The login shell has no program name.
        assert_eq!(program_name(&SpawnOptions::default()), "login shell");
    }

    #[test]
    fn conpty_is_declared_on_windows_only() {
        let pty = windows_pty();
        if cfg!(windows) {
            let pty = pty.expect("ConPTY on Windows");
            assert_eq!(pty.backend, Some(PtyBackendKind::Conpty));
            assert!(pty.build_number.is_some_and(|b| b >= 17763));
        } else {
            assert_eq!(pty, None);
        }
    }

    #[test]
    fn the_failure_screen_names_the_kind_and_the_program() {
        let lines = failure_lines(&SpawnFailure {
            kind: io::ErrorKind::NotFound,
            program: "pwsh.exe".into(),
        });
        assert_eq!(lines[0], "Failed to start terminal");
        assert_eq!(lines[1], "NotFound: pwsh.exe");
    }
}
