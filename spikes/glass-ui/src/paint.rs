//! Immediate-mode painter: every frame the UI is described from state into per-layer geometry,
//! text placements and hit regions. Clicks resolve against the previous frame's hit regions.

use crate::gpu::{LayerGeometry, Shape, StrokeVertex};
use crate::text::{Spec, TextCache};
use crate::theme::{Rgba, premul};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.right() && p.1 >= self.y && p.1 < self.bottom()
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        Rect::new(
            x,
            y,
            (self.right().min(o.right()) - x).max(0.0),
            (self.bottom().min(o.bottom()) - y).max(0.0),
        )
    }

    pub fn phys(&self, s: f32) -> [f32; 4] {
        [self.x * s, self.y * s, self.w * s, self.h * s]
    }
}

/// Draw order, back to front. Each layer interleaves shapes and text correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerId {
    Base = 0,
    Page = 1,
    Popup = 2,
    Modal = 3,
}

pub const LAYER_COUNT: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct TextItem {
    pub key: u64,
    pub x: f32,
    pub y: f32,
    pub clip: Rect,
}

#[derive(Debug, Default)]
pub struct Layer {
    pub geometry: LayerGeometry,
    pub texts: Vec<TextItem>,
}

#[derive(Debug, Clone, Copy)]
pub struct StrokeStyle {
    pub color: [f32; 4],
    pub width: f32,
    pub mode: f32,
    pub a: f32,
    pub b: f32,
    pub extra: [f32; 4],
}

impl StrokeStyle {
    pub fn solid(color: Rgba, width: f32) -> Self {
        Self {
            color: premul(color),
            width,
            mode: 0.0,
            a: 0.0,
            b: 0.0,
            extra: [0.0; 4],
        }
    }
}

pub struct Painter<'a, H> {
    pub s: f32,
    pub window: Rect,
    pub layers: Vec<Layer>,
    layer: usize,
    clip: Option<Rect>,
    pub hits: Vec<(Rect, H)>,
    pub text: &'a mut TextCache,
    pub hover: Option<H>,
}

impl<'a, H: Copy + PartialEq> Painter<'a, H> {
    pub fn new(s: f32, window: Rect, text: &'a mut TextCache, hover: Option<H>) -> Self {
        Self {
            s,
            window,
            layers: (0..LAYER_COUNT).map(|_| Layer::default()).collect(),
            layer: 0,
            clip: None,
            hits: Vec::new(),
            text,
            hover,
        }
    }

