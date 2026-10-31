//! A tiny A8 canvas over zeno: the subset of `CanvasRenderingContext2D` that
//! [`crate::custom_glyphs`] needs. Original code, not a port.
//!
//! Every primitive is rendered into a zeroed scratch buffer and then composited source-over onto
//! the canvas, because zeno overwrites the pixels it touches instead of blending them. Drawing is
//! clipped to the buffer (and to [`GlyphCanvas::clip`] when set). Strokes use the canvas
//! defaults: butt caps, miter joins, miter limit 10. Fills are non-zero.

use zeno::{Cap, Command, Fill, Join, Mask, Point, Stroke, Style};

/// Largest canvas in pixels; anything bigger is treated as empty (a cell is a few hundred
/// pixels at most, this only bounds absurd inputs).
const MAX_PIXELS: usize = 1 << 24;

/// A coverage-only drawing surface of `w` x `h` pixels.
#[derive(Debug, Clone)]
pub struct GlyphCanvas {
    w: u32,
    h: u32,
    buf: Vec<u8>,
    scratch: Vec<u8>,
    path: Vec<Command>,
    /// Stroke width in pixels (`ctx.lineWidth`).
    pub line_width: f64,
    clip: Option<[f64; 4]>,
}

/// Source-over of two coverages, rounded.
pub fn over(dst: u8, src: u8) -> u8 {
    let (d, s) = (u32::from(dst), u32::from(src));
    (s + (d * (255 - s) + 127) / 255).min(255) as u8
}

impl GlyphCanvas {
    /// A zeroed canvas. A zero-sized, or absurdly large, canvas is empty and draws nothing.
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w as usize).saturating_mul(h as usize);
        let (w, h, n) = if n == 0 || n > MAX_PIXELS {
            (0, 0, 0)
        } else {
            (w, h, n)
        };
        GlyphCanvas {
            w,
            h,
            buf: vec![0; n],
            scratch: vec![0; n],
            path: Vec::new(),
            line_width: 1.0,
            clip: None,
        }
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    /// The pixels, row-major, `width * height` bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.buf
    }

    /// The pixels, or `None` when nothing was drawn (all zero).
    pub fn into_image(self) -> Option<Vec<u8>> {
        self.buf.iter().any(|&v| v != 0).then_some(self.buf)
    }

    /// Restricts later drawing to a rectangle (`ctx.clip(rect)`); fractional edges are
    /// anti-aliased. Successive clips intersect.
    pub fn clip(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let new = [x, y, x + w, y + h];
        self.clip = Some(match self.clip {
            Some(c) => [
                c[0].max(new[0]),
                c[1].max(new[1]),
                c[2].min(new[2]),
                c[3].min(new[3]),
            ],
            None => new,
        });
    }

    /// `ctx.fillRect`.
    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (x0, y0, x1, y1) = (x as f32, y as f32, (x + w) as f32, (y + h) as f32);
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return;
        }
        let path = [
            Command::MoveTo(Point::new(x0, y0)),
            Command::LineTo(Point::new(x1, y0)),
            Command::LineTo(Point::new(x1, y1)),
            Command::LineTo(Point::new(x0, y1)),
            Command::Close,
        ];
        self.raster(&path, Style::Fill(Fill::NonZero));
    }

    /// Fills the rectangle with a repeating 0/1 pattern, anchored at the canvas origin like a
    /// `CanvasPattern`: a set cell is full coverage, a clear cell is nothing. Pixels whose
    /// centre lies in the rectangle are filled.
    pub fn fill_pattern(&mut self, pattern: &[&[u8]], x: f64, y: f64, w: f64, h: f64) {
        let ph = pattern.len();
        if ph == 0 || self.w == 0 || ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let (x1, y1) = (x + w, y + h);
        for py in 0..self.h as usize {
            let cy = py as f64 + 0.5;
            if cy < y || cy >= y1 {
                continue;
            }
            let prow = pattern[py % ph];
            if prow.is_empty() {
                continue;
            }
            for px in 0..self.w as usize {
                let cx = px as f64 + 0.5;
                if cx < x || cx >= x1 {
                    continue;
                }
                if prow[px % prow.len()] != 0 {
                    let at = py * self.w as usize + px;
                    self.buf[at] = over(self.buf[at], 255);
                }
            }
        }
    }

    /// `ctx.beginPath`.
    pub fn begin_path(&mut self) {
        self.path.clear();
    }

    /// `ctx.moveTo`. Non-finite coordinates are ignored.
    pub fn move_to(&mut self, x: f64, y: f64) {
        if let Some(p) = point(x, y) {
            self.path.push(Command::MoveTo(p));
        }
    }

    /// `ctx.lineTo`. Non-finite coordinates are ignored.
    pub fn line_to(&mut self, x: f64, y: f64) {
        if let Some(p) = point(x, y) {
            self.path.push(Command::LineTo(p));
        }
    }

    /// `ctx.bezierCurveTo`. Non-finite coordinates are ignored.
    pub fn bezier_curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        if let (Some(a), Some(b), Some(c)) = (point(x1, y1), point(x2, y2), point(x, y)) {
            self.path.push(Command::CurveTo(a, b, c));
        }
    }

    /// `ctx.stroke` with the current path and `line_width`.
    pub fn stroke(&mut self) {
        let mut s = Stroke::new(self.line_width as f32);
        s.join(Join::Miter).miter_limit(10.0).cap(Cap::Butt);
        let path = std::mem::take(&mut self.path);
        self.raster(&path, Style::Stroke(s));
        self.path = path;
    }

    /// `ctx.fill` with the current path (open subpaths are closed implicitly).
    pub fn fill(&mut self) {
        let path = std::mem::take(&mut self.path);
        self.raster(&path, Style::Fill(Fill::NonZero));
        self.path = path;
    }

    fn raster(&mut self, path: &[Command], style: Style<'_>) {
        if self.w == 0 || path.is_empty() {
            return;
        }
        self.scratch.fill(0);
        Mask::new(path)
            .style(style)
            .size(self.w, self.h)
            .render_into(&mut self.scratch, None);
        let w = self.w as usize;
        for (i, (dst, &src)) in self.buf.iter_mut().zip(&self.scratch).enumerate() {
            if src == 0 {
                continue;
            }
            let src = match self.clip {
                None => src,
                Some(c) => {
                    let cov = axis_cover((i % w) as f64, c[0], c[2])
                        * axis_cover((i / w) as f64, c[1], c[3]);
                    (f64::from(src) * cov).round() as u8
                }
            };
            *dst = over(*dst, src);
        }
    }
}

