//! Fonts (plan M9 D16): the four bundled faces, system fonts for fallback, and the terminal's
//! `TermFonts`.
//!
//! The toolkit's `TextCache` loads no system font, so the app does, once: without them emoji and
//! CJK would have no fallback face. One `FontSystem` serves the UI text and the terminal (a second
//! one would duplicate the 2.4 MB mono face).

use std::time::{Duration, Instant};

use lune_termview::{TermFonts, natural_line_height};
use lune_ui::text::TextCache;

use crate::settings::Settings;

/// The UI family (bundled RznSans).
pub const UI_FONT: &str = "RznSans";
/// The terminal family: Lunegit's bundled font until RznMono exists (M10's release gate).
pub const MONO_FONT: &str = "RobotoMono Nerd Font Mono";

/// The terminal fonts the settings page offers: the bundled families. A saved font name that is
/// not one of these (a font removed, a hand-edited file) draws as the first, [`MONO_FONT`].
/// Step 11 adds RznMono here.
pub const TERMINAL_FONTS: &[&str] = &[MONO_FONT];

/// The family the terminal draws with for the saved font name `saved` (empty is the default).
pub fn terminal_family(saved: &str) -> &'static str {
    TERMINAL_FONTS
        .iter()
        .copied()
        .find(|f| *f == saved)
        .unwrap_or(MONO_FONT)
}

const MONO_REGULAR: &[u8] = include_bytes!("../assets/RobotoMonoNerdFontMono-Regular.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../assets/RobotoMonoNerdFontMono-Bold.ttf");
const UI_REGULAR: &[u8] = include_bytes!("../assets/RznSans-Regular.ttf");
const UI_MEDIUM: &[u8] = include_bytes!("../assets/RznSans-Medium.ttf");

/// The bundled faces in the order the `TextCache` loads them.
pub fn bundled() -> [&'static [u8]; 4] {
    [MONO_REGULAR, MONO_BOLD, UI_REGULAR, UI_MEDIUM]
}

/// A `TextCache` with the bundled faces only, in a fixed order: the same fonts on every machine
/// (frame dumps and tests start here).
pub fn bundled_text_cache() -> TextCache {
    TextCache::new(&bundled(), UI_FONT, MONO_FONT)
}

/// [`bundled_text_cache`] plus the system fonts, and how long loading them took.
pub fn text_cache() -> (TextCache, Duration) {
    let mut text = bundled_text_cache();
    let started = Instant::now();
    text.fonts.db_mut().load_system_fonts();
    (text, started.elapsed())
}

#[cfg(target_os = "macos")]
fn emoji_family() -> &'static str {
    "Apple Color Emoji"
}

#[cfg(target_os = "windows")]
fn emoji_family() -> &'static str {
    "Segoe UI Emoji"
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn emoji_family() -> &'static str {
    "Noto Color Emoji"
}

/// The terminal's fonts at the settings' size. The line height is the face's natural one, so the
/// cell is 17 px tall at 13 px (the spike's approved 17/13).
pub fn term_fonts(text: &mut TextCache, settings: &Settings) -> TermFonts {
    let size = settings.font_size as f32;
    let family = terminal_family(&settings.font);
    TermFonts {
        family: family.to_owned(),
        size,
        line_height: natural_line_height(&mut text.fonts, family, size),
        emoji_family: Some(emoji_family().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lune_termview::cell_metrics;

    /// B4: the bundled mono face at 13 px with its natural line height is a 17 px cell at scale 1
    /// and 34 at scale 2, as the spike's approved look has it.
    #[test]
    fn the_bundled_font_at_13_px_makes_17_and_34_px_cells() {
        let mut text = bundled_text_cache();
        let fonts = term_fonts(&mut text, &Settings::default());
        assert_eq!(fonts.family, MONO_FONT);
        assert_eq!(fonts.size, 13.0);
        let one = cell_metrics(&mut text.fonts, &fonts, 1.0);
        let two = cell_metrics(&mut text.fonts, &fonts, 2.0);
        assert_eq!(one.cell_h, 17, "line height {}", fonts.line_height);
        assert_eq!(two.cell_h, 34, "line height {}", fonts.line_height);
        assert!(one.cell_w > 0 && two.cell_w == 2 * one.cell_w);
    }

    #[test]
    fn a_saved_font_name_resolves_to_a_bundled_family_or_the_default() {
        assert_eq!(terminal_family(""), MONO_FONT);
        assert_eq!(terminal_family(MONO_FONT), MONO_FONT);
        assert_eq!(terminal_family("Comic Sans"), MONO_FONT);
        for f in TERMINAL_FONTS {
            assert_eq!(terminal_family(f), *f);
        }
    }

    #[test]
    fn the_four_bundled_faces_are_registered() {
        let text = bundled_text_cache();
        let families: Vec<String> = text
            .fonts
            .db()
            .faces()
            .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
            .collect();
        for want in [MONO_FONT, UI_FONT] {
            assert!(families.iter().any(|f| f == want), "{want} in {families:?}");
        }
        assert_eq!(text.fonts.db().faces().count(), 4);
    }

    #[test]
    fn system_fonts_load_once_and_the_time_is_reported() {
        let (text, took) = text_cache();
        assert!(text.fonts.db().faces().count() >= 4);
        eprintln!("load_system_fonts: {} ms", took.as_millis());
    }
}
