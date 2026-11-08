//! The window shell (plan M9 D15): the glass layers, the title band, the terminal's slot and the
//! scrollbar thumb, plus the borderless-window chrome Windows needs. Everything is drawn through
//! the toolkit's `Ui`, one function for the window and for frame dumps.
//!
//! Layout, logical px: a 34 px title band, then the card inset 10 px from the left, right and
//! bottom, then the terminal's body inset 10 px inside the card. There is no tab strip until M10.

use lune_termview::{CellMetrics, GridLayout};
use lune_ui::chrome::CARD_RADIUS;
use lune_ui::color::{premul, with_alpha};
use lune_ui::input::{CursorIcon, Event, PointerButton, WindowCommand};
use lune_ui::layout::Axis;
use lune_ui::paint::Rect;
use lune_ui::scene::LayerId;
use lune_ui::text::{Spec, Style};
use lune_ui::ui::{Sense, Ui};
use lune_ui::widgets::caption::{CaptionButton, CaptionOptions};
use lune_ui::widgets::scrollbar::{self, ThumbGeometry, thumb_geometry};
use lune_ui::window::{ResizeEdge, edge_at};

/// The title band's height.
pub const TITLE_BAND: f32 = 34.0;
/// The gap between the window and the card (left, right and bottom).
pub const CARD_MARGIN: f32 = 10.0;
/// The gap between the card and the terminal body.
pub const BODY_PAD: f32 = 10.0;
/// The scrollbar thumb's opacity (accent colour).
pub const THUMB_ALPHA: f32 = 0.55;
/// The slot of the terminal in the frame's custom-draw list.
pub const TERM_SLOT: usize = 0;

/// What differs per OS in the chrome: whether the app draws the caption buttons (a borderless
/// window) and how far the centred title keeps clear of the native or drawn buttons.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chrome {
    /// The window has no native frame: draw caption buttons, drag by the band, resize by the edges.
    pub drawn_caption: bool,
    /// The title is clipped this far from both sides of the window.
    pub title_inset: f32,
}

#[cfg(target_os = "windows")]
pub const HOST: Chrome = Chrome {
    drawn_caption: true,
    // Three 46 px buttons and a margin.
    title_inset: 3.0 * 46.0 + 8.0,
};

/// macOS keeps its traffic lights (the band is drawn under them).
#[cfg(target_os = "macos")]
pub const HOST: Chrome = Chrome {
    drawn_caption: false,
    title_inset: 90.0,
};

/// Linux keeps the native decorations.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const HOST: Chrome = Chrome {
    drawn_caption: false,
    title_inset: 90.0,
};

/// Whether this OS gets an OS glass material at all (Linux has none: an opaque theme instead).
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub const HOST_GLASS: bool = true;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const HOST_GLASS: bool = false;

/// The rects of a window of a given size, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub window: Rect,
    pub band: Rect,
    pub card: Rect,
    pub body: Rect,
}

impl Layout {
    /// The layout of the window `window` (its origin is the window's).
    pub fn new(window: Rect) -> Layout {
        let card = Rect::new(
            window.x + CARD_MARGIN,
            window.y + TITLE_BAND,
            (window.w - 2.0 * CARD_MARGIN).max(0.0),
            (window.h - TITLE_BAND - CARD_MARGIN).max(0.0),
        );
        let body = Rect::new(
            card.x + BODY_PAD,
            card.y + BODY_PAD,
            (card.w - 2.0 * BODY_PAD).max(0.0),
            (card.h - 2.0 * BODY_PAD).max(0.0),
        );
        Layout {
            window,
            band: Rect::new(window.x, window.y, window.w, TITLE_BAND.min(window.h)),
            card,
            body,
        }
    }
}

/// Where the cell grid sits in the body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPlacement {
    /// The grid's top-left, logical px, snapped to a whole device pixel.
    pub origin: (f32, f32),
    pub cols: usize,
    pub rows: usize,
}

