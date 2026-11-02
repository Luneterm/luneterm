//! Screen differential: every screen golden (the 61 hand cases and the 27 oracle dumps of the
//! Lunegit vectors) is replayed through `lune_vt::Terminal` and the dump must equal `expect`.
//!
//! Each golden is replayed as recorded, unsplit, one byte per write and with three seeded
//! random splits; all must equal `expect` (divergent goldens hold the unsplit dump, FORMAT.md).

mod common;

use common::{check_all_pass, describe_difference, guarded};
use lune_vt_conformance::screen::{dump, replays};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn size_of(pair: Option<(u32, u32)>) -> Option<(usize, usize)> {
    pair.map(|(c, r)| (c as usize, r as usize))
}

/// The first failing replay of `golden`, or `None`.
fn failure_of(golden: &Golden) -> Option<String> {
    for (how, chunks) in replays(&golden.name, &golden.chunks) {
        let got = guarded(|| {
            dump(
                size_of(golden.size),
                size_of(golden.resize),
                chunks.iter().map(Vec::as_slice),
            )
        });
        match got {
            Err(panic) => return Some(format!("{how}: {panic}")),
            Ok(lines) => {
                if let Some(difference) = describe_difference(&how, &lines, &golden.expect) {
                    return Some(difference);
                }
            }
        }
    }
    None
}

#[test]
fn screen_goldens_match() -> Result<()> {
    let goldens = load(Kind::Screen)?;
    assert!(goldens.len() >= 88, "screen goldens: {}", goldens.len());
    let results: Vec<(String, Option<String>)> = goldens
        .iter()
        .map(|golden| (golden.name.clone(), failure_of(golden)))
        .collect();
    check_all_pass("screen goldens", &results);
    Ok(())
}
