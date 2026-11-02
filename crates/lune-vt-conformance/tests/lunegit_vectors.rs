//! Lunegit's 27 tmux vectors (IVtEngineConformanceTests) through `lune_vt::Terminal`.
//!
//! - The 21 non-divergent vectors: the grid in Lunegit's normalisation equals the tmux 3.6b grid.
//! - The 6 divergent vectors (xterm.js does not reflow the cursor's own wrapped line): the grid
//!   differs from the tmux grid, and the paired `lunegit_<name>` screen golden (what xterm.js
//!   produces, P10) matches.
//! - All 27: feeding one byte per write gives the grid of the recorded chunking.
//! - All of it again with the standard extensions installed (VS16 off, plan M7 step 9): the
//!   extensions change no screen state xterm.js defines.

mod common;

use common::{check_all_pass, describe_difference, guarded};
use lune_vt::StandardExtensions;
use lune_vt_conformance::screen::{dump_with, grid_with, standard_extensions};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn size_of(pair: Option<(u32, u32)>) -> Option<(usize, usize)> {
    pair.map(|(c, r)| (c as usize, r as usize))
}

/// The first failed check of `vector`, or `None`.
fn failure_of(
    vector: &Golden,
    screens: &[Golden],
    extensions: Option<&StandardExtensions>,
) -> Option<String> {
    let (size, resize) = (size_of(vector.size), size_of(vector.resize));
    let recorded = match guarded(|| {
        grid_with(
            extensions,
            size,
            resize,
            vector.chunks.iter().map(Vec::as_slice),
        )
    }) {
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
            dump_with(
                extensions,
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
    match guarded(|| grid_with(extensions, size, resize, singles.iter().copied())) {
        Err(panic) => Some(format!("1-byte: {panic}")),
        Ok(rows) => describe_difference("1-byte vs recorded", &rows, &recorded),
    }
}

#[test]
fn lunegit_vectors_match() -> Result<()> {
    check_vectors(None, "Lunegit vectors")
}

#[test]
fn lunegit_vectors_match_with_the_standard_extensions_installed() -> Result<()> {
    check_vectors(
        Some(&standard_extensions()),
        "Lunegit vectors with extensions",
    )
}

fn check_vectors(extensions: Option<&StandardExtensions>, label: &str) -> Result<()> {
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
        .map(|vector| {
            (
                vector.name.clone(),
                failure_of(vector, &screens, extensions),
            )
        })
        .collect();
    check_all_pass(label, &results);
    Ok(())
}
