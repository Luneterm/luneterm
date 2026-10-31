//! Windows DLL search hardening (M3 step 3, decision D4). `harness = false`: Windows only, other
//! platforms print "skipped".
//!
//! portable-pty loads `conpty.dll` by bare name. With the default search order a copy planted in
//! the current directory or on `PATH` would be loaded. The backend calls
//! `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` before its first spawn; this test
//! proves that with three self-re-exec children, each with `System32\version.dll` planted as
//! `conpty.dll` in a unique temp dir:
//!
//! - `control`: no backend, current dir = the temp dir; the bare-name load must find the plant
//!   (proves the instrument can see a planted DLL at all);
//! - `hardened-cwd`: current dir = the temp dir; after a backend spawn the plant must not load;
//! - `hardened-path`: temp dir first on `PATH`, current dir elsewhere; same assertion.

#[cfg(not(windows))]
fn main() {
    println!("dll_search: skipped (Windows only)");
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code)]

    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use lune_pty::{PortablePtyBackend, PtyBackend, PtySize, SpawnOptions};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, LoadLibraryW};

    pub(super) fn main() {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("child") => child(args.get(2).map(String::as_str).unwrap_or("")),
            _ => parent(),
        }
    }

    // ---- child -------------------------------------------------------------------------------

    /// Prints `RESULT <path>` or `RESULT NULL <os error>` for a bare-name `LoadLibraryW`.
    fn child(mode: &str) {
        match mode {
            "control" => {}
            "hardened" => {
                let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
                let options = SpawnOptions {
                    program: Some(PathBuf::from(comspec)),
                    args: vec!["/d".into(), "/c".into(), "exit 0".into()],
                    ..SpawnOptions::default()
                };
                let size = PtySize {
                    columns: 80,
                    rows: 24,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                // Keep the Pty alive until after the load; dropping it only reaps off-thread.
                match PortablePtyBackend::new().spawn(&options, size) {
                    Ok(pty) => {
                        println!("SPAWN ok");
                        load_conpty();
                        drop(pty);
                    }
                    Err(e) => {
                        println!("SPAWN err {:?}", e.kind());
                        std::process::exit(1);
                    }
                }
                return;
            }
            other => {
                println!("unknown child mode {other:?}");
                std::process::exit(2);
            }
        }
        load_conpty();
    }

    fn load_conpty() {
        let name: Vec<u16> = OsStr::new("conpty.dll")
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            println!(
                "RESULT NULL {}",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            );
            return;
        }
        let mut buf = vec![0u16; 32768];
        // SAFETY: `module` is a live module handle (never freed here) and `buf` is writable for
        // `buf.len()` u16s.
        let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
        let path = OsString::from_wide(&buf[..len as usize]);
        println!("RESULT {}", path.to_string_lossy());
    }

    // ---- parent ------------------------------------------------------------------------------

    enum Loaded {
        Null,
        Path(PathBuf),
    }

    struct Outcome {
        spawned: bool,
        loaded: Loaded,
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("lune-pty-dll-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).expect("create temp dir");
        dir
    }

    fn run_child(mode: &str, cwd: &Path, path_prefix: Option<&Path>) -> Outcome {
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(exe);
        cmd.args(["child", mode]).current_dir(cwd);
        if let Some(prefix) = path_prefix {
            let old = std::env::var_os("PATH").unwrap_or_default();
            let joined = std::env::join_paths(
                std::iter::once(prefix.to_path_buf()).chain(std::env::split_paths(&old)),
            )
            .expect("join PATH");
            cmd.env("PATH", joined);
        }
        let out = cmd.output().expect("run child");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let spawned = stdout.lines().any(|l| l.trim() == "SPAWN ok");
        let result = stdout
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix("RESULT "))
            .unwrap_or_else(|| {
                panic!(
                    "child {mode} printed no RESULT (status {:?}); stdout: {stdout:?}",
                    out.status.code()
                )
            });
        let loaded = match result.strip_prefix("NULL") {
            Some(_) => Loaded::Null,
            None => Loaded::Path(PathBuf::from(result)),
        };
        Outcome { spawned, loaded }
    }

    /// True when `loaded` is the planted file (compared by canonical path, so 8.3 short temp
    /// names on CI runners and `\\?\` prefixes do not matter).
    fn is_plant(loaded: &Loaded, plant: &Path) -> bool {
        match loaded {
            Loaded::Null => false,
            Loaded::Path(p) => match (std::fs::canonicalize(p), std::fs::canonicalize(plant)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            },
        }
    }

    fn describe(loaded: &Loaded) -> String {
        match loaded {
            Loaded::Null => "NULL (not found)".into(),
            Loaded::Path(p) => p.display().to_string(),
        }
    }

    pub(super) fn parent() {
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot is set on Windows");
        let system32 = PathBuf::from(system_root).join("System32");
        let system_conpty = system32.join("conpty.dll");
        let system_has_conpty = system_conpty.exists();
        println!(
            "dll_search: System32\\conpty.dll exists: {system_has_conpty} (planting path {})",
            if system_has_conpty {
                "NOT reachable: System32 comes first in the safe search order"
            } else {
                "reachable"
            }
        );

        let plant_dir = unique_dir("plant");
        let other_dir = unique_dir("other");
        let plant = plant_dir.join("conpty.dll");
        std::fs::copy(system32.join("version.dll"), &plant).expect("plant version.dll");

        // Control: the instrument must see the plant when nothing is hardened.
        let control = run_child("control", &plant_dir, None);
        println!("dll_search: control      -> {}", describe(&control.loaded));
        if system_has_conpty {
            println!("dll_search: control is inconclusive (system conpty.dll wins); recorded");
        } else {
            assert!(
                is_plant(&control.loaded, &plant),
                "control did not load the planted DLL from the current dir: {}",
                describe(&control.loaded)
            );
        }

        let by_cwd = run_child("hardened", &plant_dir, None);
        println!("dll_search: hardened-cwd -> {}", describe(&by_cwd.loaded));
        assert!(by_cwd.spawned, "hardened-cwd: backend spawn failed");
        assert!(
            !is_plant(&by_cwd.loaded, &plant),
            "hardened-cwd: planted DLL in the current dir was loaded"
        );

        let by_path = run_child("hardened", &other_dir, Some(&plant_dir));
        println!("dll_search: hardened-path-> {}", describe(&by_path.loaded));
        assert!(by_path.spawned, "hardened-path: backend spawn failed");
        assert!(
            !is_plant(&by_path.loaded, &plant),
            "hardened-path: planted DLL on PATH was loaded"
        );

        let _ = std::fs::remove_dir_all(&plant_dir);
        let _ = std::fs::remove_dir_all(&other_dir);
        println!("dll_search: ok");
    }
}
