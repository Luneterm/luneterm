//! Windows DLL search hardening before the GPU stack (M9 step 7, decision D17). `harness = false`:
//! Windows only, other platforms print "skipped".
//!
//! wgpu's DX12 backend loads `dxcompiler.dll` by bare name when the GPU instance is created. With
//! the default search order a copy planted in the current directory or on `PATH` would be loaded.
//! `main` therefore calls `lune_pty::restrict_dll_search()` first. Modelled on lune-pty's
//! `dll_search.rs`, with three self-re-exec children, each with `System32\version.dll` planted as
//! `dxcompiler.dll` in a unique temp dir:
//!
//! - `control`: no hardening, current dir = the temp dir; the bare-name load must find the plant
//!   (proves the instrument can see a planted DLL at all);
//! - `hardened-cwd`: current dir = the temp dir; after `restrict_dll_search()` the same load must
//!   not find the plant, and neither may the module the GPU stack loads;
//! - `hardened-path`: temp dir first on `PATH`, current dir elsewhere; same assertions.
//!
//! The GPU part needs an adapter; without one the test says so and asserts the search order only.

#[cfg(not(windows))]
fn main() {
    println!("dll_search_gpu: skipped (Windows only)");
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

    use lune_ui::gpu::{self, Gpu};
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleW, LoadLibraryW,
    };

    const DLL: &str = "dxcompiler.dll";

    pub(super) fn main() {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("child") => child(args.get(2).map(String::as_str).unwrap_or("")),
            _ => parent(),
        }
    }

    // ---- child -------------------------------------------------------------------------------

    fn wide(name: &str) -> Vec<u16> {
        OsStr::new(name).encode_wide().chain(Some(0)).collect()
    }

    fn module_path(module: HMODULE) -> String {
        let mut buf = vec![0u16; 32768];
        // SAFETY: `module` is a live module handle (never freed here) and `buf` is writable for
        // `buf.len()` u16s.
        let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
        OsString::from_wide(&buf[..len as usize])
            .to_string_lossy()
            .into_owned()
    }

    /// Prints `<tag> <path>` or `<tag> NULL` for the module of a bare-name `LoadLibraryW`.
    fn load_bare(tag: &str) {
        let name = wide(DLL);
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            println!("{tag} NULL");
        } else {
            println!("{tag} {}", module_path(module));
        }
    }

    /// Prints `MODULE <path>` or `MODULE NULL` for the module of that name loaded in the process
    /// (without loading anything).
    fn loaded_module() {
        let name = wide(DLL);
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        let module = unsafe { GetModuleHandleW(name.as_ptr()) };
        if module.is_null() {
            println!("MODULE NULL");
        } else {
            println!("MODULE {}", module_path(module));
        }
    }

    fn child(mode: &str) {
        match mode {
            "control" => load_bare("RESULT"),
            "hardened" => {
                if let Err(e) = lune_pty::restrict_dll_search() {
                    println!("HARDEN err {:?}", e.kind());
                    std::process::exit(1);
                }
                println!("HARDEN ok");
                load_bare("RESULT");
                // The GPU stack: the instance creation loads the compiler DLL by bare name.
                let instance = gpu::instance(None);
                match pollster::block_on(Gpu::new(instance, None)) {
                    Ok(_gpu) => println!("GPU ok"),
                    Err(_) => println!("GPU none"),
                }
                loaded_module();
            }
            other => {
                println!("unknown child mode {other:?}");
                std::process::exit(2);
            }
        }
    }

    // ---- parent ------------------------------------------------------------------------------

    enum Loaded {
        Null,
        Path(PathBuf),
    }

    struct Outcome {
        hardened: bool,
        gpu: bool,
        result: Loaded,
        module: Loaded,
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("luneterm-dll-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).expect("create temp dir");
        dir
    }

    fn parse(value: &str) -> Loaded {
        match value.strip_prefix("NULL") {
            Some(_) => Loaded::Null,
            None => Loaded::Path(PathBuf::from(value)),
        }
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
        let field = |tag: &str| {
            stdout
                .lines()
                .rev()
                .find_map(|l| l.trim().strip_prefix(tag))
                .map(|v| v.trim().to_owned())
        };
        let result = field("RESULT ").unwrap_or_else(|| {
            panic!(
                "child {mode} printed no RESULT (status {:?}); stdout: {stdout:?}",
                out.status.code()
            )
        });
        Outcome {
            hardened: field("HARDEN ").is_some_and(|v| v == "ok"),
            gpu: field("GPU ").is_some_and(|v| v == "ok"),
            result: parse(&result),
            module: field("MODULE ").map_or(Loaded::Null, |v| parse(&v)),
        }
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
            Loaded::Null => "NULL (not found / not loaded)".into(),
            Loaded::Path(p) => p.display().to_string(),
        }
    }

    pub(super) fn parent() {
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot is set on Windows");
        let system32 = PathBuf::from(system_root).join("System32");
        let system_has_dll = system32.join(DLL).exists();
        println!(
            "dll_search_gpu: System32\\{DLL} exists: {system_has_dll} (planting path {})",
            if system_has_dll {
                "NOT reachable: System32 comes first in the safe search order"
            } else {
                "reachable"
            }
        );

        let plant_dir = unique_dir("plant");
        let other_dir = unique_dir("other");
        let plant = plant_dir.join(DLL);
        std::fs::copy(system32.join("version.dll"), &plant).expect("plant version.dll");

        // Control: the instrument must see the plant when nothing is hardened.
        let control = run_child("control", &plant_dir, None);
        println!(
            "dll_search_gpu: control       -> {}",
            describe(&control.result)
        );
        if system_has_dll {
            println!("dll_search_gpu: control is inconclusive (system {DLL} wins); recorded");
        } else {
            assert!(
                is_plant(&control.result, &plant),
                "control did not load the planted DLL from the current dir: {}",
                describe(&control.result)
            );
        }

        for (name, cwd, prefix) in [
            ("hardened-cwd", &plant_dir, None),
            ("hardened-path", &other_dir, Some(plant_dir.as_path())),
        ] {
            let got = run_child("hardened", cwd, prefix);
            println!(
                "dll_search_gpu: {name:<13} -> load {}; gpu {}; module {}",
                describe(&got.result),
                if got.gpu { "initialized" } else { "no adapter" },
                describe(&got.module),
            );
            assert!(got.hardened, "{name}: restrict_dll_search failed");
            assert!(
                !is_plant(&got.result, &plant),
                "{name}: the planted DLL was loaded by a bare-name load"
            );
            assert!(
                !is_plant(&got.module, &plant),
                "{name}: the GPU stack loaded the planted DLL"
            );
            if !got.gpu {
                println!("dll_search_gpu: {name}: no GPU adapter; search order asserted only");
            }
        }

        let _ = std::fs::remove_dir_all(&plant_dir);
        let _ = std::fs::remove_dir_all(&other_dir);
        println!("dll_search_gpu: ok");
    }
}
