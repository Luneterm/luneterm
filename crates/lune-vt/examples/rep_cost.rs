//! Cost of `CSI Ps b` (REP) per input byte, for the S7 residuals (plan M7 steps 10 to 13):
//! `cargo run --release -p lune-vt --example rep_cost` (`scripts/bench-rep.sh` builds and runs it).
//!
//! Every figure is the median of 5 fresh terminals, in ns per input byte: the bytes of
//! `3 x (unit CSI 1048576 b)` written in one `Terminal::write` call, divided by their count (the
//! setups of the M6 security review's probes `rep_cost6`, `fr4probe` and `fr5probe`). Unicode 11
//! is active, as in Lunegit. Two groups, then the load average:
//!
//!  - **REP at the screen size**: plain `a`, `a` inside an open OSC 8 link, a narrow cluster
//!    (`e U+0301`), a wide cluster (an emoji and a ZWJ), `a` with an extended attribute (an
//!    underline colour), and the DECALN control (`ESC # 8` x 2000), at 80x24 and 300x100 with
//!    scrollback 1,000 and 10,000. M7 fix round 1 (review S1) adds, with the standard
//!    extensions installed and `vs16` on (the `11-vs16` provider active): VS16 (`U+26A0
//!    U+FE0F`, two cells), a keycap (`1 U+FE0F U+20E3`), the wide cluster and VS16 inside an
//!    open link, and `a` and VS16 in insert mode (wraparound on), plain and with an underline
//!    colour.
//!  - **No-wrap insert REP on a line longer than the window**: the alt screen is filled with
//!    lines of L cells, the window is shrunk to 80x24 or 300x100 (the alt buffer does not reflow,
//!    so the lines keep L cells), then `CSI ? 7 l CSI 4 h` and the REP of `a` or `e U+0301` on the
//!    last row, for L in 80 (the control), 300, 1000 and 2000, scrollback 10,000; and (M7 fix
//!    round 1) of VS16 and the keycap under `11-vs16`.
//!  - **REP below a scroll region** (M7 fix round 2, review S2): a region of rows 1..=10, the
//!    cursor on the last row, insert mode, scrollback 10,000, at 80x24 and 300x100, for plain
//!    `a`, `e U+0301`, a wide cluster, VS16 and the keycap (plain `a` is the ≤ 3× reference),
//!    and (M7 fix round 3) `a` inside an open link and with an underline colour.
//!  - **REP below a region on a long row, Windows mode** (M7 fix round 2, review S2): the
//!    non-reflowing normal buffer shrunk from L columns to 80, below a region, insert mode, for
//!    L in 300, 1,000 (M7 fix round 3) and 2,000 — linear in L; M7 fix round 3 adds `a` inside
//!    an open link and with an underline colour (the review's other rows).
//!  - **REP below a region at odd widths** (M7 fix round 3, review S2): plain `a`, VS16 and the
//!    keycap at 79x24 and 299x100, and on a row of 2,000 cells narrowed to 79 columns in
//!    Windows mode; a widened unit's joiner wraps its base there, and that wrap copies the old
//!    row's strings (upstream), so the long row's passes settle only with its tail.
//!  - **REP at odd widths over narrowed rows of strings** (M7 fix round 4, code review F6):
//!    rows of L cells each holding a letter and a combining mark, narrowed to 79 or 21 columns
//!    in Windows mode, below a region and at the scroll bottom, with and without insert mode;
//!    a widened unit's wraps copy the rows' strings there, so the cut floor grows with the
//!    rows' last key. Plain `a` in the same setup is the reference. M7 fix round 5 adds rows
//!    not narrowed (L = cols), the even widths 80 and 22 (no joiner wrap: the reference for the
//!    same unit and L), plain `a` at the scroll bottom and the keycap there.
//!  - **REP of a joining-base unit** (M7 fix round 2, review S3): a lone combining mark the
//!    cursor sits on (wraparound off, a wide char skipped at the last column); every repetition
//!    appends to the one capped cell, so the cost tracks plain REP.
//!
//! Each setup is checked once, outside the timing, to have the shape its name claims (the line
//! length after the shrink, a link in the viewport, the cluster in the cells, the scrolled
//! buffer); a failed check aborts the run, so a probe that measures the wrong thing prints
//! nothing. It is an example, not a test: nothing here is timed in CI.

