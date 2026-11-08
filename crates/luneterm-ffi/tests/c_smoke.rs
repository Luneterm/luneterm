//! Compiles `tests/c/smoke.c` with the system C compiler against `include/luneterm.h` and the
//! cdylib this build produced, runs it, and requires exit 0 (M8 step 5). A missing compiler or a
//! missing library fails with a message; it never skips. Windows CI has no C toolchain on PATH
//! and prints a skip line instead.

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn cdylib(dir: &Path) -> PathBuf {
        let name = if cfg!(target_os = "macos") {
            "libluneterm_ffi.dylib"
        } else {
            "libluneterm_ffi.so"
        };
        let path = dir.join(name);
        assert!(
            path.is_file(),
            "the luneterm_ffi cdylib was not built next to the test executable: {}",
            path.display()
        );
        path
    }

    #[test]
    fn the_c_smoke_test_passes_against_the_built_library() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe.parent().expect("test executable directory");
        let library = cdylib(dir);

        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
        let mut parts = cc.split_whitespace();
        let compiler = parts.next().expect("CC is empty");
        let out = dir.join("luneterm_c_smoke");
        let mut command = Command::new(compiler);
        command
            .args(parts)
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg("-I")
            .arg(manifest.join("include"))
            .arg(manifest.join("tests/c/smoke.c"))
            .arg(&library)
            .arg(format!("-Wl,-rpath,{}", dir.display()))
            .arg("-o")
            .arg(&out);
        if cfg!(feature = "panic-probe") {
            command.arg("-DLUNETERM_PANIC_PROBE");
        }
        let compiled = command
            .output()
            .unwrap_or_else(|e| panic!("no C compiler: could not run `{compiler}`: {e}"));
        assert!(
            compiled.status.success(),
            "compiling smoke.c failed:\n{}",
            String::from_utf8_lossy(&compiled.stderr)
        );

        let run = Command::new(&out).output().expect("run the C smoke test");
        assert!(
            run.status.success(),
            "the C smoke test failed ({:?}):\n{}{}",
            run.status,
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(String::from_utf8_lossy(&run.stdout).contains("luneterm c smoke: ok"));
    }
}

#[cfg(windows)]
#[test]
fn the_c_smoke_test_is_skipped_without_a_c_toolchain() {
    println!("skipped: no C toolchain on PATH (Windows CI)");
}
