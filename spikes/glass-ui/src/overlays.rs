//! Popups (context menus, dropdowns) and modal dialogs.

use crate::state::{DialogBtn, Hit, MenuCmd, MenuKind, Modal, Popup, State};
use crate::terminal_view::glass;
use crate::theme::*;
use crate::widgets::{self, P, icon, label};
use lune_ui::paint::{LayerId, Rect};
use lune_ui::text::{Spec, Style};

enum Entry {
    Item {
        glyph: Option<&'static str>,
        text: &'static str,
        shortcut: &'static str,
        cmd: MenuCmd,
    },
    Separator,
}

fn it(
    glyph: Option<&'static str>,
    text: &'static str,
    shortcut: &'static str,
    cmd: MenuCmd,
) -> Entry {
    Entry::Item {
        glyph,
        text,
        shortcut,
        cmd,
    }
}

/// Lunegit's tab context menu (`terminal.tab.menu.*`), plus app items on the "…" menu.
fn entries(kind: MenuKind, active: usize) -> Vec<Entry> {
    let tab = match kind {
        MenuKind::App => active,
        MenuKind::Tab(i) => i,
    };
    let mut v = Vec::new();
    if kind == MenuKind::App {
        v.push(it(Some(icon::ADD), "New Tab", "⌘T", MenuCmd::NewTab));
        v.push(Entry::Separator);
    }
    v.extend([
        it(Some(icon::CHROME_CLOSE), "Close", "⌘W", MenuCmd::Close(tab)),
        it(None, "Close Others", "", MenuCmd::CloseOthers(tab)),
        it(None, "Close All", "", MenuCmd::CloseAll),
        Entry::Separator,
        it(Some(icon::EDIT), "Rename…", "", MenuCmd::Rename(tab)),
        it(
            Some(icon::TERMINAL),
            "Duplicate Tab",
            "",
            MenuCmd::Duplicate(tab),
        ),
        it(
            Some(icon::COPY),
            "Copy Working Directory",
            "",
            MenuCmd::CopyCwd(tab),
        ),
    ]);
    if kind == MenuKind::App {
        v.push(Entry::Separator);
        v.push(it(Some(icon::GEAR), "Settings…", "⌘,", MenuCmd::Settings));
        v.push(it(Some(icon::INFO), "About Luneterm", "", MenuCmd::About));
    }
    v
}

/// Luneium's menu motion (`Popup.css`): in 0.18 s ease-out from 4 px lower at 97 %; out 0.12 s
/// ease-in to 2 px lower at 98 %. Both about the top-left corner.
const MENU_IN: f32 = 0.18;
const MENU_OUT: f32 = 0.12;

/// CSS `cubic-bezier(x1, y1, x2, y2)` at time `t`.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
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

/// Whether a menu is still fading in or out at `now` (the wall clock; scheduling only).
pub fn popup_animating(st: &State, now: std::time::Instant) -> bool {
    let age = |t: std::time::Instant| now.saturating_duration_since(t).as_secs_f32();
    let opening = matches!(st.popup, Some(Popup::Menu { .. })) && age(st.popup_at) < MENU_IN;
    let closing = st.closing.is_some_and(|(_, t)| age(t) < MENU_OUT);
    opening || closing
}

