//! Starting the shell (plan M9 D11, D12): which program, which directory, which environment, and
//! what the window shows when nothing starts. Ports Lunegit's `TerminalSessionDefaults`
//! (`Lunegit.Core/Application/Services/TerminalSessionDefaults.cs:108-153`) and `TerminalIdentity`
//! (`TerminalIdentity.cs:19-54`).
//!
//! Also starting a program tab (`luneterm ctl tab new ... -- <program>`, [`program_spawn`]). Its
//! argument vector comes from another process, so no word of it, and no directory, is ever part of
//! a string a shell parses.

use std::io;
use std::path::{Path, PathBuf};

use lune_pty::{Pty, PtyBackend, PtySize, SpawnOptions};
use lune_vt::services::options_service::{PtyBackendKind, WindowsPty};

use crate::settings::Settings;
use crate::shell_integration::{self, ShellKind};
use crate::tabs::TabId;

/// The version the app reports (`TERM_PROGRAM_VERSION`, XTVERSION).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The iTerm2 identity Lunegit reports when asked to (`TerminalIdentity.cs:19-25`).
const ITERM2_VERSION: &str = "3.6.11";

/// The variables every tab's child is given ([`tab_env`]): this run's executable, the instance
/// it belongs to, and its tab.
const BIN_VAR: &str = "LUNETERM_BIN";
const INSTANCE_VAR: &str = "LUNETERM_INSTANCE";
const TAB_ID_VAR: &str = "LUNETERM_TAB_ID";
/// The app's config directory, when the user named one.
const CONFIG_DIR_VAR: &str = "LUNETERM_CONFIG_DIR";

/// What a sh-family login shell runs for a program tab: `$1` is the directory, the rest the
/// program and its arguments.
const SH_SCRIPT: &str = r#"cd -- "$1" || exit 127; shift; exec "$@""#;
/// The same for fish, which has no `$0`: `$argv[1]` is the directory.
const FISH_SCRIPT: &str = "cd -- $argv[1]; or exit 127; set -e argv[1]; exec $argv";
/// The sh script's `$0`.
const SCRIPT_NAME: &str = "luneterm";

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

/// The running executable's canonical path, for `LUNETERM_BIN`. Ask once, at startup: an update
/// later replaces what is on disk. Empty, which a program reads as unset, when it cannot be told
/// or is not Unicode.
pub fn own_binary() -> String {
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .map(|path| without_verbatim(&path))
        .unwrap_or_default()
}

/// `path` without the `\\?\` form Windows' canonicalisation answers in, which `cmd.exe` and
/// other programs do not take. Any other path is returned as it is.
fn without_verbatim(path: &str) -> String {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    match path.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => path.to_owned(),
    }
}

/// The variables that tell a tab's child which Luneterm and which tab it runs in. All three are
/// always set, so none is inherited from a Luneterm this one was started in; an empty `bin` or
/// `instance` reads as unset.
pub fn tab_env(bin: &str, instance: &str, tab_id: TabId) -> Vec<(String, String)> {
    vec![
        (BIN_VAR.to_owned(), bin.to_owned()),
        (INSTANCE_VAR.to_owned(), instance.to_owned()),
        (TAB_ID_VAR.to_owned(), tab_id.to_string()),
    ]
}

/// A relative `LUNETERM_CONFIG_DIR` names a directory under the app's own working directory; a
/// child runs elsewhere, and `ctl` in it must find the same one, so it is told the absolute path.
fn absolute_config_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<(String, String)> {
    let set = lookup(CONFIG_DIR_VAR).filter(|v| !v.is_empty())?;
    if Path::new(&set).is_absolute() {
        return None;
    }
    let absolute = std::path::absolute(&set).ok()?;
    let absolute = absolute.into_os_string().into_string().ok()?;
    Some((CONFIG_DIR_VAR.to_owned(), absolute))
}

/// Everything added to the inherited environment of a tab's child: [`env_for`], the config
/// directory when it has to be rewritten, and `tab` ([`tab_env`]).
pub fn child_env(
    settings: &Settings,
    lookup: &dyn Fn(&str) -> Option<String>,
    os: Os,
    tab: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env = env_for(settings, lookup, os);
    env.extend(absolute_config_dir(lookup));
    env.extend_from_slice(tab);
    env
}

/// The app's own environment, as the lookups here read it.
fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
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
    launch_in(backend, settings, size, None)
}

/// [`launch`] in `cwd` when given (a directory the caller already checked, see `cwd::validated`),
/// else in the settings' directory. No shell integration (see [`launch_with`]).
pub fn launch_in(
    backend: &dyn PtyBackend,
    settings: &Settings,
    size: PtySize,
    cwd: Option<&Path>,
) -> Result<(Box<dyn Pty>, String), SpawnFailure> {
    launch_with(backend, settings, size, cwd, None, &[])
}

