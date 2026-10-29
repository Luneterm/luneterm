//! Popups (context menus, dropdowns) and modal dialogs.

use crate::paint::{LayerId, Rect};
use crate::state::{DialogBtn, Hit, MenuCmd, MenuKind, Modal, Popup, State};
use crate::terminal_view::glass;
use crate::text::{Spec, Style};
use crate::theme::*;
use crate::widgets::{self, P, icon, label};

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

pub fn draw_popup(p: &mut P<'_>, st: &State) {
    let Some(popup) = st.popup else { return };
    p.on(LayerId::Popup);
    p.hit(p.window, Hit::Dismiss);
    match popup {
        Popup::Menu { kind, at } => menu(p, st, kind, at),
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

fn menu(p: &mut P<'_>, st: &State, kind: MenuKind, at: (f32, f32)) {
    let entries = entries(kind, st.active);
    let item_h = 30.0;
    let sep_h = 9.0;
    let mut w: f32 = 200.0;
    let mut h = 12.0;
    for e in &entries {
        match e {
            Entry::Item { text, shortcut, .. } => {
                let tw = p.measure(&label(text, FG_PRIMARY)).0;
                let sw = p.measure(&label(shortcut, FG_TERTIARY)).0;
                w = w.max(36.0 + tw + 32.0 + sw + 12.0);
                h += item_h;
            }
            Entry::Separator => h += sep_h,
        }
    }
    let x = at.0.min(p.window.w - w - 8.0).max(8.0);
    let y = if at.1 + h > p.window.h - 8.0 {
        (at.1 - h).max(8.0)
    } else {
        at.1
    };
    let panel = Rect::new(x, y, w, h);
    widgets::popup_panel(p, panel, MENU_BG, MENU_BORDER);
    let mut cy = panel.y + 6.0;
    for e in &entries {
        match e {
            Entry::Item {
                glyph,
                text,
                shortcut,
                cmd,
            } => {
                let r = Rect::new(panel.x + 6.0, cy, panel.w - 12.0, item_h);
                let hit = Hit::Menu(*cmd);
                if p.hovered(hit) {
                    p.fill(r, ITEM_RADIUS, SURFACE_HOVER);
                }
                if let Some(g) = glyph {
                    p.text_centered(
                        &icon(g, 14.0, FG_SECONDARY),
                        Rect::new(r.x + 4.0, r.y, 20.0, r.h),
                    );
                }
                p.text_in(&label(text, FG_PRIMARY), r, 32.0);
                if !shortcut.is_empty() {
                    let spec = label(shortcut, FG_TERTIARY);
                    let sw = p.measure(&spec).0;
                    p.text_in(&spec, Rect::new(r.right() - 10.0 - sw, r.y, sw, r.h), 0.0);
                }
                p.hit(r, hit);
                cy += item_h;
            }
            Entry::Separator => {
                p.fill(
                    Rect::new(panel.x + 10.0, cy + 4.0, panel.w - 20.0, 1.0),
                    0.0,
                    BORDER_SUBTLE,
                );
                cy += sep_h;
            }
        }
    }
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
