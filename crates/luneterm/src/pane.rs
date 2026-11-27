//! The side pane's frame (plan D20, D26-D29): which of the two views a window shows, the pane's
//! rects, the bar view's show button and the pane's own controls (the hide button, the divider of
//! the footer and the width drag at its right edge). The window's drag regions, the footer's bell
//! and `⋯` buttons and the terminal card are the shell's ([`crate::shell`]); the tab list of the
//! pane is not drawn here yet.

use lune_ui::input::CursorIcon;
use lune_ui::paint::{Painter, Rect};
use lune_ui::theme::Theme;
use lune_ui::ui::{Sense, Ui};
use lune_ui::widgets::button::{IconButton, IconButtonExtras};

use crate::settings::{MAX_PANE_WIDTH, MIN_PANE_WIDTH};
use crate::shell::{CARD_MARGIN, Chrome, OVERFLOW_SIZE, TITLE_BAND, View};

/// The least width the terminal card keeps beside the pane (D29): below it the top bar shows.
pub const CARD_MIN_WIDTH: f32 = 480.0;
/// The pane's content inset, left and right.
pub const PAD: f32 = 8.0;
/// How far inside the content the footer's divider and buttons start.
pub const FOOTER_INSET: f32 = 4.0;
/// The gap between the footer's divider and its buttons.
pub const FOOTER_GAP: f32 = 8.0;
/// The width of the zone at the pane's right edge that drags its width.
pub const EDGE_ZONE: f32 = 6.0;
/// The codicon `layout-sidebar-left`: the hide button, the show button and their menu row.
pub const TOGGLE_GLYPH: &str = "\u{ebf3}";
/// Where the native traffic lights end (the zoom light's right edge, logical px, measured on a
/// window with a full-size content view) and the room kept after them before the show button.
const LIGHTS_END: f32 = 69.0;
const LIGHTS_GAP: f32 = 7.0;

/// Whether a window `window_w` wide shows a pane `width` wide: the card keeps [`CARD_MIN_WIDTH`]
/// (and its right margin) beside it (D29).
pub fn fits(window_w: f32, width: f32) -> bool {
    window_w >= width + CARD_MIN_WIDTH + CARD_MARGIN
}

/// The view of a frame: the pane when it is the saved choice and the window is wide enough for it
/// at its saved width, else the top bar.
pub fn resolve(wanted: bool, width: u32, window_w: f32, chrome: Chrome) -> View {
    if wanted && fits(window_w, width as f32) {
        View::Pane {
            width: width as f32,
            drawn_caption: chrome.drawn_caption,
        }
    } else {
        View::Bar
    }
}

/// Whether the top bar offers its show button: only while the window is wide enough for the pane
/// the button would show (D29).
pub fn offers_show_button(view: View, window_w: f32, width: u32) -> bool {
    view == View::Bar && fits(window_w, width as f32)
}

/// The widest pane a drag may make in a window `window_w` wide: the file's range, and no more than
/// the card leaves, so a drag never hides the pane it is dragging.
pub fn max_width(window_w: f32) -> u32 {
    let room = (window_w - CARD_MIN_WIDTH - CARD_MARGIN).floor();
    // Inside the range, so the cast is exact (a NaN clamps to NaN and casts to the least).
    (room.clamp(MIN_PANE_WIDTH as f32, MAX_PANE_WIDTH as f32) as u32).max(MIN_PANE_WIDTH)
}

/// The rects of the pane of a window, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneRects {
    /// The whole pane: the window's left column.
    pub rect: Rect,
    /// The title area at its top: the traffic lights' room and the hide button. A press on its
    /// empty part drags the window.
    pub header: Rect,
    /// The tab list's area, between the header and the footer.
    pub list: Rect,
    /// The footer's 1 px divider.
    pub divider: Rect,
    /// The footer, from the divider to the window's bottom.
    pub footer: Rect,
    /// The bell's place at the footer's left.
    pub bell: Rect,
    /// The `⋯` button's place at the footer's right.
    pub menu: Rect,
    /// The zone at the right edge that drags the width.
    pub edge: Rect,
}