/// [`launch_in`] with shell integration when the setting is on and `config_dir` (the app's
/// config directory) is given: the scripts are installed under it on first need, and a shell
/// that has them is started through them. Any problem with them leaves the spawn as it would be
/// without integration. `tab` is added to the environment ([`tab_env`]).
pub fn launch_with(
    backend: &dyn PtyBackend,
    settings: &Settings,
    size: PtySize,
    cwd: Option<&Path>,
    config_dir: Option<&Path>,
    tab: &[(String, String)],
) -> Result<(Box<dyn Pty>, String), SpawnFailure> {
    let env = child_env(settings, &process_env, Os::HOST, tab);
    let mut installed: Option<Option<shell_integration::Install>> = None;
    let mut install = || {
        installed
            .get_or_insert_with(|| {
                let dir = config_dir?;
                match shell_integration::install(dir) {
                    Ok(i) => Some(i),
                    Err(skip) => {
                        // The reason only: never a path.
                        eprintln!("luneterm: shell integration skipped: {skip:?}");
                        None
                    }
                }
            })
            .clone()
    };
    let list = spawn_list(
        settings,
        Os::HOST,
        &env,
        cwd,
        &shell_integration::process_var,
        &mut install,
    );
    first_spawned(&list, |options| backend.spawn(options, size))
}

/// The programs to try, each with its environment, directory and, when `install` gives the
/// scripts, shell integration. `install` is asked only when the setting is on and a candidate is
/// a shell it integrates, so with the setting off the list is exactly [`candidates`] with `env`
/// and `cwd` (a test pins it).
pub fn spawn_list(
    settings: &Settings,
    os: Os,
    env: &[(String, String)],
    cwd: Option<&Path>,
    var: &dyn Fn(&str) -> shell_integration::Var,
    install: &mut dyn FnMut() -> Option<shell_integration::Install>,
) -> Vec<SpawnOptions> {
    let mut list = candidates(settings, os);
    for options in &mut list {
        options.env = env.to_vec();
        if let Some(dir) = cwd {
            options.cwd = Some(dir.to_path_buf());
        }
        if !settings.shell_integration {
            continue;
        }
        let program = options.program.as_deref().and_then(Path::to_str);
        if options.program.is_some() && program.is_none() {
            continue;
        }
        // Only a shell it knows asks for the scripts to be written at all.
        let name = match program {
            Some(p) => p.to_owned(),
            None => match var("SHELL") {
                shell_integration::Var::Set(s) if os != Os::Windows => s,
                _ => continue,
            },
        };
        if matches!(
            shell_integration::detect_kind(&name, os),
            shell_integration::ShellKind::Fish
                | shell_integration::ShellKind::Cmd
                | shell_integration::ShellKind::Unknown
        ) {
            continue;
        }
        let Some(scripts) = install() else { continue };
        if let Some(aug) = shell_integration::augment(&scripts, program, &options.args, var, os) {
            if let Some(p) = aug.program {
                options.program = Some(p);
            }
            options.args = aug.args;
            options.env.extend(aug.env);
        }
    }
    list
}

/// What a program tab runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramSpec {
    /// An existing directory (the caller checks it).
    pub cwd: PathBuf,
    /// The program and its arguments.
    pub argv: Vec<String>,
}

/// Why a program tab is not started at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Windows: the program is a `.bat` or `.cmd` file, which `CreateProcessW` hands to
    /// `cmd.exe`, and that parses the arguments again as commands.
    BatchFile,
    /// No program to run: none given, not found (Windows), or a word a shell's `exec` would take
    /// as one of its options.
    NotFound,
    /// The directory cannot be passed to a shell because it is not Unicode.
    Directory,
}

/// Why [`launch_program`] started nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramFailure {
    Refused(Refused),
    Spawn(SpawnFailure),
}

