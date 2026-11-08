//! Windows parts of the portable-pty backend.

#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::thread;

use portable_pty::Child;
use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, OSVERSIONINFOW};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

/// Outcome of the one `SetDefaultDllDirectories` call: `Err` holds the OS error code. An
/// `io::Error` is not `Clone`, so the code is kept and the error rebuilt on each call.
static DLL_SEARCH: OnceLock<Result<(), i32>> = OnceLock::new();

/// Restricts the process-wide DLL search to the application directory, System32 and directories
/// added with `AddDllDirectory`, once. portable-pty loads `conpty.dll` by bare name; with the
/// default search order that finds a planted copy in the current directory or on `PATH` first.
///
/// Process-wide and permanent: every later bare-name `LoadLibrary` in this process skips the
/// current directory and `PATH`. Fails closed: the call's error is returned on every attempt,
/// and the backend then refuses to spawn.
pub(crate) fn ensure_dll_search_restricted() -> io::Result<()> {
    let result = DLL_SEARCH.get_or_init(|| {
        // SAFETY: plain Win32 call with a valid flag constant and no pointers.
        if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error().raw_os_error().unwrap_or(-1))
        }
    });
    match result {
        Ok(()) => Ok(()),
        Err(code) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("SetDefaultDllDirectories failed (os error {code}); refusing to load conpty"),
        )),
    }
}

/// The OS build number from `RtlGetVersion`. `GetVersionExW` is not used: without an application
/// manifest it reports a stale version.
pub(crate) fn os_build() -> Option<u32> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..OSVERSIONINFOW::default()
    };
    // SAFETY: `info` is a valid, writable OSVERSIONINFOW with its size field set, and outlives
    // the call; `RtlGetVersion` only writes into it.
    let status = unsafe { RtlGetVersion(&mut info) };
    // STATUS_SUCCESS is 0; any negative NTSTATUS is a failure.
    (status == 0 && info.dwBuildNumber != 0).then_some(info.dwBuildNumber)
}

/// Starts the exit-watcher thread: it waits on a duplicate of the child's process handle and
/// calls `on_exit` once the child has exited. If the wait itself fails, `on_exit` is not called
/// (the pseudoconsole is then closed when the `Pty` drops, on its reap thread).
pub(crate) fn watch_exit(
    child: &(dyn Child + Send + Sync),
    on_exit: impl FnOnce() + Send + 'static,
) -> io::Result<()> {
    let raw = child
        .as_raw_handle()
        .ok_or_else(|| io::Error::other("child has no process handle"))?;
    // SAFETY: `raw` is the process handle owned by `child`, which is alive for this whole
    // function; the borrow is used only to duplicate it into a handle the thread owns.
    let process: OwnedHandle = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()?;
    thread::Builder::new()
        .name("lune-pty-exit".into())
        .spawn(move || {
            // SAFETY: `process` is an owned, open process handle that lives until the end of
            // this closure, so it stays valid for the whole wait.
            let result =
                unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, INFINITE) };
            drop(process);
            if result == WAIT_OBJECT_0 {
                on_exit();
            }
        })?;
    Ok(())
}

