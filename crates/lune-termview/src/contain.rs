//! Glyph containment: keep every glyph inside its cell slot (ledger 1, 2 and 8).
//!
//! Ported from Lunegit `TerminalView.cs` (`ContainmentAllowance` :2127, `RunSlotWidth` :2116,
//! `IsNextCellBlank` :2143, `OverflowFitScale` :2161, `DecideContainment` :2195, the colour-glyph
//! centring of `EmojiCentringOffset` :2053 and the baseline pin of `ComputeScaledRunOrigin`
//! :2209), with the two deviations of plan decision D4: a glyph that needs scaling is
//! re-rasterized at `size * fit` by the caller rather than transform-scaled, and an oversized
//! colour glyph is scaled instead of clipped.
//!
//! All lengths are physical px except the `tolerance` the caller passes to [`fit_scale`]
//! (`0.5 * scale`, Lunegit's `OverflowClipTolerance` of 0.5 logical px, :62).
//!
//! Lunegit's run-length cases do not apply here: it coalesces ASCII into multi-cell runs
//! (`RunSlotWidth` with run lengths 3 and 4, `DecideContainment_*` multi-cell rows), while this
//! renderer places one cluster per cell, so the slot is always `max(width, 1)` cells.

use crate::layout::CellMetrics;
use crate::model::Cell;
use crate::text::ShapedGlyph;

/// Width of the slot a cell owns: `max(width, 1)` cells (`RunSlotWidth` :2116, run length 1).
pub fn slot_width(width: u8, cell_w: f32) -> f32 {
    f32::from(width.max(1)) * cell_w
}

/// How far a glyph may extend from the cell's left edge before containment applies
/// (`ContainmentAllowance` :2127): the slot plus one cell when the glyph is a non-colour,
/// width-1 glyph whose next cell is blank, else the slot.
pub fn allowance(slot: f32, cell_w: f32, is_color: bool, width: u8, next_blank: bool) -> f32 {
    if !is_color && width == 1 && next_blank {
        slot + cell_w
    } else {
        slot
    }
}

/// Blank cluster text (`TerminalRunBuilder.IsBlankGlyph`): empty, a space or NUL.
pub fn is_blank(text: &str) -> bool {
    text.is_empty() || text == " " || text == "\0"
}

/// Whether the cell after `col` exists and is blank (`IsNextCellBlank` :2143, run length 1).
/// False for the last column and past a row shorter than the grid.
pub fn next_cell_blank(row: &[Cell], col: usize) -> bool {
    row.get(col.saturating_add(1))
        .is_some_and(|c| is_blank(c.text()))
}

/// Uniform scale that fits `advance` into `allowance` (`OverflowFitScale` :2161): 1 when it fits
/// within `tolerance` (so a fractional-advance mono font never triggers a pointless rescale),
/// else `allowance / advance`.
pub fn fit_scale(advance: f32, allowance: f32, tolerance: f32) -> f32 {
    if advance > allowance + tolerance && advance > 0.0 {
        allowance / advance
    } else {
        1.0
    }
}

/// How one glyph must be contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    /// Fits: draw as shaped.
    None,
    /// Re-rasterize at `size * fit`.
    ScaleToFit,
}

/// `DecideContainment` :2195 with Lunegit's per-run-kind switch collapsed (D4 ii): colour and
/// mask glyphs alike are scaled.
pub fn decide(fit: f32) -> Containment {
    // DEVIATION (D4): Lunegit clips emoji (DecideContainment_PicksContainmentPerRunKind); we scale
    if fit >= 1.0 {
        Containment::None
    } else {
        Containment::ScaleToFit
    }
}

/// Where one glyph goes. Physical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Fit factor to re-rasterize at (`font size * raster_scale`); 1 when the glyph fits.
    pub raster_scale: f32,
    /// Pen x of the glyph: the cell's left edge, except colour glyphs, which are centred in
    /// their slot (`EmojiCentringOffset` :2053, never shifted left).
    pub origin_x: f32,
    /// Baseline y: the row's baseline, held whatever the scale (`ComputeScaledRunOrigin` :2209).
    pub baseline_y: i32,
    /// `[x0, y0, x1, y1]`: the cell plus its allowance horizontally, the row vertically. The
    /// row clip is always on (ledger 2).
    pub clip: [i32; 4],
}

// LUNETERM: D4 (iii), E2 2026-10-29 — GDEF-less fonts give marks a full advance; HarfBuzz's zero-width-mark fallback, approximated
/// The advance a cluster is fitted on: its base glyph's, not the whole run's. The shipped mono
/// face has no mark class, so every combining mark carries a full cell of advance and the whole
/// cluster would otherwise be shrunk to fit.
pub fn base_advance(glyphs: &[ShapedGlyph]) -> f32 {
    glyphs.first().map_or(0.0, |g| g.advance)
}

