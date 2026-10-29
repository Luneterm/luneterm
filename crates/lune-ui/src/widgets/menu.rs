//! Luneium's glass context menu (`ContextMenu.css`, `Popup.css`) and its motion.

use crate::icon;
use crate::paint::{Painter, Rect, Xform};
use crate::text::{Spec, Style};

/// One row of a menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuEntry<'s, H> {
    Item {
        glyph: Option<&'s str>,
        text: &'s str,
        shortcut: &'s str,
        hit: H,
    },
    Separator,
}

/// Luneium's menu motion (`Popup.css`): in 0.18 s ease-out from 4 px lower at 97 %; out 0.12 s
/// ease-in to 2 px lower at 98 %. Both about the top-left corner.
pub const MENU_IN: f32 = 0.18;
pub const MENU_OUT: f32 = 0.12;

/// Menu opacity, vertical offset and scale at one moment of its animation.
pub type MenuAnim = (f32, f32, f32);

/// CSS `cubic-bezier(x1, y1, x2, y2)` at time `t`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let coord = |a: f32, b: f32, u: f32| {
        3.0 * a * u * (1.0 - u).powi(2) + 3.0 * b * u * u * (1.0 - u) + u.powi(3)
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if coord(x1, x2, mid) < t {
            lo = mid
        } else {
            hi = mid
        }
    }
    coord(y1, y2, (lo + hi) / 2.0)
}

/// Opening motion at `t` = elapsed / [`MENU_IN`].
pub fn menu_open_anim(t: f32) -> MenuAnim {
    let e = cubic_bezier(0.215, 0.61, 0.355, 1.0, t);
    (e, 4.0 * (1.0 - e), 0.97 + 0.03 * e)
}

/// Closing motion at `t` = elapsed / [`MENU_OUT`]; `None` once it has finished (`t` ≥ 1).
pub fn menu_close_anim(t: f32) -> Option<MenuAnim> {
    if t >= 1.0 {
        return None;
    }
    let e = cubic_bezier(0.42, 0.0, 1.0, 1.0, t);
    Some((1.0 - e, 2.0 * e, 1.0 - 0.02 * e))
}

/// A glass menu at `at` (kept inside the window), the content under it blurred (30 px,
/// saturate 1.8), black 45 % over grey 165 @ 30 %, radius 9, inset 1 px ring, 6 px padding,
/// 28 px rows with radius 4, 12 px labels, 14 px icon slot, 11 px hints. `interactive` is off
/// while the menu fades out.
pub fn menu<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    entries: &[MenuEntry<'_, H>],
    at: (f32, f32),
    anim: MenuAnim,
    interactive: bool,
) {
    const PAD: f32 = 6.0;
    const ROW: f32 = 28.0;
    const SEP: f32 = 11.0;
    const RADIUS: f32 = 9.0;
    let t = p.theme;
    let label_style = Style::ui(12.0, t.menu_text_primary);
    let hint_style = Style::ui(t.font_s, t.menu_text_quaternary);
    let mut w: f32 = 190.0;
    let mut h = 2.0 * PAD;
    for e in entries {
        match e {
            MenuEntry::Item { text, shortcut, .. } => {
                let tw = p.measure(&Spec::plain(*text, label_style)).0;
                let sw = if shortcut.is_empty() {
                    0.0
                } else {
                    16.0 + p.measure(&Spec::plain(*shortcut, hint_style)).0
                };
                w = w.max(2.0 * PAD + 8.0 + 14.0 + 8.0 + tw + sw + 8.0);
                h += ROW;
            }
            MenuEntry::Separator => h += SEP,
        }
    }
    let x = at.0.min(p.window.w - w - 8.0).max(8.0);
    let y = if at.1 + h > p.window.h - 8.0 {
        (at.1 - h).max(8.0)
    } else {
        at.1
    };
    let panel = Rect::new(x, y, w, h);
    let (alpha, dy, scale) = anim;
    p.set_xform(Some(Xform {
        origin: (panel.x, panel.y),
        scale,
        dy,
        alpha,
    }));
    p.backdrop(panel, RADIUS);
    p.fill(panel, RADIUS, t.menu_glass);
    p.ring(panel, RADIUS, 1.0, t.menu_popup_border, false);
    let mut cy = panel.y + PAD;
    for e in entries {
        match e {
            MenuEntry::Item {
                glyph,
                text,
                shortcut,
                hit,
            } => {
                let r = Rect::new(panel.x + PAD, cy, panel.w - 2.0 * PAD, ROW);
                let hit = *hit;
                if interactive && p.hovered(hit) {
                    p.fill(r, 4.0, t.menu_item_hover);
                }
                let icon_slot = Rect::new(r.x + 8.0, r.y, 14.0, r.h);
                if let Some(g) = glyph {
                    p.text_centered(&icon::glyph(g, 13.0, t.menu_text_tertiary), icon_slot);
                }
                p.text_in(
                    &Spec::plain(*text, label_style),
                    Rect::new(icon_slot.right() + 8.0, r.y, 0.0, r.h),
                    0.0,
                );
                if !shortcut.is_empty() {
                    let spec = Spec::plain(*shortcut, hint_style);
                    let sw = p.measure(&spec).0;
                    p.text_in(&spec, Rect::new(r.right() - 8.0 - sw, r.y, sw, r.h), 0.0);
                }
                if interactive {
                    p.hit(r, hit);
                }
                cy += ROW;
            }
            MenuEntry::Separator => {
                p.fill(
                    Rect::new(
                        panel.x + PAD + 2.0,
                        cy + 5.0,
                        panel.w - 2.0 * PAD - 4.0,
                        1.0,
                    ),
                    0.0,
                    t.menu_separator,
                );
                cy += SEP;
            }
        }
    }
    p.set_xform(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_bezier_endpoints() {
        for (x1, y1, x2, y2) in [(0.215, 0.61, 0.355, 1.0), (0.42, 0.0, 1.0, 1.0)] {
            assert!(cubic_bezier(x1, y1, x2, y2, -1.0).abs() < 1e-5);
            assert!(cubic_bezier(x1, y1, x2, y2, 0.0).abs() < 1e-5);
            assert!((cubic_bezier(x1, y1, x2, y2, 1.0) - 1.0).abs() < 1e-5);
            assert!((cubic_bezier(x1, y1, x2, y2, 2.0) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn open_curve_ends_at_rest() {
        let (alpha, dy, scale) = menu_open_anim(1.0);
        assert!((alpha - 1.0).abs() < 1e-5);
        assert!(dy.abs() < 1e-4);
        assert!((scale - 1.0).abs() < 1e-5);
    }

    #[test]
    fn close_curve_ends_at_none() {
        assert!(menu_close_anim(1.0).is_none());
        assert!(menu_close_anim(3.0).is_none());
        let (alpha, ..) = menu_close_anim(0.0).expect("closing at t=0");
        assert!((alpha - 1.0).abs() < 1e-5);
    }
}
