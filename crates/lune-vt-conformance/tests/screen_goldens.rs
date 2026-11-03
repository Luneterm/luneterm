//! Screen differential: every screen golden (the 61 hand cases and the 27 oracle dumps of the
//! Lunegit vectors) is replayed through `lune_vt::Terminal` and the dump must equal `expect`.
//!
//! Each golden is replayed as recorded, unsplit, one byte per write and with three seeded
//! random splits; all must equal `expect` (divergent goldens hold the unsplit dump, FORMAT.md).
//! Then again with the standard extensions installed (VS16 off, plan M7 step 9): the extensions
//! change no screen state xterm.js defines, so the same `expect` holds. The one exception is a
//! reply the extensions add on purpose (the kitty flags query `CSI ? u`, the modifyOtherKeys
//! query `CSI ? 4 m`, XTVERSION): `response` lines of exactly those shapes are dropped from the
//! extension run's dump before the comparison; every other line, and every other reply, must
//! match.

mod common;

use common::{check_all_pass, describe_difference, guarded};
use lune_vt::StandardExtensions;
use lune_vt_conformance::screen::{dump_with, replays, standard_extensions};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn size_of(pair: Option<(u32, u32)>) -> Option<(usize, usize)> {
    pair.map(|(c, r)| (c as usize, r as usize))
}

/// A `response` line of a reply only the standard extensions send: `CSI ? <flags> u` (kitty),
/// `CSI > 4 ; <level> m` (modifyOtherKeys) or `DCS > | <name> ST` (XTVERSION), in the dump's
/// escaped form.
fn is_extension_reply(line: &str) -> bool {
    let Some(body) = line.strip_prefix("response \"") else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = body
        .strip_prefix("\\u{1B}[?")
        .and_then(|r| r.strip_suffix("u\""))
    {
        return digits(rest);
    }
    if let Some(rest) = body
        .strip_prefix("\\u{1B}[>4;")
        .and_then(|r| r.strip_suffix("m\""))
    {
        return digits(rest);
    }
    body == "\\u{1B}P>|lune-vt-test\\u{1B}\\\\\""
}

/// The first failing replay of `golden`, or `None`.
fn failure_of(golden: &Golden, extensions: Option<&StandardExtensions>) -> Option<String> {
    for (how, chunks) in replays(&golden.name, &golden.chunks) {
        let got = guarded(|| {
            dump_with(
                extensions,
                size_of(golden.size),
                size_of(golden.resize),
                chunks.iter().map(Vec::as_slice),
            )
        });
        match got {
            Err(panic) => return Some(format!("{how}: {panic}")),
            Ok(mut lines) => {
                if extensions.is_some() {
                    lines.retain(|line| !is_extension_reply(line));
                }
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
        .map(|golden| (golden.name.clone(), failure_of(golden, None)))
        .collect();
    check_all_pass("screen goldens", &results);
    Ok(())
}

#[test]
fn the_extension_reply_filter_is_exact() {
    for yes in [
        "response \"\\u{1B}[?5u\"",
        "response \"\\u{1B}[?0u\"",
        "response \"\\u{1B}[>4;2m\"",
        "response \"\\u{1B}P>|lune-vt-test\\u{1B}\\\\\"",
    ] {
        assert!(is_extension_reply(yes), "{yes}");
    }
    for no in [
        "response \"\\u{1B}[?1;2c\"",
        "response \"\\u{1B}[?u\"",
        "response \"\\u{1B}[?5;1u\"",
        "response \"\\u{1B}[>4;m\"",
        "response \"\\u{1B}P>|other\\u{1B}\\\\\"",
        "response \"\\u{1B}[0n\"",
        "line 0 0 \"x\"",
    ] {
        assert!(!is_extension_reply(no), "{no}");
    }
}

#[test]
fn screen_goldens_match_with_the_standard_extensions_installed() -> Result<()> {
    let goldens = load(Kind::Screen)?;
    assert!(goldens.len() >= 88, "screen goldens: {}", goldens.len());
    let extensions = standard_extensions();
    let results: Vec<(String, Option<String>)> = goldens
        .iter()
        .map(|golden| (golden.name.clone(), failure_of(golden, Some(&extensions))))
        .collect();
    check_all_pass("screen goldens with extensions", &results);
    Ok(())
}