pub fn draw_popup(p: &mut P<'_>, st: &State) {
    p.on(LayerId::Popup);
    if st.popup.is_none()
        && let Some((Popup::Menu { kind, at }, t0)) = st.closing
    {
        let t = st.age(t0) / MENU_OUT;
        if t < 1.0 {
            let e = cubic_bezier(0.42, 0.0, 1.0, 1.0, t);
            let xf = (1.0 - e, 2.0 * e, 1.0 - 0.02 * e);
            menu(p, st, kind, at, xf, false);
        }
    }
    let Some(popup) = st.popup else { return };
    p.hit(p.window, Hit::Dismiss);
    match popup {
        Popup::Menu { kind, at } => {
            let t = st.age(st.popup_at) / MENU_IN;
            let e = cubic_bezier(0.215, 0.61, 0.355, 1.0, t);
            menu(p, st, kind, at, (e, 4.0 * (1.0 - e), 0.97 + 0.03 * e), true);
        }
        Popup::Combo { id, anchor } => {
            let options = st.settings.options(id);
            let selected = st.settings.selected(id);
            let item_h = 32.0;
            let h = options.len() as f32 * item_h + 8.0;
            let below = anchor.bottom() + 4.0;
            let y = if below + h > p.window.h - 8.0 {
                (anchor.y - 4.0 - h).max(8.0)
            } else {
                below
            };
            let panel = Rect::new(anchor.x, y, anchor.w, h);
            widgets::popup_panel(p, panel, DROPDOWN_BG, BORDER);
            for (i, (text, enabled)) in options.iter().enumerate() {
                let r = Rect::new(
                    panel.x + 4.0,
                    panel.y + 4.0 + i as f32 * item_h,
                    panel.w - 8.0,
                    item_h,
                );
                let hit = Hit::ComboItem(id, i);
                if i == selected {
                    p.fill(r, 4.0, SURFACE_SELECTED);
                } else if *enabled && p.hovered(hit) {
                    p.fill(r, 4.0, SURFACE_HOVER);
                }
                let color = if *enabled { FG_PRIMARY } else { FG_TERTIARY };
                p.text_in(&label(text, color), r, 10.0);
                if i == selected {
                    let check = Rect::new(r.right() - 26.0, r.y, 20.0, r.h);
                    p.text_centered(&icon(icon::CHECK, 12.0, FG_SECONDARY), check);
                }
                if *enabled {
                    p.hit(r, hit);
                }
            }
        }
    }
}

