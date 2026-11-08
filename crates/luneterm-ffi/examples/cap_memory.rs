//! Peak memory at the ABI's caps (M8 step 6, D5): `cargo run --profile ffi-release -p luneterm-ffi
//! --example cap_memory`. (`--release` does not compile: the crate refuses `panic = "abort"`, D4.)
//!
//! Each case runs in a child process, because the peak resident set (`getrusage` `ru_maxrss`)
//! only ever grows:
//!   instr        the instrument check: a 256 MiB `Vec` touched page by page must read >= 256 MiB
//!   a            an engine at 4096 x 2048, scrollback 0, after filling the screen, and after one
//!                `luneterm_engine_viewport` call
//!   c            as (a), but every cell carries a string (a letter and a combining mark): the
//!                worst fill of the cap
//!   b <count>    the security ledger's `rep-scroll-bottom-chain-memory` construction at the
//!                column cap: the alternate screen, 24 rows of 4096 cells that carry a string
//!                (letter + combining mark), narrowed to 3 columns (the alternate buffer does not
//!                reflow), the cursor on the bottom row, Unicode 11-vs16, then one REP of a
//!                widening unit (`a U+FE0F CSI <count> b`)
//! With no argument it runs all of them and prints one line each. Numbers only; unix only.

#[cfg(unix)]
mod imp {
    use std::process::Command;
    use std::ptr::null_mut;

    use luneterm_ffi::*;

