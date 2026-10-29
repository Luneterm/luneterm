//! Controls, styled after Lunegit's Fluent theme overrides (`DarkTheme.axaml`).

use crate::paint::{Painter, Rect};
use crate::state::Hit;
use crate::text::{Spec, Style};
use crate::theme::*;

pub type P<'a> = Painter<'a, Hit>;

pub fn icon(glyph: &str, size: f32, color: Rgba) -> Spec {
    Spec::plain(glyph, Style::mono(size, size * 1.25, color))
}

pub fn label(text: &str, color: Rgba) -> Spec {
    Spec::plain(text, Style::ui(FONT_BASE, color))
}

/// Fluent ToggleSwitch: 40×20 track, accent fill when on.
pub fn toggle(p: &mut P<'_>, r: Rect, on: bool, hovered: bool, hit: Hit) {
    let track = Rect::new(r.x, r.y + (r.h - 20.0) / 2.0, 40.0, 20.0);
    let knob = if hovered { 14.0 } else { 12.0 };
    if on {
        p.fill(track, 10.0, ACCENT);
        let k = Rect::new(
            track.right() - 4.0 - knob,
            track.y + (20.0 - knob) / 2.0,
            knob,
            knob,
        );
        p.fill(k, knob / 2.0, [0xFF, 0xFF, 0xFF, 0xFF]);
    } else {
        if hovered {
            p.fill(track, 10.0, SURFACE_HOVER);
        }
        p.ring(track, 10.0, 1.0, FG_SECONDARY, false);
        let k = Rect::new(track.x + 4.0, track.y + (20.0 - knob) / 2.0, knob, knob);
        p.fill(k, knob / 2.0, FG_PRIMARY);
    }
    p.hit(track.inset(-4.0, -6.0), hit);
}

fn input_box(p: &mut P<'_>, r: Rect, hovered: bool, focused: bool) {
    p.fill(
        r,
        ITEM_RADIUS,
        if hovered {
            SURFACE_HOVER
        } else {
            SURFACE_INPUT
        },
    );
    p.ring(
        r,
        ITEM_RADIUS,
        1.0,
        if focused { ACCENT } else { BORDER },
        false,
    );
}

/// ComboBox: current value plus a chevron.
pub fn combo(p: &mut P<'_>, r: Rect, value: &str, hovered: bool, open: bool, hit: Hit) {
    input_box(p, r, hovered, open);
    let text_clip = Rect::new(r.x, r.y, r.w - 30.0, r.h);
    p.set_clip(Some(text_clip));
    p.text_in(&label(value, FG_PRIMARY), r, 10.0);
    p.set_clip(None);
    let chevron = Rect::new(r.right() - 28.0, r.y, 20.0, r.h);
    p.text_centered(&icon(icon::CHEVRON_DOWN, 12.0, FG_SECONDARY), chevron);
    p.hit(r, hit);
}

/// NumericUpDown: value with − / + buttons.
pub fn stepper(p: &mut P<'_>, r: Rect, value: u32, minus: Hit, plus: Hit) {
    input_box(p, r, false, false);
    p.text_in(&label(&value.to_string(), FG_PRIMARY), r, 10.0);
    for (k, (glyph, hit)) in [("−", minus), ("+", plus)].into_iter().enumerate() {
        let b = Rect::new(
            r.right() - 56.0 + k as f32 * 28.0,
            r.y + 4.0,
            24.0,
            r.h - 8.0,
        );
        if p.hovered(hit) {
            p.fill(b, 4.0, SURFACE_HOVER);
        }
        p.text_centered(&label(glyph, FG_SECONDARY), b);
        p.hit(b, hit);
    }
}

/// TextBox with placeholder; draws a caret when focused.
pub fn field(p: &mut P<'_>, r: Rect, text: &str, placeholder: &str, focused: bool, hit: Hit) {
    input_box(p, r, p.hovered(hit) && !focused, focused);
    p.set_clip(Some(r.inset(6.0, 0.0)));
    let (w, h) = if text.is_empty() {
        p.text_in(&label(placeholder, FG_TERTIARY), r, 10.0);
        (0.0, 0.0)
    } else {
        p.text_in(&label(text, FG_PRIMARY), r, 10.0)
    };
    if focused {
        let ch = if h > 0.0 { h } else { 16.0 };
        let caret = Rect::new(r.x + 10.0 + w + 1.0, r.y + (r.h - ch) / 2.0, 1.5, ch);
        p.fill_over(caret, 0.0, premul(FG_PRIMARY));
    }
    p.set_clip(None);
    p.hit(r, hit);
}

pub fn button(p: &mut P<'_>, r: Rect, text: &str, primary: bool, hit: Hit) {
    let hovered = p.hovered(hit);
    let bg = match (primary, hovered) {
        (true, false) => ACCENT,
        (true, true) => [0x86, 0x8D, 0x95, 0xFF],
        (false, false) => SURFACE_HOVER,
        (false, true) => SURFACE_SELECTED,
    };
    p.fill(r, ITEM_RADIUS, bg);
    let fg = if primary {
        [0xFF, 0xFF, 0xFF, 0xF0]
    } else {
        FG_PRIMARY
    };
    p.text_centered(&label(text, fg), r);
    p.hit(r, hit);
}

pub fn button_width(p: &mut P<'_>, text: &str) -> f32 {
    p.measure(&label(text, FG_PRIMARY)).0 + 32.0
}

pub fn checkbox(p: &mut P<'_>, r: Rect, checked: bool, text: &str, hit: Hit) {
    let b = Rect::new(r.x, r.y + (r.h - 16.0) / 2.0, 16.0, 16.0);
    if checked {
        p.fill(b, 3.0, ACCENT);
        p.text_centered(&icon(icon::CHECK, 12.0, [0xFF, 0xFF, 0xFF, 0xFF]), b);
    } else {
        p.ring(b, 3.0, 1.0, FG_SECONDARY, false);
    }
    let (w, _) = p.text_in(
        &label(text, FG_SECONDARY),
        Rect::new(b.right() + 8.0, r.y, r.w, r.h),
        0.0,
    );
    p.hit(Rect::new(r.x, r.y, 24.0 + w, r.h), hit);
}

/// Menu / dropdown surface.
pub fn popup_panel(p: &mut P<'_>, r: Rect, bg: Rgba, border: Rgba) {
    p.fill(r, 8.0, bg);
    p.ring(r, 8.0, 1.0, border, false);
}