/// The grid that fits `body` for cells of `metrics` (its `scale` is the display's), anchored at the
/// body's top-left on a device pixel.
pub fn place_grid(body: Rect, metrics: CellMetrics) -> GridPlacement {
    let layout = GridLayout::new((body.x, body.y), metrics);
    let scale = if metrics.scale.is_finite() && metrics.scale > 0.0 {
        metrics.scale
    } else {
        1.0
    };
    let (cols, rows) = layout.grid_size(body.w, body.h);
    GridPlacement {
        origin: (
            layout.origin_px.0 as f32 / scale,
            layout.origin_px.1 as f32 / scale,
        ),
        cols,
        rows,
    }
}

/// The scroll state the thumb shows, in lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub viewport_top: usize,
    pub base_y: usize,
    pub total_lines: usize,
}

/// The scrollbar of the body: `None` while there is no scrollback (`base_y == 0`) or the numbers
/// do not make a bar. `cell_h` is one line in logical px.
pub fn scroll_geometry(body: Rect, cell_h: f32, scroll: ScrollView) -> Option<ThumbGeometry> {
    if scroll.base_y == 0 {
        return None;
    }
    let line = f64::from(cell_h);
    thumb_geometry(
        Axis::Vertical,
        body,
        scroll.viewport_top as f64 * line,
        scroll.total_lines as f64 * line,
        false,
    )
}

/// The thumb's rect in the body, see [`scroll_geometry`].
pub fn scroll_thumb(body: Rect, cell_h: f32, scroll: ScrollView) -> Option<Rect> {
    scroll_geometry(body, cell_h, scroll).map(|g| g.thumb)
}

/// How far outside the 6 px thumb a press still grabs it (the track is 9 px wide).
const THUMB_GRAB: f32 = 3.0;

/// Whether a press at `pos` (logical px) grabs the thumb.
pub fn thumb_hit(body: Rect, cell_h: f32, scroll: ScrollView, pos: (f32, f32)) -> bool {
    scroll_thumb(body, cell_h, scroll).is_some_and(|t| t.inset(-THUMB_GRAB, 0.0).contains(pos))
}

/// A drag of the scrollbar thumb: pointer movement becomes whole lines to scroll, the rest of a
/// line carried to the next move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbDrag {
    last_y: f32,
    rest: f64,
}

impl ThumbDrag {
    /// A drag that started with the pointer at `y` (logical px).
    pub fn start(y: f32) -> ThumbDrag {
        ThumbDrag {
            last_y: y,
            rest: 0.0,
        }
    }

    /// The pointer is now at `y`. Returns the lines to scroll, positive toward newer output, using
    /// the thumb's current geometry (a bar that vanished gives 0).
    pub fn moved(&mut self, y: f32, body: Rect, cell_h: f32, scroll: ScrollView) -> i64 {
        let dy = f64::from(y - self.last_y);
        self.last_y = y;
        let line = f64::from(cell_h);
        let Some(g) = scroll_geometry(body, cell_h, scroll) else {
            return 0;
        };
        if !(line.is_finite() && line > 0.0 && dy.is_finite()) {
            return 0;
        }
        let lines = self.rest + g.offset_delta(dy) / line;
        let whole = lines.trunc();
        self.rest = lines - whole;
        // The offset range is finite, so this is far inside i64.
        whole as i64
    }
}

/// What the shell draws a frame from.
#[derive(Debug, Clone, Copy)]
pub struct ShellView<'a> {
    pub title: &'a str,
    pub maximized: bool,
    /// The window has keyboard focus (inactive caption glyphs are dimmer).
    pub focused: bool,
    /// The terminal's scroll state, for the thumb.
    pub scroll: Option<ScrollView>,
    /// One terminal line in logical px.
    pub cell_h: f32,
    pub chrome: Chrome,
}

/// What a drawn frame hands back.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellOut {
    /// The terminal body, logical px.
    pub body: Rect,
    /// Window operations the chrome asks for this frame, in order.
    pub commands: Vec<WindowCommand>,
}

