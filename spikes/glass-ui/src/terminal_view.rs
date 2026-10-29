//! Terminal panel: glass card, tab strip, mock terminal body, chrome loop and running animation.

use std::f32::consts::PI;

use luneterm_glass::lunegit_dark;

use crate::chrome;
use crate::paint::{LayerId, Rect, StrokeStyle, polyline_length};
use crate::state::{Hit, OutlineAnim, State};
use crate::text::{Spec, Style};
use crate::theme::*;
use crate::widgets::{P, icon};

pub const TITLE_BAND: f32 = 34.0;
pub const CARD_MARGIN: f32 = 10.0;
pub const HEADER: f32 = 32.0;
const BODY_PAD: f32 = 10.0;
const TAB_MIN_W: f32 = 108.0;
const TAB_MAX_W: f32 = 270.0;
const TAB_SPACING: f32 = 5.0;
const SWEEP_PERIOD: f32 = 2.4;
const BREATH_PERIOD: f32 = 1.8;
pub const SWITCH_FADE: f32 = 0.25;

/// Premultiplied acrylic colours for this frame (glass style, focus, Linux fallback).
#[derive(Debug, Clone, Copy)]
pub struct Glass {
    pub panel: [f32; 4],
    pub card: [f32; 4],
    pub dialog: [f32; 4],
    pub noise: bool,
}

pub fn glass(st: &State) -> Glass {
    use crate::state::GlassStyle;
    let s = &st.settings;
    if s.glass != GlassStyle::Off && !s.keep_glass_inactive && !st.focused {
        let solid = premul(INACTIVE_SOLID);
        return Glass {
            panel: solid,
            card: solid,
            dialog: solid,
            noise: false,
        };
    }
    let mut g = Glass {
        panel: lunegit_dark::PANEL.premultiplied(),
        card: lunegit_dark::CARD.premultiplied(),
        dialog: lunegit_dark::WINDOW.premultiplied(),
        noise: true,
    };
    if s.glass == GlassStyle::Off {
        let base = premul(OPAQUE_BASE);
        g.panel = over(g.panel, base);
        g.card = over(g.card, base);
        g.dialog = over(g.dialog, base);
    }
    g
}

pub fn card_rect(win: Rect) -> Rect {
    Rect::new(
        CARD_MARGIN,
        TITLE_BAND,
        win.w - 2.0 * CARD_MARGIN,
        win.h - TITLE_BAND - CARD_MARGIN,
    )
}

fn sine_ease_in_out(t: f32) -> f32 {
    0.5 - 0.5 * (PI * t.clamp(0.0, 1.0)).cos()
}

