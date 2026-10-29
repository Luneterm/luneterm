//! Tab chrome loop: one closed outline that wraps the active tab and the terminal body.
//!
//! Port of Lunegit `Controls/Terminal/TerminalChromeGeometry.cs` (`BuildLoopFigure`), flattened to
//! a polyline. Card-local coordinates, logical pixels, y down. Clockwise on screen.

pub const CARD_RADIUS: f32 = 8.0;
pub const SEAM_RADIUS: f32 = 8.0;
pub const FLARE_RADIUS: f32 = 6.0;
pub const STROKE: f32 = 1.0;
const MIN_NOTCH_WIDTH: f32 = 12.0;
const MIN_RADIUS: f32 = 0.01;

#[derive(Debug, Clone, Copy)]
pub struct TabSpan {
    pub left: f32,
    pub right: f32,
}

/// Closed polyline (last point connects back to the first).
pub fn outline(width: f32, height: f32, seam: f32, tab: Option<TabSpan>) -> Vec<[f32; 2]> {
    build_loop(width, height, seam, tab, STROKE / 2.0)
}

struct Path {
    pts: Vec<[f32; 2]>,
}

impl Path {
    fn start(p: [f32; 2]) -> Self {
        Self { pts: vec![p] }
    }

    fn line(&mut self, p: [f32; 2]) {
        self.pts.push(p);
    }

    /// Small arc from the current point to `p`. `clockwise` is visual (screen, y down).
    fn arc(&mut self, p: [f32; 2], r: f32, clockwise: bool) {
        let Some(&p0) = self.pts.last() else { return };
        if r < MIN_RADIUS {
            self.line(p);
            return;
        }
        let (mx, my) = ((p0[0] + p[0]) / 2.0, (p0[1] + p[1]) / 2.0);
        let (dx, dy) = (p[0] - p0[0], p[1] - p0[1]);
        let d = (dx * dx + dy * dy).sqrt();
        if d < 1e-4 {
            return;
        }
        let h = (r * r - d * d / 4.0).max(0.0).sqrt();
        let (nx, ny) = (-dy / d, dx / d);
        for sign in [1.0f32, -1.0] {
            let c = [mx + nx * h * sign, my + ny * h * sign];
            let a0 = (p0[1] - c[1]).atan2(p0[0] - c[0]);
            let a1 = (p[1] - c[1]).atan2(p[0] - c[0]);
            let mut delta = a1 - a0;
            while delta > std::f32::consts::PI {
                delta -= std::f32::consts::TAU;
            }
            while delta < -std::f32::consts::PI {
                delta += std::f32::consts::TAU;
            }
            // y-down: increasing angle is clockwise on screen.
            if (delta > 0.0) == clockwise {
                let steps = ((delta.abs() * r) / 0.75).ceil().max(4.0) as usize;
                for i in 1..=steps {
                    let a = a0 + delta * i as f32 / steps as f32;
                    self.pts.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
                }
                return;
            }
        }
        self.line(p);
    }

    fn card_bottom(&mut self, l: f32, rt: f32, b: f32, r: f32) {
        self.line([rt, b - r]);
        self.arc([rt - r, b], r, true);
        self.line([l + r, b]);
        self.arc([l, b - r], r, true);
    }
}