/// The cursor for resizing from `edge`.
pub fn resize_cursor(edge: ResizeEdge) -> CursorIcon {
    match edge {
        ResizeEdge::North | ResizeEdge::South => CursorIcon::NsResize,
        ResizeEdge::East | ResizeEdge::West => CursorIcon::EwResize,
        ResizeEdge::NorthEast | ResizeEdge::SouthWest => CursorIcon::NeswResize,
        ResizeEdge::NorthWest | ResizeEdge::SouthEast => CursorIcon::NwseResize,
    }
}

/// The edge a primary press of this frame landed on, for a window of `size` (logical).
pub fn edge_press(events: &[Event], size: (f32, f32)) -> Option<ResizeEdge> {
    events.iter().find_map(|e| match e {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            ..
        } => edge_at(*pos, size.0, size.1),
        _ => None,
    })
}

/// The window command of a caption button.
pub fn caption_command(button: CaptionButton) -> WindowCommand {
    match button {
        CaptionButton::Minimize => WindowCommand::Minimize,
        CaptionButton::Maximize => WindowCommand::ToggleMaximize,
        CaptionButton::Close => WindowCommand::Close,
    }
}

/// What a press in the title band does: a double click maximizes or restores (borderless windows
/// only: elsewhere the OS owns that), any other press drags the window.
pub fn band_command(press_count: u8, chrome: Chrome) -> WindowCommand {
    if press_count >= 2 && chrome.drawn_caption {
        WindowCommand::ToggleMaximize
    } else {
        WindowCommand::DragMove
    }
}