use std::process::Command;
use std::time::Instant;

use lune_vt::services::options_service::OptionValue;
use lune_vt::{StandardExtensions, Terminal};

const RUNS: usize = 5;
const REPS_PER_RUN: usize = 3;
/// `CSI 1048576 b`.
const REP: &[u8] = b"\x1b[1048576b";
const NARROW_CLUSTER: &str = "e\u{301}";
/// An emoji and a zero width joiner: a wide base with a combined code point (xterm.js has no
/// grapheme clustering, so the next emoji of a ZWJ sequence would start a cell of its own).
const WIDE_CLUSTER: &str = "\u{1F468}\u{200D}";
/// U+26A0 U+FE0F: two cells under `11-vs16` (a joiner that widens a narrow base, review S1).
const VS16: &str = "\u{26a0}\u{fe0f}";
/// A keycap, `1 U+FE0F U+20E3`: a joiner after the widening one.
const KEYCAP: &str = "1\u{fe0f}\u{20e3}";
const LINK: &str = "\x1b]8;;http://x\x1b\\";
/// Insert mode (wraparound stays on), and with an underline colour (extended attributes).
const INSERT: &str = "\x1b[4h";
const INSERT_UL: &str = "\x1b[4h\x1b[58:5:3;4m";
const SIZES: [(usize, usize); 2] = [(80, 24), (300, 100)];
/// A terminal of `cols`, `rows` and scrollback.
type Make = fn(usize, usize, usize) -> Terminal;
const SCROLLBACKS: [usize; 2] = [1_000, 10_000];
const LINE_LENGTHS: [usize; 4] = [80, 300, 1_000, 2_000];

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    sorted[sorted.len() / 2]
}

fn new_terminal(cols: usize, rows: usize, scrollback: usize) -> Terminal {
    let mut term = Terminal::new([
        OptionValue::Cols(cols),
        OptionValue::Rows(rows),
        OptionValue::Scrollback(scrollback),
    ]);
    // the oracle's and Lunegit's version; a failure here is a bug in this example
    assert!(term.unicode_mut().set_active_version("11").is_ok());
    term
}

/// [`new_terminal`] with the standard extensions installed and `vs16` on, as Lunegit runs
/// (the `11-vs16` provider active).
fn new_vs16_terminal(cols: usize, rows: usize, scrollback: usize) -> Terminal {
    let mut term = new_terminal(cols, rows, scrollback);
    let config = StandardExtensions {
        vs16: true,
        ..StandardExtensions::default()
    };
    assert!(term.install_standard_extensions(&config).is_ok());
    term
}

/// [`new_terminal`] in Windows mode (no reflow on resize), so a column shrink leaves rows
/// longer than the window — the Windows 10 configuration of review S2.
fn new_winmode_terminal(cols: usize, rows: usize, scrollback: usize) -> Terminal {
    let mut term = Terminal::new([
        OptionValue::Cols(cols),
        OptionValue::Rows(rows),
        OptionValue::Scrollback(scrollback),
        OptionValue::WindowsMode(true),
    ]);
    assert!(term.unicode_mut().set_active_version("11").is_ok());
    term
}

/// [`new_winmode_terminal`] with the standard extensions installed and `vs16` on.
fn new_winmode_vs16_terminal(cols: usize, rows: usize, scrollback: usize) -> Terminal {
    let mut term = new_winmode_terminal(cols, rows, scrollback);
    let config = StandardExtensions {
        vs16: true,
        ..StandardExtensions::default()
    };
    assert!(term.install_standard_extensions(&config).is_ok());
    term
}

