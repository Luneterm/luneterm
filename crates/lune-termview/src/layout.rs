//! Pixel <-> cell mapping on integer device-pixel cells (plan decision D3).

use lune_ui::paint::Rect;

/// Cell geometry. Physical pixels except `font_size` (logical). Computed from the font in
/// the text core; plain data here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    pub char_h: u32,
    pub char_top: i32,
    pub baseline: i32,
    pub font_size: f32,
    pub scale: f32,
}

/// Where the grid sits in the window and how pixels map to cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    pub metrics: CellMetrics,
    /// Logical origin as given.
    pub origin: (f32, f32),
    /// Origin in physical px: `round(origin * scale)`.
    pub origin_px: (i32, i32),
}

fn sane_scale(s: f32) -> f32 {
    if s.is_finite() && s > 0.0 { s } else { 1.0 }
}

impl GridLayout {
    pub fn new(origin: (f32, f32), m: CellMetrics) -> GridLayout {
        let s = f64::from(sane_scale(m.scale));
        let px = |v: f32| (f64::from(v) * s).round().clamp(-1e9, 1e9) as i32;
        GridLayout {
            metrics: m,
            origin,
            origin_px: if origin.0.is_finite() && origin.1.is_finite() {
                (px(origin.0), px(origin.1))
            } else {
                (0, 0)
            },
        }
    }

    /// Cell rectangle in physical px as [x, y, w, h]; `width_cells` is 1 or 2.
    pub fn cell_rect_px(&self, row: usize, col: usize, width_cells: usize) -> [i32; 4] {
        let (cw, ch) = (
            i64::from(self.metrics.cell_w),
            i64::from(self.metrics.cell_h),
        );
        let x = i64::from(self.origin_px.0) + col as i64 * cw;
        let y = i64::from(self.origin_px.1) + row as i64 * ch;
        let c = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        [c(x), c(y), c(width_cells as i64 * cw), c(ch)]
    }

    /// Cell rectangle in logical units.
    pub fn cell_rect(&self, row: usize, col: usize, width_cells: usize) -> Rect {
        let [x, y, w, h] = self.cell_rect_px(row, col, width_cells);
        let s = sane_scale(self.metrics.scale);
        Rect::new(x as f32 / s, y as f32 / s, w as f32 / s, h as f32 / s)
    }

    /// Physical-pixel offset from the grid origin, or `None` for a non-finite point.
    fn rel_px(&self, p: (f32, f32)) -> Option<(f64, f64)> {
        if !p.0.is_finite() || !p.1.is_finite() {
            return None;
        }
        let s = f64::from(sane_scale(self.metrics.scale));
        Some((
            f64::from(p.0) * s - f64::from(self.origin_px.0),
            f64::from(p.1) * s - f64::from(self.origin_px.1),
        ))
    }

    /// (row, col) under a logical point, `None` outside the `cols` x `rows` grid.
    pub fn cell_at(&self, p: (f32, f32), cols: usize, rows: usize) -> Option<(usize, usize)> {
        let (x, y) = self.rel_px(p)?;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let col = (x / f64::from(self.metrics.cell_w.max(1))).floor();
        let row = (y / f64::from(self.metrics.cell_h.max(1))).floor();
        if col >= cols as f64 || row >= rows as f64 {
            return None;
        }
        Some((row as usize, col as usize))
    }

    /// Like `cell_at` but clamped into the grid; (0, 0) for an empty grid or a non-finite point.
    pub fn cell_at_clamped(&self, p: (f32, f32), cols: usize, rows: usize) -> (usize, usize) {
        let Some((x, y)) = self.rel_px(p) else {
            return (0, 0);
        };
        let pick = |v: f64, size: u32, n: usize| -> usize {
            let i = (v / f64::from(size.max(1))).floor().max(0.0);
            (i.min(n.saturating_sub(1) as f64)) as usize
        };
        (
            pick(y, self.metrics.cell_h, rows),
            pick(x, self.metrics.cell_w, cols),
        )
    }

    /// (cols, rows) that fit a logical `w` x `h`, each at least 1; (1, 1) for non-finite input.
    pub fn grid_size(&self, w: f32, h: f32) -> (usize, usize) {
        if !w.is_finite() || !h.is_finite() {
            return (1, 1);
        }
        let s = f64::from(sane_scale(self.metrics.scale));
        let n = |v: f32, size: u32| -> usize {
            let n = (f64::from(v) * s / f64::from(size.max(1))).floor();
            n.clamp(1.0, 1e9) as usize
        };
        (n(w, self.metrics.cell_w), n(h, self.metrics.cell_h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay() -> GridLayout {
        GridLayout::new(
            (20.0, 76.0),
            CellMetrics {
                cell_w: 16,
                cell_h: 34,
                char_h: 30,
                char_top: 2,
                baseline: 26,
                font_size: 13.0,
                scale: 2.0,
            },
        )
    }

    #[test]
    fn cell_at_boundaries() {
        let l = lay();
        assert_eq!(l.origin_px, (40, 152));
        assert_eq!(l.cell_at((20.0, 76.0), 80, 24), Some((0, 0)));
        assert_eq!(l.cell_at((27.99, 92.99), 80, 24), Some((0, 0)));
        assert_eq!(l.cell_at((28.0, 93.0), 80, 24), Some((1, 1)));
        assert_eq!(l.cell_at((19.9, 76.0), 80, 24), None);
        assert_eq!(l.cell_at((20.0, 75.9), 80, 24), None);
        // One past the last column / row.
        assert_eq!(l.cell_at((20.0 + 40.0, 76.0), 5, 24), None);
        assert_eq!(l.cell_at((20.0, 76.0), 0, 0), None);
    }

    #[test]
    fn cell_at_clamped_edges() {
        let l = lay();
        assert_eq!(l.cell_at_clamped((-5.0, -5.0), 80, 24), (0, 0));
        assert_eq!(l.cell_at_clamped((1e6, 1e6), 80, 24), (23, 79));
        assert_eq!(l.cell_at_clamped((1e6, 1e6), 0, 0), (0, 0));
    }

    #[test]
    fn grid_size_floors() {
        let l = lay();
        assert_eq!(l.grid_size(1000.0, 500.0), (125, 29));
        assert_eq!(l.grid_size(1.0, 1.0), (1, 1));
        assert_eq!(l.grid_size(-50.0, 0.0), (1, 1));
    }

    #[test]
    fn rects() {
        let l = lay();
        assert_eq!(l.cell_rect_px(1, 2, 2), [72, 186, 32, 34]);
        assert_eq!(l.cell_rect(1, 2, 2), Rect::new(36.0, 93.0, 16.0, 17.0));
    }

    #[test]
    fn non_finite_input_never_panics() {
        let l = lay();
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(l.cell_at((v, 1.0), 80, 24), None);
            assert_eq!(l.cell_at((1.0, v), 80, 24), None);
            assert_eq!(l.cell_at_clamped((v, v), 80, 24), (0, 0));
            assert_eq!(l.grid_size(v, 100.0), (1, 1));
            assert_eq!(l.grid_size(100.0, v), (1, 1));
        }
        let m = lay().metrics;
        let g = GridLayout::new((f32::NAN, f32::INFINITY), m);
        assert_eq!(g.origin_px, (0, 0));
        let g = GridLayout::new(
            (0.0, 0.0),
            CellMetrics {
                cell_w: 0,
                scale: f32::NAN,
                ..m
            },
        );
        let _ = (
            g.cell_at((5.0, 5.0), 3, 3),
            g.grid_size(10.0, 10.0),
            g.cell_rect(0, 0, 1),
        );
    }
}
