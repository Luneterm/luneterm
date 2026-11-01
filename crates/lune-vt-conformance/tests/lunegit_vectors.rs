//! Lunegit's 27 tmux vectors (IVtEngineConformanceTests) through `lune_vt::Terminal`.
//!
//! - The 21 non-divergent vectors: the grid in Lunegit's normalisation equals the tmux 3.6b grid.
//! - The 6 divergent vectors (xterm.js does not reflow the cursor's own wrapped line): the grid
//!   differs from the tmux grid, and the paired `lunegit_<name>` screen golden (what xterm.js
//!   produces, P10) matches.
//! - All 27: feeding one byte per write gives the grid of the recorded chunking.
//!
//! `PENDING` lists the vectors expected to fail because their handlers are ported in a later
//! step of milestone M6 (plan steps 5-8); the test fails when the failing set differs from it
//! in either direction. It must be empty at the end of step 8.

mod common;

use common::{check_pending, describe_difference, guarded};
use lune_vt_conformance::screen::{dump, grid};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Vectors that fail until their handlers are ported (steps 5-8). Shrinks to empty.
const PENDING: &[&str] = &[
    "el_clearline_afterwrap",
    "el_to_eol",
    "erase_display_below",
    "rep_csi_b",
    "ich_insertchar",
    "dch_deletechar",
    "inplace_wrap_redraw",
    "ech_erasechar",
    "wrap_widechar_acct",
    "el0_after_widechar",
];

fn size_of(pair: Option<(u32, u32)>) -> Option<(usize, usize)> {
    pair.map(|(c, r)| (c as usize, r as usize))
}

/// The first failed check of `vector`, or `None`.
fn failure_of(vector: &Golden, screens: &[Golden]) -> Option<String> {
    let (size, resize) = (size_of(vector.size), size_of(vector.resize));
    let recorded = match guarded(|| grid(size, resize, vector.chunks.iter().map(Vec::as_slice))) {
        Ok(rows) => rows,
        Err(panic) => return Some(format!("recorded: {panic}")),
    };

    if vector.divergent.is_some() {
        if recorded == vector.expect {
            return Some("divergent vector now equals the tmux grid".to_string());
        }
        let name = format!("lunegit_{}", vector.name);
        let Some(screen) = screens.iter().find(|g| g.name == name) else {
            return Some(format!("no paired screen golden '{name}'"));
        };
        match guarded(|| {
            dump(
                size_of(screen.size),
                size_of(screen.resize),
                screen.chunks.iter().map(Vec::as_slice),
            )
        }) {
            Err(panic) => return Some(format!("{name}: {panic}")),
            Ok(lines) => {
                if let Some(difference) = describe_difference(&name, &lines, &screen.expect) {
                    return Some(difference);
                }
            }
        }
    } else if let Some(difference) = describe_difference("recorded", &recorded, &vector.expect) {
        return Some(difference);
    }

    let flat: Vec<u8> = vector.chunks.iter().flatten().copied().collect();
    let singles: Vec<&[u8]> = flat.chunks(1).collect();
    match guarded(|| grid(size, resize, singles.iter().copied())) {
        Err(panic) => Some(format!("1-byte: {panic}")),
        Ok(rows) => describe_difference("1-byte vs recorded", &rows, &recorded),
    }
}

#[test]
fn lunegit_vectors_match_except_pending() -> Result<()> {
    let vectors = load(Kind::Grid)?;
    let screens = load(Kind::Screen)?;
    assert_eq!(vectors.len(), 27, "Lunegit vectors");
    assert_eq!(
        vectors.iter().filter(|v| v.divergent.is_some()).count(),
        6,
        "divergent vectors"
    );
    let results: Vec<(String, Option<String>)> = vectors
        .iter()
        .map(|vector| (vector.name.clone(), failure_of(vector, &screens)))
        .collect();
    check_pending("Lunegit vectors", &results, PENDING);
    Ok(())
}
