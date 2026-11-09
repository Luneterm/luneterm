//! Text core: cell metrics from the font (plan decision D3) and a bounded shaping cache over
//! cosmic-text (D10). The app supplies the families; nothing here branches on the OS.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap,
};

use crate::contain::is_blank;
use crate::layout::CellMetrics;

/// Most shaped clusters kept before the cache is cleared and refilled.
pub const MAX_CACHED: usize = 4096;

/// The terminal font. Sizes are logical px.
#[derive(Debug, Clone, PartialEq)]
pub struct TermFonts {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    /// Family tried first for clusters containing U+FE0F (probe finding: a bare family falls
    /// back to a text-presentation face for `⚠\u{FE0F}`).
    pub emoji_family: Option<String>,
    /// Family for a private-use code point that `family` has no glyph for. The shaper's own
    /// fallback tries the platform's list first (Segoe UI Symbol on Windows maps part of the
    /// private-use area), so the first face that happens to map the code point would win.
    pub icon_family: Option<String>,
}

fn sane_scale(s: f32) -> f32 {
    if s.is_finite() && s > 0.0 { s } else { 1.0 }
}

fn sane_px(v: f32, default: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { default }
}

fn make_buffer(size: f32, line_height: f32) -> Buffer {
    let mut buffer = Buffer::new_empty(Metrics::new(size, line_height));
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, None);
    buffer
}

/// The Unicode private-use planes and area: what icon fonts map their glyphs to.
fn is_private_use(c: char) -> bool {
    matches!(u32::from(c), 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD)
}

/// Every glyph is a real glyph of a face of `family`, none came from the shaper's fallback.
fn all_from_family(fs: &FontSystem, glyphs: &[ShapedGlyph], family: &str) -> bool {
    !glyphs.is_empty()
        && glyphs.iter().all(|g| {
            g.glyph_id != 0
                && fs
                    .db()
                    .face(g.font_id)
                    .is_some_and(|f| f.families.iter().any(|(name, _)| name == family))
        })
}

/// Shape `text` in one line; returns the glyphs (font, glyph id, x, y relative to the baseline,
/// weight, fake-italic) and the line width, all logical px.
fn shape_line(
    fs: &mut FontSystem,
    buffer: &mut Buffer,
    attrs: &Attrs,
    text: &str,
) -> (Vec<ShapedGlyph>, f32) {
    // cosmic-text panics ("no default font found") when the database has no face at all
    if fs.db().faces().next().is_none() {
        return (Vec::new(), 0.0);
    }
    buffer.set_text(text, attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);
    let mut glyphs = Vec::new();
    let mut advance = 0.0_f32;
    for run in buffer.layout_runs() {
        advance = advance.max(run.line_w);
        for g in run.glyphs {
            glyphs.push(ShapedGlyph {
                font_id: g.font_id,
                glyph_id: g.glyph_id,
                x: g.x + g.font_size * g.x_offset,
                y: g.y - g.font_size * g.y_offset,
                pen_x: g.x,
                advance: g.w,
                weight: g.font_weight,
                italic: g.cache_key_flags.contains(CacheKeyFlags::FAKE_ITALIC),
            });
        }
    }
    (glyphs, advance)
}

/// Shapes "W" in `family`; returns the resolved face's (ascent, descent) in em, read with swash,
/// and the shaped advance in logical px. `None` when no face resolves.
fn w_face_em(
    fs: &mut FontSystem,
    family: &str,
    size: f32,
    line_height: f32,
) -> (Option<(f64, f64)>, f32) {
    let attrs = Attrs::new().family(Family::Name(family));
    let mut buffer = make_buffer(size, line_height);
    let (glyphs, advance) = shape_line(fs, &mut buffer, &attrs, "W");
    let face = glyphs.first().and_then(|g| {
        let font = fs.get_font(g.font_id, g.weight)?;
        let m = font.as_swash().metrics(&[]);
        let upem = f64::from(m.units_per_em);
        (upem > 0.0).then(|| (f64::from(m.ascent) / upem, f64::from(m.descent) / upem))
    });
    (face, advance)
}

