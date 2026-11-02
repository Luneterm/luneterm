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
    println!();
    println!("load average after: {}", load_average());
}