impl PaneRects {
    /// The rects of a pane `width` wide at the left of `window`.
    pub fn new(window: Rect, width: f32) -> PaneRects {
        let rect = Rect::new(window.x, window.y, width.min(window.w).max(0.0), window.h);
        let header = Rect::new(rect.x, rect.y, rect.w, TITLE_BAND.min(rect.h));
        // The buttons stand on the card's bottom line.
        let button_y = rect.bottom() - CARD_MARGIN - OVERFLOW_SIZE;
        let divider_y = button_y - FOOTER_GAP - 1.0;
        let inset = PAD + FOOTER_INSET;
        PaneRects {
            rect,
            header,
            list: Rect::new(
                rect.x + PAD,
                header.bottom(),
                (rect.w - 2.0 * PAD).max(0.0),
                (divider_y - header.bottom()).max(0.0),
            ),
            divider: Rect::new(
                rect.x + inset,
                divider_y,
                (rect.w - 2.0 * inset).max(0.0),
                1.0,
            ),
            footer: Rect::new(
                rect.x,
                divider_y,
                rect.w,
                (rect.bottom() - divider_y).max(0.0),
            ),
            bell: Rect::new(rect.x + inset, button_y, OVERFLOW_SIZE, OVERFLOW_SIZE),
            menu: Rect::new(
                rect.right() - inset - OVERFLOW_SIZE,
                button_y,
                OVERFLOW_SIZE,
                OVERFLOW_SIZE,
            ),
            edge: Rect::new(rect.right() - EDGE_ZONE, rect.y, EDGE_ZONE, rect.h),
        }
    }
}

/// The y of a button in the title area: on the line of the native traffic lights where there are
/// some, else centred like the drawn caption buttons.
fn title_button_y(chrome: Chrome) -> f32 {
    if chrome.native_lights { 4.0 } else { 5.0 }
}

/// The hide button, at the right end of the pane's header.
pub fn hide_button(rects: &PaneRects, chrome: Chrome) -> Rect {
    Rect::new(
        rects.rect.right() - PAD - OVERFLOW_SIZE,
        rects.rect.y + title_button_y(chrome),
        OVERFLOW_SIZE,
        OVERFLOW_SIZE,
    )
}

/// The show button of the top bar: right of the native traffic lights where there are some, else
/// at the band's left, on the card's left line.
pub fn show_button(window: Rect, chrome: Chrome) -> Rect {
    let x = if chrome.native_lights {
        LIGHTS_END + LIGHTS_GAP
    } else {
        CARD_MARGIN
    };
    Rect::new(
        window.x + x,
        window.y + title_button_y(chrome),
        OVERFLOW_SIZE,
        OVERFLOW_SIZE,
    )
}

/// What the pane's edge zone saw in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaneEdge {
    /// A drag of it began this frame.
    pub started: bool,
    /// How far the pointer moved along x since the previous frame while dragging (the first frame:
    /// since the press).
    pub dx: f32,
    /// The press on it is still held.
    pub held: bool,
}

/// A drag of the pane's right edge: the width it began at and the pointer's travel since. The
/// width follows the pointer however far past a limit it went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidthDrag {
    start: u32,
    travel: f32,
}

impl WidthDrag {
    /// A drag that began with the pane `width` wide.
    pub fn start(width: u32) -> WidthDrag {
        WidthDrag {
            start: width,
            travel: 0.0,
        }
    }

    /// The width after the pointer moved `dx` further, in a window `window_w` wide.
    fn moved(&mut self, dx: f32, window_w: f32) -> u32 {
        if dx.is_finite() {
            self.travel += dx;
        }
        let wanted = (self.start as f32 + self.travel).round();
        // Clamped into the range first, so the cast is exact.
        wanted.clamp(MIN_PANE_WIDTH as f32, max_width(window_w) as f32) as u32
    }
}

/// What the app does after a frame of the edge zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DragStep {
    /// The pane is this wide now (it changed this frame).
    pub width: Option<u32>,
    /// The drag ended with another width than it began with: save it, this once.
    pub save: bool,
}

/// One frame of a width drag: `drag` is the one under way (`None`: none), `edge` what the zone saw
/// (`None`: there was no zone, so a drag under way ends), `width` the pane's width now.
pub fn drag_step(
    drag: &mut Option<WidthDrag>,
    edge: Option<PaneEdge>,
    width: u32,
    window_w: f32,
) -> DragStep {
    if edge.is_some_and(|e| e.started) {
        *drag = Some(WidthDrag::start(width));
    }
    let Some(under_way) = drag else {
        return DragStep::default();
    };
    let mut now = width;
    if let Some(e) = edge.filter(|e| e.started || e.dx != 0.0) {
        now = under_way.moved(e.dx, window_w);
    }
    let start = under_way.start;
    let ended = !edge.is_some_and(|e| e.held);
    if ended {
        *drag = None;
    }
    DragStep {
        width: (now != width).then_some(now),
        save: ended && now != start,
    }
}

