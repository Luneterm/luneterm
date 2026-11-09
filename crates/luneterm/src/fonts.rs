//! Fonts (plan M9 D16, M10 D9): the bundled faces, system fonts for fallback, and the terminal's
//! `TermFonts`.
//!
//! The terminal draws with RznMono (Regular, Bold, Italic, Bold Italic: the four styles termview
//! asks for, so no synthetic italic or bold). RznMono has no Nerd icons yet (its stage 4), so the
//! RobotoMono Nerd Font Mono faces stay bundled for the private-use code points it lacks, named as
//! `TermFonts::icon_family` so a system face cannot take them first. The toolkit's `TextCache` loads no
//! system font, so the app does, once: without them emoji and CJK would have no fallback face. One
//! `FontSystem` serves the UI text and the terminal (a second one would duplicate the faces).

use std::time::{Duration, Instant};

use lune_termview::{TermFonts, natural_line_height};
use lune_ui::text::TextCache;

use crate::settings::Settings;

/// The UI family (bundled RznSans).
pub const UI_FONT: &str = "RznSans";
/// The terminal family: the bundled RznMono.
pub const MONO_FONT: &str = "RznMono";
/// Lunegit's terminal font, still bundled: an option on the settings page and the fallback face
/// for the Nerd icons RznMono does not have yet.
pub const NERD_FONT: &str = "RobotoMono Nerd Font Mono";

/// The terminal fonts the settings page offers: the bundled families. A saved font name that is
/// not one of these (a font removed, a hand-edited file) draws as the first, [`MONO_FONT`].
pub const TERMINAL_FONTS: &[&str] = &[MONO_FONT, NERD_FONT];

/// The family the terminal draws with for the saved font name `saved` (empty is the default).
pub fn terminal_family(saved: &str) -> &'static str {
    TERMINAL_FONTS
        .iter()
        .copied()
        .find(|f| *f == saved)
        .unwrap_or(MONO_FONT)
}

const MONO_REGULAR: &[u8] = include_bytes!("../assets/RznMono-Regular.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../assets/RznMono-Bold.ttf");
const MONO_ITALIC: &[u8] = include_bytes!("../assets/RznMono-Italic.ttf");
const MONO_BOLD_ITALIC: &[u8] = include_bytes!("../assets/RznMono-BoldItalic.ttf");
const NERD_REGULAR: &[u8] = include_bytes!("../assets/RobotoMonoNerdFontMono-Regular.ttf");
const NERD_BOLD: &[u8] = include_bytes!("../assets/RobotoMonoNerdFontMono-Bold.ttf");
const UI_REGULAR: &[u8] = include_bytes!("../assets/RznSans-Regular.ttf");
const UI_MEDIUM: &[u8] = include_bytes!("../assets/RznSans-Medium.ttf");

