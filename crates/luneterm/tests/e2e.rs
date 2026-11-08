//! End-to-end harness (plan M9 step 8, D19): the real session on real PTY children, headless.
//!
//! A custom runner (`harness = false`). With `--child <mode> ...` this binary is the PTY child
//! (see `e2e/child.rs`); otherwise it runs the cases E1-E9 in order, or those whose id or name
//! contains the first non-flag argument (`cargo test -p luneterm --test e2e -- E5`). It prints
//! `ok` / `FAILED` / `SKIPPED` per case and exits non-zero on any failure. `LUNE_REQUIRE_GPU=1`
//! turns E8's missing adapter into a failure.

#[path = "e2e/cases.rs"]
mod cases;
#[path = "e2e/child.rs"]
mod child;
#[path = "e2e/rss.rs"]
mod rss;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::time::Instant;

use cases::{CASES, Status};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        return child::run(&args[1..]);
    }
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let mut failed = 0usize;
    let mut ran = 0usize;
    let total = Instant::now();
    for case in CASES {
        if let Some(f) = &filter
            && !case.id.eq_ignore_ascii_case(f)
            && !case.name.to_lowercase().contains(&f.to_lowercase())
        {
            continue;
        }
        ran += 1;
        println!("{} {} ...", case.id, case.name);
        let started = Instant::now();
        let result =
            catch_unwind(AssertUnwindSafe(case.run)).unwrap_or_else(|_| Err("panicked".to_owned()));
        let secs = started.elapsed().as_secs_f64();
        match result {
            Ok(Status::Ok(detail)) => println!("{} ok ({secs:.1} s): {detail}", case.id),
            Ok(Status::Skipped(why)) => println!("{} SKIPPED: {why}", case.id),
            Err(e) => {
                failed += 1;
                println!("{} FAILED ({secs:.1} s): {e}", case.id);
            }
        }
    }
    println!(
        "e2e: {ran} cases, {failed} failed, {:.1} s",
        total.elapsed().as_secs_f64()
    );
    if failed > 0 || ran == 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