// LUNETERM: D4 (iii), E2 2026-10-29 — GDEF-less fonts give marks a full advance; HarfBuzz's zero-width-mark fallback, approximated
/// Where glyph `i` of a cluster starts, logical px from the cluster origin. The base keeps its
/// own `x`. A later glyph sits at the base's pen after the base plus its own offset
/// (`x - pen_x`), so a mark with a negative left bearing overprints the base instead of
/// advancing past it. A font with GPOS marks (zero advance, offset in `x`) is placed as before.
pub fn glyph_x(glyphs: &[ShapedGlyph], i: usize) -> f32 {
    let (Some(base), Some(g)) = (glyphs.first(), glyphs.get(i)) else {
        return 0.0;
    };
    if i == 0 {
        base.x
    } else {
        base.pen_x + base.advance + (g.x - g.pen_x)
    }
}

/// Place one cluster of `advance_px` physical px at `cell_x` on the row starting at `row_top`.
pub fn place(
    cell_x: i32,
    row_top: i32,
    metrics: &CellMetrics,
    width: u8,
    advance_px: f32,
    is_color: bool,
    next_blank: bool,
) -> Placement {
    let cell_w = metrics.cell_w as f32;
    let slot = slot_width(width, cell_w);
    let allow = allowance(slot, cell_w, is_color, width, next_blank);
    let tolerance = 0.5 * metrics.scale;
    let fit = fit_scale(advance_px, allow, tolerance);
    let raster_scale = match decide(fit) {
        Containment::None => 1.0,
        Containment::ScaleToFit => fit,
    };
    let origin_x = if is_color {
        cell_x as f32 + (slot - advance_px * raster_scale).max(0.0) / 2.0
    } else {
        cell_x as f32
    };
    Placement {
        raster_scale,
        origin_x,
        baseline_y: row_top.saturating_add(metrics.baseline),
        clip: [
            cell_x,
            row_top,
            cell_x.saturating_add(allow.ceil() as i32),
            row_top.saturating_add(metrics.cell_h as i32),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> CellMetrics {
        CellMetrics {
            cell_w: 16,
            cell_h: 34,
            char_h: 35,
            char_top: 0,
            baseline: 27,
            font_size: 13.0,
            scale: 1.0,
        }
    }

    #[test]
    fn fit_scale_lunegit_cases() {
        let fit = |w| fit_scale(w, 16.2, 0.5);
        assert_eq!(fit(16.2), 1.0);
        assert_eq!(fit(10.0), 1.0);
        assert_eq!(fit(16.6), 1.0);
        assert!((fit(27.0) - 0.6).abs() < 1e-6);
        assert!((fit_scale(40.0, 32.4, 0.5) - 0.81).abs() < 1e-6);
    }

    #[test]
    fn fit_scale_result_always_fits() {
        let mut w = 17.2_f32;
        while w < 60.0 {
            let f = fit_scale(w, 16.2, 0.5);
            assert!(f < 1.0, "w {w}");
            assert!(f * w <= 16.2 + 1e-4, "w {w}");
            w += 0.1;
        }
    }

    #[test]
    fn allowance_cases() {
        let cw = 8.4014_f32;
        let slot = slot_width(1, cw);
        let a = allowance(slot, cw, false, 1, true);
        assert!((a - 16.8028).abs() < 1e-4);
        assert_eq!(fit_scale(14.0, a, 0.5), 1.0);
        let a = allowance(slot, cw, false, 1, false);
        assert!((fit_scale(14.0, a, 0.5) - 0.6001).abs() < 1e-4);
        // last column: next_cell_blank is false there, so the allowance is the slot
        assert_eq!(allowance(slot, cw, false, 1, false), slot);
        // width 2 or colour: slot, whatever follows
        let slot2 = slot_width(2, cw);
        assert_eq!(allowance(slot2, cw, false, 2, true), slot2);
        assert_eq!(allowance(slot, cw, true, 1, true), slot);
        let a = allowance(slot, cw, false, 1, true);
        assert!((fit_scale(21.0, a, 0.5) - 0.80013).abs() < 1e-4);
    }

    #[test]
    fn slot_width_is_at_least_one_cell() {
        assert_eq!(slot_width(0, 8.0), 8.0);
        assert_eq!(slot_width(1, 8.0), 8.0);
        assert_eq!(slot_width(2, 8.0), 16.0);
    }

    #[test]
    fn blank_text() {
        for t in ["", " ", "\0"] {
            assert!(is_blank(t), "{t:?}");
        }
        for t in ["a", "\u{2733}"] {
            assert!(!is_blank(t), "{t:?}");
        }
    }

    #[test]
    fn next_cell_blank_on_a_row() {
        let mk = |s: &str| {
            let mut c = Cell::default();
            c.set_text(s);
            c
        };
        let row = [mk("\u{2733}"), mk(" "), mk("a"), mk("\u{2733}")];
        assert!(next_cell_blank(&row, 0));
        assert!(!next_cell_blank(&row, 1));
        assert!(!next_cell_blank(&row, 3));
        assert!(!next_cell_blank(&row, 99));
        assert!(!next_cell_blank(&row, usize::MAX));
    }

    // Lunegit `DecideContainment_PicksContainmentPerRunKind` (emoji, len 1, 0.6) -> Clip; we
    // scale colour glyphs too (D4 ii): a width-1 colour-only glyph such as U+23FA would
    // otherwise show only the middle of the glyph.
    #[test]
    fn decide_scales_colour_and_mask_alike() {
        assert_eq!(decide(1.0), Containment::None);
        assert_eq!(decide(0.6), Containment::ScaleToFit);
    }

    // Lunegit run-length cases not applicable to per-cell placement: RunSlotWidth with run
    // length 3 or 4 and multi-cell DecideContainment rows (a coalesced ASCII run is never
    // scaled), ComputeRunOrigin's run origin. One cluster per cell here.

    #[test]
    fn scaled_placement_keeps_left_edge_and_baseline() {
        let m = metrics();
        // 27 px advance in a 16 px slot, next cell not blank: fit = 16 / 27
        let p = place(48, 68, &m, 1, 27.0, false, false);
        assert!((p.raster_scale - 16.0 / 27.0).abs() < 1e-6);
        assert_eq!(p.origin_x, 48.0);
        assert_eq!(p.baseline_y, 68 + 27);
        assert_eq!(p.clip, [48, 68, 64, 68 + 34]);
    }

    #[test]
    fn spill_into_a_blank_neighbour_widens_the_clip() {
        let m = metrics();
        let p = place(0, 0, &m, 1, 20.0, false, true);
        assert_eq!(p.raster_scale, 1.0);
        assert_eq!(p.clip, [0, 0, 32, 34]);
    }

    #[test]
    fn colour_glyph_is_centred_in_its_slot() {
        let m = metrics();
        let p = place(10, 0, &m, 2, 26.0, true, true);
        assert_eq!(p.raster_scale, 1.0);
        assert_eq!(p.origin_x, 13.0);
        assert_eq!(p.clip, [10, 0, 42, 34]);
    }

    fn glyph(pen_x: f32, advance: f32, x: f32) -> ShapedGlyph {
        ShapedGlyph {
            font_id: cosmic_text::fontdb::ID::dummy(),
            glyph_id: 1,
            x,
            y: 0.0,
            pen_x,
            advance,
            weight: cosmic_text::Weight::NORMAL,
            italic: false,
        }
    }

    #[test]
    fn glyph_x_single_glyph_keeps_its_own_x() {
        let g = [glyph(0.0, 7.8, 0.0)];
        assert_eq!(glyph_x(&g, 0), 0.0);
        assert_eq!(base_advance(&g), 7.8);
        assert_eq!(glyph_x(&[], 0), 0.0);
        assert_eq!(glyph_x(&g, 5), 0.0);
        assert_eq!(base_advance(&[]), 0.0);
    }

    #[test]
    fn glyph_x_gpos_cluster_is_unchanged() {
        // base pen 0, advance 7.8; the mark has pen 7.8, advance 0 and GPOS offset -3
        let g = [glyph(0.0, 7.8, 0.0), glyph(7.8, 0.0, 4.8)];
        assert!((glyph_x(&g, 1) - 4.8).abs() < 1e-6);
    }

    #[test]
    fn glyph_x_gdef_less_cluster_overprints_the_base() {
        let g = [
            glyph(0.0, 7.8, 0.0),
            glyph(7.8, 7.8, 7.8),
            glyph(15.6, 7.8, 15.6),
        ];
        assert!((glyph_x(&g, 1) - 7.8).abs() < 1e-6);
        assert!((glyph_x(&g, 2) - 7.8).abs() < 1e-6);
        assert_eq!(base_advance(&g), 7.8);
    }

    #[test]
    fn oversized_colour_glyph_is_scaled_and_centred() {
        let m = metrics();
        let p = place(0, 0, &m, 1, 32.0, true, true);
        assert_eq!(p.raster_scale, 0.5);
        assert_eq!(p.origin_x, 0.0);
    }
}