/// Whether some viewport cell holds `text` as one cell `width` wide.
fn some_cell_is(term: &Terminal, text: &str, width: u8) -> bool {
    term.viewport().cells.iter().any(|row| {
        row.iter()
            .any(|cell| cell.text == text && cell.width == width)
    })
}

/// `3 x (unit REP)`.
fn stream(unit: &str) -> Vec<u8> {
    let mut data = Vec::new();
    for _ in 0..REPS_PER_RUN {
        data.extend_from_slice(unit.as_bytes());
        data.extend_from_slice(REP);
    }
    data
}

/// ns per input byte of writing `data` into a terminal `make` builds, median of [`RUNS`]; `check`
/// sees the first terminal after its write.
fn measure(make: impl Fn() -> Terminal, data: &[u8], check: impl Fn(&Terminal)) -> f64 {
    let mut samples = Vec::new();
    for run in 0..RUNS {
        let mut term = make();
        let start = Instant::now();
        term.write(data);
        let elapsed = start.elapsed();
        if run == 0 {
            check(&term);
        }
        samples.push(elapsed.as_nanos() as f64 / data.len() as f64);
    }
    median(&samples)
}

/// ns per byte of `ESC # 8` x 2000 at a size (the cost of a full-screen operation).
fn decaln(cols: usize, rows: usize, scrollback: usize) -> f64 {
    let data = b"\x1b#8".repeat(2000);
    measure(
        || new_terminal(cols, rows, scrollback),
        &data,
        |term| {
            let viewport = term.viewport();
            assert!(
                viewport
                    .cells
                    .iter()
                    .all(|row| row.iter().all(|cell| cell.text == "E")),
                "DECALN did not fill the screen with E"
            );
        },
    )
}

fn some_cell_has(term: &Terminal, text: &str) -> bool {
    term.viewport()
        .cells
        .iter()
        .any(|row| row.iter().any(|cell| cell.text == text))
}

/// The number of lines scrolled off the top (`ybase`), proof that the REP printed more than a screen.
fn scrolled_lines(term: &Terminal) -> usize {
    term.core().input_handler().services().buffer.buffer().ybase
}

fn format_ns(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.1} us", ns / 1_000.0)
    } else {
        format!("{ns:.0} ns")
    }
}