/// The bundled faces in the order the `TextCache` loads them.
pub fn bundled() -> [&'static [u8]; 8] {
    [
        MONO_REGULAR,
        MONO_BOLD,
        MONO_ITALIC,
        MONO_BOLD_ITALIC,
        NERD_REGULAR,
        NERD_BOLD,
        UI_REGULAR,
        UI_MEDIUM,
    ]
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
        icon_family: Some(NERD_FONT.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lune_termview::{Shaper, cell_metrics};

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

    /// The faces' style as the db reports it (the app has no direct `fontdb` dependency).
    fn is_italic(style: &impl std::fmt::Debug) -> bool {
        format!("{style:?}") != "Normal"
    }

    #[test]
    fn the_bundled_faces_are_registered() {
        let text = bundled_text_cache();
        let faces: Vec<(String, u16, bool)> = text
            .fonts
            .db()
            .faces()
            .filter_map(|f| {
                let italic = is_italic(&f.style);
                f.families
                    .first()
                    .map(|(n, _)| (n.clone(), f.weight.0, italic))
            })
            .collect();
        let has = |family: &str, weight: u16, italic: bool| {
            faces
                .iter()
                .any(|f| *f == (family.to_owned(), weight, italic))
        };
        for (weight, italic) in [(400, false), (700, false), (400, true), (700, true)] {
            assert!(
                has(MONO_FONT, weight, italic),
                "{MONO_FONT} {weight} {italic} in {faces:?}"
            );
        }
        for weight in [400, 700] {
            assert!(
                has(NERD_FONT, weight, false),
                "{NERD_FONT} {weight} in {faces:?}"
            );
        }
        assert!(
            faces.iter().any(|f| f.0 == UI_FONT),
            "{UI_FONT} in {faces:?}"
        );
        assert_eq!(faces.len(), bundled().len());
        assert_eq!(faces.len(), 8);
    }

    /// The family and style of the face `glyph` was shaped with.
    fn face_of(text: &TextCache, glyph: &lune_termview::ShapedGlyph) -> (String, u16, bool) {
        let f = text
            .fonts
            .db()
            .face(glyph.font_id)
            .expect("shaped face is in the db");
        (f.families[0].0.clone(), f.weight.0, is_italic(&f.style))
    }

    /// The terminal's `TermFonts` for the bundled default at the default size.
    fn shaped(
        text: &mut TextCache,
        cluster: &str,
        bold: bool,
        italic: bool,
    ) -> lune_termview::Shaped {
        let fonts = term_fonts(text, &Settings::default());
        Shaper::new()
            .shape(&mut text.fonts, &fonts, cluster, bold, italic)
            .clone()
    }

    /// Bold and italic come from the real RznMono faces: the face is the requested weight and
    /// slant, and nothing is slanted synthetically (the A5 flag stays off).
    #[test]
    fn bold_and_italic_use_the_real_rznmono_faces() {
        let mut text = bundled_text_cache();
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            let s = shaped(&mut text, "a", bold, italic);
            assert_eq!(s.glyphs.len(), 1);
            let g = &s.glyphs[0];
            assert!(!g.italic, "bold {bold} italic {italic}: fake italic");
            assert_eq!(
                face_of(&text, g),
                (MONO_FONT.to_owned(), if bold { 700 } else { 400 }, italic),
                "bold {bold} italic {italic}"
            );
        }
    }

    /// A4: a private-use Nerd icon is not in RznMono; it draws from the bundled RobotoMono Nerd
    /// face through `icon_family`, with the icon's own glyph (not .notdef), and its
    /// advance is the cell's, so it is cell-contained (the golden `nerd_fallback` draws it).
    #[test]
    fn a_private_use_icon_falls_back_to_the_bundled_nerd_face() {
        let mut text = bundled_text_cache();
        let fonts = term_fonts(&mut text, &Settings::default());
        let cell = cell_metrics(&mut text.fonts, &fonts, 1.0);
        let w = shaped(&mut text, "W", false, false).advance;
        for cluster in ["\u{F015}", "\u{F07B}", "\u{E725}", "\u{F121}"] {
            for bold in [false, true] {
                let s = shaped(&mut text, cluster, bold, false);
                assert_eq!(s.glyphs.len(), 1, "{cluster:?}");
                let g = &s.glyphs[0];
                assert_ne!(g.glyph_id, 0, "{cluster:?} is .notdef");
                assert_eq!(face_of(&text, g).0, NERD_FONT, "{cluster:?} bold {bold}");
                // the Nerd face's advance is RznMono's to within a hundredth of a pixel, and the
                // cell (8 px, snapped up from 7.8) holds it
                assert!(
                    (s.advance - w).abs() < 0.05,
                    "{cluster:?} advance {}",
                    s.advance
                );
                assert!(
                    s.advance <= cell.cell_w as f32,
                    "{cluster:?} advance {}",
                    s.advance
                );
            }
        }
        // and ordinary text stays on RznMono
        let s = shaped(&mut text, "W", false, false);
        assert_eq!(face_of(&text, &s.glyphs[0]).0, MONO_FONT);
    }

    /// A4 in the app's real stack: the bundled faces first, then the system fonts, as the window
    /// loads them. Some system faces map the same private-use range (Wingdings and Webdings on a
    /// Mac, Segoe UI Symbol on Windows), so a pass means the bundled Nerd face wins, regular and bold. The instrument is
    /// checked in the same test: a cache built without the Nerd faces must not resolve to them.
    #[test]
    fn the_icon_fallback_holds_with_system_fonts_loaded() {
        let (mut text, _) = text_cache();
        for (bold, weight) in [(false, 400), (true, 700)] {
            for cluster in [
                "\u{F015}",
                "\u{F07B}",
                "\u{F013}",
                "\u{E725}",
                "\u{F121}",
                "\u{F0001}",
            ] {
                let s = shaped(&mut text, cluster, bold, false);
                assert_eq!(s.glyphs.len(), 1, "{cluster:?}");
                let (family, w, italic) = face_of(&text, &s.glyphs[0]);
                assert_eq!(family, NERD_FONT, "{cluster:?} bold {bold}");
                assert_eq!((w, italic), (weight, false), "{cluster:?} bold {bold}");
            }
        }
        // the same lookup without the bundled Nerd faces resolves elsewhere (or to nothing)
        let without: Vec<&[u8]> = bundled()
            .into_iter()
            .filter(|f| *f != NERD_REGULAR && *f != NERD_BOLD)
            .collect();
        let mut bare = TextCache::new(&without, UI_FONT, MONO_FONT);
        bare.fonts.db_mut().load_system_fonts();
        let s = shaped(&mut bare, "\u{F015}", false, false);
        assert!(
            s.glyphs.iter().all(|g| face_of(&bare, g).0 != NERD_FONT),
            "the check cannot tell the Nerd face from another"
        );
    }

    /// The premise of the toolkit's fit-on-base rule (termview D4 iii) still holds for the second
    /// bundled font, which the settings page offers: RobotoMono Nerd's marks advance like letters
    /// (its GDEF has no mark class), where RznMono's (no GDEF) have no advance. A saved
    /// RobotoMono name draws with it, so the rule must keep its input.
    #[test]
    fn marks_advance_in_robotomono_and_not_in_rznmono() {
        let mut text = bundled_text_cache();
        let mut settings = Settings::default();
        let mut width = |text: &mut TextCache, font: &str| {
            settings.font = font.to_owned();
            let fonts = term_fonts(text, &settings);
            Shaper::new()
                .shape(&mut text.fonts, &fonts, "a\u{301}\u{303}", false, false)
                .advance
        };
        let rzn = width(&mut text, MONO_FONT);
        let nerd = width(&mut text, NERD_FONT);
        assert!((rzn - 7.8).abs() < 0.02, "{rzn}");
        assert!((nerd - 15.6).abs() < 0.05, "{nerd}");
    }

    #[test]
    fn system_fonts_load_once_and_the_time_is_reported() {
        let (text, took) = text_cache();
        assert!(text.fonts.db().faces().count() >= 4);
        eprintln!("load_system_fonts: {} ms", took.as_millis());
    }
}