/// Draws the shell into `ui` and returns the terminal body and the chrome's window commands.
///
/// The glass layers go on the fixed base layer (panel over the window, card on top); the
/// terminal's custom slot sits in the body, below the thumb.
pub fn draw(ui: &mut Ui<'_>, view: &ShellView<'_>) -> ShellOut {
    let theme = *ui.theme();
    let win = ui.painter().window;
    let layout = Layout::new(win);
    {
        let p = ui.painter();
        p.on(LayerId::Base);
        let panel = theme.glass.panel.premultiplied();
        let card = theme.glass.card.premultiplied();
        p.acrylic(win, 0.0, panel, [0.0; 4], true);
        p.acrylic(layout.card, CARD_RADIUS, card, panel, true);

        // The title, centred in the band and kept clear of the window buttons.
        let side = view.chrome.title_inset;
        let spec = Spec::plain(view.title, Style::ui(12.0, theme.fg_secondary).medium());
        p.set_clip(Some(Rect::new(
            side,
            0.0,
            (win.w - 2.0 * side).max(0.0),
            TITLE_BAND,
        )));
        p.text_centered(&spec, Rect::new(0.0, 0.0, win.w, TITLE_BAND - 2.0));
        p.set_clip(None);

        p.custom(TERM_SLOT, layout.body);
        if let Some(thumb) = view
            .scroll
            .and_then(|s| scroll_thumb(layout.body, view.cell_h, s))
        {
            let color = premul(with_alpha(theme.accent, THUMB_ALPHA));
            p.fill_over(thumb, scrollbar::RADIUS, color);
        }
    }

    let mut commands = Vec::new();
    let pressed_edge = if view.chrome.drawn_caption && !view.maximized {
        if let Some(edge) = ui.pointer_pos().and_then(|p| edge_at(p, win.w, win.h)) {
            ui.set_cursor(resize_cursor(edge));
        }
        edge_press(&ui.input().events, (win.w, win.h))
    } else {
        None
    };
    if let Some(edge) = pressed_edge {
        commands.push(WindowCommand::DragResize(edge));
    }

    // The band first, so the caption buttons registered after it sit on top.
    let band_id = ui.id("title-band");
    let band = ui.interact(band_id, layout.band, Sense::CLICK);
    if band.press_started && pressed_edge.is_none() {
        commands.push(band_command(band.press_count, view.chrome));
    }
    if view.chrome.drawn_caption {
        let opts = CaptionOptions {
            height: TITLE_BAND,
            maximized: view.maximized,
            active: view.focused,
        };
        if let Some(button) = ui.caption_buttons("caption", win, opts) {
            commands.push(caption_command(button));
        }
    }
    ShellOut {
        body: layout.body,
        commands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts;
    use lune_ui::input::{Modifiers, RawInput, keymap};
    use lune_ui::lunegit_dark::THEME;
    use lune_ui::motion::MotionTier;
    use lune_ui::shortcuts::Shortcuts;
    use lune_ui::strings::{DisplayFormat, StringTable};
    use lune_ui::text::TextCache;
    use lune_ui::ui::{FrameConfig, UiStore};
    use std::time::Duration;

    fn metrics(cell_w: u32, cell_h: u32, scale: f32) -> CellMetrics {
        CellMetrics {
            cell_w,
            cell_h,
            char_h: cell_h - 2,
            char_top: 1,
            baseline: 13,
            font_size: 13.0,
            scale,
        }
    }

    /// The card, body and grid rects for the first window size (1040 x 660), at scales 1 and 2.
    #[test]
    fn layout_pins_the_card_the_body_and_the_grid() {
        let win = Rect::new(0.0, 0.0, 1040.0, 660.0);
        let l = Layout::new(win);
        assert_eq!(l.band, Rect::new(0.0, 0.0, 1040.0, 34.0));
        assert_eq!(l.card, Rect::new(10.0, 34.0, 1020.0, 616.0));
        assert_eq!(l.body, Rect::new(20.0, 44.0, 1000.0, 596.0));

        // Scale 1: an 8 x 17 cell; scale 2: 16 x 34 (the same cell in logical px).
        let one = place_grid(l.body, metrics(8, 17, 1.0));
        assert_eq!(
            one,
            GridPlacement {
                origin: (20.0, 44.0),
                cols: 125,
                rows: 35
            }
        );
        let two = place_grid(l.body, metrics(16, 34, 2.0));
        assert_eq!(
            two,
            GridPlacement {
                origin: (20.0, 44.0),
                cols: 125,
                rows: 35
            }
        );
    }

    #[test]
    fn the_grid_origin_snaps_to_a_device_pixel() {
        // A body at 20.3 logical px is 40.6 -> 41 device px at scale 2: 20.5 logical.
        let body = Rect::new(20.3, 44.0, 400.0, 300.0);
        let p = place_grid(body, metrics(16, 34, 2.0));
        assert_eq!(p.origin, (20.5, 44.0));
        // A bad scale counts as 1.
        let p = place_grid(body, metrics(8, 17, 0.0));
        assert_eq!(p.origin, (20.0, 44.0));
    }

    #[test]
    fn a_tiny_window_has_empty_rects_not_negative_ones() {
        let l = Layout::new(Rect::new(0.0, 0.0, 15.0, 20.0));
        assert_eq!((l.card.w, l.card.h), (0.0, 0.0));
        assert_eq!((l.body.w, l.body.h), (0.0, 0.0));
        assert_eq!(l.band.h, 20.0);
    }

    #[test]
    fn the_thumb_shows_only_with_scrollback() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let none = ScrollView {
            viewport_top: 0,
            base_y: 0,
            total_lines: 35,
        };
        assert_eq!(scroll_thumb(body, 17.0, none), None);
        // 500 lines, 35 shown, scrolled to the bottom: the thumb is at the bottom of the track.
        let bottom = ScrollView {
            viewport_top: 465,
            base_y: 465,
            total_lines: 500,
        };
        let t = scroll_thumb(body, 17.0, bottom).expect("thumb");
        assert_eq!(t.w, scrollbar::THUMB);
        assert!((t.y + t.h - (body.y + body.h)).abs() < 1.0, "{t:?}");
        let top = ScrollView {
            viewport_top: 0,
            ..bottom
        };
        let t = scroll_thumb(body, 17.0, top).expect("thumb");
        assert!((t.y - body.y).abs() < 1.0, "{t:?}");
        assert!(t.h >= scrollbar::MIN_LENGTH);
    }

    #[test]
    fn dragging_the_thumb_scrolls_in_whole_lines_and_carries_the_rest() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let scroll = ScrollView {
            viewport_top: 100,
            base_y: 465,
            total_lines: 500,
        };
        let g = scroll_geometry(body, 17.0, scroll).expect("bar");
        // A full travel is the whole range: base_y lines.
        let mut drag = ThumbDrag::start(100.0);
        let all = drag.moved(100.0 + g.travel as f32, body, 17.0, scroll);
        assert!((all - 465).abs() <= 1, "{all}");
        // Up is negative; small moves add up without losing the remainder.
        let mut drag = ThumbDrag::start(300.0);
        assert!(drag.moved(290.0, body, 17.0, scroll) < 0);
        let mut drag = ThumbDrag::start(300.0);
        let per_px = 465.0 / g.travel;
        let mut total = 0;
        for i in 1..=200 {
            total += drag.moved(300.0 + i as f32 * 0.1, body, 17.0, scroll);
        }
        let want = (20.0 * per_px).floor() as i64;
        assert!(
            (total - want).abs() <= 1,
            "total {total}, want about {want}"
        );
        // No bar, no scroll.
        let none = ScrollView {
            base_y: 0,
            ..scroll
        };
        assert_eq!(ThumbDrag::start(0.0).moved(50.0, body, 17.0, none), 0);
    }

    #[test]
    fn a_press_grabs_the_thumb_a_little_outside_its_6_px() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let scroll = ScrollView {
            viewport_top: 0,
            base_y: 465,
            total_lines: 500,
        };
        let t = scroll_thumb(body, 17.0, scroll).expect("thumb");
        let mid = t.y + t.h / 2.0;
        assert!(thumb_hit(body, 17.0, scroll, (t.x + 1.0, mid)));
        assert!(thumb_hit(body, 17.0, scroll, (t.x - 2.0, mid)));
        assert!(!thumb_hit(body, 17.0, scroll, (t.x - 10.0, mid)));
        assert!(!thumb_hit(body, 17.0, scroll, (t.x + 1.0, t.y + t.h + 5.0)));
        let none = ScrollView {
            base_y: 0,
            ..scroll
        };
        assert!(!thumb_hit(body, 17.0, none, (t.x + 1.0, mid)));
    }

    #[test]
    fn the_caption_buttons_and_the_band_map_to_window_commands() {
        assert_eq!(
            caption_command(CaptionButton::Minimize),
            WindowCommand::Minimize
        );
        assert_eq!(
            caption_command(CaptionButton::Maximize),
            WindowCommand::ToggleMaximize
        );
        assert_eq!(caption_command(CaptionButton::Close), WindowCommand::Close);
        let drawn = Chrome {
            drawn_caption: true,
            title_inset: 146.0,
        };
        let native = Chrome {
            drawn_caption: false,
            ..drawn
        };
        assert_eq!(band_command(1, drawn), WindowCommand::DragMove);
        assert_eq!(band_command(2, drawn), WindowCommand::ToggleMaximize);
        assert_eq!(band_command(2, native), WindowCommand::DragMove);
    }

    /// One toolkit frame of the shell at 1040 x 660, scale 1, no GPU: what `draw` asks of the window.
    fn frame(
        store: &mut UiStore,
        text: &mut TextCache,
        events: Vec<Event>,
        chrome: Chrome,
        maximized: bool,
    ) -> ShellOut {
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::ZERO,
            focused: true,
            events,
        };
        let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Off,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let view = ShellView {
            title: "Luneterm",
            maximized,
            focused: true,
            scroll: None,
            cell_h: 17.0,
            chrome,
        };
        let mut ui = Ui::begin(store, &input, config, text);
        let out = draw(&mut ui, &view);
        let _ = ui.finish();
        out
    }

    fn button(pos: (f32, f32), pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        }
    }

    const BORDERLESS: Chrome = Chrome {
        drawn_caption: true,
        title_inset: 146.0,
    };

    /// The chrome through a real `Ui` frame: the first frame registers the hit regions, later
    /// frames carry the input (the toolkit resolves input against the previous frame's regions).
    #[test]
    fn a_borderless_window_drags_by_the_band_and_maximizes_on_a_double_press() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events, maximized| {
            frame(&mut store, &mut text, events, BORDERLESS, maximized).commands
        };
        assert_eq!(go(vec![], false), vec![]);
        let at = (500.0, 17.0);
        assert_eq!(
            go(vec![button(at, true)], false),
            vec![WindowCommand::DragMove]
        );
        assert_eq!(go(vec![button(at, false)], false), vec![]);
        assert_eq!(
            go(vec![button(at, true)], false),
            vec![WindowCommand::ToggleMaximize]
        );
    }

    #[test]
    fn a_native_window_only_drags_by_the_band() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let native = Chrome {
            drawn_caption: false,
            ..BORDERLESS
        };
        let mut go = |events| frame(&mut store, &mut text, events, native, false).commands;
        go(vec![]);
        let at = (500.0, 17.0);
        for _ in 0..2 {
            assert_eq!(go(vec![button(at, true)]), vec![WindowCommand::DragMove]);
            go(vec![button(at, false)]);
        }
        // No resize edges and no caption buttons: a press at the corner is just the band.
        assert_eq!(
            go(vec![button((1030.0, 17.0), true)]),
            vec![WindowCommand::DragMove]
        );
    }

    #[test]
    fn the_caption_buttons_ask_for_minimize_maximize_and_close() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events| frame(&mut store, &mut text, events, BORDERLESS, false).commands;
        go(vec![]);
        // Three 46 px buttons along the top right: 902..948, 948..994 and 994..1040.
        for (x, want) in [
            (925.0, WindowCommand::Minimize),
            (971.0, WindowCommand::ToggleMaximize),
            (1017.0, WindowCommand::Close),
        ] {
            let press = go(vec![button((x, 17.0), true)]);
            assert!(!press.contains(&WindowCommand::DragMove), "{press:?}");
            assert_eq!(go(vec![button((x, 17.0), false)]), vec![want]);
        }
    }

    #[test]
    fn a_press_on_the_border_resizes_and_a_maximized_window_does_not() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events, maximized| {
            frame(&mut store, &mut text, events, BORDERLESS, maximized).commands
        };
        go(vec![], false);
        assert_eq!(
            go(vec![button((2.0, 300.0), true)], false),
            vec![WindowCommand::DragResize(ResizeEdge::West)]
        );
        go(vec![button((2.0, 300.0), false)], false);
        // The top border lies in the band: the resize wins over the band's drag.
        assert_eq!(
            go(vec![button((500.0, 2.0), true)], false),
            vec![WindowCommand::DragResize(ResizeEdge::North)]
        );
        go(vec![button((500.0, 2.0), false)], false);
        // Maximized: the same press on the left border asks for nothing.
        assert_eq!(go(vec![button((2.0, 300.0), true)], true), vec![]);
    }

    #[test]
    fn an_edge_press_is_a_primary_press_on_the_border() {
        let size = (1040.0, 660.0);
        let press = |pos, button, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::default(),
        };
        let on_edge = press((2.0, 300.0), PointerButton::Primary, true);
        assert_eq!(
            edge_press(std::slice::from_ref(&on_edge), size),
            Some(ResizeEdge::West)
        );
        assert_eq!(
            edge_press(
                &[press((1039.0, 659.0), PointerButton::Primary, true)],
                size
            ),
            Some(ResizeEdge::SouthEast)
        );
        // A release, a secondary press and a press inside the window are not edge presses.
        for e in [
            press((2.0, 300.0), PointerButton::Primary, false),
            press((2.0, 300.0), PointerButton::Secondary, true),
            press((500.0, 300.0), PointerButton::Primary, true),
            Event::PointerMoved { pos: (2.0, 300.0) },
        ] {
            assert_eq!(edge_press(&[e], size), None);
        }
        assert_eq!(resize_cursor(ResizeEdge::NorthWest), CursorIcon::NwseResize);
        assert_eq!(resize_cursor(ResizeEdge::South), CursorIcon::NsResize);
    }
}