/// The font's natural line height at `size` logical px: `(ascent + descent) / units_per_em ×
/// size` of the face cosmic-text resolves for `family`, unrounded. With no face at all it is
/// `1.2 × size`, `cell_metrics`'s fallback. A `size` that is not a positive finite number counts
/// as 13, as in `cell_metrics`; the result is always finite and above 0.
pub fn natural_line_height(fs: &mut FontSystem, family: &str, size: f32) -> f32 {
    // The cap keeps `1.2 * size` finite for any finite input.
    let size = sane_px(size, 13.0).min(1.0e6);
    let (face, _) = w_face_em(fs, family, size, size * 1.3);
    let height = match face {
        Some((asc_em, desc_em)) => ((asc_em + desc_em) * f64::from(size)) as f32,
        None => 1.2 * size,
    };
    if height.is_finite() && height > 0.0 {
        height
    } else {
        1.2 * size
    }
}

/// Cell metrics per D3, in physical px. `advance` is the shaped advance of "W" in
/// `fonts.family`. With no face at all, the cell is 0.6 em wide and 1.2 em tall (ascent 0.95 em,
/// descent 0.25 em); a bad `scale` counts as 1 and nothing panics.
pub fn cell_metrics(fs: &mut FontSystem, fonts: &TermFonts, scale: f32) -> CellMetrics {
    let scale = sane_scale(scale);
    let size = sane_px(fonts.size, 13.0);
    let line_height = sane_px(fonts.line_height, size * 1.3);
    let s = f64::from(scale);
    let size64 = f64::from(size);

    let (face, advance) = w_face_em(fs, &fonts.family, size, line_height);
    let (cell_w, asc_em, desc_em) = match face {
        Some((a, d)) if advance > 0.0 => ((f64::from(advance) * s).round().max(1.0), a, d),
        _ => ((0.6 * size64 * s).round().max(1.0), 0.95, 0.25),
    };

    let cell_h = (f64::from(line_height) * s).round().max(1.0);
    let char_h = ((asc_em + desc_em) * size64 * s).ceil().max(1.0);
    let char_top = ((cell_h - char_h) / 2.0 + 0.5).floor();
    let baseline = char_top + (asc_em * size64 * s).round();
    CellMetrics {
        cell_w: cell_w as u32,
        cell_h: cell_h as u32,
        char_h: char_h as u32,
        char_top: char_top as i32,
        baseline: baseline as i32,
        font_size: size,
        scale,
    }
}

/// One glyph of a shaped cluster. Positions are logical px: `x` from the cluster origin, `y`
/// from the baseline (0 for a plain glyph).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub x: f32,
    pub y: f32,
    /// The glyph's pen position (cosmic `LayoutGlyph.x`, logical px), before its own offset;
    /// `x` is `pen_x` plus the offset.
    pub pen_x: f32,
    /// The glyph's advance (cosmic `LayoutGlyph.w`, logical px).
    pub advance: f32,
    pub weight: Weight,
    /// Rasterize slanted (`CacheKeyFlags::FAKE_ITALIC`): italic was asked for and the face has
    /// no italic.
    pub italic: bool,
}

/// A shaped cluster.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shaped {
    pub glyphs: Vec<ShapedGlyph>,
    /// Logical px.
    pub advance: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct FontSig {
    family: String,
    size_bits: u32,
    line_height_bits: u32,
    emoji_family: Option<String>,
    icon_family: Option<String>,
}

impl FontSig {
    fn matches(&self, f: &TermFonts) -> bool {
        self.size_bits == f.size.to_bits()
            && self.line_height_bits == f.line_height.to_bits()
            && self.family == f.family
            && self.emoji_family == f.emoji_family
            && self.icon_family == f.icon_family
    }
}

/// Shaping cache keyed by (text, bold, italic). The cache is cleared when it holds
/// [`MAX_CACHED`] entries, by [`Shaper::clear`], and whenever the fonts passed in change.
pub struct Shaper {
    // indexed by `bold as usize | (italic as usize) << 1`
    cache: [HashMap<String, Shaped>; 4],
    len: usize,
    sig: Option<FontSig>,
    buffer: Option<Buffer>,
    empty: Shaped,
}

