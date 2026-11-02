//! Shared by the engine differential tests: panic capture and the pass/fail report.
#![allow(dead_code, unreachable_pub)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use lune_vt_conformance::trace::{clip, first_difference};

/// Longest failure line shown.
pub const CLIP: usize = 160;

/// Runs `f`, turning a panic into `Err(message)` so one bad golden does not hide the others.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        format!("PANIC: {}", clip(Some(&message), CLIP))
    })
}

/// `None` when `got == expected`, else the first difference with both lines clipped.
pub fn describe_difference(how: &str, got: &[String], expected: &[String]) -> Option<String> {
    let index = first_difference(got, expected)?;
    Some(format!(
        "{how}: first difference at line {index}\n    got:      {}\n    expected: {}",
        clip(got.get(index), CLIP),
        clip(expected.get(index), CLIP),
    ))
}

/// Every golden must pass.
///
/// `results` holds, per golden name, the first failure description or `None` when it passes.
pub fn check_all_pass(label: &str, results: &[(String, Option<String>)]) {
    let report: Vec<String> = results
        .iter()
        .filter_map(|(name, failure)| failure.as_ref().map(|f| format!("FAIL {name}: {f}")))
        .collect();
    assert!(
        report.is_empty(),
        "{label}: {} of {} fail\n{}",
        report.len(),
        results.len(),
        report
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