fn build_loop(w: f32, h: f32, seam: f32, tab: Option<TabSpan>, inset: f32) -> Vec<[f32; 2]> {
    let (l, rt, t, b) = (inset, w - inset, inset, h - inset);
    if rt - l <= 0.0 || b - t <= 0.0 {
        return vec![[l, t]];
    }
    let r = CARD_RADIUS.min((rt - l) / 2.0).min((b - t) / 2.0);
    let s = seam.clamp(t, b);

    if b - t < 2.0 * r + 1.0 || s >= b - r {
        let mut p = Path::start([l, b - r]);
        p.line([l, t + r]);
        p.arc([l + r, t], r, true);
        p.line([rt - r, t]);
        p.arc([rt, t + r], r, true);
        p.card_bottom(l, rt, b, r);
        return p.pts;
    }

    let seam_room = ((b - r) - s).max(0.0);
    let tab = tab.filter(|t| t.right.min(rt) - t.left.max(l) >= MIN_NOTCH_WIDTH);

    let Some(tab) = tab else {
        let sc = SEAM_RADIUS.min(seam_room).min((rt - l) / 2.0);
        let mut p = Path::start([l, s + sc]);
        p.arc([l + sc, s], sc, true);
        p.line([rt - sc, s]);
        p.arc([rt, s + sc], sc, true);
        p.card_bottom(l, rt, b, r);
        return p.pts;
    };

    let tx = tab.left.clamp(l, rt);
    let tr = tab.right.clamp(tx, rt);
    let tab_h = (s - t).max(0.0);
    let flush_l = tx <= l + 1.0;
    let flush_r = tr >= rt - 1.0;
    let half_span = ((if flush_r { rt } else { tr }) - (if flush_l { l } else { tx })) / 2.0;
    let r_tab = CARD_RADIUS.min(half_span.min(tab_h).max(0.0));
    let f_l = if flush_l {
        0.0
    } else {
        FLARE_RADIUS.min(tx - l).clamp(0.0, tab_h)
    };
    let f_r = if flush_r {
        0.0
    } else {
        FLARE_RADIUS.min(rt - tr).clamp(0.0, tab_h)
    };
    let s_l = if flush_l {
        0.0
    } else {
        SEAM_RADIUS.min((tx - f_l) - l).clamp(0.0, seam_room)
    };
    let s_r = if flush_r {
        0.0
    } else {
        SEAM_RADIUS.min(rt - (tr + f_r)).clamp(0.0, seam_room)
    };

    let mut p = Path::start([l, b - r]);
    let top_start = if flush_l {
        p.line([l, t + r]);
        p.arc([l + r, t], r, true);
        l + r
    } else {
        p.line([l, s + s_l]);
        p.arc([l + s_l, s], s_l, true);
        p.line([tx - f_l, s]);
        p.arc([tx, s - f_l], f_l, false);
        p.line([tx, t + r_tab]);
        p.arc([tx + r_tab, t], r_tab, true);
        tx + r_tab
    };
    let top_end = (if flush_r { rt - r } else { tr - r_tab }).max(top_start);
    p.line([top_end, t]);
    if flush_r {
        p.arc([rt, t + r], r, true);
    } else {
        p.arc([tr, t + r_tab], r_tab, true);
        p.line([tr, s - f_r]);
        p.arc([tr + f_r, s], f_r, false);
        p.line([rt - s_r, s]);
        p.arc([rt, s + s_r], s_r, true);
    }
    p.card_bottom(l, rt, b, r);
    p.pts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed_and_inside(pts: &[[f32; 2]], w: f32, h: f32) {
        assert!(pts.len() > 8);
        for p in pts {
            assert!(
                p[0] >= 0.0 && p[0] <= w && p[1] >= 0.0 && p[1] <= h,
                "{p:?}"
            );
        }
    }

    #[test]
    fn notched_loop_reaches_card_top_only_at_the_tab() {
        let pts = outline(
            400.0,
            300.0,
            32.0,
            Some(TabSpan {
                left: 120.0,
                right: 240.0,
            }),
        );
        closed_and_inside(&pts, 400.0, 300.0);
        for p in pts.iter().filter(|p| p[1] < 31.0) {
            assert!(
                p[0] >= 113.0 && p[0] <= 247.0,
                "above seam outside tab: {p:?}"
            );
        }
    }

    #[test]
    fn flush_left_tab_uses_card_corner() {
        let pts = outline(
            400.0,
            300.0,
            32.0,
            Some(TabSpan {
                left: 0.0,
                right: 150.0,
            }),
        );
        closed_and_inside(&pts, 400.0, 300.0);
        assert!(pts.iter().any(|p| p[0] < 1.0 && p[1] < 10.0));
    }

    #[test]
    fn no_tab_loop_stays_below_seam() {
        let pts = outline(400.0, 300.0, 32.0, None);
        assert!(pts.iter().all(|p| p[1] >= 32.0 - 1e-3));
    }
}