/// Resolves what to spawn to an absolute path of an existing file, so portable-pty never hands
/// `CreateProcessW` a relative `lpApplicationName`. Windows completes a relative name there with
/// the host process's current directory (not [`crate::SpawnOptions::cwd`]) and does not search
/// `PATH`, so a relative name would run a copy planted in that directory.
///
/// - `None`: the effective `ComSpec` if it is absolute, else `<GetSystemDirectoryW>\cmd.exe`.
/// - An absolute path is used as is.
/// - A bare file name (`pwsh.exe`, `pwsh`) is searched in the absolute, non-empty entries of the
///   effective `PATH` only, first as given, then with each `PATHEXT` entry that is non-empty and
///   starts with `.` appended (`.EXE` when `PATHEXT` is unset).
/// - Anything else (`.\x.exe`, `dir\x.exe`, `C:x.exe`, `\x.exe`) is relative to the process's
///   current directory and refused.
///
/// "Effective" means the child's environment: the last `env` entry whose key matches
/// (case-insensitively), else the inherited variable. Every failure is `NotFound`.
pub(crate) fn resolve_program(
    program: Option<&Path>,
    env: &[(String, String)],
) -> io::Result<PathBuf> {
    let var = |key: &str| -> Option<OsString> {
        match env.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some((_, value)) => Some(value.into()),
            None => std::env::var_os(key),
        }
    };
    match program {
        None => {
            let comspec = var("ComSpec")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute());
            let shell = match comspec {
                Some(path) => path,
                None => system_directory()?.join("cmd.exe"),
            };
            existing_file(shell)
        }
        Some(path) if path.is_absolute() => existing_file(path.to_path_buf()),
        Some(name) if is_bare_name(name) => search_path(
            name.as_os_str(),
            var("PATH").as_deref(),
            var("PATHEXT").as_deref(),
        )
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "program {} not found in the absolute PATH entries",
                    name.display()
                ),
            )
        }),
        Some(path) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "program {} is relative to the current directory; give an absolute path or a \
                 bare file name",
                path.display()
            ),
        )),
    }
}

fn existing_file(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("program {} is not an existing file", path.display()),
        ))
    }
}

/// One normal component and nothing else: no prefix, root, `.`/`..` or separator.
fn is_bare_name(path: &Path) -> bool {
    let mut components = path.components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// The `PATH` search for a bare `name`; `None` if nothing matches. Empty and relative entries
/// are skipped (both would resolve against the current directory), as are `PATHEXT` entries
/// that are empty or lack the leading `.`.
fn search_path(name: &OsStr, path: Option<&OsStr>, pathext: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    let extensions: Vec<OsString> = match pathext {
        None => vec![".EXE".into()],
        Some(list) => {
            let wide: Vec<u16> = list.encode_wide().collect();
            wide.split(|&c| c == u16::from(b';'))
                .filter(|ext| ext.first() == Some(&u16::from(b'.')) && ext.len() > 1)
                .map(OsString::from_wide)
                .collect()
        }
    };
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute())
        .find_map(|dir| {
            let exact = dir.join(name);
            if exact.is_file() {
                return Some(exact);
            }
            extensions.iter().find_map(|ext| {
                let mut file = name.to_os_string();
                file.push(ext);
                let candidate = dir.join(file);
                candidate.is_file().then_some(candidate)
            })
        })
}

