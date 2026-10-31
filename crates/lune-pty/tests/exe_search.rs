//! Windows executable resolution (M3 fix round 1, finding S1). `harness = false`: Windows only,
//! other platforms print "skipped".
//!
//! portable-pty hands `CreateProcessW` the program as `lpApplicationName`, and Windows completes a
//! relative name there with the host process's current directory. The backend resolves the
//! program to an absolute path first. Two self-re-exec children run with the current directory
//! set to a temp dir that holds two plants: `lunepl-<pid>.exe` (a copy of `cmd.exe`, on no `PATH`)
//! and `cmd.exe` (not an executable, so running it fails with os error 193 or 216):
//!
//! - `control`: portable-pty directly. A bare `lunepl-<pid>.exe` spawns (the plant ran), and
//!   `ComSpec=cmd.exe` fails with 193/216 (the plant was picked). This proves the hole is real.
//! - `hardened`: the backend. The bare plant name fails with `NotFound`, also with empty and `.`
//!   entries on `PATH`; `program: None` with a relative `ComSpec` spawns (System32's `cmd.exe`,
//!   since the plant would fail with 193/216); bare `cmd.exe` and `cmd` (with empty `PATHEXT`
//!   entries, which made portable-pty 0.9.0 panic) still spawn.

#[cfg(not(windows))]
fn main() {
    println!("exe_search: skipped (Windows only)");
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use lune_pty::{PortablePtyBackend, PtyBackend, PtySize, SpawnOptions};

    const SIZE: PtySize = PtySize {
        columns: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    };

    pub(super) fn main() {
        let args: Vec<String> = std::env::args().collect();
        match (args.get(1).map(String::as_str), args.get(2)) {
            (Some("control"), Some(plant)) => control(plant),
            (Some("hardened"), Some(plant)) => hardened(plant),
            _ => parent(),
        }
    }

    // ---- children ----------------------------------------------------------------------------

    fn report(case: &str, result: Result<(), String>) {
        match result {
            Ok(()) => println!("CASE {case} ok"),
            Err(e) => println!("CASE {case} err {e}"),
        }
    }

    fn control(plant: &str) {
        use portable_pty::{CommandBuilder, native_pty_system};
        let spawn = |cmd: CommandBuilder| -> Result<(), String> {
            let pair = native_pty_system()
                .openpty(portable_pty::PtySize::default())
                .map_err(|e| format!("{e:#}"))?;
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| format!("{e:#}"))?;
            // Leak: dropping a ConPTY can wait for output that nobody reads; the process exits.
            std::mem::forget((pair, child));
            Ok(())
        };
        let mut bare = CommandBuilder::new(plant);
        bare.args(["/d", "/c", "exit 0"]);
        report("control-bare", spawn(bare));
        let mut comspec = CommandBuilder::new_default_prog();
        comspec.env("ComSpec", "cmd.exe");
        report("control-comspec", spawn(comspec));
        std::process::exit(0);
    }

    fn hardened(plant: &str) {
        let spawn = |program: Option<&str>, args: &[&str], env: &[(&str, &str)]| {
            let options = SpawnOptions {
                program: program.map(PathBuf::from),
                args: args.iter().map(|a| a.to_string()).collect(),
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                ..SpawnOptions::default()
            };
            match PortablePtyBackend::new().spawn(&options, SIZE) {
                Ok(pty) => {
                    std::mem::forget(pty);
                    Ok(())
                }
                Err(e) => Err(format!("{:?} {e}", e.kind())),
            }
        };
        let exit0 = ["/d", "/c", "exit 0"];
        let path = std::env::var("PATH").unwrap_or_default();
        report("bare-plant", spawn(Some(plant), &exit0, &[]));
        let cwd_path = format!(";.;{path};");
        report(
            "bare-plant-cwd-path",
            spawn(Some(plant), &exit0, &[("PATH", &cwd_path)]),
        );
        report(
            "default-relative-comspec",
            spawn(None, &[], &[("ComSpec", "cmd.exe")]),
        );
        report("bare-cmd", spawn(Some("cmd.exe"), &exit0, &[]));
        report(
            "bare-cmd-empty-pathext",
            spawn(Some("cmd"), &exit0, &[("PATHEXT", ";.EXE;;")]),
        );
        std::process::exit(0);
    }

    // ---- parent ------------------------------------------------------------------------------

    fn run_child(mode: &str, cwd: &Path, plant: &str) -> Vec<(String, String)> {
        let exe = std::env::current_exe().expect("current_exe");
        let out = Command::new(exe)
            .args([mode, plant])
            .current_dir(cwd)
            .output()
            .expect("run child");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let cases: Vec<(String, String)> = stdout
            .lines()
            .filter_map(|l| l.trim().strip_prefix("CASE "))
            .filter_map(|l| l.split_once(' '))
            .map(|(case, rest)| (case.to_string(), rest.to_string()))
            .collect();
        for (case, rest) in &cases {
            println!("exe_search: {mode} {case} -> {rest}");
        }
        assert!(
            out.status.success(),
            "child {mode} failed (status {:?}); stderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        cases
    }

    fn outcome<'a>(cases: &'a [(String, String)], case: &str) -> &'a str {
        cases
            .iter()
            .find(|(c, _)| c == case)
            .map(|(_, rest)| rest.as_str())
            .unwrap_or_else(|| panic!("case {case} missing"))
    }

    pub(super) fn parent() {
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot is set on Windows");
        let system_cmd = PathBuf::from(system_root).join("System32").join("cmd.exe");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("lune-pty-exe-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).expect("create temp dir");
        let plant = format!("lunepl-{}.exe", std::process::id());
        std::fs::copy(&system_cmd, dir.join(&plant)).expect("plant cmd copy");
        std::fs::write(dir.join("cmd.exe"), b"not an executable").expect("plant cmd.exe");

        let control = run_child("control", &dir, &plant);
        assert_eq!(
            outcome(&control, "control-bare"),
            "ok",
            "control: portable-pty did not run the planted bare name from the current dir"
        );
        let comspec = outcome(&control, "control-comspec");
        // 193 (bad exe format) or 216 (machine type mismatch, what windows-latest reports for a
        // text file): either means the plant was opened; a miss would be os error 2.
        assert!(
            comspec.contains("os error 193") || comspec.contains("os error 216"),
            "control: relative ComSpec did not pick the planted cmd.exe: {comspec}"
        );

        let hardened = run_child("hardened", &dir, &plant);
        for case in ["bare-plant", "bare-plant-cwd-path"] {
            let got = outcome(&hardened, case);
            assert!(got.starts_with("err NotFound"), "{case}: {got}");
        }
        for case in [
            "default-relative-comspec",
            "bare-cmd",
            "bare-cmd-empty-pathext",
        ] {
            assert_eq!(outcome(&hardened, case), "ok", "{case}");
        }

        // The control child's leaked cmd copy may still hold the plant open for a moment.
        let _ = std::fs::remove_dir_all(&dir);
        println!("exe_search: ok");
    }
}