/// Luneium's glass context menu (`ContextMenu.css`, `Popup.css`): the content under it blurred
/// (30 px, saturate 1.8), black 45 % over grey 165 @ 30 %, radius 9, inset 1 px ring, 6 px padding,
/// 28 px rows with radius 4, 12 px labels, 14 px icon slot, 11 px hints.
fn menu(
    p: &mut P<'_>,
    st: &State,
    kind: MenuKind,
    at: (f32, f32),
    anim: (f32, f32, f32),
    interactive: bool,
) {
    const PAD: f32 = 6.0;
    const ROW: f32 = 28.0;
    const SEP: f32 = 11.0;
    const RADIUS: f32 = 9.0;
    let entries = entries(kind, st.active);
    let label_style = Style::ui(12.0, LUNEIUM_TEXT_PRIMARY);
    let hint_style = Style::ui(FONT_S, LUNEIUM_TEXT_QUATERNARY);
    let mut w: f32 = 190.0;
    let mut h = 2.0 * PAD;
    for e in &entries {
        match e {
            Entry::Item { text, shortcut, .. } => {
                let tw = p.measure(&Spec::plain(*text, label_style)).0;
                let sw = if shortcut.is_empty() {
                    0.0
                } else {
                    16.0 + p.measure(&Spec::plain(*shortcut, hint_style)).0
                };
                w = w.max(2.0 * PAD + 8.0 + 14.0 + 8.0 + tw + sw + 8.0);
                h += ROW;
            }
            Entry::Separator => h += SEP,
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
    p.set_xform(Some(lune_ui::paint::Xform {
        origin: (panel.x, panel.y),
        scale,
        dy,
        alpha,
    }));
    p.backdrop(panel, RADIUS);
    p.fill(panel, RADIUS, LUNEIUM_MENU_GLASS);
    p.ring(panel, RADIUS, 1.0, LUNEIUM_POPUP_BORDER, false);
    let mut cy = panel.y + PAD;
    for e in &entries {
        match e {
            Entry::Item {
                glyph,
                text,
                shortcut,
                cmd,
            } => {
                let r = Rect::new(panel.x + PAD, cy, panel.w - 2.0 * PAD, ROW);
                let hit = Hit::Menu(*cmd);
                if interactive && p.hovered(hit) {
                    p.fill(r, 4.0, LUNEIUM_ITEM_HOVER);
                }
                let icon_slot = Rect::new(r.x + 8.0, r.y, 14.0, r.h);
                if let Some(g) = glyph {
                    p.text_centered(&icon(g, 13.0, LUNEIUM_TEXT_TERTIARY), icon_slot);
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
            Entry::Separator => {
                p.fill(
                    Rect::new(
                        panel.x + PAD + 2.0,
                        cy + 5.0,
                        panel.w - 2.0 * PAD - 4.0,
                        1.0,
                    ),
                    0.0,
                    LUNEIUM_SEPARATOR,
                );
                cy += SEP;
            }
        }
    }
    p.set_xform(None);
}

/// Lunegit dialog: dim backdrop, 448 px acrylic card, 24 px padding, darker footer block.
pub fn draw_modal(p: &mut P<'_>, st: &State) {
    let Some(modal) = &st.modal else { return };
    p.on(LayerId::Modal);
    let win = p.window;
    p.hit(win, Hit::Block);
    p.fill(win, 0.0, DIALOG_BACKDROP);

    let w = 448.0;
    let inner = w - 48.0;
    let title_style = Style::ui(FONT_M, FG_PRIMARY).medium();
    let body_style = Style::ui(FONT_BASE, FG_SECONDARY);
    let (title, message, confirm) = match modal {
        Modal::Rename { .. } => (
            "Rename Tab",
            "Enter a new name for this terminal tab.",
            "Rename",
        ),
        Modal::ConfirmClose { tabs, .. } if tabs.len() > 1 => (
            "Close Terminal Session",
            "One or more terminal sessions are still running. Closing them will end those sessions.",
            "Close",
        ),
        Modal::ConfirmClose { .. } => (
            "Close Terminal Session",
            "A terminal session is still running. Closing it will end the session.",
            "Close",
        ),
        Modal::About => (
            "Luneterm",
            "Glass UI spike: a native Rust window with mock terminal data. Built with winit, wgpu and glyphon; glass from NSVisualEffectView, colours from Lunegit's acrylic tokens.",
            "OK",
        ),
    };
    let title_spec = Spec::plain(title, title_style);
    let msg_spec = Spec::plain(message, body_style).wrapped(inner);
    let (_, th) = p.measure(&title_spec);
    let (_, mh) = p.measure(&msg_spec);
    let extra = match modal {
        Modal::Rename { .. } => 22.0 + 34.0 + 4.0,
        Modal::ConfirmClose { .. } => 34.0,
        Modal::About => 0.0,
    };
    let body_h = 24.0 + th + 10.0 + mh + extra + 24.0;
    let footer_h = 18.0 + 32.0 + 18.0;
    let card = Rect::new(
        (win.w - w) / 2.0,
        ((win.h - body_h - footer_h) / 2.0).max(20.0),
        w,
        body_h + footer_h,
    );

    let g = glass(st);
    let under = over(premul(DIALOG_BACKDROP), g.panel);
    p.acrylic(card, 8.0, g.dialog, under, g.noise);
    p.ring(card, 8.0, 1.0, BORDER_SUBTLE, false);
    p.hit(card, Hit::Block);

    let mut y = card.y + 24.0;
    p.text(&title_spec, card.x + 24.0, y);
    y += th + 10.0;
    p.text(&msg_spec, card.x + 24.0, y);
    y += mh;
    match modal {
        Modal::Rename { text, .. } => {
            y += 18.0;
            let field_label = Spec::plain("TAB NAME", Style::ui(FONT_S, FG_TERTIARY).medium());
            p.text(&field_label, card.x + 24.0, y);
            y += 18.0;
            let r = Rect::new(card.x + 24.0, y, inner, 32.0);
            widgets::field(p, r, text, "Tab name", true, Hit::Block);
        }
        Modal::ConfirmClose { dont_ask, .. } => {
            y += 12.0;
            widgets::checkbox(
                p,
                Rect::new(card.x + 24.0, y, inner, 22.0),
                *dont_ask,
                "Don't ask again",
                Hit::DontAsk,
            );
        }
        Modal::About => {}
    }

    let footer = Rect::new(card.x, card.bottom() - footer_h, w, footer_h);
    p.fill_radii(footer, [0.0, 0.0, 8.0, 8.0], DIALOG_FOOTER);
    let cw = widgets::button_width(p, confirm);
    let confirm_r = Rect::new(footer.right() - 24.0 - cw, footer.y + 18.0, cw, 32.0);
    widgets::button(p, confirm_r, confirm, true, Hit::Dialog(DialogBtn::Confirm));
    if !matches!(modal, Modal::About) {
        let xw = widgets::button_width(p, "Cancel");
        let cancel_r = Rect::new(confirm_r.x - 8.0 - xw, confirm_r.y, xw, 32.0);
        widgets::button(p, cancel_r, "Cancel", false, Hit::Dialog(DialogBtn::Cancel));
    }
}