/// Overlap of the pixel `[p, p + 1]` with `[lo, hi]`, 0..=1.
fn axis_cover(p: f64, lo: f64, hi: f64) -> f64 {
    (hi.min(p + 1.0) - lo.max(p)).clamp(0.0, 1.0)
}

fn point(x: f64, y: f64) -> Option<Point> {
    let (x, y) = (x as f32, y as f32);
    (x.is_finite() && y.is_finite()).then(|| Point::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_blends_source_over() {
        assert!((i32::from(over(128, 128)) - 192).abs() <= 1);
        assert_eq!(over(0, 0), 0);
        assert_eq!(over(255, 0), 255);
        assert_eq!(over(10, 255), 255);
    }

    #[test]
    fn empty_and_huge_canvases_draw_nothing() {
        let mut c = GlyphCanvas::new(0, 0);
        c.fill_rect(0.0, 0.0, 5.0, 5.0);
        assert!(c.into_image().is_none());
        let mut c = GlyphCanvas::new(100_000, 100_000);
        c.fill_rect(0.0, 0.0, 5.0, 5.0);
        assert_eq!((c.width(), c.height()), (0, 0));
        assert!(c.into_image().is_none());
    }

    #[test]
    fn fill_rect_is_anti_aliased_and_clipped_to_the_buffer() {
        let mut c = GlyphCanvas::new(4, 4);
        c.fill_rect(-10.0, 0.5, 100.0, 1.0);
        let p = c.pixels();
        // half coverage, either side of 127.5
        assert!(p[0..8].iter().all(|&v| v == 127 || v == 128), "{p:?}");
        assert_eq!(&p[8..16], &[0; 8]);
    }

    #[test]
    fn strokes_composite_instead_of_overwriting() {
        // two crossing strokes: the crossing must not be dimmer than either arm
        let mut c = GlyphCanvas::new(8, 8);
        c.line_width = 2.0;
        c.begin_path();
        c.move_to(0.0, 4.0);
        c.line_to(8.0, 4.0);
        c.stroke();
        c.begin_path();
        c.move_to(4.0, 0.0);
        c.line_to(4.0, 8.0);
        c.stroke();
        let p = c.pixels();
        assert_eq!(p[3 * 8 + 3], 255);
        assert_eq!(p[3 * 8], 255);
        assert_eq!(p[3], 255);
        assert_eq!(p[0], 0);
    }

    #[test]
    fn pattern_fill_sets_whole_pixels_from_the_canvas_origin() {
        let mut c = GlyphCanvas::new(4, 2);
        c.fill_pattern(&[&[1, 0], &[0, 1]], 0.0, 0.0, 4.0, 2.0);
        assert_eq!(c.pixels(), &[255, 0, 255, 0, 0, 255, 0, 255]);
    }

    #[test]
    fn fill_closes_open_subpaths_and_clip_cuts_the_canvas() {
        let mut c = GlyphCanvas::new(8, 8);
        c.begin_path();
        c.move_to(-4.0, 0.0);
        c.line_to(12.0, 4.0);
        c.line_to(-4.0, 8.0);
        c.clip(2.0, 0.0, 4.0, 8.0);
        c.fill();
        let p = c.pixels();
        assert_eq!(p[4 * 8 + 3], 255, "inside the triangle and the clip");
        assert_eq!(p[4 * 8 + 1], 0, "left of the clip");
        assert_eq!(p[4 * 8 + 6], 0, "right of the clip");
    }

    #[test]
    fn non_finite_coordinates_are_ignored() {
        let mut c = GlyphCanvas::new(4, 4);
        c.begin_path();
        c.move_to(f64::NAN, 1.0);
        c.line_to(2.0, f64::INFINITY);
        c.stroke();
        c.fill_rect(f64::NAN, 0.0, 1.0, 1.0);
        assert!(c.into_image().is_none());
    }
}