/// How to start `spec`'s program, with `env` added to the environment. `lookup` reads the app's
/// own environment. Neither `shell_args` nor shell integration applies.
///
/// - Unix with a sh-family shell or fish (the `shell` setting, else an absolute `$SHELL`, else
///   `/bin/sh`): an interactive login shell, so the program gets the environment and `PATH` a new
///   tab has, runs a constant script that changes to the directory and replaces itself with the
///   program. The directory and each word are arguments of their own.
/// - Unix with any other shell: the program itself.
/// - Windows: the program itself, by the absolute path found here. Batch files are refused.
pub fn program_spawn(
    settings: &Settings,
    os: Os,
    lookup: &dyn Fn(&str) -> Option<String>,
    spec: &ProgramSpec,
    env: &[(String, String)],
) -> Result<SpawnOptions, Refused> {
    let Some((program, rest)) = spec.argv.split_first().filter(|(p, _)| !p.is_empty()) else {
        return Err(Refused::NotFound);
    };
    let options = |program: PathBuf, args: Vec<String>| SpawnOptions {
        program: Some(program),
        args,
        cwd: Some(spec.cwd.clone()),
        env: env.to_vec(),
    };
    if os == Os::Windows {
        let found = windows_program(program, lookup, env)?;
        return Ok(options(found, rest.to_vec()));
    }
    let shell = program_shell(settings, lookup);
    let Some((script, name)) = wrapper(&shell, os) else {
        return Ok(options(PathBuf::from(program), rest.to_vec()));
    };
    // bash, zsh and ksh read `exec -c`, `-l` and `-a` as their own options, even from `"$@"`.
    if program.starts_with('-') {
        return Err(Refused::NotFound);
    }
    let cwd = spec.cwd.to_str().ok_or(Refused::Directory)?;
    let args = ["-l", "-i", "-c", script]
        .into_iter()
        .chain(name)
        .chain([cwd])
        .map(str::to_owned)
        .chain(spec.argv.iter().cloned())
        .collect();
    Ok(options(PathBuf::from(shell), args))
}

/// The shell a Unix program tab goes through.
fn program_shell(settings: &Settings, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let set = settings.shell.trim();
    if !set.is_empty() {
        return set.to_owned();
    }
    lookup("SHELL")
        .filter(|s| s.starts_with('/'))
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// The script `shell` runs for a program tab and the script's `$0` if it has one; `None` for a
/// shell whose script syntax is not known.
fn wrapper(shell: &str, os: Os) -> Option<(&'static str, Option<&'static str>)> {
    match shell_integration::detect_kind(shell, os) {
        ShellKind::Zsh | ShellKind::Bash => Some((SH_SCRIPT, Some(SCRIPT_NAME))),
        ShellKind::Fish => Some((FISH_SCRIPT, None)),
        _ => {
            let name = shell.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
            matches!(name.as_str(), "sh" | "dash" | "ksh").then_some((SH_SCRIPT, Some(SCRIPT_NAME)))
        }
    }
}

/// The absolute path of the file Windows is to run for `program`: an absolute path as it is, a
/// bare name searched in the child's `PATH` (as it is, then with each `PATHEXT` extension). Empty
/// and relative `PATH` entries and relative paths are never used: they would find a file in
/// whatever directory the app happens to be in.
fn windows_program(
    program: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    env: &[(String, String)],
) -> Result<PathBuf, Refused> {
    // The child's variable: the last one added, else the inherited one.
    let var = |key: &str| {
        env.iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .or_else(|| lookup(key))
    };
    // A colon in the file name is an NTFS stream (`x.bat::$DATA`), which would hide the
    // extension from the check below.
    if program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.contains(':'))
    {
        return Err(Refused::NotFound);
    }
    let path = Path::new(program);
    let found = if path.is_absolute() {
        path.is_file().then(|| path.to_path_buf())
    } else if program.contains(['/', '\\', ':']) {
        None
    } else {
        let extensions = var("PATHEXT").unwrap_or_else(|| ".EXE".to_owned());
        let extensions: Vec<&str> = extensions
            .split(';')
            .filter(|e| e.len() > 1 && e.starts_with('.'))
            .collect();
        var("PATH").and_then(|dirs| {
            std::env::split_paths(&dirs)
                .filter(|dir| dir.is_absolute())
                .find_map(|dir| {
                    std::iter::once("")
                        .chain(extensions.iter().copied())
                        .map(|ext| dir.join(format!("{program}{ext}")))
                        .find(|candidate| candidate.is_file())
                })
        })
    };
    let found = found.ok_or(Refused::NotFound)?;
    if is_batch_file(&found.to_string_lossy()) {
        return Err(Refused::BatchFile);
    }
    Ok(found)
}

/// Whether Windows would run the file at `path` through `cmd.exe`: a `.bat` or `.cmd` name, in
/// any case, once the trailing dots and spaces Windows ignores are gone.
fn is_batch_file(path: &str) -> bool {
    let name = path.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name.ends_with(".bat") || name.ends_with(".cmd")
}

