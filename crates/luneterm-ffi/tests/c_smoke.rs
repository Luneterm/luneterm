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

    /// Every numeric `#define LUNETERM_*` of the header, as the C preprocessor reports it
    /// (`$CC -E -dM`), against the Rust constants. Fails closed: a header number the table does not
    /// know, or a table entry the header lacks, is an error.
    #[test]
    fn the_header_constants_equal_the_rust_constants() {
        use luneterm_ffi::*;
        use std::collections::BTreeMap;

        macro_rules! table {
            ($($name:ident),+ $(,)?) => {
                vec![$((stringify!($name), i64::from($name))),+]
            };
        }
        let mut expected: BTreeMap<&str, i64> = table![
            LUNETERM_OK,
            LUNETERM_ERR_NULL_POINTER,
            LUNETERM_ERR_INVALID_ARGUMENT,
            LUNETERM_ERR_INVALID_UTF8,
            LUNETERM_ERR_TOO_LARGE,
            LUNETERM_ERR_PANIC,
            LUNETERM_ERR_POISONED,
            LUNETERM_ERR_BUSY,
            LUNETERM_MAX_COLS,
            LUNETERM_MAX_ROWS,
            LUNETERM_MAX_SCROLLBACK,
            LUNETERM_UNICODE_6,
            LUNETERM_UNICODE_11,
            LUNETERM_UNICODE_11_VS16,
            LUNETERM_COLOR_DEFAULT,
            LUNETERM_COLOR_PALETTE,
            LUNETERM_COLOR_RGB,
            LUNETERM_CELL_BOLD,
            LUNETERM_CELL_DIM,
            LUNETERM_CELL_ITALIC,
            LUNETERM_CELL_BLINK,
            LUNETERM_CELL_INVERSE,
            LUNETERM_CELL_INVISIBLE,
            LUNETERM_CELL_STRIKETHROUGH,
            LUNETERM_CELL_OVERLINE,
            LUNETERM_UNDERLINE_NONE,
            LUNETERM_UNDERLINE_SINGLE,
            LUNETERM_UNDERLINE_DOUBLE,
            LUNETERM_UNDERLINE_CURLY,
            LUNETERM_UNDERLINE_DOTTED,
            LUNETERM_UNDERLINE_DASHED,
            LUNETERM_CURSOR_BLOCK,
            LUNETERM_CURSOR_UNDERLINE,
            LUNETERM_CURSOR_BAR,
            LUNETERM_EVENT_RESPONSE,
            LUNETERM_EVENT_BELL,
            LUNETERM_EVENT_TITLE_CHANGED,
            LUNETERM_EVENT_COLOR,
            LUNETERM_EVENT_FOCUS_REPORT_REQUESTED,
            LUNETERM_EVENT_WINDOW_REPORT,
            LUNETERM_EVENT_A11Y_TEXT,
            LUNETERM_EVENT_A11Y_TAB,
            LUNETERM_EVENT_UNKNOWN_OSC,
            LUNETERM_EVENT_CLIPBOARD_WRITE,
            LUNETERM_EVENT_CWD_CHANGED,
            LUNETERM_EVENT_PROGRESS,
            LUNETERM_EVENT_COMMAND_MARK,
            LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND,
            LUNETERM_EVENT_HAS_EXIT_CODE,
            LUNETERM_COLOR_REQUEST_REPORT,
            LUNETERM_COLOR_REQUEST_SET,
            LUNETERM_COLOR_REQUEST_RESTORE,
            LUNETERM_COLOR_REQUEST_RESTORE_ALL,
            LUNETERM_COLOR_INDEX_FOREGROUND,
            LUNETERM_COLOR_INDEX_BACKGROUND,
            LUNETERM_COLOR_INDEX_CURSOR,
            LUNETERM_MOUSE_TRACKING_NONE,
            LUNETERM_MOUSE_TRACKING_X10,
            LUNETERM_MOUSE_TRACKING_VT200,
            LUNETERM_MOUSE_TRACKING_DRAG,
            LUNETERM_MOUSE_TRACKING_ANY,
            LUNETERM_MOUSE_ENCODING_DEFAULT,
            LUNETERM_MOUSE_ENCODING_SGR,
            LUNETERM_MOUSE_ENCODING_SGR_PIXELS,
            LUNETERM_MOD_SHIFT,
            LUNETERM_MOD_ALT,
            LUNETERM_MOD_CONTROL,
        ]
        .into_iter()
        .collect();
        expected.insert("LUNETERM_ABI_VERSION", i64::from(ABI_VERSION));
        // Ordinals the Rust side maps by explicit tables (input.rs), pinned here as Lunegit's.
        for (name, value) in [
            ("LUNETERM_KEY_ENTER", 0),
            ("LUNETERM_KEY_TAB", 1),
            ("LUNETERM_KEY_BACKSPACE", 2),
            ("LUNETERM_KEY_ESCAPE", 3),
            ("LUNETERM_KEY_SPACE", 4),
            ("LUNETERM_KEY_UP", 5),
            ("LUNETERM_KEY_DOWN", 6),
            ("LUNETERM_KEY_RIGHT", 7),
            ("LUNETERM_KEY_LEFT", 8),
            ("LUNETERM_KEY_HOME", 9),
            ("LUNETERM_KEY_END", 10),
            ("LUNETERM_KEY_PAGE_UP", 11),
            ("LUNETERM_KEY_PAGE_DOWN", 12),
            ("LUNETERM_KEY_INSERT", 13),
            ("LUNETERM_KEY_DELETE", 14),
            ("LUNETERM_KEY_F1", 15),
            ("LUNETERM_KEY_F12", 26),
            ("LUNETERM_KEY_CHARACTER", 27),
            ("LUNETERM_MOUSE_BUTTON_NONE", 0),
            ("LUNETERM_MOUSE_BUTTON_LEFT", 1),
            ("LUNETERM_MOUSE_BUTTON_MIDDLE", 2),
            ("LUNETERM_MOUSE_BUTTON_RIGHT", 3),
            ("LUNETERM_MOUSE_BUTTON_WHEEL_UP", 4),
            ("LUNETERM_MOUSE_BUTTON_WHEEL_DOWN", 5),
            ("LUNETERM_MOUSE_KIND_DOWN", 0),
            ("LUNETERM_MOUSE_KIND_UP", 1),
            ("LUNETERM_MOUSE_KIND_MOVE", 2),
            ("LUNETERM_MOUSE_KIND_DRAG", 3),
            ("LUNETERM_MOUSE_KIND_WHEEL_UP", 4),
            ("LUNETERM_MOUSE_KIND_WHEEL_DOWN", 5),
        ] {
            expected.insert(name, value);
        }

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
        let mut parts = cc.split_whitespace();
        let compiler = parts.next().expect("CC is empty");
        let output = Command::new(compiler)
            .args(parts)
            .args(["-E", "-dM", "-x", "c"])
            .arg("-I")
            .arg(manifest.join("include"))
            .arg(manifest.join("include/luneterm.h"))
            .output()
            .unwrap_or_else(|e| panic!("no C compiler: could not run `{compiler}`: {e}"));
        assert!(output.status.success(), "preprocessing luneterm.h failed");
        let mut found = BTreeMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut fields = line.splitn(3, ' ');
            let (Some("#define"), Some(name), Some(body)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if !name.starts_with("LUNETERM_") {
                continue;
            }
            // Numeric macros only: `(-1)`, `4096u`, `0x0001u`.
            let digits = body
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim_end_matches('u');
            let (negative, digits) = match digits.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, digits),
            };
            let parsed = match digits.strip_prefix("0x") {
                Some(hex) => i64::from_str_radix(hex, 16),
                None => digits.parse::<i64>(),
            };
            if let Ok(value) = parsed {
                found.insert(name.to_owned(), if negative { -value } else { value });
            }
        }
        assert!(
            found.len() > 50,
            "the preprocessor found only {} numeric macros",
            found.len()
        );
        for (name, value) in &found {
            match expected.get(name.as_str()) {
                Some(rust) => assert_eq!(value, rust, "{name}: header {value}, Rust {rust}"),
                None => panic!("{name} is in the header but not in the Rust table"),
            }
        }
        for name in expected.keys() {
            assert!(
                found.contains_key(*name),
                "{name} is in the Rust table but not in the header"
            );
        }
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
