//! Combo-box drop-down list.

use super::{label, popup_panel};
use crate::icon;
use crate::paint::{Painter, Rect};

/// A list of options under (or, near the window's bottom edge, over) `anchor`. `options` pairs
/// each text with whether it can be picked; `hit` names the hit region of option `i`.
pub fn dropdown<H: Copy + PartialEq, S: AsRef<str>>(
    p: &mut Painter<'_, H>,
    anchor: Rect,
    options: &[(S, bool)],
    selected: usize,
    hit: impl Fn(usize) -> H,
) {
    let t = p.theme;
    let item_h = 32.0;
    let h = options.len() as f32 * item_h + 8.0;
    let below = anchor.bottom() + 4.0;
    let y = if below + h > p.window.h - 8.0 {
        (anchor.y - 4.0 - h).max(8.0)
    } else {
        below
    };
    let panel = Rect::new(anchor.x, y, anchor.w, h);
    popup_panel(p, panel, t.dropdown_bg, t.border);
    for (i, (text, enabled)) in options.iter().enumerate() {
        let r = Rect::new(
            panel.x + 4.0,
            panel.y + 4.0 + i as f32 * item_h,
            panel.w - 8.0,
            item_h,
        );
        let hit = hit(i);
        if i == selected {
            p.fill(r, 4.0, t.surface_selected);
        } else if *enabled && p.hovered(hit) {
            p.fill(r, 4.0, t.surface_hover);
        }
        let color = if *enabled {
            t.fg_primary
        } else {
            t.fg_tertiary
        };
        p.text_in(&label(&t, text.as_ref(), color), r, 10.0);
        if i == selected {
            let check = Rect::new(r.right() - 26.0, r.y, 20.0, r.h);
            p.text_centered(&icon::glyph(icon::CHECK, 12.0, t.fg_secondary), check);
        }
        if *enabled {
            p.hit(r, hit);
        }
    }
}
