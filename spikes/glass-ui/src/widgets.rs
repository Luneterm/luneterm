//! The toolkit's controls, bound to this app's hit type.

use crate::state::Hit;
use crate::theme::Rgba;
use lune_ui::lunegit_dark::THEME;
use lune_ui::paint::Painter;
use lune_ui::text::Spec;

pub use lune_ui::widgets::{checkbox, combo, field, stepper, toggle};

pub type P<'a> = Painter<'a, Hit>;

pub fn icon(glyph: &str, size: f32, color: Rgba) -> Spec {
    lune_ui::icon::glyph(glyph, size, color)
}

pub fn label(text: &str, color: Rgba) -> Spec {
    lune_ui::widgets::label(&THEME, text, color)
}