/// The load average as the OS reports it (`uptime`), or `unknown`.
fn load_average() -> String {
    Command::new("uptime")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| {
            text.split_once("load average").map(|(_, rest)| {
                rest.trim_start_matches('s')
                    .trim_start_matches(':')
                    .trim()
                    .to_string()
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn at_screen_size() {
    println!("REP at the screen size (ns per input byte, median of {RUNS} fresh terminals)");
    println!(
        "{:<22} {:>12} {:>12} {:>12} {:>12}",
        "variant", "80x24 sb1k", "80x24 sb10k", "300x100 sb1k", "300x100 sb10k"
    );
    let configs: Vec<(usize, usize, usize)> = SIZES
        .iter()
        .flat_map(|&(cols, rows)| SCROLLBACKS.iter().map(move |&sb| (cols, rows, sb)))
        .collect();
    type Variant = (&'static str, &'static str, String, fn(&Terminal));
    let variants: Vec<Variant> = vec![
        ("plain a", "", "a".to_string(), |t| {
            assert!(scrolled_lines(t) > 0, "plain REP did not scroll");
        }),
        ("link a", LINK, "a".to_string(), |t| {
            assert!(!t.viewport().links.is_empty(), "no link in the viewport");
        }),
        ("narrow cluster", "", NARROW_CLUSTER.to_string(), |t| {
            assert!(some_cell_has(t, NARROW_CLUSTER), "no narrow cluster cell");
        }),
        ("wide cluster", "", WIDE_CLUSTER.to_string(), |t| {
            assert!(some_cell_has(t, WIDE_CLUSTER), "no wide cluster cell");
        }),
        (
            "ext-attr a (ul colour)",
            "\x1b[58:5:3;4m",
            "a".to_string(),
            |t| {
                assert!(
                    t.viewport()
                        .cells
                        .iter()
                        .flatten()
                        .any(|cell| cell.text == "a"
                            && cell.underline_color != lune_vt::CellColor::Default),
                    "no cell with an underline colour"
                );
            },
        ),
    ];
    // (M7 fix round 1) under `11-vs16`, the standard extensions installed
    let vs16_variants: Vec<Variant> = vec![
        ("VS16 (11-vs16)", "", VS16.to_string(), |t| {
            assert!(some_cell_is(t, VS16, 2), "no 2-wide VS16 cell");
            assert!(scrolled_lines(t) > 0, "VS16 REP did not scroll");
        }),
        ("keycap (11-vs16)", "", KEYCAP.to_string(), |t| {
            assert!(some_cell_is(t, KEYCAP, 2), "no 2-wide keycap cell");
        }),
        ("link wide cluster", LINK, WIDE_CLUSTER.to_string(), |t| {
            assert!(!t.viewport().links.is_empty(), "no link in the viewport");
            assert!(some_cell_is(t, WIDE_CLUSTER, 2), "no wide cluster cell");
        }),
        ("link VS16 (11-vs16)", LINK, VS16.to_string(), |t| {
            assert!(!t.viewport().links.is_empty(), "no link in the viewport");
            assert!(some_cell_is(t, VS16, 2), "no 2-wide VS16 cell");
        }),
        // insert mode, wraparound on: the line fill's inserts (a widened unit inserts one
        // cell per repetition)
        ("insert a (wrap)", INSERT, "a".to_string(), |t| {
            assert!(scrolled_lines(t) > 0, "insert REP did not scroll");
        }),
        ("insert VS16 (wrap)", INSERT, VS16.to_string(), |t| {
            assert!(some_cell_is(t, VS16, 2), "no 2-wide VS16 cell");
        }),
        ("insert ul a (wrap)", INSERT_UL, "a".to_string(), |t| {
            assert!(scrolled_lines(t) > 0, "insert REP did not scroll");
        }),
        ("insert ul VS16 (wrap)", INSERT_UL, VS16.to_string(), |t| {
            assert!(some_cell_is(t, VS16, 2), "no 2-wide VS16 cell");
        }),
    ];
    let groups: [(&[Variant], Make); 2] = [
        (&variants, new_terminal),
        (&vs16_variants, new_vs16_terminal),
    ];
    for (variants, make) in groups {
        for (name, pre, unit, check) in variants {
            let data = stream(unit);
            let mut row = format!("{name:<22}");
            for &(cols, rows, sb) in &configs {
                let ns = measure(
                    || {
                        let mut term = make(cols, rows, sb);
                        term.write(pre.as_bytes());
                        term
                    },
                    &data,
                    check,
                );
                row.push_str(&format!(" {:>12}", format_ns(ns)));
            }
            println!("{row}");
        }
    }
    let mut row = format!("{:<22}", "DECALN (control)");
    for &(cols, rows, sb) in &configs {
        row.push_str(&format!(" {:>12}", format_ns(decaln(cols, rows, sb))));
    }
    println!("{row}");
}

fn no_wrap_insert() {
    println!();
    println!(
        "No-wrap insert REP on a line of L cells after the alt screen is shrunk (ns per input \
         byte, scrollback 10,000, median of {RUNS})"
    );
    println!(
        "{:<22} {:>6} {:>14} {:>14}",
        "variant", "L", "-> 80x24", "-> 300x100"
    );
    let units: [(&str, &str, Make); 4] = [
        ("insert a", "a", new_terminal),
        ("insert e+U+0301", NARROW_CLUSTER, new_terminal),
        ("insert VS16", VS16, new_vs16_terminal),
        ("insert keycap", KEYCAP, new_vs16_terminal),
    ];
    for (name, unit, new_term) in units {
        let data = stream(unit);
        for &line_length in &LINE_LENGTHS {
            let mut row = format!("{name:<22} {line_length:>6}");
            for &(cols, rows) in &SIZES {
                if line_length < cols {
                    // the window would grow: no line longer than the window
                    row.push_str(&format!(" {:>14}", "-"));
                    continue;
                }
                let make = || {
                    let mut term = new_term(line_length, rows, 10_000);
                    term.write(b"\x1b[?1049h");
                    for _ in 0..rows {
                        term.write(&vec![b'x'; line_length]);
                    }
                    term.resize(cols, rows);
                    // no wraparound, last row, insert mode
                    term.write(b"\x1b[?7l\x1b[999;1H\x1b[4h");
                    term
                };
                // the setup claims lines of `line_length` cells in a window of `cols`
                {
                    let term = make();
                    let buffer = term.core().input_handler().services().buffer.buffer();
                    let length = buffer.lines.get(buffer.ybase).map_or(0, |line| line.len());
                    assert_eq!(
                        length,
                        line_length.max(cols),
                        "line length after the shrink"
                    );
                    assert_eq!(term.cols(), cols, "window width after the shrink");
                }
                let ns = measure(make, &data, |term| {
                    assert!(some_cell_has(term, unit), "the REP printed nothing");
                });
                row.push_str(&format!(" {:>14}", format_ns(ns)));
            }
            println!("{row}");
        }
    }
}

/// REP below a scroll region (review S2 round 2): a region of rows 1..=10, the cursor on the
/// last row, insert mode, wraparound on. Below the region nothing scrolls or trims, so the cut
/// floor drops the scrollback; the cost should track plain REP, not grow with the 10,000-line
/// scrollback. Plain `a` is in the table for the ratio the target bounds (≤ 3×).
fn below_region() {
    println!();
    println!(
        "REP below a scroll region (CSI 1;10 r, cursor last row, insert mode; ns per input byte, \
         scrollback 10,000, median of {RUNS})"
    );
    println!("{:<22} {:>14} {:>14}", "variant", "80x24", "300x100");
    const BELOW: &[u8] = b"\x1b[1;10r\x1b[999;1H\x1b[4h";
    type Variant = (&'static str, String, Make, fn(&Terminal));
    let mut variants: Vec<Variant> = vec![
        ("plain a", "a".to_string(), new_terminal, |t| {
            assert!(some_cell_has(t, "a"), "below-region REP printed no a");
        }),
        ("e+U+0301", NARROW_CLUSTER.to_string(), new_terminal, |t| {
            assert!(
                some_cell_has(t, NARROW_CLUSTER),
                "no cluster below the region"
            );
        }),
        (
            "wide cluster",
            WIDE_CLUSTER.to_string(),
            new_terminal,
            |t| {
                assert!(
                    some_cell_has(t, WIDE_CLUSTER),
                    "no wide cluster below the region"
                );
            },
        ),
        ("VS16 (11-vs16)", VS16.to_string(), new_vs16_terminal, |t| {
            assert!(some_cell_is(t, VS16, 2), "no VS16 cell below the region");
        }),
        (
            "keycap (11-vs16)",
            KEYCAP.to_string(),
            new_vs16_terminal,
            |t| {
                assert!(
                    some_cell_is(t, KEYCAP, 2),
                    "no keycap cell below the region"
                );
            },
        ),
    ];
    // (M7 fix round 3) `a` inside an open link and with an underline colour: the setup
    // writes the sequence before the stream
    variants.push(("link a", "a".to_string(), new_terminal, |t| {
        assert!(some_cell_has(t, "a"), "below-region link REP printed no a");
    }));
    variants.push(("ul colour a", "a".to_string(), new_terminal, |t| {
        assert!(some_cell_has(t, "a"), "below-region ul REP printed no a");
    }));
    for (name, unit, make, check) in &variants {
        let data = stream(unit);
        let pre: &[u8] = match *name {
            "link a" => LINK.as_bytes(),
            "ul colour a" => b"\x1b[58:5:3;4m",
            _ => b"",
        };
        let mut row = format!("{name:<22}");
        for &(cols, rows) in &SIZES {
            let ns = measure(
                || {
                    let mut term = make(cols, rows, 10_000);
                    term.write(BELOW);
                    term.write(pre);
                    term
                },
                &data,
                check,
            );
            row.push_str(&format!(" {:>14}", format_ns(ns)));
        }
        println!("{row}");
    }
}

/// REP below a region on a long row in a non-reflowing normal buffer (review S2 round 2):
/// Windows mode (no reflow), a buffer of L cells shrunk to 80 columns (the rows keep L cells),
/// a region of rows 1..=10, the cursor on the last row, insert mode, wraparound on. Linear in
/// L; the orchestrator's target bounds it against the step-13 no-wrap insert `e+U+0301` figure.
fn below_region_no_reflow() {
    println!();
    println!(
        "REP below a region on a long row, Windows mode, no reflow (ns per input byte, 80x24, \
         scrollback 10,000, median of {RUNS})"
    );
    println!("{:<22} {:>8} {:>14}", "variant", "L", "80x24");
    // (M7 fix round 3) and inside an open link and with an underline colour, the review's
    // other rows
    let units: [(&str, &str, &str, Make); 5] = [
        ("a", "", "a", new_winmode_terminal),
        ("e+U+0301", "", NARROW_CLUSTER, new_winmode_terminal),
        ("VS16 (11-vs16)", "", VS16, new_winmode_vs16_terminal),
        ("link a", LINK, "a", new_winmode_terminal),
        ("ul colour a", "\x1b[58:5:3;4m", "a", new_winmode_terminal),
    ];
    for (name, pre, unit, new_term) in units {
        let data = stream(unit);
        for &l in &[300usize, 1_000, 2_000] {
            let make = || {
                let mut term = new_term(l, 24, 10_000);
                for _ in 0..24 {
                    term.write(&vec![b'x'; l]);
                }
                term.resize(80, 24);
                term.write(b"\x1b[1;10r\x1b[999;1H\x1b[4h");
                term.write(pre.as_bytes());
                term
            };
            // the long row survived the shrink (no reflow in Windows mode)
            {
                let term = make();
                let buffer = term.core().input_handler().services().buffer.buffer();
                let last = buffer.ybase + 23;
                let len = buffer.lines.get(last).map_or(0, |line| line.len());
                assert_eq!(len, l, "the long row did not survive the shrink");
            }
            let ns = measure(make, &data, |t| {
                assert!(
                    some_cell_has(t, unit),
                    "below-region long-row REP printed nothing"
                );
            });
            println!("{name:<22} {l:>8} {:>14}", format_ns(ns));
        }
    }
}

/// REP below a scroll region at odd widths (M7 fix round 3, review S2): a widened unit (VS16,
/// the keycap) whose base lands on the last column is wrapped by its joiner, and that wrap
/// copies the old row's strings from the last column on (upstream's copy), so a long row's
/// passes settle only once its tail has. Short rows (79x24, 299x100), and a row of L = 2000
/// cells narrowed to 79 columns in Windows mode, against plain `a` in the same setup.
fn below_region_odd() {
    println!();
    println!(
        "REP below a scroll region at odd widths (insert mode, scrollback 10,000; ns per input \
         byte, median of {RUNS})"
    );
    println!(
        "{:<22} {:>14} {:>14} {:>16}",
        "variant", "79x24", "299x100", "L=2000 -> 79x24"
    );
    const BELOW: &[u8] = b"\x1b[1;10r\x1b[999;1H\x1b[4h";
    let units: [(&str, &str, Make, Make); 3] = [
        ("plain a", "a", new_terminal, new_winmode_terminal),
        (
            "VS16 (11-vs16)",
            VS16,
            new_vs16_terminal,
            new_winmode_vs16_terminal,
        ),
        (
            "keycap (11-vs16)",
            KEYCAP,
            new_vs16_terminal,
            new_winmode_vs16_terminal,
        ),
    ];
    for (name, unit, make, make_long) in units {
        let data = stream(unit);
        let mut row = format!("{name:<22}");
        for (cols, rows) in [(79, 24), (299, 100)] {
            let ns = measure(
                || {
                    let mut term = make(cols, rows, 10_000);
                    term.write(BELOW);
                    term
                },
                &data,
                |t| assert!(some_cell_has(t, unit), "odd-width REP printed nothing"),
            );
            row.push_str(&format!(" {:>14}", format_ns(ns)));
        }
        let long = || {
            let mut term = make_long(2_000, 24, 10_000);
            for _ in 0..24 {
                term.write(&vec![b'x'; 2_000]);
            }
            term.resize(79, 24);
            term.write(BELOW);
            term
        };
        {
            let term = long();
            let buffer = term.core().input_handler().services().buffer.buffer();
            let len = buffer
                .lines
                .get(buffer.ybase + 23)
                .map_or(0, |line| line.len());
            assert_eq!(len, 2_000, "the long row did not survive the shrink");
        }
        let ns = measure(long, &data, |t| {
            assert!(
                some_cell_has(t, unit),
                "odd-width long-row REP printed nothing"
            );
        });
        row.push_str(&format!(" {:>16}", format_ns(ns)));
        println!("{row}");
    }
}

/// REP of a widened unit at odd widths over narrowed rows of strings (M7 fix round 4, code
/// review F6): Windows mode (no reflow), every row written as L cells each holding a letter and a
/// combining mark (the marks cycle through U+0300..U+036F, so neighbouring strings differ), then
/// shrunk to `cols` (odd); below a region of rows 1..=10 (the cursor on the last row) and at the
/// scroll bottom, with and without insert mode. A widened unit's joiner wraps its base on an odd
/// number of columns, and each such wrap copies the old row's strings from the last column on,
/// `cols - 1` to the left (upstream's copy), so the REP cut floor grows with the rows' last key:
/// `ceil((K + 1) / (cols - 1)) + 1` more lines of units. Plain `a` in the same setup is the
/// reference.
fn odd_widths_over_strings() {
    println!();
    println!(
        "REP at odd widths over narrowed rows of strings, Windows mode, scrollback 10,000 (ns per \
         input byte, median of {RUNS})"
    );
    // (M7 fix round 5) rows not narrowed (L = cols), and the even widths 80 and 22 (no joiner
    // wrap there: the reference for the same unit and L)
    let configs: [(usize, usize); 8] = [
        (79, 79),
        (79, 300),
        (79, 1_000),
        (79, 2_000),
        (21, 21),
        (21, 2_000),
        (80, 2_000),
        (22, 2_000),
    ];
    let mut header = format!("{:<34}", "variant");
    for (cols, l) in configs {
        header.push_str(&format!(" {:>13}", format!("{l}->{cols}")));
    }
    println!("{header}");
    const BELOW: &str = "\x1b[1;10r\x1b[999;1H";
    const BOTTOM: &str = "\x1b[999;1H";
    let cases: [(&str, &str, &str, Make); 10] = [
        (
            "plain a, below, insert",
            "a",
            "\x1b[4h",
            new_winmode_terminal,
        ),
        ("plain a, below", "a", "", new_winmode_terminal),
        (
            "plain a, scroll bottom, insert",
            "a",
            "\x1b[4h",
            new_winmode_terminal,
        ),
        ("plain a, scroll bottom", "a", "", new_winmode_terminal),
        (
            "VS16, below, insert",
            VS16,
            "\x1b[4h",
            new_winmode_vs16_terminal,
        ),
        ("VS16, below", VS16, "", new_winmode_vs16_terminal),
        ("keycap, below", KEYCAP, "", new_winmode_vs16_terminal),
        (
            "VS16, scroll bottom, insert",
            VS16,
            "\x1b[4h",
            new_winmode_vs16_terminal,
        ),
        ("VS16, scroll bottom", VS16, "", new_winmode_vs16_terminal),
        (
            "keycap, scroll bottom",
            KEYCAP,
            "",
            new_winmode_vs16_terminal,
        ),
    ];
    for (name, unit, modes, new_term) in cases {
        let data = stream(unit);
        let place = if name.contains("below") {
            BELOW
        } else {
            BOTTOM
        };
        let mut row = format!("{name:<34}");
        for (cols, l) in configs {
            let make = || {
                let mut term = new_term(l, 24, 10_000);
                let mut text = String::new();
                for c in 0..l {
                    text.push(char::from(b'a' + (c % 26) as u8));
                    text.push(char::from_u32(0x300 + (c % 112) as u32).unwrap_or('\u{301}'));
                }
                for _ in 0..24 {
                    term.write(text.as_bytes());
                }
                term.resize(cols, 24);
                term.write(place.as_bytes());
                term.write(modes.as_bytes());
                term
            };
            // the long row survived the shrink and holds its strings
            {
                let term = make();
                let buffer = term.core().input_handler().services().buffer.buffer();
                let last = buffer.lines.get(buffer.ybase + 23);
                assert_eq!(
                    last.map_or(0, |line| line.len()),
                    l,
                    "the long row did not survive"
                );
                assert!(
                    last.is_some_and(|line| line.combined_len() + 2 >= l),
                    "the long row lost its strings"
                );
            }
            let ns = measure(make, &data, |t| {
                assert!(
                    some_cell_has(t, unit),
                    "odd-width string-row REP printed nothing"
                );
            });
            row.push_str(&format!(" {:>13}", format_ns(ns)));
        }
        println!("{row}");
    }
}

/// REP of a unit whose first code point joins (review S3 round 2): a lone combining mark left
/// at `cols - 3` by a wide char skipped at the last column without wraparound, the cursor on
/// it; `3 x REP` after the one construction. Every repetition appends to the one cell, which
/// caps, so the cut bounds the cost near plain REP (the target: ≤ 3×).
fn joining_base() {
    println!();
    println!("REP of a joining-base unit (review S3 round 2; ns per input byte, median of {RUNS})");
    println!(
        "{:<24} {:>12} {:>12} {:>12} {:>12}",
        "variant", "80x24 sb1k", "80x24 sb10k", "300x100 sb1k", "300x100 sb10k"
    );
    let configs: Vec<(usize, usize, usize)> = SIZES
        .iter()
        .flat_map(|&(cols, rows)| SCROLLBACKS.iter().map(move |&sb| (cols, rows, sb)))
        .collect();
    let data: Vec<u8> = REP
        .iter()
        .copied()
        .cycle()
        .take(REP.len() * REPS_PER_RUN)
        .collect();
    let cases: [(&str, &str, Make); 3] = [
        ("joining base", "", new_terminal),
        ("joining base, insert", "\x1b[4h", new_terminal),
        ("joining base (11-vs16)", "", new_vs16_terminal),
    ];
    for (name, pre, make) in cases {
        let mut row = format!("{name:<24}");
        for &(cols, rows, sb) in &configs {
            let ns = measure(
                || {
                    let mut term = make(cols, rows, sb);
                    term.write(b"\x1b[?7l");
                    term.write(pre.as_bytes());
                    term.write("\x1b[999C\x1b[2D\u{301}x\u{4f60}".as_bytes());
                    term
                },
                &data,
                |t| {
                    assert!(
                        t.viewport()
                            .cells
                            .iter()
                            .flatten()
                            .any(|cell| cell.text.contains('\u{301}')),
                        "joining-base REP left no combining mark"
                    );
                },
            );
            row.push_str(&format!(" {:>12}", format_ns(ns)));
        }
        println!("{row}");
    }
}

fn main() {
    println!("rep_cost: lune-vt {}", env!("CARGO_PKG_VERSION"));
    println!(
        "machine: {} {}; load average: {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        load_average()
    );
    println!(
        "stream: {REPS_PER_RUN} x (unit + CSI 1048576 b), Unicode 11, median of {RUNS} fresh terminals"
    );
    println!();
    at_screen_size();
    no_wrap_insert();
    below_region();
    below_region_no_reflow();
    below_region_odd();
    odd_widths_over_strings();
    joining_base();
    println!();
    println!("load average after: {}", load_average());
}
