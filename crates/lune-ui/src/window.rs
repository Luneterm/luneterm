//! Helpers for a borderless glass window: its attributes, edge resizing, title-bar double click
//! and Windows 11 style caption buttons. The caller owns the winit loop and calls these from it.

use std::time::{Duration, Instant};

use winit::dpi::LogicalSize;
use winit::window::{ResizeDirection, Window, WindowAttributes};

use crate::paint::{Painter, Rect};

#[cfg(target_os = "macos")]
mod platform {
    use winit::platform::macos::WindowAttributesExtMacOS;
    use winit::window::WindowAttributes;

    pub(super) fn glass(attrs: WindowAttributes) -> WindowAttributes {
        attrs
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
    use winit::window::WindowAttributes;

    pub(super) fn glass(attrs: WindowAttributes) -> WindowAttributes {
        // Borderless so the glass reaches the top edge; the app draws the caption buttons. No
        // redirection bitmap: wgpu presents through DirectComposition, which keeps alpha.
        attrs
            .with_decorations(false)
            .with_no_redirection_bitmap(true)
            .with_undecorated_shadow(true)
            .with_corner_preference(CornerPreference::Round)
            .with_theme(Some(winit::window::Theme::Dark))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use winit::window::WindowAttributes;

    pub(super) fn glass(attrs: WindowAttributes) -> WindowAttributes {
        attrs
    }
}

/// Window attributes for a transparent glass window. Sizes are logical pixels. macOS gets a
/// transparent, full-size title bar; Windows gets a borderless window (see [`caption_buttons`]).
pub fn glass_attributes(
    title: &str,
    size: (f64, f64),
    min_size: (f64, f64),
    visible: bool,
) -> WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(LogicalSize::new(size.0, size.1))
        .with_min_inner_size(LogicalSize::new(min_size.0, min_size.1))
        .with_transparent(true)
        .with_visible(visible);
    platform::glass(attrs)
}

/// Width of the resize border of a borderless window, in logical pixels.
pub const EDGE_BORDER: f32 = 6.0;

/// Which edge of a borderless window of size `w` × `h` the point `p` is on, for resizing.
pub fn edge_at(p: (f32, f32), w: f32, h: f32) -> Option<ResizeDirection> {
    use ResizeDirection as D;
    const B: f32 = EDGE_BORDER;
    let (l, r, t, b) = (p.0 < B, p.0 >= w - B, p.1 < B, p.1 >= h - B);
    Some(match (l, r, t, b) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return None,
    })
}

/// Two clicks closer together than this are a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Detects double clicks from click times the caller supplies.
#[derive(Debug, Clone, Copy, Default)]
pub struct DoubleClick {
    last: Option<Instant>,
}

impl DoubleClick {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a click at `now`. True when the gap since the previous click is strictly less than
    /// [`DOUBLE_CLICK`].
    pub fn click(&mut self, now: Instant) -> bool {
        let double = self
            .last
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK);
        self.last = Some(now);
        double
    }
}

/// Windows 11 style caption buttons (minimize, maximize or restore, close) along the top right of
/// `win`, each `height` tall. `hits` are the hit values for [minimize, maximize, close].
pub fn caption_buttons<H: Copy + PartialEq>(
    p: &mut Painter<'_, H>,
    win: Rect,
    height: f32,
    maximized: bool,
    hits: [H; 3],
) {
    use crate::icon;
    let t = p.theme;
    let restore = if maximized {
        icon::CHROME_RESTORE
    } else {
        icon::CHROME_MAXIMIZE
    };
    let glyphs = [icon::CHROME_MINIMIZE, restore, icon::CHROME_CLOSE];
    for (k, (glyph, hit)) in glyphs.into_iter().zip(hits).enumerate() {
        let is_close = k == 2;
        let r = Rect::new(win.w - 46.0 * (3 - k) as f32, 0.0, 46.0, height);
        let hovered = p.hovered(hit);
        let mut fg = t.fg_primary;
        if hovered && is_close {
            p.fill(r, 0.0, t.caption_close_hover);
            fg = t.caption_close_fg;
        } else if hovered {
            p.fill(r, 0.0, t.surface_hover);
        }
        p.text_centered(&icon::glyph(glyph, 10.0, fg), r);
        p.hit(r, hit);
    }
}
