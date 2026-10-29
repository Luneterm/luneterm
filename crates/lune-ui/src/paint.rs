//! Immediate-mode painter: every frame the UI is described from state into per-layer geometry,
//! text placements and hit regions. Clicks resolve against the previous frame's hit regions.

use crate::color::{Rgba, premul};
use crate::custom::CustomItem;
use crate::gpu::{LayerGeometry, Shape, StrokeVertex};
use crate::text::{Spec, TextCache};
use crate::theme::Theme;

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
    /// Glyph scale relative to the shaped size (popup open/close animation).
    pub scale: f32,
}

#[derive(Debug, Default)]
pub struct Layer {
    pub geometry: LayerGeometry,
    pub texts: Vec<TextItem>,
    /// App-owned draws, in record order; drawn after the under shapes, before the text.
    pub customs: Vec<CustomItem>,
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

/// Animation transform for a popup: scale about `origin`, then move down `dy`, and fade.
#[derive(Debug, Clone, Copy)]
pub struct Xform {
    pub origin: (f32, f32),
    pub scale: f32,
    pub dy: f32,
    pub alpha: f32,
}

impl Xform {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.origin.0 + (x - self.origin.0) * self.scale,
            self.origin.1 + (y - self.origin.1) * self.scale + self.dy,
        )
    }

    fn rect(&self, r: Rect) -> Rect {
        let (x, y) = self.point(r.x, r.y);
        Rect::new(x, y, r.w * self.scale, r.h * self.scale)
    }
}

#[derive(Debug)]
pub struct Painter<'a, H> {
    pub s: f32,
    pub window: Rect,
    pub layers: Vec<Layer>,
    layer: usize,
    clip: Option<Rect>,
    pub hits: Vec<(Rect, H)>,
    pub text: &'a mut TextCache,
    pub hover: Option<H>,
    /// Tokens the toolkit widgets read.
    pub theme: Theme,
    xform: Option<Xform>,
    /// Blurred-backdrop region requested by a glass menu (physical px).
    pub backdrop: Option<crate::blur::Backdrop>,
}

impl<'a, H: Copy + PartialEq> Painter<'a, H> {
    pub fn new(
        s: f32,
        window: Rect,
        text: &'a mut TextCache,
        hover: Option<H>,
        theme: Theme,
    ) -> Self {
        Self {
            s,
            window,
            layers: (0..LAYER_COUNT).map(|_| Layer::default()).collect(),
            layer: 0,
            clip: None,
            hits: Vec::new(),
            text,
            hover,
            theme,
            xform: None,
            backdrop: None,
        }
    }