/// `GetSystemDirectoryW`, e.g. `C:\Windows\System32`. Not taken from `%SystemRoot%`, which
/// the environment controls.
fn system_directory() -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 260];
    loop {
        let capacity = u32::try_from(buf.len()).map_err(|_| io::Error::other("path too long"))?;
        // SAFETY: `buf` is writable for `capacity` u16s and outlives the call.
        let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), capacity) } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < buf.len() {
            let dir = PathBuf::from(OsString::from_wide(&buf[..len]));
            if !dir.is_absolute() {
                return Err(io::Error::other(
                    "GetSystemDirectoryW returned a relative path",
                ));
            }
            return Ok(dir);
        }
        // Too small: `len` is the size needed, including the terminating NUL.
        buf.resize(len, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "lune-pty-resolve-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).expect("create temp dir");
        dir
    }

    fn touch(path: &Path) {
        std::fs::write(path, b"").expect("write file");
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn bare_name_found_only_in_absolute_entries() {
        let dir = temp_dir("abs");
        touch(&dir.join("lunetool.exe"));
        let path = format!(";.;{};", dir.display());
        // PATHEXT pinned: the inherited one is upper case and the file system ignores case.
        let got = resolve_program(
            Some(Path::new("lunetool")),
            &env(&[("PATH", &path), ("PATHEXT", ".exe")]),
        )
        .expect("resolve");
        assert_eq!(got, dir.join("lunetool.exe"));
        let got = resolve_program(Some(Path::new("lunetool.exe")), &env(&[("Path", &path)]))
            .expect("resolve exact");
        assert_eq!(got, dir.join("lunetool.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entries_relative_to_the_current_dir_are_skipped() {
        // The current dir has the file; empty and relative entries would find it there.
        let cwd = std::env::current_dir().expect("current dir");
        let name = std::fs::read_dir(&cwd)
            .expect("read current dir")
            .filter_map(Result::ok)
            .find(|e| e.path().is_file())
            .map(|e| e.file_name())
            .expect("a file in the current dir");
        assert!(cwd.join(&name).is_file());
        for path in ["", ";", ".", r".\", ";.;;"] {
            let err = resolve_program(Some(Path::new(&name)), &env(&[("PATH", path)]))
                .expect_err("cwd-relative entries must not resolve");
            assert_eq!(err.kind(), io::ErrorKind::NotFound, "PATH={path:?}");
        }
    }

    #[test]
    fn pathext_empty_and_dotless_entries_are_skipped() {
        let dir = temp_dir("ext");
        touch(&dir.join("lunetool.CMD"));
        touch(&dir.join("lunetoolEXE"));
        let path = dir.display().to_string();
        // An empty entry made portable-pty 0.9.0 panic (`&ext[1..]`); here it is skipped.
        let pathext = ";EXE;;.CMD;";
        let got = resolve_program(
            Some(Path::new("lunetool")),
            &env(&[("PATH", &path), ("PATHEXT", pathext)]),
        )
        .expect("resolve via .CMD");
        assert_eq!(got, dir.join("lunetool.CMD"));
        let err = resolve_program(
            Some(Path::new("lunetool")),
            &env(&[("PATH", &path), ("PATHEXT", ";EXE;.")]),
        )
        .expect_err("dotless and bare-dot entries are skipped");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_env_entry_wins_case_insensitively() {
        let dir = temp_dir("env");
        touch(&dir.join("lunetool.exe"));
        let path = dir.display().to_string();
        let got = resolve_program(
            Some(Path::new("lunetool.exe")),
            &env(&[("PATH", ""), ("pAtH", &path)]),
        )
        .expect("later entry wins");
        assert_eq!(got, dir.join("lunetool.exe"));
        let err = resolve_program(
            Some(Path::new("lunetool.exe")),
            &env(&[("pAtH", &path), ("PATH", "")]),
        )
        .expect_err("empty PATH finds nothing");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_paths_with_directories_are_refused() {
        for program in [
            r".\cmd.exe",
            r"..\cmd.exe",
            r"sub\cmd.exe",
            r"C:cmd.exe",
            r"\cmd.exe",
        ] {
            let err = resolve_program(Some(Path::new(program)), &[])
                .expect_err("relative program must be refused");
            assert_eq!(err.kind(), io::ErrorKind::NotFound, "{program}");
        }
    }

    #[test]
    fn absolute_program_must_exist() {
        let dir = temp_dir("absprog");
        let file = dir.join("lunetool.exe");
        let err = resolve_program(Some(&file), &[]).expect_err("missing");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        touch(&file);
        assert_eq!(resolve_program(Some(&file), &[]).expect("exists"), file);
        let err = resolve_program(Some(&dir), &[]).expect_err("a directory is not a program");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_shell_falls_back_to_system_cmd() {
        let system_cmd = system_directory().expect("system dir").join("cmd.exe");
        assert!(system_cmd.is_absolute() && system_cmd.is_file());
        for comspec in ["", "cmd.exe", r"sub\cmd.exe", r"\cmd.exe"] {
            let got = resolve_program(None, &env(&[("ComSpec", comspec)])).expect("fallback");
            assert_eq!(got, system_cmd, "ComSpec={comspec:?}");
        }
        let got = resolve_program(
            None,
            &env(&[("COMSPEC", &system_cmd.display().to_string())]),
        )
        .expect("absolute ComSpec");
        assert_eq!(got, system_cmd);
    }
}
