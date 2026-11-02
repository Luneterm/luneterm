//! Throughput of `Terminal::write` over the stream `tools/xterm-oracle/bench.mjs --generate`
//! writes, for comparison with `@xterm/headless` (`scripts/bench-vt.sh`).
//!
//! `cargo run --release -p lune-vt --example throughput -- <file>`
//!
//! Same bytes, 64 KiB chunks, 80x24, Unicode 11 active, 5 runs, median. The host's duties are
//! included: each chunk's events and damage are drained, as a real host would. Output is the
//! `key=value` lines `bench.mjs --run` prints. Throughput is MiB/s.

use std::time::Instant;

use lune_vt::Terminal;
use lune_vt::services::options_service::OptionValue;

const CHUNK: usize = 64 * 1024;
const COLS: usize = 80;
const ROWS: usize = 24;
const RUNS: usize = 5;

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5_u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

fn join(values: &[f64]) -> String {
    let parts: Vec<String> = values.iter().map(|v| format!("{v:.2}")).collect();
    parts.join(",")
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    sorted[sorted.len() / 2]
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: throughput <file>");
        std::process::exit(2);
    };
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("throughput: cannot read {path}: {err}");
            std::process::exit(2);
        }
    };
    let mib = data.len() as f64 / (1024.0 * 1024.0);
    println!("engine=lune-vt {}", env!("CARGO_PKG_VERSION"));
    println!(
        "bytes={} fnv1a={:08x} chunk={CHUNK} grid={COLS}x{ROWS} unicode=11",
        data.len(),
        fnv1a(&data)
    );

    let mut rates = Vec::new();
    let mut summary = String::new();
    for _ in 0..RUNS {
        let mut term = Terminal::new([OptionValue::Cols(COLS), OptionValue::Rows(ROWS)]);
        let _ = term.unicode_mut().set_active_version("11");
        let start = Instant::now();
        for chunk in data.chunks(CHUNK) {
            term.write(chunk);
            drop(term.drain_events());
            let _ = term.take_damage();
        }
        rates.push(mib / start.elapsed().as_secs_f64());
        let view = term.viewport();
        summary = format!(
            "cursor={},{} base_y={} total_lines={}",
            view.cursor.x, view.cursor.y, view.base_y, view.total_lines
        );
    }
    println!("write_mib_s={} median={:.2}", join(&rates), median(&rates));
    println!("final_screen={summary}");
}