    pub fn on(&mut self, layer: LayerId) {
        self.layer = layer as usize;
        self.clip = None;
    }

    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }

    fn clip_phys(&self) -> [f32; 4] {
        self.clip.map_or([0.0; 4], |c| c.phys(self.s))
    }

    fn geo(&mut self) -> &mut LayerGeometry {
        &mut self.layers[self.layer].geometry
    }

    fn shape(&self, r: Rect, radii: [f32; 4], color: [f32; 4]) -> Shape {
        Shape {
            rect: r.phys(self.s),
            radii: radii.map(|v| v * self.s),
            color,
            under: [0.0; 4],
            params: [0.0; 4],
            clip: self.clip_phys(),
        }
    }

    /// Fill below this layer's text.
    pub fn fill(&mut self, r: Rect, radius: f32, color: Rgba) {
        self.fill_radii(r, [radius; 4], color);
    }

    pub fn fill_radii(&mut self, r: Rect, radii: [f32; 4], color: Rgba) {
        let sh = self.shape(r, radii, premul(color));
        self.geo().under.push(sh);
    }

    /// Fill above this layer's text (selection, cursor, overlays).
    pub fn fill_over(&mut self, r: Rect, radius: f32, color: [f32; 4]) {
        let sh = self.shape(r, [radius; 4], color);
        self.geo().over.push(sh);
    }

    /// 1px-style outline inside `r`.
    pub fn ring(&mut self, r: Rect, radius: f32, width: f32, color: Rgba, above_text: bool) {
        let mut sh = self.shape(r, [radius; 4], premul(color));
        sh.params = [1.0, width * self.s, 0.0, 0.0];
        if above_text {
            self.geo().over.push(sh);
        } else {
            self.geo().under.push(sh);
        }
    }

    /// Acrylic layer with Skia `Src` semantics: replaces what is below, `under` only feeds the
    /// antialiased edge. Colours premultiplied.
    pub fn acrylic(&mut self, r: Rect, radius: f32, color: [f32; 4], under: [f32; 4], noise: bool) {
        let mut sh = self.shape(r, [radius; 4], color);
        sh.under = under;
        sh.params = [0.0, 0.0, 1.0, if noise { 1.0 } else { 0.0 }];
        self.geo().acrylic.push(sh);
    }

    pub fn text(&mut self, spec: &Spec, x: f32, y: f32) -> (f32, f32) {
        let (key, size) = self.text.prepare(spec);
        let clip = self.clip.unwrap_or(self.window);
        self.layers[self.layer]
            .texts
            .push(TextItem { key, x, y, clip });
        size
    }

    /// Text vertically centred in `r` at its left edge (plus `pad`).
    pub fn text_in(&mut self, spec: &Spec, r: Rect, pad: f32) -> (f32, f32) {
        let (_, (w, h)) = self.text.prepare(spec);
        self.text(spec, r.x + pad, r.y + (r.h - h) / 2.0);
        (w, h)
    }

    pub fn text_centered(&mut self, spec: &Spec, r: Rect) {
        let (_, (w, h)) = self.text.prepare(spec);
        self.text(spec, r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0);
    }

    pub fn measure(&mut self, spec: &Spec) -> (f32, f32) {
        self.text.prepare(spec).1
    }

    pub fn hit(&mut self, r: Rect, h: H) {
        let r = match self.clip {
            Some(c) => r.intersect(&c),
            None => r,
        };
        self.hits.push((r, h));
    }

    pub fn hovered(&self, h: H) -> bool {
        self.hover == Some(h)
    }

    /// Tessellates a polyline (logical px) into antialiased triangles (physical px).
    pub fn stroke(&mut self, pts: &[[f32; 2]], closed: bool, st: StrokeStyle) {
        let s = self.s;
        let clip = self.clip_phys();
        let mut p: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last()
                .is_none_or(|l: &[f32; 2]| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() > 1e-3)
            {
                p.push(q);
            }
        }
        if closed && p.len() > 2 && p[0] == p[p.len() - 1] {
            p.pop();
        }
        let n = p.len();
        if n < 2 {
            return;
        }
        let seg_normal = |a: [f32; 2], b: [f32; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            [-dy / len, dx / len]
        };
        let normals: Vec<([f32; 2], f32)> = (0..n)
            .map(|i| {
                let prev = if i > 0 {
                    Some(p[i - 1])
                } else if closed {
                    Some(p[n - 1])
                } else {
                    None
                };
                let next = if i + 1 < n {
                    Some(p[i + 1])
                } else if closed {
                    Some(p[0])
                } else {
                    None
                };
                match (prev, next) {
                    (Some(a), Some(b)) => {
                        let n0 = seg_normal(a, p[i]);
                        let n1 = seg_normal(p[i], b);
                        let m = [n0[0] + n1[0], n0[1] + n1[1]];
                        let len = (m[0] * m[0] + m[1] * m[1]).sqrt().max(1e-6);
                        let m = [m[0] / len, m[1] / len];
                        let cos = (m[0] * n0[0] + m[1] * n0[1]).max(0.3);
                        (m, 1.0 / cos)
                    }
                    (None, Some(b)) => (seg_normal(p[i], b), 1.0),
                    (Some(a), None) => (seg_normal(a, p[i]), 1.0),
                    (None, None) => ([0.0, 0.0], 1.0),
                }
            })
            .collect();
        let half = st.width * s / 2.0;
        let expand = half + 1.0;
        let mut along = 0.0;
        let segs = if closed { n } else { n - 1 };
        let out = &mut self.layers[self.layer].geometry.strokes;
        for i in 0..segs {
            let j = (i + 1) % n;
            let len = ((p[j][0] - p[i][0]).powi(2) + (p[j][1] - p[i][1]).powi(2)).sqrt();
            let v = |k: usize, side: f32, d: f32| {
                let (nrm, miter) = normals[k];
                StrokeVertex {
                    pos: [
                        p[k][0] * s + nrm[0] * expand * miter * side,
                        p[k][1] * s + nrm[1] * expand * miter * side,
                    ],
                    aa: [expand * side, d],
                    color: st.color,
                    style: [st.mode, half, st.a, st.b],
                    extra: st.extra,
                    clip,
                }
            };
            let (a0, a1) = (v(i, -1.0, along), v(i, 1.0, along));
            let (b0, b1) = (v(j, -1.0, along + len), v(j, 1.0, along + len));
            out.extend_from_slice(&[a0, a1, b0, a1, b1, b0]);
            along += len;
        }
    }
}

pub fn polyline_length(pts: &[[f32; 2]]) -> f32 {
    let dist = |a: [f32; 2], b: [f32; 2]| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    pts.windows(2).map(|w| dist(w[0], w[1])).sum::<f32>()
        + pts
            .first()
            .zip(pts.last())
            .map_or(0.0, |(a, b)| dist(*a, *b))
}
