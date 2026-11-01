//! Parser differential: every parser golden is replayed through `lune-vt`'s decoder and parser
//! (fallback handlers only) and the normalised trace must equal the golden's `expect`.
//!
//! Divergent goldens (chunk-dependent upstream output, FORMAT.md) hold the unsplit trace, which
//! `lune-vt` must produce for any chunking. A failure names the golden, the first differing line
//! index and both lines (clipped); never the whole trace.

use lune_vt_conformance::trace::{clip, first_difference, trace_chunks};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CLIP: usize = 160;

/// Compares `got` with `golden.expect`; `how` says which way the input was fed.
fn compare(golden: &Golden, how: &str, got: &[String], failures: &mut Vec<String>) {
    if let Some(index) = first_difference(got, &golden.expect) {
        failures.push(format!(
            "{} ({how}): first difference at line {index}\n  got:      {}\n  expected: {}",
            golden.name,
            clip(got.get(index), CLIP),
            clip(golden.expect.get(index), CLIP),
        ));
    }
}

#[test]
fn all_parser_goldens_match() -> Result<()> {
    let goldens = load(Kind::Parser)?;
    assert!(goldens.len() >= 240, "parser goldens: {}", goldens.len());
    let mut failures: Vec<String> = Vec::new();
    for golden in &goldens {
        // As recorded: chunk by chunk.
        let chunked = trace_chunks(golden.chunks.iter().map(Vec::as_slice));
        compare(golden, "chunked", &chunked, &mut failures);
        // Whole input in one write: equal to the chunked result for every case (the decoder
        // keeps no state a split can corrupt).
        let whole_input: Vec<u8> = golden.chunks.iter().flatten().copied().collect();
        let unsplit = trace_chunks([whole_input.as_slice()]);
        compare(golden, "unsplit", &unsplit, &mut failures);
    }
    assert!(
        failures.is_empty(),
        "{} of {} parser goldens fail:\n{}",
        failures.len(),
        goldens.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

const UTF8_SPLIT: &str = "upstream Utf8ToUtf32 split-sequence bug (TextDecoder.ts:161)";
const DCS_DEL: &str = "upstream DCS passthrough DEL quirk (EscapeSequenceParser.ts:346, :744)";

/// The two kinds of divergence from upstream (ledger rows 3 and 18) are the only ones, and
/// `expect` of each is what the port gives for any chunking.
#[test]
fn divergent_goldens_are_the_known_upstream_bugs() -> Result<()> {
    let goldens = load(Kind::Parser)?;
    let divergent: Vec<&Golden> = goldens.iter().filter(|g| g.divergent.is_some()).collect();
    let reason_of = |name: &str| {
        divergent
            .iter()
            .find(|g| g.name == name)
            .and_then(|g| g.divergent.as_deref())
    };
    assert_eq!(
        reason_of("utf8_split_after_80"),
        Some(UTF8_SPLIT),
        "utf8_split_after_80 must be marked divergent"
    );
    assert_eq!(
        reason_of("dcs_del_inside"),
        Some(DCS_DEL),
        "dcs_del_inside must be marked divergent"
    );
    for golden in divergent {
        let reason = golden.divergent.as_deref().unwrap_or_default();
        assert!(
            reason == UTF8_SPLIT
                || reason == DCS_DEL
                || reason == format!("{UTF8_SPLIT}; {DCS_DEL}"),
            "{}: unknown divergence reason {reason:?}",
            golden.name
        );
        if reason.contains(UTF8_SPLIT) {
            assert!(
                golden.chunks.len() > 1,
                "{} is divergent but has one chunk",
                golden.name
            );
        }
        if reason.contains(DCS_DEL) {
            assert!(
                golden.expect.iter().any(|l| l.contains("\\u{7F}")),
                "{}: DEL divergence without a DEL in expect",
                golden.name
            );
        }
        // The point of the divergence: chunking changes nothing.
        let chunked = trace_chunks(golden.chunks.iter().map(Vec::as_slice));
        assert_eq!(
            first_difference(&chunked, &golden.expect),
            None,
            "{}",
            golden.name
        );
    }
    Ok(())
}