    pub fn on(&mut self, layer: LayerId) {
        self.layer = layer as usize;
        self.clip = None;
    }

    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }

    pub fn set_xform(&mut self, xform: Option<Xform>) {
        self.xform = xform;
    }

    fn tr(&self, r: Rect) -> Rect {
        self.xform.map_or(r, |x| x.rect(r))
    }

    fn fade(&self, c: [f32; 4]) -> [f32; 4] {
        let a = self.xform.map_or(1.0, |x| x.alpha);
        c.map(|v| v * a)
    }

    fn clip_phys(&self) -> [f32; 4] {
        self.clip.map_or([0.0; 4], |c| self.tr(c).phys(self.s))
    }

    /// Ask for the blurred scene inside `r` (drawn under this layer).
    pub fn backdrop(&mut self, r: Rect, radius: f32) {
        let k = self.xform.map_or(1.0, |x| x.scale);
        self.backdrop = Some(crate::blur::Backdrop {
            rect: self.tr(r).phys(self.s),
            radius: radius * k * self.s,
            alpha: self.xform.map_or(1.0, |x| x.alpha),
        });
    }

    fn geo(&mut self) -> &mut LayerGeometry {
        &mut self.layers[self.layer].geometry
    }

    fn shape(&self, r: Rect, radii: [f32; 4], color: [f32; 4]) -> Shape {
        let k = self.xform.map_or(1.0, |x| x.scale);
        Shape {
            rect: self.tr(r).phys(self.s),
            radii: radii.map(|v| v * self.s * k),
            color: self.fade(color),
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

    /// Records an app-owned draw ([`CustomDraw`](crate::custom::CustomDraw) number `slot`) on the
    /// current layer, clipped to `r`, the current clip and the window. Nothing is recorded when
    /// that is empty. The clip is in physical px, left and top floored, right and bottom
    /// ceiled. The popup transform is ignored: custom items are not animated.
    pub fn custom(&mut self, slot: usize, r: Rect) {
        let mut area = r.intersect(&self.window);
        if let Some(c) = self.clip {
            area = area.intersect(&c);
        }
        if area.w <= 0.0 || area.h <= 0.0 {
            return;
        }
        let [x, y, w, h] = area.phys(self.s);
        let (x0, y0) = (x.floor().max(0.0), y.floor().max(0.0));
        let (x1, y1) = ((x + w).ceil().max(0.0), (y + h).ceil().max(0.0));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.layers[self.layer].customs.push(CustomItem {
            slot,
            clip: [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32],
        });
    }

    pub fn text(&mut self, spec: &Spec, x: f32, y: f32) -> (f32, f32) {
        let size = self.text.prepare(spec).1;
        let (key, scale) = match self.xform {
            Some(xf) if xf.alpha < 0.999 => {
                let mut faded = spec.clone();
                for (_, style) in &mut faded.runs {
                    style.color[3] = (f32::from(style.color[3]) * xf.alpha).round() as u8;
                }
                (self.text.prepare(&faded).0, xf.scale)
            }
            Some(xf) => (self.text.prepare(spec).0, xf.scale),
            None => (self.text.prepare(spec).0, 1.0),
        };
        let clip = self.tr(self.clip.unwrap_or(self.window));
        let (x, y) = self.xform.map_or((x, y), |xf| xf.point(x, y));
        self.layers[self.layer].texts.push(TextItem {
            key,
            x,
            y,
            clip,
            scale,
        });
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
        let r = self.tr(r);
        self.hits.push((r, h));
    }

    pub fn hovered(&self, h: H) -> bool {
        self.hover == Some(h)
    }

    /// Tessellates a polyline (logical px) into antialiased triangles (physical px).
    pub fn stroke(&mut self, pts: &[[f32; 2]], closed: bool, st: StrokeStyle) {
        let s = self.s;
        let clip = self.clip_phys();
        let (pts, st) = match self.xform {
            Some(xf) => (
                pts.iter()
                    .map(|q| {
                        let (x, y) = xf.point(q[0], q[1]);
                        [x, y]
                    })
                    .collect::<Vec<_>>(),
                StrokeStyle {
                    color: st.color.map(|v| v * xf.alpha),
                    width: st.width * xf.scale,
                    ..st
                },
            ),
            None => (pts.to_vec(), st),
        };
        let pts = pts.as_slice();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::lunegit_dark::THEME;

    fn with_painter(f: impl FnOnce(&mut Painter<'_, u8>)) {
        let mut text = TextCache::new(2.0, &[], "sans-serif", "monospace");
        let mut p = Painter::new(
            2.0,
            Rect::new(0.0, 0.0, 200.0, 100.0),
            &mut text,
            None,
            THEME,
        );
        f(&mut p);
    }

    #[test]
    fn custom_records_physical_clip_on_base() {
        with_painter(|p| {
            p.custom(3, Rect::new(10.0, 20.0, 30.25, 40.0));
            assert_eq!(
                p.layers[0].customs,
                [CustomItem {
                    slot: 3,
                    clip: [20, 40, 61, 80]
                }]
            );
        });
    }

    #[test]
    fn custom_is_cut_by_the_current_clip() {
        with_painter(|p| {
            p.set_clip(Some(Rect::new(0.0, 0.0, 15.0, 100.0)));
            p.custom(3, Rect::new(10.0, 20.0, 30.25, 40.0));
            assert_eq!(
                p.layers[0].customs,
                [CustomItem {
                    slot: 3,
                    clip: [20, 40, 10, 80]
                }]
            );
        });
    }

    #[test]
    fn custom_outside_the_window_records_nothing() {
        with_painter(|p| {
            p.custom(0, Rect::new(300.0, 20.0, 30.0, 40.0));
            p.custom(0, Rect::new(-50.0, 20.0, 30.0, 40.0));
            p.custom(0, Rect::new(10.0, 20.0, 0.0, 40.0));
            assert!(p.layers.iter().all(|l| l.customs.is_empty()));
        });
    }

    #[test]
    fn custom_lands_on_the_current_layer() {
        with_painter(|p| {
            p.on(LayerId::Popup);
            p.custom(1, Rect::new(10.0, 20.0, 30.0, 40.0));
            assert!(p.layers[0].customs.is_empty());
            assert_eq!(p.layers[2].customs.len(), 1);
            assert_eq!(p.layers[2].customs[0].slot, 1);
        });
    }
}