/// The pane's own paint under its controls: the footer's divider. The pane has no layer of its own
/// (D33): the window's panel is behind it.
pub fn paint<H: Copy + PartialEq>(p: &mut Painter<'_, H>, rects: &PaneRects, theme: &Theme) {
    p.fill(rects.divider, 0.0, theme.border);
}

/// What the pane's controls asked in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaneOut {
    /// The hide button was pressed.
    pub toggle: bool,
    /// What the edge zone saw; `None` while the press belongs to the window's own resize edge.
    pub edge: Option<PaneEdge>,
}

/// The pane's controls: the edge zone and the hide button. They are registered after the window's
/// drag regions, so a press on them never drags the window. `window_edge` is whether the pointer is
/// at an edge the app resizes the window by, and `edge_pressed` whether this frame's press landed
/// on one: such a press is the window's and starts no width drag, while a drag already under way
/// goes on past those edges.
pub fn controls(
    ui: &mut Ui<'_>,
    rects: &PaneRects,
    chrome: Chrome,
    name: &str,
    window_edge: bool,
    edge_pressed: bool,
) -> PaneOut {
    let id = ui.id("pane-edge");
    let zone = ui.interact(id, rects.edge, Sense::DRAG);
    let edge = (!edge_pressed).then(|| {
        if (zone.hovered && !window_edge) || zone.dragged {
            ui.set_cursor(CursorIcon::ColResize);
        }
        let moving = zone.drag_started || zone.dragged || zone.drag_stopped;
        PaneEdge {
            started: zone.drag_started && !window_edge,
            dx: if moving { zone.drag_delta.0 } else { 0.0 },
            held: zone.pressed,
        }
    });
    let radius = ui.theme().item_radius;
    let hide = IconButton {
        glyph: TOGGLE_GLYPH,
        glyph_size: 16.0,
        color: None,
        radius,
        name,
    };
    // The pressed look while the pane is shown (D20).
    let open = IconButtonExtras::new().with_open(true);
    let toggle = ui
        .icon_button_with("pane-hide", hide_button(rects, chrome), &hide, &open)
        .clicked;
    PaneOut { toggle, edge }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{LINUX_CHROME, MACOS_CHROME, WINDOWS_CHROME};

    const WINDOW: Rect = Rect::new(0.0, 0.0, 1040.0, 660.0);

    #[test]
    fn the_pane_shows_at_its_width_plus_490_and_not_one_pixel_below() {
        assert!(fits(738.0, 248.0));
        assert!(!fits(737.0, 248.0));
        for chrome in [MACOS_CHROME, WINDOWS_CHROME, LINUX_CHROME] {
            assert_eq!(
                resolve(true, 248, 738.0, chrome),
                View::Pane {
                    width: 248.0,
                    drawn_caption: chrome.drawn_caption
                }
            );
            assert_eq!(resolve(true, 248, 737.0, chrome), View::Bar);
            assert_eq!(resolve(false, 248, 1040.0, chrome), View::Bar);
        }
        // The saved width decides: a wider pane needs a wider window.
        assert_eq!(resolve(true, 420, 909.0, MACOS_CHROME), View::Bar);
        assert_ne!(resolve(true, 420, 910.0, MACOS_CHROME), View::Bar);
    }

    #[test]
    fn the_show_button_is_offered_only_by_a_bar_wide_enough_for_the_pane() {
        assert!(offers_show_button(View::Bar, 738.0, 248));
        assert!(!offers_show_button(View::Bar, 737.0, 248));
        let pane = resolve(true, 248, 1040.0, MACOS_CHROME);
        assert!(!offers_show_button(pane, 1040.0, 248));
        // The saved choice is the pane but the window is narrow: bar, no button.
        let narrow = resolve(true, 248, 737.0, MACOS_CHROME);
        assert!(!offers_show_button(narrow, 737.0, 248));
    }

    #[test]
    fn the_pane_rects_follow_d26_and_the_directors_footer() {
        let r = PaneRects::new(WINDOW, 248.0);
        assert_eq!(r.rect, Rect::new(0.0, 0.0, 248.0, 660.0));
        assert_eq!(r.header, Rect::new(0.0, 0.0, 248.0, 34.0));
        assert_eq!(r.edge, Rect::new(242.0, 0.0, 6.0, 660.0));
        // Footer buttons end on the card's bottom line (660 - 10), the divider 8 above them.
        assert_eq!(r.bell, Rect::new(12.0, 626.0, 24.0, 24.0));
        assert_eq!(r.menu, Rect::new(212.0, 626.0, 24.0, 24.0));
        assert_eq!(r.divider, Rect::new(12.0, 617.0, 224.0, 1.0));
        assert_eq!(r.footer, Rect::new(0.0, 617.0, 248.0, 43.0));
        assert_eq!(r.list, Rect::new(8.0, 34.0, 232.0, 583.0));
        // No control overlaps the edge zone.
        for c in [r.bell, r.menu, r.list, hide_button(&r, MACOS_CHROME)] {
            assert!(c.right() <= r.edge.x, "{c:?}");
        }
    }

    #[test]
    fn the_title_buttons_sit_on_the_lights_line_or_the_caption_buttons() {
        let r = PaneRects::new(WINDOW, 248.0);
        assert_eq!(
            hide_button(&r, MACOS_CHROME),
            Rect::new(216.0, 4.0, 24.0, 24.0)
        );
        assert_eq!(
            hide_button(&r, WINDOWS_CHROME),
            Rect::new(216.0, 5.0, 24.0, 24.0)
        );
        assert_eq!(
            show_button(WINDOW, MACOS_CHROME),
            Rect::new(76.0, 4.0, 24.0, 24.0)
        );
        for chrome in [WINDOWS_CHROME, LINUX_CHROME] {
            assert_eq!(
                show_button(WINDOW, chrome),
                Rect::new(10.0, 5.0, 24.0, 24.0)
            );
        }
    }

    #[test]
    fn a_drag_is_limited_by_the_range_and_by_what_the_window_leaves() {
        assert_eq!(max_width(1040.0), 420);
        assert_eq!(max_width(910.0), 420);
        assert_eq!(max_width(800.0), 310);
        assert_eq!(max_width(738.0), 248);
        assert_eq!(max_width(100.0), 200);
        assert_eq!(max_width(f32::NAN), 200);
    }

    /// A frame of the zone: it started, moved `dx`, and is held or not.
    fn edge(started: bool, dx: f32, held: bool) -> Option<PaneEdge> {
        Some(PaneEdge { started, dx, held })
    }

    #[test]
    fn the_width_follows_the_pointer_clamped_and_is_saved_once_at_the_release() {
        let mut drag = None;
        let mut width = 248;
        let mut saves = 0;
        // Idle, then a press that has not moved yet, the start, far past both limits and back.
        let frames = [
            (edge(false, 0.0, false), 248),
            (edge(false, 0.0, true), 248),
            (edge(true, 10.0, true), 258),
            (edge(false, 500.0, true), 420),
            (edge(false, -10.0, true), 420),
            (edge(false, -100.0, true), 420),
            (edge(false, -300.0, true), 348),
            (edge(false, -900.0, true), 200),
            (edge(false, 900.0, true), 348),
            (edge(false, 0.0, true), 348),
        ];
        for (i, (frame, want)) in frames.into_iter().enumerate() {
            let step = drag_step(&mut drag, frame, width, 1040.0);
            width = step.width.unwrap_or(width);
            saves += usize::from(step.save);
            assert_eq!(width, want, "frame {i}");
            assert_eq!(saves, 0, "frame {i}: nothing is saved while the drag runs");
        }
        // The release: one save, and nothing more after it.
        let step = drag_step(&mut drag, edge(false, 2.0, false), width, 1040.0);
        assert_eq!(
            step,
            DragStep {
                width: Some(350),
                save: true
            }
        );
        let after = drag_step(&mut drag, edge(false, 0.0, false), 350, 1040.0);
        assert_eq!(after, DragStep::default());
        assert!(drag.is_none());
    }

    #[test]
    fn a_drag_that_ends_where_it_began_saves_nothing_and_a_vanished_zone_ends_it() {
        let mut drag = None;
        let step = drag_step(&mut drag, edge(true, 20.0, true), 248, 1040.0);
        assert_eq!(step.width, Some(268));
        let back = drag_step(&mut drag, edge(false, -20.0, false), 268, 1040.0);
        assert_eq!(
            back,
            DragStep {
                width: Some(248),
                save: false
            }
        );
        // The view changed under the pointer (no zone): the width so far is saved.
        drag_step(&mut drag, edge(true, 20.0, true), 248, 1040.0);
        let gone = drag_step(&mut drag, None, 268, 1040.0);
        assert_eq!(
            gone,
            DragStep {
                width: None,
                save: true
            }
        );
        assert!(drag.is_none());
        // A move with no drag under way does nothing.
        let stray = drag_step(&mut drag, edge(false, 50.0, true), 248, 1040.0);
        assert_eq!(stray, DragStep::default());
        // A start and release in one frame.
        let quick = drag_step(&mut drag, edge(true, 30.0, false), 248, 1040.0);
        assert_eq!(
            quick,
            DragStep {
                width: Some(278),
                save: true
            }
        );
    }
}
