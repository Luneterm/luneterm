//! Screen differential: every screen golden (the 61 hand cases and the 27 oracle dumps of the
//! Lunegit vectors) is replayed through `lune_vt::Terminal` and the dump must equal `expect`.
//!
//! Each golden is replayed as recorded, unsplit, one byte per write and with three seeded
//! random splits; all must equal `expect` (divergent goldens hold the unsplit dump, FORMAT.md).
//!
//! `PENDING` lists the goldens expected to fail because their handlers are ported in a later
//! step of milestone M6 (plan steps 5-8). The test fails when the failing set differs from it in
//! either direction, so a newly passing golden must be removed and a regression is caught.
//! It must be empty at the end of step 8.

mod common;

use common::{check_pending, describe_difference, guarded};
use lune_vt_conformance::screen::{dump, replays};
use lune_vt_conformance::{Golden, Kind, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Goldens that fail until their handlers are ported (steps 5-8). Shrinks to empty.
const PENDING: &[&str] = &[
    "sgr_attributes",
    "sgr_fg_bg_16",
    "sgr_bold_with_palette",
    "sgr_256",
    "sgr_rgb",
    "sgr_underline_styles",
    "sgr_underline_colors",
    "sgr_resets",
    "sgr_bad_params",
    "erase_with_background",
    "osc8_links",
    "osc8_link_with_underline",
    "decset_modes",
    "mouse_x10",
    "mouse_vt200",
    "mouse_drag",
    "mouse_any_sgr",
    "mouse_utf8_encoding",
    "mouse_urxvt_encoding",
    "mouse_sgr_pixels",
    "bracketed_paste_and_sync",
    "alt_screen_1049",
    "alt_screen_47_1047",
    "scrollback_attributes",
    "da1_da2",
    "dsr_5_6",
    "dsr_6_origin_mode",
    "decrqm_dec_modes",
    "decrqm_ansi_modes",
    "decrqss_sgr_and_margins",
    "title_osc",
    "title_unicode_and_long",
    "title_st_terminators",
    "title_stack",
    "charsets_dec_graphics",
    "insert_mode_and_tabs",
    "origin_mode_margins",
    "save_restore_cursor",
    "wrap_pending_and_reverse",
    "rep_and_ech",
    "decaln_and_ris",
    "keypad_modes",
    "c1_as_utf8",
    "resize_shrink_cols",
    "resize_with_alt_buffer",
    "chunked_ascii_and_csi",
    "lunegit_el_clearline_afterwrap",
    "lunegit_el_to_eol",
    "lunegit_erase_display_below",
    "lunegit_rep_csi_b",
    "lunegit_ich_insertchar",
    "lunegit_dch_deletechar",
    "lunegit_inplace_wrap_redraw",
    "lunegit_ech_erasechar",
    "lunegit_wrap_widechar_acct",
    "lunegit_el0_after_widechar",
    "lunegit_altscreen_resize",
];

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
fn screen_goldens_match_except_pending() -> Result<()> {
    let goldens = load(Kind::Screen)?;
    assert!(goldens.len() >= 88, "screen goldens: {}", goldens.len());
    let results: Vec<(String, Option<String>)> = goldens
        .iter()
        .map(|golden| (golden.name.clone(), failure_of(golden)))
        .collect();
    check_pending("screen goldens", &results, PENDING);
    Ok(())
}