/// Starts a program tab's program on `backend`, at `size`, with the app's environment and `tab`
/// ([`tab_env`]). Returns the PTY and the name of what was started (the shell, when it goes
/// through one).
pub fn launch_program(
    backend: &dyn PtyBackend,
    settings: &Settings,
    size: PtySize,
    spec: &ProgramSpec,
    tab: &[(String, String)],
) -> Result<(Box<dyn Pty>, String), ProgramFailure> {
    let env = child_env(settings, &process_env, Os::HOST, tab);
    let options = program_spawn(settings, Os::HOST, &process_env, spec, &env)
        .map_err(ProgramFailure::Refused)?;
    first_spawned(&[options], |o| backend.spawn(o, size)).map_err(ProgramFailure::Spawn)
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
    use std::cell::RefCell;

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

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // ------------------------------------------------------------------ the tab's variables

    #[test]
    fn every_tab_variable_is_set_even_when_it_has_nothing_to_say() {
        assert_eq!(
            tab_env("", "", u64::MAX),
            pairs(&[
                ("LUNETERM_BIN", ""),
                ("LUNETERM_INSTANCE", ""),
                ("LUNETERM_TAB_ID", "18446744073709551615"),
            ])
        );
    }

    #[test]
    fn only_a_relative_config_dir_is_rewritten_and_it_becomes_absolute() {
        let s = Settings::default();
        let dir = |set: &[(&str, &str)]| {
            let env = child_env(&s, &lookup(set), Os::HOST, &[]);
            value(&env, "LUNETERM_CONFIG_DIR").map(str::to_owned)
        };
        assert_eq!(dir(&[]), None);
        assert_eq!(dir(&[("LUNETERM_CONFIG_DIR", "")]), None);
        let absolute = std::env::temp_dir();
        let absolute = absolute.to_str().expect("utf-8 temp dir");
        assert_eq!(
            dir(&[("LUNETERM_CONFIG_DIR", absolute)]),
            None,
            "inherited as it is"
        );
        let got = dir(&[("LUNETERM_CONFIG_DIR", "trial/cfg")]).expect("rewritten");
        let want = std::env::current_dir().expect("cwd").join("trial/cfg");
        assert!(Path::new(&got).is_absolute());
        assert_eq!(Path::new(&got), want);
    }

    #[test]
    fn the_verbatim_prefix_of_a_windows_path_is_dropped() {
        for (given, want) in [
            (r"\\?\C:\Apps\luneterm.exe", r"C:\Apps\luneterm.exe"),
            (r"\\?\UNC\host\share\l.exe", r"\\host\share\l.exe"),
            (r"\\?\Volume{x}\l.exe", r"\\?\Volume{x}\l.exe"),
            (r"C:\Apps\luneterm.exe", r"C:\Apps\luneterm.exe"),
            ("/usr/bin/luneterm", "/usr/bin/luneterm"),
        ] {
            assert_eq!(without_verbatim(given), want, "{given}");
        }
    }

    #[test]
    fn the_own_binary_is_the_canonical_path_of_this_executable() {
        let exe = std::env::current_exe().expect("exe");
        let exe = std::fs::canonicalize(exe).expect("canonical");
        let own = own_binary();
        assert_eq!(std::fs::canonicalize(&own).expect("exists"), exe);
        assert!(!own.starts_with(r"\\?\"), "{own}");
    }

    // ------------------------------------------------------------------ program tabs

    /// A backend that records what it is asked to spawn and starts nothing.
    #[derive(Default)]
    struct Refusing(RefCell<Vec<SpawnOptions>>);

    impl PtyBackend for Refusing {
        fn spawn(&self, options: &SpawnOptions, _: PtySize) -> io::Result<Box<dyn Pty>> {
            self.0.borrow_mut().push(options.clone());
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }

    const SIZE: PtySize = PtySize {
        columns: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    };

    fn spec(cwd: &str, argv: &[&str]) -> ProgramSpec {
        ProgramSpec {
            cwd: PathBuf::from(cwd),
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    const WORDS: [&str; 9] = [
        "/opt/prog",
        "two words",
        "$(echo x)",
        "\"q\"",
        "a\\b",
        "%PATH%",
        "",
        "--",
        "-n; rm -rf ~",
    ];

    #[test]
    fn a_program_tab_goes_through_the_login_shell_with_each_word_its_own_argument() {
        let env = pairs(&[("TERM", "xterm-256color")]);
        let cwd = "/work/a dir; $(echo no)";
        let with_args = Settings {
            shell_args: "--never -used".into(),
            shell_integration: true,
            ..Settings::default()
        };
        for shell in [
            "/bin/zsh",
            "/usr/local/bin/bash",
            "/bin/sh",
            "/usr/bin/dash",
            "/bin/ksh",
            "/usr/bin/fish",
        ] {
            for os in [Os::Macos, Os::Linux] {
                let set = [("SHELL", shell)];
                let got = program_spawn(&with_args, os, &lookup(&set), &spec(cwd, &WORDS), &env)
                    .expect("spawn");
                let mut want = vec!["-l", "-i", "-c"];
                if shell.ends_with("fish") {
                    want.push("cd -- $argv[1]; or exit 127; set -e argv[1]; exec $argv");
                } else {
                    want.push("cd -- \"$1\" || exit 127; shift; exec \"$@\"");
                    want.push("luneterm");
                }
                want.push(cwd);
                want.extend(WORDS);
                assert_eq!(got.program.as_deref(), Some(Path::new(shell)), "{os:?}");
                assert_eq!(got.args, want, "{shell} on {os:?}");
                assert_eq!(got.cwd.as_deref(), Some(Path::new(cwd)));
                assert_eq!(got.env, env, "no shell integration");
            }
        }
    }

    #[test]
    fn the_program_tab_shell_is_the_setting_then_an_absolute_shell_then_sh() {
        let shell_of = |setting: &str, set: &[(&str, &str)]| {
            let got = program_spawn(
                &settings(setting, "", ""),
                Os::Linux,
                &lookup(set),
                &spec("/", &["prog"]),
                &[],
            )
            .expect("spawn");
            got.program.expect("a program").display().to_string()
        };
        let zsh = [("SHELL", "/bin/zsh")];
        assert_eq!(shell_of(" /bin/bash ", &zsh), "/bin/bash");
        assert_eq!(shell_of("fish", &zsh), "fish");
        assert_eq!(shell_of("", &zsh), "/bin/zsh");
        assert_eq!(shell_of("   ", &zsh), "/bin/zsh");
        assert_eq!(shell_of("", &[]), "/bin/sh");
        assert_eq!(shell_of("", &[("SHELL", "")]), "/bin/sh");
        assert_eq!(shell_of("", &[("SHELL", "zsh")]), "/bin/sh");
        assert_eq!(shell_of("", &[("SHELL", "./zsh")]), "/bin/sh");
    }

    #[test]
    fn another_shell_is_bypassed_and_the_program_started_itself() {
        let env = pairs(&[("TERM", "xterm-256color")]);
        for shell in [
            "/usr/bin/nu",
            "/bin/tcsh",
            "/bin/csh",
            "/usr/bin/xonsh",
            "/bin/elvish",
            "/opt/pwsh",
            "/bin/rbash",
        ] {
            let got = program_spawn(
                &settings(shell, "-x", ""),
                Os::Linux,
                &lookup(&[]),
                &spec("/work", &["-odd", "a b", ""]),
                &env,
            )
            .expect("spawn");
            assert_eq!(got.program.as_deref(), Some(Path::new("-odd")), "{shell}");
            assert_eq!(got.args, ["a b", ""], "{shell}");
            assert_eq!(got.cwd.as_deref(), Some(Path::new("/work")));
            assert_eq!(got.env, env);
        }
    }

    #[test]
    fn no_program_or_one_exec_would_read_as_an_option_is_refused() {
        let run = |argv: &[&str]| {
            program_spawn(
                &settings("/bin/bash", "", ""),
                Os::Linux,
                &lookup(&[]),
                &spec("/", argv),
                &[],
            )
            .map(|_| ())
        };
        for argv in [&[][..], &[""], &["-c", "env"], &["-a", "x"], &["-"]] {
            assert_eq!(run(argv), Err(Refused::NotFound), "{argv:?}");
        }
        for os in [Os::Macos, Os::Windows] {
            let got = program_spawn(
                &Settings::default(),
                os,
                &lookup(&[]),
                &spec("/", &[""]),
                &[],
            );
            assert_eq!(got.map(|_| ()), Err(Refused::NotFound), "{os:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_is_not_unicode_cannot_go_through_a_shell() {
        use std::os::unix::ffi::OsStringExt;
        let spec = ProgramSpec {
            cwd: PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/\xff".to_vec())),
            argv: vec!["prog".into()],
        };
        let got = program_spawn(&Settings::default(), Os::Linux, &lookup(&[]), &spec, &[]);
        assert_eq!(got.map(|_| ()), Err(Refused::Directory));
    }

    // ------------------------------------------------------------------ Windows resolution
    // The lookup is host-independent apart from what an absolute path is, so these run anywhere.

    /// A directory this test creates, with `files` in it.
    fn bin_dir(tag: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luneterm-spawn-{tag}-{}", std::process::id()));
        assert!(!dir.exists(), "{} already exists", dir.display());
        std::fs::create_dir(&dir).expect("create the scratch dir");
        for file in files {
            std::fs::write(dir.join(file), b"").expect("write");
        }
        dir
    }

    fn windows(
        program: &str,
        set: &[(&str, &str)],
        env: &[(String, String)],
    ) -> Result<SpawnOptions, Refused> {
        program_spawn(
            // The shell setting does not matter on Windows.
            &settings("pwsh.exe", "-x", ""),
            Os::Windows,
            &lookup(set),
            &spec("/work", &[program, "a b", "%PATH%", ""]),
            env,
        )
    }

    fn found(got: Result<SpawnOptions, Refused>) -> PathBuf {
        got.expect("resolves").program.expect("a program")
    }

    #[test]
    fn windows_starts_the_program_itself_by_the_path_found_in_path_and_pathext() {
        let dir = bin_dir("win", &["tool.exe", "tool.com", "plain", "only.zz"]);
        let path = dir.to_str().expect("utf-8");
        let set = [("PATH", path), ("PATHEXT", ".com;.exe")];
        let got = windows("tool", &set, &[]).expect("resolves");
        assert_eq!(got.program.as_deref(), Some(dir.join("tool.com").as_path()));
        assert_eq!(got.args, ["a b", "%PATH%", ""], "no shell in between");
        assert_eq!(got.cwd.as_deref(), Some(Path::new("/work")));
        assert_eq!(found(windows("tool.exe", &set, &[])), dir.join("tool.exe"));
        assert_eq!(found(windows("plain", &set, &[])), dir.join("plain"));
        // PATHEXT unset is `.EXE`; entries that are empty or lack the dot are skipped.
        let upper = bin_dir("win-upper", &["up.EXE"]);
        let up = [("PATH", upper.to_str().expect("utf-8"))];
        assert_eq!(found(windows("up", &up, &[])), upper.join("up.EXE"));
        let odd = [("PATH", path), ("PATHEXT", ";zz;.;.zz")];
        assert_eq!(found(windows("only", &odd, &[])), dir.join("only.zz"));
        let none = [("PATH", path), ("PATHEXT", "")];
        assert_eq!(
            windows("only", &none, &[]).map(|_| ()),
            Err(Refused::NotFound)
        );
        // An absolute path is used as it is, with no PATH at all.
        let full = dir.join("tool.exe");
        assert_eq!(
            found(windows(full.to_str().expect("utf-8"), &[], &[])),
            full
        );
        // The variable the child gets wins over the app's, whatever its case.
        let other = [("PATH", "/nowhere-5f3a")];
        let child = pairs(&[("Path", path)]);
        assert_eq!(
            found(windows("tool.exe", &other, &child)),
            dir.join("tool.exe")
        );
        // The directories are tried in order.
        let both = std::env::join_paths([&upper, &dir]).expect("join");
        let both = [("PATH", both.to_str().expect("utf-8"))];
        assert_eq!(found(windows("tool.exe", &both, &[])), dir.join("tool.exe"));
        std::fs::remove_dir_all(&dir).expect("remove");
        std::fs::remove_dir_all(&upper).expect("remove");
    }

    #[test]
    fn windows_never_looks_in_the_current_directory_or_through_a_relative_path() {
        // The test runs in the crate's directory, which holds this file.
        assert!(Path::new("Cargo.toml").is_file() && Path::new("src/spawn.rs").is_file());
        for path in ["", ".", "src"] {
            let set = [("PATH", path)];
            for program in ["Cargo.toml", "spawn.rs"] {
                assert_eq!(
                    windows(program, &set, &[]).map(|_| ()),
                    Err(Refused::NotFound),
                    "{program} with PATH={path:?}"
                );
            }
        }
        let dir = bin_dir("win-rel", &["tool.exe"]);
        let set = [("PATH", dir.to_str().expect("utf-8"))];
        for program in [
            "src/spawn.rs",
            "./Cargo.toml",
            r".\tool.exe",
            r"sub\tool.exe",
            "C:tool.exe",
            "missing",
            "..",
        ] {
            assert_eq!(
                windows(program, &set, &[]).map(|_| ()),
                Err(Refused::NotFound),
                "{program}"
            );
        }
        assert_eq!(
            windows("tool.exe", &[], &[]).map(|_| ()),
            Err(Refused::NotFound),
            "no PATH"
        );
        let missing = dir.join("nope.exe");
        for program in [&dir, &missing] {
            assert_eq!(
                windows(program.to_str().expect("utf-8"), &set, &[]).map(|_| ()),
                Err(Refused::NotFound),
                "a directory or a missing file is no program"
            );
        }
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn windows_refuses_batch_files_however_they_are_named() {
        let dir = bin_dir(
            "win-bat",
            &[
                "run.cmd",
                "old.bat",
                "Up.CMD",
                "mix.BaT",
                "dots.cmd. .",
                "x.exe",
            ],
        );
        let set = [
            ("PATH", dir.to_str().expect("utf-8")),
            ("PATHEXT", ".exe;.bat;.cmd"),
        ];
        for program in [
            "run",
            "run.cmd",
            "old",
            "old.bat",
            "Up.CMD",
            "mix.BaT",
            "dots.cmd. .",
        ] {
            assert_eq!(
                windows(program, &set, &[]).map(|_| ()),
                Err(Refused::BatchFile),
                "{program}"
            );
            let full = dir.join(program);
            if full.is_file() {
                assert_eq!(
                    windows(full.to_str().expect("utf-8"), &[], &[]).map(|_| ()),
                    Err(Refused::BatchFile),
                    "{program} by its path"
                );
            }
        }
        // An NTFS stream name would hide the extension; it is no program here.
        for program in ["run.cmd::$DATA", "x.exe:y"] {
            std::fs::write(dir.join(program), b"").expect("write");
            let full = dir.join(program);
            for given in [program, full.to_str().expect("utf-8")] {
                assert_eq!(
                    windows(given, &set, &[]).map(|_| ()),
                    Err(Refused::NotFound),
                    "{given}"
                );
            }
        }
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn a_batch_name_is_judged_without_its_trailing_dots_and_spaces() {
        for name in [
            "a.bat",
            "a.cmd",
            "A.CMD",
            "a.Bat.",
            "a.cmd ",
            "a.cmd . . ",
            r"C:\x y\a.bat...",
            ".cmd",
            "a.exe.cmd",
        ] {
            assert!(is_batch_file(name), "{name:?}");
        }
        for name in [
            "a.exe",
            "a.com",
            "a.cmd.exe",
            "acmd",
            "a.bat/x",
            "bat",
            "a.cmdx",
            r"C:\a.cmd\tool.exe",
            "",
        ] {
            assert!(!is_batch_file(name), "{name:?}");
        }
    }

    #[test]
    fn a_program_tab_is_launched_with_the_tab_variables_and_a_refusal_spawns_nothing() {
        let settings = Settings::default();
        let tab = tab_env("/opt/lt", "token-0123456789", 3);
        let cwd = std::env::temp_dir();
        let program = std::env::current_exe().expect("exe");
        let backend = Refusing::default();
        let spec = ProgramSpec {
            cwd: cwd.clone(),
            argv: vec![program.to_str().expect("utf-8").to_owned()],
        };
        let got = launch_program(&backend, &settings, SIZE, &spec, &tab);
        assert!(
            matches!(got, Err(ProgramFailure::Spawn(ref f)) if f.kind == io::ErrorKind::NotFound)
        );
        {
            let asked = backend.0.borrow();
            assert_eq!(asked.len(), 1, "one program, no other candidate");
            assert_eq!(asked[0].cwd.as_deref(), Some(cwd.as_path()));
            assert_eq!(value(&asked[0].env, "TERM"), Some("xterm-256color"));
            assert_eq!(value(&asked[0].env, "LUNETERM_BIN"), Some("/opt/lt"));
            assert_eq!(
                value(&asked[0].env, "LUNETERM_INSTANCE"),
                Some("token-0123456789")
            );
            assert_eq!(value(&asked[0].env, "LUNETERM_TAB_ID"), Some("3"));
        }
        let none = ProgramSpec { cwd, argv: vec![] };
        let backend = Refusing::default();
        let got = launch_program(&backend, &settings, SIZE, &none, &tab);
        assert!(matches!(
            got,
            Err(ProgramFailure::Refused(Refused::NotFound))
        ));
        assert!(backend.0.borrow().is_empty());
    }

    #[test]
    fn every_shell_candidate_is_launched_with_the_tab_variables() {
        let backend = Refusing::default();
        let settings = Settings {
            shell_integration: false,
            ..Settings::default()
        };
        let tab = tab_env("/opt/lt", "token-0123456789", 4);
        let got = launch_with(&backend, &settings, SIZE, None, None, &tab);
        assert!(got.is_err());
        let asked = backend.0.borrow();
        assert_eq!(asked.len(), candidates(&settings, Os::HOST).len());
        for options in asked.iter() {
            for (key, want) in &tab {
                assert_eq!(value(&options.env, key), Some(want.as_str()), "{key}");
            }
        }
    }

    // ------------------------------------------------------------------ shell integration

    fn fake_scripts() -> shell_integration::Install {
        // Only its text is used; nothing here touches a disk.
        shell_integration::install_for_tests("/cfg/shell-integration")
    }

    fn var_of<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> shell_integration::Var + 'a {
        move |k| {
            set.iter()
                .find(|(name, _)| *name == k)
                .map_or(shell_integration::Var::Unset, |(_, v)| {
                    shell_integration::Var::Set((*v).to_owned())
                })
        }
    }

    /// With the setting off, the spawn list is exactly the candidates with the app's environment
    /// and directory: the scripts are never asked for, whatever shell and arguments. The
    /// environment is the terminal's identity and the tab's three variables, and nothing else.
    #[test]
    fn with_integration_off_the_spawn_is_identical_to_one_without_it() {
        let env = child_env(
            &Settings::default(),
            &lookup(&[("LANG", "en_US.UTF-8")]),
            Os::Macos,
            &tab_env("/opt/lt/luneterm", "AbCdEf0123456789-_AbCd", 7),
        );
        assert_eq!(
            env,
            pairs(&[
                ("TERM", "xterm-256color"),
                ("COLORTERM", "truecolor"),
                ("TERM_PROGRAM", "Luneterm"),
                ("TERM_PROGRAM_VERSION", VERSION),
                ("LUNETERM_BIN", "/opt/lt/luneterm"),
                ("LUNETERM_INSTANCE", "AbCdEf0123456789-_AbCd"),
                ("LUNETERM_TAB_ID", "7"),
            ])
        );
        let cwd = std::env::temp_dir();
        for os in [Os::Macos, Os::Linux, Os::Windows] {
            for (shell, args) in [
                ("", ""),
                ("/bin/zsh", "-l"),
                ("/bin/bash", ""),
                ("/bin/bash", "-l"),
                ("pwsh.exe", ""),
                ("/usr/bin/fish", ""),
            ] {
                let off = Settings {
                    shell: shell.into(),
                    shell_args: args.into(),
                    shell_integration: false,
                    ..Settings::default()
                };
                let mut never = || -> Option<shell_integration::Install> {
                    panic!("integration is off: the scripts must not be asked for")
                };
                let got = spawn_list(
                    &off,
                    os,
                    &env,
                    Some(&cwd),
                    &var_of(&[("SHELL", "/bin/zsh"), ("HOME", "/h")]),
                    &mut never,
                );
                let mut want = candidates(&off, os);
                for o in &mut want {
                    o.env.clone_from(&env);
                    o.cwd = Some(cwd.clone());
                }
                let fields = |l: &[SpawnOptions]| -> Vec<_> {
                    l.iter()
                        .map(|o| {
                            (
                                o.program.clone(),
                                o.args.clone(),
                                o.cwd.clone(),
                                o.env.clone(),
                            )
                        })
                        .collect()
                };
                assert_eq!(fields(&got), fields(&want), "{shell:?} {args:?} on {os:?}");
            }
        }
    }

    #[test]
    fn with_integration_on_a_known_shell_gets_its_scripts_and_the_rest_is_unchanged() {
        let env = vec![("TERM".to_owned(), "xterm-256color".to_owned())];
        let scripts = fake_scripts();
        let on = |shell: &str, args: &str| Settings {
            shell: shell.into(),
            shell_args: args.into(),
            ..Settings::default()
        };
        let home = if cfg!(windows) {
            "C:\\Users\\ann".to_owned()
        } else {
            "/Users/ann".to_owned()
        };
        let vars = [("SHELL", "/bin/zsh"), ("HOME", home.as_str())];
        let var = var_of(&vars);
        let mut asked = 0;
        let mut install = || {
            asked += 1;
            Some(scripts.clone())
        };
        // The login shell is zsh: ZDOTDIR and the user's directory join the environment.
        let list = spawn_list(&on("", ""), Os::Macos, &env, None, &var, &mut install);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].program, None);
        assert!(list[0].args.is_empty());
        assert_eq!(value(&list[0].env, "TERM"), Some("xterm-256color"));
        assert!(value(&list[0].env, "ZDOTDIR").is_some());
        assert_eq!(
            value(&list[0].env, "LUNETERM_USER_ZDOTDIR"),
            Some(home.as_str())
        );
        // An explicit bash with the default arguments gets the rc file.
        let list = spawn_list(
            &on("/bin/bash", "-l"),
            Os::Linux,
            &env,
            None,
            &var,
            &mut install,
        );
        assert_eq!(list[0].program.as_deref(), Some(Path::new("/bin/bash")));
        assert_eq!(list[0].args[0], "--rcfile");
        assert_eq!(list[0].args[2], "-i");
        assert_eq!(list[0].env, env, "bash needs no environment");
        // Windows tries pwsh, then powershell, then cmd: the first two get the script, cmd none.
        let list = spawn_list(&on("", ""), Os::Windows, &env, None, &var, &mut install);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].args[0], "-NoExit");
        assert_eq!(list[1].args[0], "-NoExit");
        assert!(list[2].args.is_empty() && list[2].env == env);
        assert!(asked >= 3);
        // fish is not integrated, and does not even write the scripts.
        let mut never = || -> Option<shell_integration::Install> { panic!("fish: no scripts") };
        let list = spawn_list(
            &on("/usr/bin/fish", ""),
            Os::Linux,
            &env,
            None,
            &var,
            &mut never,
        );
        assert_eq!(list[0].env, env);
        assert!(list[0].args.is_empty());
        // The scripts could not be installed: every candidate is as without integration.
        let mut none = || None;
        let list = spawn_list(&on("", ""), Os::Macos, &env, None, &var, &mut none);
        assert_eq!(list[0].env, env);
    }
}
