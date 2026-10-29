//! Controls, styled after Lunegit's Fluent theme overrides (`DarkTheme.axaml`). Every widget is
//! generic over the app's hit type and reads its tokens from the painter's [`Theme`].

pub mod dialog;
pub mod dropdown;
pub mod menu;

use crate::color::{Rgba, premul};
use crate::icon;
use crate::paint::{Painter, Rect};
use crate::text::{Spec, Style};
use crate::theme::Theme;

/// UI-font text at the theme's base size.
pub fn label(theme: &Theme, text: &str, color: Rgba) -> Spec {
    Spec::plain(text, Style::ui(theme.font_base, color))
}

/// Fluent ToggleSwitch: 40×20 track, accent fill when on.
pub fn toggle<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    on: bool,
    hovered: bool,
    hit: H,
) {
    let t = p.theme;
    let track = Rect::new(r.x, r.y + (r.h - 20.0) / 2.0, 40.0, 20.0);
    let knob = if hovered { 14.0 } else { 12.0 };
    if on {
        p.fill(track, 10.0, t.accent);
        let k = Rect::new(
            track.right() - 4.0 - knob,
            track.y + (20.0 - knob) / 2.0,
            knob,
            knob,
        );
        p.fill(k, knob / 2.0, t.on_accent);
    } else {
        if hovered {
            p.fill(track, 10.0, t.surface_hover);
        }
        p.ring(track, 10.0, 1.0, t.fg_secondary, false);
        let k = Rect::new(track.x + 4.0, track.y + (20.0 - knob) / 2.0, knob, knob);
        p.fill(k, knob / 2.0, t.fg_primary);
    }
    p.hit(track.inset(-4.0, -6.0), hit);
}

fn input_box<H: Copy + PartialEq>(p: &mut Painter<'_, H>, r: Rect, hovered: bool, focused: bool) {
    let t = p.theme;
    p.fill(
        r,
        t.item_radius,
        if hovered {
            t.surface_hover
        } else {
            t.surface_input
        },
    );
    p.ring(
        r,
        t.item_radius,
        1.0,
        if focused { t.accent } else { t.border },
        false,
    );
}

/// ComboBox: current value plus a chevron.
pub fn combo<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    value: &str,
    hovered: bool,
    open: bool,
    hit: H,
) {
    let t = p.theme;
    input_box(p, r, hovered, open);
    let text_clip = Rect::new(r.x, r.y, r.w - 30.0, r.h);
    p.set_clip(Some(text_clip));
    p.text_in(&label(&t, value, t.fg_primary), r, 10.0);
    p.set_clip(None);
    let chevron = Rect::new(r.right() - 28.0, r.y, 20.0, r.h);
    p.text_centered(
        &icon::glyph(icon::CHEVRON_DOWN, 12.0, t.fg_secondary),
        chevron,
    );
    p.hit(r, hit);
}

/// NumericUpDown: value with − / + buttons.
pub fn stepper<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    value: u32,
    minus: H,
    plus: H,
) {
    let t = p.theme;
    input_box(p, r, false, false);
    p.text_in(&label(&t, &value.to_string(), t.fg_primary), r, 10.0);
    for (k, (glyph, hit)) in [("−", minus), ("+", plus)].into_iter().enumerate() {
        let b = Rect::new(
            r.right() - 56.0 + k as f32 * 28.0,
            r.y + 4.0,
            24.0,
            r.h - 8.0,
        );
        if p.hovered(hit) {
            p.fill(b, 4.0, t.surface_hover);
        }
        p.text_centered(&label(&t, glyph, t.fg_secondary), b);
        p.hit(b, hit);
    }
}

/// TextBox with placeholder; draws a caret when focused.
pub fn field<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    text: &str,
    placeholder: &str,
    focused: bool,
    hit: H,
) {
    let t = p.theme;
    input_box(p, r, p.hovered(hit) && !focused, focused);
    p.set_clip(Some(r.inset(6.0, 0.0)));
    let (w, h) = if text.is_empty() {
        p.text_in(&label(&t, placeholder, t.fg_tertiary), r, 10.0);
        (0.0, 0.0)
    } else {
        p.text_in(&label(&t, text, t.fg_primary), r, 10.0)
    };
    if focused {
        let ch = if h > 0.0 { h } else { 16.0 };
        let caret = Rect::new(r.x + 10.0 + w + 1.0, r.y + (r.h - ch) / 2.0, 1.5, ch);
        p.fill_over(caret, 0.0, premul(t.fg_primary));
    }
    p.set_clip(None);
    p.hit(r, hit);
}

pub fn button<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    text: &str,
    primary: bool,
    hit: H,
) {
    let t = p.theme;
    let hovered = p.hovered(hit);
    let bg = match (primary, hovered) {
        (true, false) => t.accent,
        (true, true) => t.accent_hover,
        (false, false) => t.surface_hover,
        (false, true) => t.surface_selected,
    };
    p.fill(r, t.item_radius, bg);
    let fg = if primary {
        t.on_accent_text
    } else {
        t.fg_primary
    };
    p.text_centered(&label(&t, text, fg), r);
    p.hit(r, hit);
}

pub fn button_width<H: Copy + PartialEq>(p: &mut Painter<'_, H>, text: &str) -> f32 {
    let t = p.theme;
    p.measure(&label(&t, text, t.fg_primary)).0 + 32.0
}

pub fn checkbox<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    r: Rect,
    checked: bool,
    text: &str,
    hit: H,
) {
    let t = p.theme;
    let b = Rect::new(r.x, r.y + (r.h - 16.0) / 2.0, 16.0, 16.0);
    if checked {
        p.fill(b, 3.0, t.accent);
        p.text_centered(&icon::glyph(icon::CHECK, 12.0, t.on_accent), b);
    } else {
        p.ring(b, 3.0, 1.0, t.fg_secondary, false);
    }
    let (w, _) = p.text_in(
        &label(&t, text, t.fg_secondary),
        Rect::new(b.right() + 8.0, r.y, r.w, r.h),
        0.0,
    );
    p.hit(Rect::new(r.x, r.y, 24.0 + w, r.h), hit);
}

/// Menu / dropdown surface.
pub fn popup_panel<H: Copy + PartialEq>(p: &mut Painter<'_, H>, r: Rect, bg: Rgba, border: Rgba) {
    p.fill(r, 8.0, bg);
    p.ring(r, 8.0, 1.0, border, false);
}