pub fn draw(p: &mut P<'_>, st: &mut State) {
    p.on(LayerId::Base);
    let win = p.window;
    let g = glass(st);
    let card = card_rect(win);
    let t = st.started.elapsed().as_secs_f32();

    // Glass layers: panel everywhere, card on top (each replaces what is below).
    p.acrylic(win, 0.0, g.panel, [0.0; 4], g.noise);
    p.acrylic(card, chrome::CARD_RADIUS, g.card, g.panel, g.noise);
    p.hit(Rect::new(0.0, 0.0, win.w, TITLE_BAND), Hit::TitleBand);

    // Tab strip.
    let title_style = |active: bool| Style::ui(FONT_S, if active { FG_PRIMARY } else { FG_MUTED });
    let mut x = card.x;
    let mut tab_rects = Vec::with_capacity(st.tabs.len());
    for (i, tab) in st.tabs.iter().enumerate() {
        let active = i == st.active;
        let title = Spec::plain(tab.title.clone(), title_style(active));
        let spinner = if tab.running { 14.0 } else { 0.0 };
        let tw = p.measure(&title).0;
        let w = (10.0 + spinner + tw + 4.0 + 20.0 + 8.0).clamp(TAB_MIN_W, TAB_MAX_W);
        let r = Rect::new(x, card.y, w, HEADER);
        let close = Rect::new(r.right() - 28.0, r.y + 6.0, 20.0, 20.0);
        let hovered = p.hovered(Hit::Tab(i)) || p.hovered(Hit::TabClose(i));
        if hovered && !active {
            p.fill_radii(r, [8.0, 8.0, 0.0, 0.0], SURFACE_HOVER);
        }
        let text_x = r.x + 10.0 + spinner;
        p.set_clip(Some(Rect::new(
            text_x,
            r.y,
            (close.x - 4.0 - text_x).max(0.0),
            HEADER,
        )));
        p.text_in(&title, Rect::new(text_x, r.y, 0.0, HEADER), 0.0);
        p.set_clip(None);
        if active || hovered {
            if p.hovered(Hit::TabClose(i)) {
                p.fill(close, 4.0, SURFACE_HOVER);
            }
            p.text_centered(&icon(icon::CHROME_CLOSE, 10.0, FG_TERTIARY), close);
        }
        if tab.running {
            let c = [r.x + 15.0, r.y + HEADER / 2.0];
            let start = t * 2.0 * PI;
            let arc: Vec<[f32; 2]> = (0..=24)
                .map(|k| {
                    let a = start + k as f32 / 24.0 * 1.5 * PI;
                    [c[0] + 4.0 * a.cos(), c[1] + 4.0 * a.sin()]
                })
                .collect();
            p.stroke(&arc, false, StrokeStyle::solid(FG_MUTED, 1.2));
        }
        p.hit(r, Hit::Tab(i));
        if active || hovered {
            p.hit(close, Hit::TabClose(i));
        }
        tab_rects.push(r);
        x += w + TAB_SPACING;
    }
    let new_tab = Rect::new(x, card.y + 4.0, 24.0, 24.0);
    let more = Rect::new(card.right() - 32.0, card.y + 4.0, 24.0, 24.0);
    for (r, glyph, hit) in [
        (new_tab, icon::ADD, Hit::NewTab),
        (more, icon::ELLIPSIS, Hit::More),
    ] {
        if p.hovered(hit) {
            p.fill(r, 4.0, SURFACE_HOVER);
        }
        p.text_centered(&icon(glyph, 14.0, FG_MUTED), r);
        p.hit(r, hit);
    }

    // Body text (fades in after a tab switch, like Lunegit's 0.25 s crossfade).
    let s = &st.settings;
    let palette = &PALETTES[s.palette];
    let size = s.font_size as f32;
    let line = s.line_height();
    let fade = if s.motion == crate::state::Motion::None {
        1.0
    } else {
        sine_ease_in_out(st.switched_at.elapsed().as_secs_f32() / SWITCH_FADE)
    };
    let alpha = (fade * 255.0).round() as u8;
    let body = Rect::new(
        card.x + BODY_PAD,
        card.y + HEADER + BODY_PAD,
        card.w - 2.0 * BODY_PAD,
        card.h - HEADER - 2.0 * BODY_PAD,
    );
    let cell_w =
        p.measure(&Spec::plain(
            "0123456789",
            Style::mono(size, line, FG_PRIMARY),
        ))
        .0 / 10.0;
    st.term_body = body;
    st.cell = (cell_w, line);

    let tab = &st.tabs[st.active];
    let mut runs = Vec::new();
    for (i, l) in tab.screen.lines.iter().enumerate() {
        if i > 0 {
            runs.push(("\n".to_owned(), Style::mono(size, line, [0, 0, 0, 0])));
        }
        for span in &l.spans {
            let [r, g, b] = palette.rgb(span.fg);
            let mut style = Style::mono(size, line, [r, g, b, alpha]);
            if span.bold {
                style = style.bold();
            }
            runs.push((span.text.clone(), style));
        }
    }
    p.set_clip(Some(body));
    p.text(&Spec { runs, wrap: None }, body.x, body.y);
    p.hit(body, Hit::Body);

    // Overlays: selection, cursor, scrollbar, link underline.
    let cols = (body.w / cell_w).floor() as usize;
    let rows = (body.h / line).floor() as usize;
    let cell_rect = |row: usize, c0: usize, c1: usize| {
        Rect::new(
            body.x + c0 as f32 * cell_w,
            body.y + row as f32 * line,
            c1.saturating_sub(c0) as f32 * cell_w,
            line,
        )
    };
    if let Some((a, b)) = tab.selection {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        for row in start.0..=end.0.min(rows.saturating_sub(1)) {
            let c0 = if row == start.0 { start.1 } else { 0 };
            let c1 = if row == end.0 { end.1 + 1 } else { cols };
            p.fill_over(cell_rect(row, c0, c1.min(cols)), 0.0, premul(SELECTION));
        }
    }
    let (crow, ccol) = tab.screen.cursor;
    if crow < rows {
        let r = cell_rect(crow, ccol, ccol + 1);
        if st.focused {
            p.fill_over(r, 0.0, premul(CURSOR));
        } else {
            p.ring(r, 0.0, 1.0, CURSOR, true);
        }
    }
    p.set_clip(None);
    let thumb_h = (body.h * 0.35).max(24.0);
    let thumb = Rect::new(card.right() - 7.0, body.bottom() - thumb_h, 4.0, thumb_h);
    p.fill_over(thumb, 2.0, premul(with_alpha(ACCENT, 0.55)));
    if st.cmd && link_at(st).is_some() {
        for l in &tab.screen.links {
            let r = cell_rect(l.row, l.col_start, l.col_end);
            p.fill_over(
                Rect::new(r.x, r.y + line - 2.0, r.w, 2.0),
                0.0,
                premul(ACCENT),
            );
        }
    }

    // Chrome loop wrapping the active tab, then the running animation on top.
    let span = tab_rects.get(st.active).map(|r| chrome::TabSpan {
        left: r.x - card.x,
        right: r.right() - card.x,
    });
    let pts: Vec<[f32; 2]> = chrome::outline(card.w, card.h, HEADER, span)
        .iter()
        .map(|q| [q[0] + card.x, q[1] + card.y])
        .collect();
    let border = if st.focused { BORDER } else { BORDER_SUBTLE };
    p.stroke(&pts, true, StrokeStyle::solid(border, chrome::STROKE));
    let running_anim = if tab.running {
        s.effective_outline()
    } else {
        None
    };
    let accent = premul(ACCENT);
    match running_anim {
        Some(OutlineAnim::Sweep) => {
            let phase = (t / SWEEP_PERIOD).fract();
            let center = [
                (card.x + card.w / 2.0) * p.s,
                (card.y + card.h / 2.0) * p.s,
                0.0,
                0.0,
            ];
            for (width, opacity) in [(3.0, 0.25), (chrome::STROKE, 1.0)] {
                let style = StrokeStyle {
                    color: accent.map(|v| v * opacity),
                    width,
                    mode: 1.0,
                    a: phase,
                    b: 0.0,
                    extra: center,
                };
                p.stroke(&pts, true, style);
            }
        }
        Some(OutlineAnim::Comet) => {
            let contour = polyline_length(&pts);
            let phase = (t / SWEEP_PERIOD).fract();
            let style = StrokeStyle {
                color: accent,
                width: chrome::STROKE,
                mode: 2.0,
                a: contour * 0.14,
                b: phase * contour,
                extra: [0.0, 0.0, contour, 0.0],
            };
            p.stroke(&pts, true, style);
        }
        Some(OutlineAnim::Breath) => {
            let ph = (t / BREATH_PERIOD).fract();
            let tri = if ph < 0.5 { ph * 2.0 } else { (1.0 - ph) * 2.0 };
            let opacity = 0.38 + sine_ease_in_out(tri) * 0.62;
            let mut style = StrokeStyle::solid(ACCENT, 1.0);
            style.color = accent.map(|v| v * opacity);
            p.stroke(&pts, true, style);
        }
        None => {}
    }
}

pub fn cell_at(st: &State) -> (usize, usize) {
    let b = st.term_body;
    let col = ((st.mouse.0 - b.x) / st.cell.0).max(0.0) as usize;
    let row = ((st.mouse.1 - b.y) / st.cell.1).max(0.0) as usize;
    (row, col)
}

pub fn link_at(st: &State) -> Option<&'static str> {
    if !st.term_body.contains(st.mouse) {
        return None;
    }
    let (row, col) = cell_at(st);
    st.tabs[st.active]
        .screen
        .links
        .iter()
        .any(|l| l.row == row && (l.col_start..l.col_end).contains(&col))
        .then_some("https://xtermjs.org")
}