impl std::fmt::Debug for Shaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shaper")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl Default for Shaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaper {
    pub fn new() -> Shaper {
        Shaper {
            cache: Default::default(),
            len: 0,
            sig: None,
            buffer: None,
            empty: Shaped::default(),
        }
    }

    /// Drop every cached cluster.
    pub fn clear(&mut self) {
        for m in &mut self.cache {
            m.clear();
        }
        self.len = 0;
    }

    /// Clusters currently cached.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Shape one cluster. Blank text ("", " ", "\0") has no glyphs and zero advance.
    pub fn shape(
        &mut self,
        fs: &mut FontSystem,
        fonts: &TermFonts,
        text: &str,
        bold: bool,
        italic: bool,
    ) -> &Shaped {
        if is_blank(text) {
            return &self.empty;
        }
        if !self.sig.as_ref().is_some_and(|s| s.matches(fonts)) {
            self.clear();
            self.sig = Some(FontSig {
                family: fonts.family.clone(),
                size_bits: fonts.size.to_bits(),
                line_height_bits: fonts.line_height.to_bits(),
                emoji_family: fonts.emoji_family.clone(),
                icon_family: fonts.icon_family.clone(),
            });
            self.buffer = None;
        }
        let slot = usize::from(bold) | (usize::from(italic) << 1);
        if !self.cache[slot].contains_key(text) {
            if self.len >= MAX_CACHED {
                self.clear();
            }
            let size = sane_px(fonts.size, 13.0);
            let line_height = sane_px(fonts.line_height, size * 1.3);
            let buffer = self
                .buffer
                .get_or_insert_with(|| make_buffer(size, line_height));
            // An icon is never an emoji-font glyph, whatever selector follows it.
            let private_use = text.chars().any(is_private_use);
            let family = match &fonts.emoji_family {
                Some(e) if text.contains('\u{FE0F}') && !private_use => e.as_str(),
                _ => fonts.family.as_str(),
            };
            let attrs_of = |family| {
                Attrs::new()
                    .family(Family::Name(family))
                    .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
                    .style(if italic { Style::Italic } else { Style::Normal })
            };
            let (mut glyphs, mut advance) = shape_line(fs, buffer, &attrs_of(family), text);
            // The icon family replaces only what the terminal's own family cannot draw.
            if let Some(icon) = fonts.icon_family.as_deref().filter(|i| *i != family)
                && private_use
                && !all_from_family(fs, &glyphs, family)
            {
                (glyphs, advance) = shape_line(fs, buffer, &attrs_of(icon), text);
            }
            self.cache[slot].insert(text.to_owned(), Shaped { glyphs, advance });
            self.len += 1;
        }
        &self.cache[slot][text]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::SwashCache;
    use cosmic_text::SwashContent;

    const REGULAR: &[u8] = include_bytes!("../tests/fonts/RznMono-Regular.subset.ttf");
    const BOLD: &[u8] = include_bytes!("../tests/fonts/RznMono-Bold.subset.ttf");
    const SYMBOLS: &[u8] = include_bytes!("../tests/fonts/NotoSansSymbols2.subset.ttf");
    const TWEMOJI: &[u8] = include_bytes!("../tests/fonts/TwemojiLunegit.subset.ttf");

    struct Rig {
        fs: FontSystem,
        mono: String,
        symbols: String,
        twemoji: String,
    }

    fn family_of(db: &fontdb::Database, id: fontdb::ID) -> String {
        db.face(id).expect("face").families[0].0.clone()
    }

    fn rig() -> Rig {
        let mut db = fontdb::Database::new();
        let mono = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(REGULAR)));
        db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(BOLD)));
        let symbols = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(SYMBOLS)));
        let twemoji = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(TWEMOJI)));
        assert_eq!(db.len(), 4);
        Rig {
            mono: family_of(&db, mono[0]),
            symbols: family_of(&db, symbols[0]),
            twemoji: family_of(&db, twemoji[0]),
            fs: FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
        }
    }

    fn fonts(r: &Rig, emoji: bool) -> TermFonts {
        TermFonts {
            family: r.mono.clone(),
            size: 13.0,
            line_height: 17.0,
            emoji_family: emoji.then(|| r.twemoji.clone()),
            icon_family: None,
        }
    }

    fn face_family(r: &Rig, id: fontdb::ID) -> String {
        family_of(r.fs.db(), id)
    }

    #[test]
    fn metrics_at_scale_2_and_1() {
        let mut r = rig();
        let f = fonts(&r, false);
        let m = cell_metrics(&mut r.fs, &f, 2.0);
        assert_eq!(
            (m.cell_w, m.cell_h, m.char_h, m.char_top, m.baseline),
            (16, 34, 35, 0, 27),
            "{m:?}"
        );
        let m = cell_metrics(&mut r.fs, &f, 1.0);
        assert_eq!(
            (m.cell_w, m.cell_h, m.char_h, m.char_top, m.baseline),
            (8, 17, 18, 0, 14),
            "{m:?}"
        );
        assert_eq!((m.font_size, m.scale), (13.0, 1.0));
    }

    #[test]
    fn natural_line_height_is_the_swash_formula_for_the_face() {
        let mut r = rig();
        // The test's own reading of the committed font: parse the face and use swash directly.
        let id =
            r.fs.db()
                .faces()
                .find(|f| f.families[0].0 == r.mono && f.weight == fontdb::Weight::NORMAL)
                .expect("regular face")
                .id;
        let m =
            r.fs.get_font(id, Weight::NORMAL)
                .expect("font")
                .as_swash()
                .metrics(&[]);
        let upem = f64::from(m.units_per_em);
        for size in [13.0_f32, 12.0, 20.5, 1.0] {
            let want =
                ((f64::from(m.ascent) + f64::from(m.descent)) / upem * f64::from(size)) as f32;
            let got = natural_line_height(&mut r.fs, &r.mono.clone(), size);
            assert_eq!(got, want, "size {size}");
        }
        // Not rounded to whole px, and what cell_metrics' char_h at 1x rounds up from.
        let got = natural_line_height(&mut r.fs, &r.mono.clone(), 13.0);
        assert_ne!(got, got.round(), "{got}");
        let f = fonts(&r, false);
        let m1 = cell_metrics(&mut r.fs, &f, 1.0);
        assert_eq!(got.ceil() as u32, m1.char_h);
    }

    #[test]
    fn natural_line_height_without_a_face_is_1_2_em() {
        let mut fs =
            FontSystem::new_with_locale_and_db("en-US".to_owned(), fontdb::Database::new());
        assert_eq!(natural_line_height(&mut fs, "Nope", 13.0), 1.2 * 13.0);
        assert_eq!(natural_line_height(&mut fs, "Nope", 20.0), 1.2 * 20.0);
    }

    #[test]
    fn natural_line_height_sanitizes_the_size() {
        let mut r = rig();
        let mono = r.mono.clone();
        let default = natural_line_height(&mut r.fs, &mono, 13.0);
        for bad in [0.0, -3.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(natural_line_height(&mut r.fs, &mono, bad), default, "{bad}");
        }
        let mut empty =
            FontSystem::new_with_locale_and_db("en-US".to_owned(), fontdb::Database::new());
        for bad in [0.0, f32::NAN, f32::MAX] {
            let h = natural_line_height(&mut empty, "Nope", bad);
            assert!(h.is_finite() && h > 0.0, "{bad}: {h}");
        }
        let huge = natural_line_height(&mut r.fs, &mono, f32::MAX);
        assert!(huge.is_finite() && huge > 0.0);
    }

    #[test]
    fn metrics_without_any_face_do_not_panic() {
        let mut fs =
            FontSystem::new_with_locale_and_db("en-US".to_owned(), fontdb::Database::new());
        let f = TermFonts {
            family: "Nope".to_owned(),
            size: 13.0,
            line_height: 17.0,
            emoji_family: None,
            icon_family: None,
        };
        let m = cell_metrics(&mut fs, &f, 2.0);
        // round(0.6 * 13 * 2) = 16; ceil(1.2 * 13 * 2) = 32
        assert_eq!((m.cell_w, m.cell_h, m.char_h), (16, 34, 32));
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let m = cell_metrics(&mut fs, &f, bad);
            assert_eq!(m.scale, 1.0);
            assert_eq!((m.cell_w, m.cell_h), (8, 17));
        }
    }

    #[test]
    fn shapes_w_from_the_mono_face() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let s = sh.shape(&mut r.fs, &f, "W", false, false).clone();
        assert_eq!(s.glyphs.len(), 1);
        assert_eq!(face_family(&r, s.glyphs[0].font_id), r.mono);
        assert!((s.advance - 7.8).abs() < 0.01, "{}", s.advance);
    }

    #[test]
    fn combining_mark_stays_in_the_mono_face_and_one_cell() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let s = sh.shape(&mut r.fs, &f, "e\u{301}", false, false).clone();
        assert!(!s.glyphs.is_empty());
        for g in &s.glyphs {
            assert_eq!(face_family(&r, g.font_id), r.mono);
        }
        assert!((s.advance - 7.8).abs() < 0.01, "{}", s.advance);
    }

    #[test]
    fn marks_in_rznmono_have_no_advance_and_stay_on_the_base() {
        // RznMono has no GDEF table, so HarfBuzz classes its marks by Unicode category and
        // zeroes their advance: the cluster is one cell wide and the marks draw on the base.
        // (RobotoMono Nerd's GDEF has no mark class, so there every mark advances like a letter;
        // that premise of D4 (iii) is pinned for the app's second bundled font in
        // the app's own font tests, and the fit-on-base rule stays for it.) HarfBuzz composes a +
        // U+0301 into the font's single "a acute" glyph, so the cluster is [a acute, tilde].
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let s = sh
            .shape(&mut r.fs, &f, "a\u{301}\u{303}", false, false)
            .clone();
        assert_eq!(s.glyphs.len(), 2, "{s:?}");
        let (base, mark) = (s.glyphs[0], s.glyphs[1]);
        assert!((base.advance - 7.8).abs() < 0.01, "{}", base.advance);
        assert_eq!(mark.advance, 0.0, "{mark:?}");
        assert_eq!(base.pen_x, 0.0);
        assert!((mark.pen_x - 7.8).abs() < 0.01, "{}", mark.pen_x);
        assert!((s.advance - 7.8).abs() < 0.02, "{}", s.advance);
    }

    #[test]
    fn emoji_comes_from_the_emoji_family() {
        let mut r = rig();
        let f = fonts(&r, true);
        let mut sh = Shaper::new();
        let s = sh.shape(&mut r.fs, &f, "\u{1F3A8}", false, false).clone();
        assert!(!s.glyphs.is_empty());
        assert_eq!(face_family(&r, s.glyphs[0].font_id), r.twemoji);
    }

    #[test]
    fn vs16_prefers_the_emoji_family_and_bare_falls_back_to_symbols() {
        let mut r = rig();
        let mut sh = Shaper::new();
        let with = fonts(&r, true);
        let s = sh
            .shape(&mut r.fs, &with, "\u{26A0}\u{FE0F}", false, false)
            .clone();
        assert_eq!(face_family(&r, s.glyphs[0].font_id), r.twemoji);
        let without = fonts(&r, false);
        let s = sh
            .shape(&mut r.fs, &without, "\u{26A0}\u{FE0F}", false, false)
            .clone();
        assert_eq!(face_family(&r, s.glyphs[0].font_id), r.symbols);
    }

    // A2: fallback reaches a face outside the requested family in a test-only db.
    #[test]
    fn fallback_reaches_symbols_for_a_star() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let s = sh.shape(&mut r.fs, &f, "\u{2605}", false, false).clone();
        assert_eq!(face_family(&r, s.glyphs[0].font_id), r.symbols);
    }

    /// The Windows case on every host: a fallback list like the platform's names a face that maps
    /// the same private-use code points as the bundled icon face, and comes before the walk over
    /// every face. The decoy is the icon face itself under another name, so its coverage is the
    /// icon face's coverage.
    struct IconRig {
        fs: FontSystem,
        mono: String,
        icons: String,
    }

    const DECOY: &str = "Decoy Symbols";

    struct DecoyFirst;

    impl cosmic_text::Fallback for DecoyFirst {
        fn common_fallback(&self) -> &[&'static str] {
            &[DECOY]
        }
        fn forbidden_fallback(&self) -> &[&'static str] {
            &[]
        }
        fn script_fallback(
            &self,
            _script: unicode_script::Script,
            _locale: &str,
        ) -> &[&'static str] {
            &[]
        }
    }

    /// The mono faces, `icon_faces` (the icon family under test) and a decoy copy of each of those
    /// that comes first in the fallback list.
    fn rig_with(icon_faces: &[&'static [u8]]) -> IconRig {
        let mut db = fontdb::Database::new();
        let mono = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(REGULAR)));
        db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(BOLD)));
        let mut icons = Vec::new();
        for data in icon_faces {
            icons.extend(db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(*data))));
        }
        for id in &icons {
            let mut decoy = db.face(*id).expect("face").clone();
            decoy.families[0].0 = DECOY.to_owned();
            db.push_face_info(decoy);
        }
        IconRig {
            mono: family_of(&db, mono[0]),
            icons: family_of(&db, icons[0]),
            fs: FontSystem::new_with_locale_and_db_and_fallback("en-US".to_owned(), db, DecoyFirst),
        }
    }

    fn icon_rig() -> IconRig {
        const ICONS_REGULAR: &[u8] =
            include_bytes!("../tests/fonts/RobotoMonoNerdFontMono-Regular.pua.subset.ttf");
        const ICONS_BOLD: &[u8] =
            include_bytes!("../tests/fonts/RobotoMonoNerdFontMono-Bold.pua.subset.ttf");
        rig_with(&[ICONS_REGULAR, ICONS_BOLD])
    }

    /// A rig of its own: the symbols face as the icon family. It maps U+2605, which is not
    /// private-use, and no private-use code point, so the icon cases keep `icon_rig`.
    fn symbols_rig() -> IconRig {
        rig_with(&[SYMBOLS])
    }

    fn icon_fonts(family: &str, icon_family: Option<&str>, emoji: Option<&str>) -> TermFonts {
        TermFonts {
            family: family.to_owned(),
            size: 13.0,
            line_height: 17.0,
            emoji_family: emoji.map(str::to_owned),
            icon_family: icon_family.map(str::to_owned),
        }
    }

    fn only_face(r: &IconRig, s: &Shaped) -> (String, u16) {
        assert_eq!(s.glyphs.len(), 1);
        assert_ne!(s.glyphs[0].glyph_id, 0, ".notdef");
        let f = r.fs.db().face(s.glyphs[0].font_id).expect("face");
        (f.families[0].0.clone(), f.weight.0)
    }

    #[test]
    fn a_listed_face_wins_a_private_use_icon_without_an_icon_family() {
        let mut r = icon_rig();
        let f = icon_fonts(&r.mono.clone(), None, None);
        let s = Shaper::new()
            .shape(&mut r.fs, &f, "\u{F013}", false, false)
            .clone();
        assert_eq!(only_face(&r, &s).0, DECOY);
    }

    #[test]
    fn a_private_use_icon_the_font_lacks_comes_from_the_icon_family() {
        let mut r = icon_rig();
        let f = icon_fonts(&r.mono.clone(), Some(&r.icons.clone()), None);
        let mut sh = Shaper::new();
        for text in ["\u{F013}", "\u{F015}", "\u{F07B}"] {
            for (bold, weight) in [(false, 400), (true, 700)] {
                let s = sh.shape(&mut r.fs, &f, text, bold, false).clone();
                assert_eq!(only_face(&r, &s), (r.icons.clone(), weight), "{text:?}");
            }
        }
    }

    /// The emoji family here has the icon (it is the decoy), so a selector that did send the icon
    /// there would draw it from the decoy and the icon family would never be asked.
    #[test]
    fn an_emoji_selector_does_not_send_an_icon_to_the_emoji_family() {
        let mut r = icon_rig();
        let f = icon_fonts(&r.mono.clone(), Some(&r.icons.clone()), Some(DECOY));
        let s = Shaper::new()
            .shape(&mut r.fs, &f, "\u{F013}\u{FE0F}", false, false)
            .clone();
        let faces: Vec<_> = s
            .glyphs
            .iter()
            .map(|g| family_of(r.fs.db(), g.font_id))
            .collect();
        assert_eq!(faces.first(), Some(&r.icons), "{faces:?}");
    }

    /// The icon family only fills a gap: a font that has the glyph keeps it.
    #[test]
    fn the_icon_family_does_not_take_what_the_font_has() {
        let mut r = icon_rig();
        // the decoy has the icon, as a RznMono with its own icons would
        let f = icon_fonts(DECOY, Some(&r.icons.clone()), None);
        let s = Shaper::new()
            .shape(&mut r.fs, &f, "\u{F013}", false, false)
            .clone();
        assert_eq!(only_face(&r, &s).0, DECOY);
    }

    /// Text that is not private-use is left to the ordinary fallback, even when the icon family
    /// could draw it: here the icon family is the symbols face, which has the star, and the
    /// fallback list reaches a decoy copy of it first.
    #[test]
    fn the_icon_family_does_not_take_text_that_is_not_private_use() {
        let mut r = symbols_rig();
        let f = icon_fonts(&r.mono.clone(), Some(&r.icons.clone()), None);
        let s = Shaper::new()
            .shape(&mut r.fs, &f, "\u{2605}", false, false)
            .clone();
        assert_eq!(only_face(&r, &s).0, DECOY);
    }

    #[test]
    fn the_private_use_ranges_are_the_unicode_ones() {
        for c in [
            '\u{E000}',
            '\u{F013}',
            '\u{F8FF}',
            '\u{F0000}',
            '\u{FFFFD}',
            '\u{100000}',
            '\u{10FFFD}',
        ] {
            assert!(is_private_use(c), "{c:?}");
        }
        for c in [
            '\u{D7FF}',
            '\u{F900}',
            '\u{EFFFF}',
            '\u{FFFFE}',
            '\u{10FFFE}',
        ] {
            assert!(!is_private_use(c), "{c:?}");
        }
    }

    // A3: the Twemoji subset still rasterizes as a colour bitmap.
    #[test]
    fn twemoji_subset_rasterizes_as_colour() {
        let mut r = rig();
        let f = fonts(&r, true);
        let mut sh = Shaper::new();
        let s = sh.shape(&mut r.fs, &f, "\u{1F3A8}", false, false).clone();
        let g = s.glyphs[0];
        let (key, _, _) = cosmic_text::CacheKey::new(
            g.font_id,
            g.glyph_id,
            26.0,
            (0.0, 0.0),
            g.weight,
            CacheKeyFlags::empty(),
        );
        let img = SwashCache::new()
            .get_image_uncached(&mut r.fs, key)
            .expect("image");
        assert!(matches!(img.content, SwashContent::Color));
    }

    // A5: cosmic-text sets FAKE_ITALIC itself when the family has no italic face.
    #[test]
    fn italic_without_an_italic_face_is_fake() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        assert!(sh.shape(&mut r.fs, &f, "a", false, true).glyphs[0].italic);
        assert!(!sh.shape(&mut r.fs, &f, "a", false, false).glyphs[0].italic);
    }

    #[test]
    fn bold_picks_the_bold_face() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let reg = sh.shape(&mut r.fs, &f, "a", false, false).glyphs[0];
        let bold = sh.shape(&mut r.fs, &f, "a", true, false).glyphs[0];
        assert_ne!(reg.font_id, bold.font_id);
        assert_eq!(bold.weight, Weight::BOLD);
    }

    #[test]
    fn blank_text_has_no_glyphs() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        for t in ["", " ", "\0"] {
            let s = sh.shape(&mut r.fs, &f, t, false, false);
            assert!(s.glyphs.is_empty() && s.advance == 0.0, "{t:?}");
        }
        assert!(sh.is_empty());
    }

    #[test]
    fn same_input_twice_is_served_from_the_cache() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        let a = sh.shape(&mut r.fs, &f, "W", false, false).clone();
        assert_eq!(sh.len(), 1);
        let b = sh.shape(&mut r.fs, &f, "W", false, false).clone();
        assert_eq!(sh.len(), 1);
        assert_eq!(a, b);
        sh.shape(&mut r.fs, &f, "W", true, false);
        assert_eq!(sh.len(), 2);
        sh.clear();
        assert_eq!(sh.len(), 0);
    }

    #[test]
    fn cache_is_bounded() {
        let mut r = rig();
        let f = fonts(&r, false);
        let mut sh = Shaper::new();
        for i in 0..(MAX_CACHED as u32 + 10) {
            let t = format!("{}", char::from_u32(0x2500 + (i % 0x60)).expect("char"));
            let t = format!("{t}{i}");
            sh.shape(&mut r.fs, &f, &t, false, false);
            assert!(sh.len() <= MAX_CACHED);
        }
    }

    #[test]
    fn changing_the_fonts_drops_stale_entries() {
        let mut r = rig();
        let mut f = fonts(&r, false);
        let mut sh = Shaper::new();
        let a = sh.shape(&mut r.fs, &f, "W", false, false).advance;
        f.size = 26.0;
        f.line_height = 34.0;
        let b = sh.shape(&mut r.fs, &f, "W", false, false).advance;
        assert!((b - 2.0 * a).abs() < 0.02, "{a} {b}");
        assert_eq!(sh.len(), 1);
    }
}
