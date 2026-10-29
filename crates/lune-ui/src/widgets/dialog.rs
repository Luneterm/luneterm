//! Lunegit dialog: dim backdrop, 448 px acrylic card, 24 px padding, darker footer block.

use super::{button, button_width};
use crate::color::{over, premul};
use crate::paint::{LayerId, Painter, Rect};
use crate::text::{Spec, Style};

/// What the dialog says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogSpec<'s> {
    pub title: &'s str,
    pub message: &'s str,
    /// Height of the body the app draws between the message and the footer.
    pub extra_h: f32,
    pub confirm: &'s str,
    /// The cancel button's label; `None` leaves only the confirm button.
    pub cancel: Option<&'s str>,
}

/// Acrylic colours for the card, from the app's glass policy (premultiplied).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogGlass {
    /// What the panel layer shows behind the dim backdrop (feeds the card's antialiased edge).
    pub panel: [f32; 4],
    /// The card's own fill.
    pub fill: [f32; 4],
    pub noise: bool,
}

/// Hit regions of a dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogHits<H> {
    /// The backdrop and the card, so a click does not reach what is below.
    pub block: H,
    pub confirm: H,
    pub cancel: H,
}

/// Draws a dialog on the modal layer. `body` draws the app's part between the message and the
/// footer; it gets the painter, the card's content x, the y just below the message and the
/// content width.
pub fn dialog<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    spec: &DialogSpec<'_>,
    glass: &DialogGlass,
    hits: &DialogHits<H>,
    body: impl FnOnce(&mut Painter<'_, H>, f32, f32, f32),
) {
    let t = p.theme;
    p.on(LayerId::Modal);
    let win = p.window;
    p.hit(win, hits.block);
    p.fill(win, 0.0, t.dialog_backdrop);

    let w = 448.0;
    let inner = w - 48.0;
    let title_style = Style::ui(t.font_m, t.fg_primary).medium();
    let body_style = Style::ui(t.font_base, t.fg_secondary);
    let title_spec = Spec::plain(spec.title, title_style);
    let msg_spec = Spec::plain(spec.message, body_style).wrapped(inner);
    let (_, th) = p.measure(&title_spec);
    let (_, mh) = p.measure(&msg_spec);
    let body_h = 24.0 + th + 10.0 + mh + spec.extra_h + 24.0;
    let footer_h = 18.0 + 32.0 + 18.0;
    let card = Rect::new(
        (win.w - w) / 2.0,
        ((win.h - body_h - footer_h) / 2.0).max(20.0),
        w,
        body_h + footer_h,
    );

    let under = over(premul(t.dialog_backdrop), glass.panel);
    p.acrylic(card, 8.0, glass.fill, under, glass.noise);
    p.ring(card, 8.0, 1.0, t.border_subtle, false);
    p.hit(card, hits.block);

    let mut y = card.y + 24.0;
    p.text(&title_spec, card.x + 24.0, y);
    y += th + 10.0;
    p.text(&msg_spec, card.x + 24.0, y);
    y += mh;
    body(p, card.x + 24.0, y, inner);

    let footer = Rect::new(card.x, card.bottom() - footer_h, w, footer_h);
    p.fill_radii(footer, [0.0, 0.0, 8.0, 8.0], t.dialog_footer);
    let cw = button_width(p, spec.confirm);
    let confirm_r = Rect::new(footer.right() - 24.0 - cw, footer.y + 18.0, cw, 32.0);
    button(p, confirm_r, spec.confirm, true, hits.confirm);
    if let Some(cancel) = spec.cancel {
        let xw = button_width(p, cancel);
        let cancel_r = Rect::new(confirm_r.x - 8.0 - xw, confirm_r.y, xw, 32.0);
        button(p, cancel_r, cancel, false, hits.cancel);
    }
}