    /// `struct rusage` on 64-bit Linux and macOS: two timevals, then fourteen longs, the first of
    /// which is `ru_maxrss` (kilobytes on Linux, bytes on macOS).
    #[repr(C)]
    struct RUsage {
        times: [i64; 4],
        maxrss: i64,
        rest: [i64; 13],
    }

    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut RUsage) -> i32;
    }

    /// Peak resident set size of this process in bytes.
    fn peak_rss() -> u64 {
        let mut usage = RUsage {
            times: [0; 4],
            maxrss: 0,
            rest: [0; 13],
        };
        // SAFETY: `usage` is a live, correctly sized `struct rusage`; RUSAGE_SELF is 0.
        let code = unsafe { getrusage(0, &mut usage) };
        assert_eq!(code, 0, "getrusage failed");
        let raw = u64::try_from(usage.maxrss).expect("ru_maxrss");
        if cfg!(target_os = "macos") {
            raw
        } else {
            raw * 1024
        }
    }

    fn new_engine(cols: u32, rows: u32, scrollback: u32, unicode: u32) -> *mut LunetermEngine {
        let config = LunetermConfig {
            cols,
            rows,
            scrollback,
            unicode,
            xtversion: std::ptr::null(),
            xtversion_len: 0,
            clipboard_write: 0,
            reserved: [0; 7],
        };
        let mut engine = null_mut();
        // SAFETY: a live config and out-param.
        let code = unsafe { luneterm_engine_new(&config, &mut engine) };
        assert_eq!(code, LUNETERM_OK, "engine_new");
        engine
    }

    fn write(engine: *mut LunetermEngine, data: &[u8]) {
        // SAFETY: a live handle and a live slice.
        let code = unsafe { luneterm_engine_write(engine, data.as_ptr(), data.len()) };
        assert_eq!(code, LUNETERM_OK, "write");
    }

    fn mib(bytes: u64) -> f64 {
        bytes as f64 / (1024.0 * 1024.0)
    }

    fn instrument() {
        const SIZE: usize = 256 * 1024 * 1024;
        let before = peak_rss();
        let mut buffer = vec![0u8; SIZE];
        for page in (0..SIZE).step_by(4096) {
            buffer[page] = 1;
        }
        let touched = buffer
            .iter()
            .step_by(4096)
            .map(|b| u64::from(*b))
            .sum::<u64>();
        let after = peak_rss();
        println!(
            "instr: allocated and touched {} MiB ({touched} pages set); peak rss before {:.1} MiB, after {:.1} MiB; reads at least the allocation: {}",
            SIZE / (1024 * 1024),
            mib(before),
            mib(after),
            after >= SIZE as u64
        );
        assert!(
            after >= SIZE as u64,
            "the RSS reader reports less than a touched 256 MiB allocation"
        );
    }

    fn case_a(strings: bool) {
        let (cols, rows) = (LUNETERM_MAX_COLS, LUNETERM_MAX_ROWS);
        let cells = u64::from(cols) * u64::from(rows);
        let start = peak_rss();
        let engine = new_engine(cols, rows, 0, LUNETERM_UNICODE_11_VS16);
        let created = peak_rss();
        // Fill the screen: every row full, with colour changes, wide characters and strings, the
        // last row left without a line feed so nothing scrolls.
        let mut row = String::new();
        if strings {
            // Every cell a string: a letter and a combining mark, `cols` times.
            for _ in 0..cols {
                row.push_str("a\u{301}");
            }
        } else {
            row.push_str("\x1b[31;1m");
            while row.chars().count() < (cols as usize) - 8 {
                row.push_str("ab\u{301}\u{3042}x");
            }
            row.push_str("\x1b[0m");
        }
        let mut screen = Vec::new();
        for y in 0..rows {
            let fill = if strings {
                row.clone()
            } else {
                format!("{row}{}", "z".repeat(cols as usize))
            };
            screen.extend_from_slice(fill.as_bytes());
            if y + 1 < rows {
                screen.extend_from_slice(b"\r\n");
            }
        }
        write(engine, &screen);
        drop(screen);
        let filled = peak_rss();
        let mut viewport = LunetermViewport {
            cols: 0,
            ..empty_viewport()
        };
        // SAFETY: a live handle and out-param.
        let code = unsafe { luneterm_engine_viewport(engine, &mut viewport) };
        assert_eq!(code, LUNETERM_OK);
        assert_eq!(viewport.cell_count, cells);
        let viewed = peak_rss();
        // Reading it again reuses the slot.
        // SAFETY: as above.
        let code = unsafe { luneterm_engine_viewport(engine, &mut viewport) };
        assert_eq!(code, LUNETERM_OK);
        let viewed_twice = peak_rss();
        println!(
            "{}: {cols}x{rows} scrollback 0 ({cells} cells): peak rss at start {:.1} MiB, after new {:.1} MiB, after filling the screen {:.1} MiB ({:.1} bytes/cell over start), after one viewport call {:.1} MiB (+{:.1} MiB, slot {} bytes/cell x {cells} = {:.1} MiB; text pool {} bytes), after a second call {:.1} MiB",
            if strings { "c" } else { "a" },
            mib(start),
            mib(created),
            mib(filled),
            (filled - start) as f64 / cells as f64,
            mib(viewed),
            mib(viewed - filled),
            std::mem::size_of::<LunetermCell>(),
            mib(cells * std::mem::size_of::<LunetermCell>() as u64),
            viewport.text_len,
            mib(viewed_twice)
        );
        // SAFETY: a live handle, used for the last time.
        assert_eq!(unsafe { luneterm_engine_free(engine) }, LUNETERM_OK);
    }

    fn empty_viewport() -> LunetermViewport {
        LunetermViewport {
            cols: 0,
            rows: 0,
            cells: std::ptr::null(),
            cell_count: 0,
            text: std::ptr::null(),
            text_len: 0,
            links: std::ptr::null(),
            link_count: 0,
            link_data: std::ptr::null(),
            link_data_len: 0,
            title: std::ptr::null(),
            title_len: 0,
            viewport_top: 0,
            base_y: 0,
            total_lines: 0,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: 0,
            cursor_style: 0,
            cursor_blink: 0,
            is_at_bottom: 0,
            alt_screen: 0,
            reserved: [0; 3],
        }
    }

    fn case_b(count: &str) {
        let (long, narrow, rows) = (LUNETERM_MAX_COLS, 3u32, 24u32);
        let start = peak_rss();
        let engine = new_engine(long, rows, 0, LUNETERM_UNICODE_11_VS16);
        write(engine, b"\x1b[?1049h");
        // `rows` rows of `long` cells that each carry a string (a letter and a combining mark).
        let mut screen = Vec::new();
        for y in 0..rows {
            for _ in 0..long {
                screen.extend_from_slice("a\u{301}".as_bytes());
            }
            if y + 1 < rows {
                screen.extend_from_slice(b"\r\n");
            }
        }
        write(engine, &screen);
        drop(screen);
        // Narrow the window: the alternate buffer keeps the 4096-cell rows.
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { luneterm_engine_resize(engine, narrow, rows) },
            LUNETERM_OK
        );
        // The cursor to the bottom row, then one REP of a widening unit.
        write(engine, format!("\x1b[{rows};1H").as_bytes());
        let before = peak_rss();
        let rep = format!("a\u{FE0F}\x1b[{count}b");
        let started = std::time::Instant::now();
        write(engine, rep.as_bytes());
        let elapsed = started.elapsed();
        let after = peak_rss();
        println!(
            "b: {long} -> {narrow} columns x {rows} rows, one REP of {} input bytes (count {count}): peak rss at start {:.1} MiB, before the REP {:.1} MiB, after the REP {:.1} MiB; REP took {:.1} ms",
            rep.len(),
            mib(start),
            mib(before),
            mib(after),
            elapsed.as_secs_f64() * 1000.0
        );
        // SAFETY: a live handle, used for the last time.
        assert_eq!(unsafe { luneterm_engine_free(engine) }, LUNETERM_OK);
    }

    pub(super) fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["instr"] => instrument(),
            ["a"] => case_a(false),
            ["c"] => case_a(true),
            ["b", count] => case_b(count),
            [] => {
                let exe = std::env::current_exe().expect("current_exe");
                // REP counts: below the alternate buffer's row count (a chain would survive), and above.
                for case in [
                    vec!["instr"],
                    vec!["a"],
                    vec!["c"],
                    vec!["b", "1"],
                    vec!["b", "10"],
                    vec!["b", "23"],
                    vec!["b", "100"],
                    vec!["b", "1048576"],
                ] {
                    let output = Command::new(&exe)
                        .args(&case)
                        .output()
                        .expect("run the case");
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                    if !output.status.success() {
                        println!("case {case:?} failed: {}", output.status);
                        std::process::exit(1);
                    }
                }
            }
            other => panic!("usage: cap_memory [instr | a | b <count>], got {other:?}"),
        }
    }
}

#[cfg(unix)]
fn main() {
    imp::main();
}

#[cfg(not(unix))]
fn main() {
    println!("cap_memory: unix only (getrusage)");
}
