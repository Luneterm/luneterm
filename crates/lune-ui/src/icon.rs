//! Codicons, as mapped into the Nerd Font (Lunegit uses the Codicons icon pack). Icons are drawn
//! with the mono font role.

use crate::color::Rgba;
use crate::text::{Spec, Style};

pub const ADD: &str = "\u{ea60}";
pub const GEAR: &str = "\u{eb51}";
pub const ELLIPSIS: &str = "\u{ea7c}";
pub const CHROME_CLOSE: &str = "\u{eab8}";
pub const EDIT: &str = "\u{ea73}";
pub const COPY: &str = "\u{ebcc}";
pub const INFO: &str = "\u{ea74}";
pub const CHEVRON_DOWN: &str = "\u{eab4}";
pub const ARROW_LEFT: &str = "\u{ea9b}";
pub const CHECK: &str = "\u{eab2}";
pub const CHROME_MINIMIZE: &str = "\u{eaba}";
pub const CHROME_MAXIMIZE: &str = "\u{eab9}";
pub const CHROME_RESTORE: &str = "\u{eabb}";

/// Text spec for one icon glyph at `size`.
pub fn glyph(glyph: &str, size: f32, color: Rgba) -> Spec {
    Spec::plain(glyph, Style::mono(size, size * 1.25, color))
}
