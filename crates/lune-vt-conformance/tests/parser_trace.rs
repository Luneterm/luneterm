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

#[test]
fn divergent_goldens_are_the_split_sequence_cases() -> Result<()> {
    let goldens = load(Kind::Parser)?;
    let divergent: Vec<&Golden> = goldens.iter().filter(|g| g.divergent.is_some()).collect();
    assert!(
        divergent.iter().any(|g| g.name == "utf8_split_after_80"),
        "utf8_split_after_80 must be marked divergent"
    );
    for golden in divergent {
        assert_eq!(
            golden.divergent.as_deref(),
            Some("upstream Utf8ToUtf32 split-sequence bug (TextDecoder.ts:161)"),
            "{}",
            golden.name
        );
        // The point of the divergence: chunking changes nothing.
        let chunked = trace_chunks(golden.chunks.iter().map(Vec::as_slice));
        assert!(
            golden.chunks.len() > 1,
            "{} is divergent but has one chunk",
            golden.name
        );
        assert_eq!(
            first_difference(&chunked, &golden.expect),
            None,
            "{}",
            golden.name
        );
    }
    Ok(())
}
