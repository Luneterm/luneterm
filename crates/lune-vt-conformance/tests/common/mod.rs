//! Shared by the engine differential tests: panic capture and the `PENDING` bookkeeping.
#![allow(dead_code, unreachable_pub)]

use std::collections::BTreeSet;
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

/// The failing set must equal `pending` (names expected to fail until a later step ports their
/// handlers): a new failure and a newly passing name both fail the test.
///
/// `results` holds, per golden name, the first failure description or `None` when it passes.
pub fn check_pending(label: &str, results: &[(String, Option<String>)], pending: &[&str]) {
    let failing: BTreeSet<&str> = results
        .iter()
        .filter(|(_, failure)| failure.is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    let pending_set: BTreeSet<&str> = pending.iter().copied().collect();
    assert_eq!(
        pending_set.len(),
        pending.len(),
        "{label}: PENDING has duplicate names"
    );
    let mut report: Vec<String> = Vec::new();
    for (name, failure) in results {
        if let Some(failure) = failure {
            if !pending_set.contains(name.as_str()) {
                report.push(format!("NEW FAILURE {name}: {failure}"));
            }
        }
    }
    let fixed: Vec<&&str> = pending_set.difference(&failing).collect();
    if !fixed.is_empty() {
        report.push(format!(
            "PENDING names that now pass (or do not exist), remove them: {fixed:?}"
        ));
    }
    assert!(
        report.is_empty(),
        "{label}: {} of {} fail, {} pending\n{}",
        failing.len(),
        results.len(),
        pending.len(),
        report
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
