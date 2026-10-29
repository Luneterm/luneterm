//! Popups (context menus, dropdowns) and modal dialogs.

use crate::state::{DialogBtn, Hit, MenuCmd, MenuKind, Modal, Popup, State};
use crate::terminal_view::glass;
use crate::theme::*;
use crate::widgets::P;
use lune_ui::paint::{LayerId, Rect};
use lune_ui::text::{Spec, Style};
use lune_ui::widgets::dialog::{DialogGlass, DialogHits, DialogSpec, dialog};
use lune_ui::widgets::dropdown::dropdown;
use lune_ui::widgets::menu::{MENU_IN, MENU_OUT, MenuEntry, menu, menu_close_anim, menu_open_anim};

/// Lunegit's tab context menu (`terminal.tab.menu.*`), plus app items on the "…" menu.
fn entries(kind: MenuKind, active: usize) -> Vec<MenuEntry<'static, Hit>> {
    let tab = match kind {
        MenuKind::App => active,
        MenuKind::Tab(i) => i,
    };
    let mut v = Vec::new();
    if kind == MenuKind::App {
        v.push(it(Some(icon::ADD), "New Tab", "⌘T", MenuCmd::NewTab));
        v.push(MenuEntry::Separator);
    }
    v.extend([
        it(Some(icon::CHROME_CLOSE), "Close", "⌘W", MenuCmd::Close(tab)),
        it(None, "Close Others", "", MenuCmd::CloseOthers(tab)),
        it(None, "Close All", "", MenuCmd::CloseAll),
        MenuEntry::Separator,
        it(Some(icon::EDIT), "Rename…", "", MenuCmd::Rename(tab)),
        it(
            Some(ICON_TERMINAL),
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
        v.push(MenuEntry::Separator);
        v.push(it(Some(icon::GEAR), "Settings…", "⌘,", MenuCmd::Settings));
        v.push(it(Some(icon::INFO), "About Luneterm", "", MenuCmd::About));
    }
    v
}

fn it(
    glyph: Option<&'static str>,
    text: &'static str,
    shortcut: &'static str,
    cmd: MenuCmd,
) -> MenuEntry<'static, Hit> {
    MenuEntry::Item {
        glyph,
        text,
        shortcut,
        hit: Hit::Menu(cmd),
    }
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
        && let Some(xf) = menu_close_anim(st.age(t0) / MENU_OUT)
    {
        menu(p, &entries(kind, st.active), at, xf, false);
    }
    let Some(popup) = st.popup else { return };
    p.hit(p.window, Hit::Dismiss);
    match popup {
        Popup::Menu { kind, at } => {
            let xf = menu_open_anim(st.age(st.popup_at) / MENU_IN);
            menu(p, &entries(kind, st.active), at, xf, true);
        }
        Popup::Combo { id, anchor } => {
            let options = st.settings.options(id);
            let selected = st.settings.selected(id);
            dropdown(p, anchor, &options, selected, |i| Hit::ComboItem(id, i));
        }
    }
}

/// Lunegit dialog: the copy and the per-kind body are the app's; the toolkit draws the card.
pub fn draw_modal(p: &mut P<'_>, st: &State) {
    let Some(modal) = &st.modal else { return };
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
    let extra_h = match modal {
        Modal::Rename { .. } => 22.0 + 34.0 + 4.0,
        Modal::ConfirmClose { .. } => 34.0,
        Modal::About => 0.0,
    };
    let g = glass(st);
    dialog(
        p,
        &DialogSpec {
            title,
            message,
            extra_h,
            confirm,
            cancel: (!matches!(modal, Modal::About)).then_some("Cancel"),
        },
        &DialogGlass {
            panel: g.panel,
            fill: g.dialog,
            noise: g.noise,
        },
        &DialogHits {
            block: Hit::Block,
            confirm: Hit::Dialog(DialogBtn::Confirm),
            cancel: Hit::Dialog(DialogBtn::Cancel),
        },
        |p, x, mut y, inner| match modal {
            Modal::Rename { text, .. } => {
                y += 18.0;
                let field_label = Spec::plain("TAB NAME", Style::ui(FONT_S, FG_TERTIARY).medium());
                p.text(&field_label, x, y);
                y += 18.0;
                let r = Rect::new(x, y, inner, 32.0);
                crate::widgets::field(p, r, text, "Tab name", true, Hit::Block);
            }
            Modal::ConfirmClose { dont_ask, .. } => {
                y += 12.0;
                crate::widgets::checkbox(
                    p,
                    Rect::new(x, y, inner, 22.0),
                    *dont_ask,
                    "Don't ask again",
                    Hit::DontAsk,
                );
            }
            Modal::About => {}
        },
    );
}
