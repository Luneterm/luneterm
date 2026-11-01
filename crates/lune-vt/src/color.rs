//! Port of xterm.js src/common/Color.ts @ f447274 (tests: Color.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5):
//!  - TypeScript namespaces are modules: [`channels`], [`color`], [`css`], [`rgb`], [`rgba`].
//!    The module-level scratch variables (`$r $g $b $a`) are locals.
//!  - JS numbers used as bit fields are `u32`; `Math.round` is [`js_round`] (half rounds up,
//!    unlike `f64::round`).
//!  - `css::to_color`: only the formats that work on Node are ported, `#rgb[a]`, `#rrggbb[aa]`,
//!    `rgb()` and `rgba()`. The canvas fallback (`Color.ts:119-135`, `:196-217`) is
//!    `// replaced:` -- no canvas here -- so every other string is `Err` where upstream throws
//!    (`:188-189`).
//!  - The regular expressions are hand-written matchers over UTF-16 code units (JS string
//!    indices), so nothing slices a `&str` at a computed byte offset.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::string_slice,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use std::fmt;
use std::sync::LazyLock;

/// A colour: its CSS string and RGBA as `0xRRGGBBAA` (upstream `IColor`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IColor {
    pub css: String,
    /// 32-bit int with rgba in each byte.
    pub rgba: u32,
}

/// Red, green, blue (upstream `IColorRGB`).
pub type ColorRgb = [u32; 3];

pub static NULL_COLOR: LazyLock<IColor> = LazyLock::new(|| IColor {
    css: "#00000000".to_string(),
    rgba: 0,
});

/// What `css::to_color` rejects where upstream throws `css.toColor: Unsupported css format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedCssFormat;

impl fmt::Display for UnsupportedCssFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("css.toColor: Unsupported css format")
    }
}

impl std::error::Error for UnsupportedCssFormat {}

/// `Math.round`: halves round toward +infinity (`f64::round` rounds them away from zero).
fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `bg + Math.round((fg - bg) * a)` for bytes and `a` in 0..=1: the result is within 0..=255.
fn blend_channel(bg: u32, fg: u32, a: f64) -> u32 {
    let delta = js_round((f64::from(fg) - f64::from(bg)) * a);
    // Both operands are small integers (|delta| <= 255), so the i64 sum is exact.
    (i64::from(bg) + delta as i64) as u32
}

/// Helper functions where the source type is "channels" (individual color channels as numbers).
pub mod channels {
    use super::{IColor, to_padded_hex};

    pub fn to_css(r: u32, g: u32, b: u32, a: Option<u32>) -> String {
        match a {
            Some(a) => format!(
                "#{}{}{}{}",
                to_padded_hex(r),
                to_padded_hex(g),
                to_padded_hex(b),
                to_padded_hex(a)
            ),
            None => format!(
                "#{}{}{}",
                to_padded_hex(r),
                to_padded_hex(g),
                to_padded_hex(b)
            ),
        }
    }

    /// `a` defaults to 0xFF.
    pub fn to_rgba(r: u32, g: u32, b: u32, a: Option<u32>) -> u32 {
        // Note: The aggregated number is RGBA32 (BE), thus needs to be converted to ABGR32
        // on LE systems, before it can be used for direct 32-bit buffer writes.
        (r << 24) | (g << 16) | (b << 8) | a.unwrap_or(0xFF)
    }

    pub fn to_color(r: u32, g: u32, b: u32, a: Option<u32>) -> IColor {
        IColor {
            css: to_css(r, g, b, a),
            rgba: to_rgba(r, g, b, a),
        }
    }
}

/// Helper functions where the source type is `IColor`.
// The name is upstream's namespace (`color.blend(...)`), kept so call sites read alike.
#[allow(clippy::module_inception)]
pub mod color {
    use super::{ColorRgb, IColor, blend_channel, channels, js_round, rgba};

    pub fn blend(bg: &IColor, fg: &IColor) -> IColor {
        let a = f64::from(fg.rgba & 0xFF) / 255.0;
        if a == 1.0 {
            return fg.clone();
        }
        let fg_r = (fg.rgba >> 24) & 0xFF;
        let fg_g = (fg.rgba >> 16) & 0xFF;
        let fg_b = (fg.rgba >> 8) & 0xFF;
        let bg_r = (bg.rgba >> 24) & 0xFF;
        let bg_g = (bg.rgba >> 16) & 0xFF;
        let bg_b = (bg.rgba >> 8) & 0xFF;
        let r = blend_channel(bg_r, fg_r, a);
        let g = blend_channel(bg_g, fg_g, a);
        let b = blend_channel(bg_b, fg_b, a);
        IColor {
            css: channels::to_css(r, g, b, None),
            rgba: channels::to_rgba(r, g, b, None),
        }
    }

    pub fn is_opaque(color: &IColor) -> bool {
        (color.rgba & 0xFF) == 0xFF
    }

    pub fn ensure_contrast_ratio(bg: &IColor, fg: &IColor, ratio: f64) -> Option<IColor> {
        let result = rgba::ensure_contrast_ratio(bg.rgba, fg.rgba, ratio)?;
        Some(channels::to_color(
            (result >> 24) & 0xFF,
            (result >> 16) & 0xFF,
            (result >> 8) & 0xFF,
            None,
        ))
    }

    pub fn opaque(color: &IColor) -> IColor {
        let rgba_color = color.rgba | 0xFF;
        let [r, g, b, _] = rgba::to_channels(rgba_color);
        IColor {
            css: channels::to_css(r, g, b, None),
            rgba: rgba_color,
        }
    }

    /// `opacity` is expected in 0..=1; a value outside it saturates the scaled alpha to the
    /// `u32` range (upstream formats the out-of-range number as is).
    pub fn opacity(color: &IColor, opacity: f64) -> IColor {
        let a = js_round(opacity * f64::from(0xFF_u8)) as u32;
        let [r, g, b, _] = rgba::to_channels(color.rgba);
        IColor {
            css: channels::to_css(r, g, b, Some(a)),
            rgba: channels::to_rgba(r, g, b, Some(a)),
        }
    }

    pub fn multiply_opacity(color: &IColor, factor: f64) -> IColor {
        let a = color.rgba & 0xFF;
        opacity(color, (f64::from(a) * factor) / f64::from(0xFF_u8))
    }

    pub fn to_color_rgb(color: &IColor) -> ColorRgb {
        [
            (color.rgba >> 24) & 0xFF,
            (color.rgba >> 16) & 0xFF,
            (color.rgba >> 8) & 0xFF,
        ]
    }
}

/// Helper functions where the source type is "css" (string: '#rgb', '#rgba', '#rrggbb',
/// '#rrggbbaa').
pub mod css {
    use super::{IColor, UnsupportedCssFormat, channels, js_round};

    /// Converts a css string to an `IColor`. The ideal format to use is `#rrggbb[aa]` as it's the
    /// fastest to parse.
    ///
    /// Only `#rgb[a]`, `#rrggbb[aa]`, `rgb()` and `rgba()` formats are supported (what upstream
    /// supports in a Node environment); everything else is `Err` (upstream throws).
    ///
    /// Strings that contain a `#` followed by three hex digits anywhere are routed by their
    /// UTF-16 length, as upstream, and the 7 and 9 length forms read the number with
    /// `parseInt(css.slice(1), 16)` semantics, so `"#abcdeg"` gives `0xabcde` like upstream.
    // LUNETERM: the 4 and 5 length forms build their channels with `parseInt(c + c, 16)`, which
    // is NaN for a non-hex-digit and gives upstream the nonsense css `"#NaNNaNNaN"`; here that
    // is `Err`, since a NaN channel has no `u32` representation.
    pub fn to_color(css: &str) -> Result<IColor, UnsupportedCssFormat> {
        let units: Vec<u16> = css.encode_utf16().collect();

        // Formats: #rgb[a] and #rrggbb[aa]
        if has_hash_hex_run(&units) {
            match units.len() {
                4 => {
                    // #rgb
                    let r = doubled_hex_digit(&units, 1)?;
                    let g = doubled_hex_digit(&units, 2)?;
                    let b = doubled_hex_digit(&units, 3)?;
                    return Ok(channels::to_color(r, g, b, None));
                }
                5 => {
                    // #rgba
                    let r = doubled_hex_digit(&units, 1)?;
                    let g = doubled_hex_digit(&units, 2)?;
                    let b = doubled_hex_digit(&units, 3)?;
                    let a = doubled_hex_digit(&units, 4)?;
                    return Ok(channels::to_color(r, g, b, Some(a)));
                }
                7 => {
                    // #rrggbb
                    let value = units
                        .get(1..)
                        .and_then(parse_int_hex)
                        .map_or(0, |v| v as u32);
                    return Ok(IColor {
                        css: css.to_string(),
                        rgba: (value << 8) | 0xFF,
                    });
                }
                9 => {
                    // #rrggbbaa
                    let value = units
                        .get(1..)
                        .and_then(parse_int_hex)
                        .map_or(0, |v| v as u32);
                    return Ok(IColor {
                        css: css.to_string(),
                        rgba: value,
                    });
                }
                _ => {}
            }
        }

        // Formats: rgb() or rgba()
        if let Some([r, g, b, a]) =
            (0..units.len()).find_map(|start| match_rgba_function(&units, start))
        {
            return Ok(channels::to_color(r, g, b, Some(a)));
        }

        // The canvas-based parsing of every other css color is not ported.
        Err(UnsupportedCssFormat)
    }

    fn unit_is(units: &[u16], index: usize, ch: char) -> bool {
        units.get(index).copied().map(u32::from) == Some(u32::from(ch))
    }

    fn hex_digit(unit: u16) -> Option<u32> {
        char::from_u32(u32::from(unit)).and_then(|c| c.to_digit(16))
    }

    fn decimal_digit(unit: u16) -> Option<u32> {
        char::from_u32(u32::from(unit)).and_then(|c| c.to_digit(10))
    }

    /// JS `\s` / `String.prototype.trim` whitespace.
    fn is_js_space(unit: u16) -> bool {
        char::from_u32(u32::from(unit))
            .is_some_and(|c| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}')
    }

    /// `css.match(/#[\da-f]{3,8}/i)`: a `#` followed by at least three hex digits, anywhere.
    fn has_hash_hex_run(units: &[u16]) -> bool {
        (0..units.len()).any(|i| {
            unit_is(units, i, '#')
                && (1..=3).all(|k| units.get(i + k).and_then(|&u| hex_digit(u)).is_some())
        })
    }

    /// `parseInt(css.slice(index, index + 1).repeat(2), 16)`; `Err` where upstream gets NaN.
    fn doubled_hex_digit(units: &[u16], index: usize) -> Result<u32, UnsupportedCssFormat> {
        units
            .get(index)
            .and_then(|&u| hex_digit(u))
            .map(|digit| digit * 16 + digit)
            .ok_or(UnsupportedCssFormat)
    }

    /// `parseInt(text, 16)` for text of at most a few units: leading whitespace, an optional
    /// sign, an optional `0x`, then the longest run of hex digits. `None` is NaN. The value is
    /// the integer (it may be negative); callers convert with `as u32` for `ToUint32`.
    fn parse_int_hex(units: &[u16]) -> Option<i64> {
        let mut rest = units;
        while let Some((&first, tail)) = rest.split_first() {
            if is_js_space(first) {
                rest = tail;
            } else {
                break;
            }
        }
        let mut negative = false;
        if let Some((&first, tail)) = rest.split_first() {
            if unit_is(&[first], 0, '-') {
                negative = true;
                rest = tail;
            } else if unit_is(&[first], 0, '+') {
                rest = tail;
            }
        }
        if unit_is(rest, 0, '0') && (unit_is(rest, 1, 'x') || unit_is(rest, 1, 'X')) {
            rest = rest.get(2..).unwrap_or_default();
        }
        let mut value: i64 = 0;
        let mut any = false;
        for &unit in rest {
            let Some(digit) = hex_digit(unit) else { break };
            any = true;
            value = value.saturating_mul(16).saturating_add(i64::from(digit));
        }
        any.then_some(if negative { -value } else { value })
    }

    /// The `rgba?\(...\)` regular expression anchored at `start`; returns r, g, b and the alpha
    /// already scaled to 0..255 (`Math.round(alpha * 0xFF)`, 255 when absent).
    fn match_rgba_function(units: &[u16], start: usize) -> Option<[u32; 4]> {
        let skip_space = |mut at: usize| {
            while units.get(at).is_some_and(|&u| is_js_space(u)) {
                at += 1;
            }
            at
        };
        // `\d{1,3}`: returns the value and the next position.
        let channel = |at: usize| -> Option<(u32, usize)> {
            let mut value = 0;
            let mut end = at;
            while end < at + 3 {
                match units.get(end).and_then(|&u| decimal_digit(u)) {
                    Some(d) => {
                        value = value * 10 + d;
                        end += 1;
                    }
                    None => break,
                }
            }
            (end > at).then_some((value, end))
        };

        if !(unit_is(units, start, 'r')
            && unit_is(units, start + 1, 'g')
            && unit_is(units, start + 2, 'b'))
        {
            return None;
        }
        let mut at = start + 3;
        if unit_is(units, at, 'a') {
            at += 1;
        }
        if !unit_is(units, at, '(') {
            return None;
        }
        at = skip_space(at + 1);
        let (r, next) = channel(at)?;
        at = skip_space(next);
        if !unit_is(units, at, ',') {
            return None;
        }
        at = skip_space(at + 1);
        let (g, next) = channel(at)?;
        at = skip_space(next);
        if !unit_is(units, at, ',') {
            return None;
        }
        at = skip_space(at + 1);
        let (b, next) = channel(at)?;
        at = skip_space(next);

        if unit_is(units, at, ')') {
            return Some([r, g, b, 255]);
        }
        // `(,\s*(0|1|\d?\.(\d+))\s*)?\)`: once a comma is there the group has to match.
        if !unit_is(units, at, ',') {
            return None;
        }
        at = skip_space(at + 1);
        let alpha_end = match_alpha(units, at, &skip_space)?;
        let text: String = units
            .get(at..alpha_end)?
            .iter()
            .filter_map(|&u| char::from_u32(u32::from(u)))
            .collect();
        let alpha: f64 = text.parse().ok()?;
        let after = skip_space(alpha_end);
        if !unit_is(units, after, ')') {
            return None;
        }
        Some([r, g, b, js_round(alpha * 255.0) as u32])
    }

    /// `0|1|\d?\.(\d+)` at `at`, each alternative also followed by `\s*\)` (the alternation
    /// backtracks on that). Returns the end of the matched alpha text.
    fn match_alpha(units: &[u16], at: usize, skip_space: &dyn Fn(usize) -> usize) -> Option<usize> {
        let followed_by_close = |end: usize| unit_is(units, skip_space(end), ')');
        for literal in ['0', '1'] {
            if unit_is(units, at, literal) && followed_by_close(at + 1) {
                return Some(at + 1);
            }
        }
        // `\d?\.(\d+)`: the optional digit first (greedy), then without it.
        let digits_from = |dot: usize| -> Option<usize> {
            if !unit_is(units, dot, '.') {
                return None;
            }
            let mut end = dot + 1;
            while units.get(end).is_some_and(|&u| decimal_digit(u).is_some()) {
                end += 1;
            }
            (end > dot + 1 && followed_by_close(end)).then_some(end)
        };
        if units.get(at).is_some_and(|&u| decimal_digit(u).is_some()) {
            if let Some(end) = digits_from(at + 1) {
                return Some(end);
            }
        }
        digits_from(at)
    }
}

/// Helper functions where the source type is "rgb" (number: 0xrrggbb).
pub mod rgb {
    /// Gets the relative luminance of an RGB color, this is useful in determining the contrast
    /// ratio between two colors. See <https://www.w3.org/TR/WCAG20/#relativeluminancedef>.
    pub fn relative_luminance(rgb: u32) -> f64 {
        relative_luminance2((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF)
    }

    /// Gets the relative luminance of an RGB color given as channels (0x00 to 0xFF).
    pub fn relative_luminance2(r: u32, g: u32, b: u32) -> f64 {
        let rs = f64::from(r) / 255.0;
        let gs = f64::from(g) / 255.0;
        let bs = f64::from(b) / 255.0;
        let rr = linear(rs);
        let rg = linear(gs);
        let rb = linear(bs);
        rr * 0.2126 + rg * 0.7152 + rb * 0.0722
    }

    fn linear(c: f64) -> f64 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
}

/// Helper functions where the source type is "rgba" (number: 0xrrggbbaa).
pub mod rgba {
    use super::{blend_channel, channels, contrast_ratio, rgb};

    pub fn blend(bg: u32, fg: u32) -> u32 {
        let a = f64::from(fg & 0xFF) / f64::from(0xFF_u8);
        if a == 1.0 {
            return fg;
        }
        let fg_r = (fg >> 24) & 0xFF;
        let fg_g = (fg >> 16) & 0xFF;
        let fg_b = (fg >> 8) & 0xFF;
        let bg_r = (bg >> 24) & 0xFF;
        let bg_g = (bg >> 16) & 0xFF;
        let bg_b = (bg >> 8) & 0xFF;
        let r = blend_channel(bg_r, fg_r, a);
        let g = blend_channel(bg_g, fg_g, a);
        let b = blend_channel(bg_b, fg_b, a);
        channels::to_rgba(r, g, b, None)
    }

    /// Given a foreground color and a background color, either increase or reduce the luminance
    /// of the foreground color until the specified contrast ratio is met. If pure white or black
    /// is hit without the contrast ratio being met, go the other direction using the background
    /// color as the foreground color and take either the first or second result depending on
    /// which has the higher contrast ratio.
    ///
    /// `None` is returned if the contrast ratio is already met.
    pub fn ensure_contrast_ratio(bg_rgba: u32, fg_rgba: u32, ratio: f64) -> Option<u32> {
        let bg_l = rgb::relative_luminance(bg_rgba >> 8);
        let fg_l = rgb::relative_luminance(fg_rgba >> 8);
        let cr = contrast_ratio(bg_l, fg_l);
        if cr < ratio {
            if fg_l < bg_l {
                let result_a = reduce_luminance(bg_rgba, fg_rgba, ratio);
                let result_a_ratio = contrast_ratio(bg_l, rgb::relative_luminance(result_a >> 8));
                if result_a_ratio < ratio {
                    let result_b = increase_luminance(bg_rgba, fg_rgba, ratio);
                    let result_b_ratio =
                        contrast_ratio(bg_l, rgb::relative_luminance(result_b >> 8));
                    return Some(if result_a_ratio > result_b_ratio {
                        result_a
                    } else {
                        result_b
                    });
                }
                return Some(result_a);
            }
            let result_a = increase_luminance(bg_rgba, fg_rgba, ratio);
            let result_a_ratio = contrast_ratio(bg_l, rgb::relative_luminance(result_a >> 8));
            if result_a_ratio < ratio {
                let result_b = reduce_luminance(bg_rgba, fg_rgba, ratio);
                let result_b_ratio = contrast_ratio(bg_l, rgb::relative_luminance(result_b >> 8));
                return Some(if result_a_ratio > result_b_ratio {
                    result_a
                } else {
                    result_b
                });
            }
            return Some(result_a);
        }
        None
    }

    pub fn reduce_luminance(bg_rgba: u32, fg_rgba: u32, ratio: f64) -> u32 {
        // This is a naive but fast approach to reducing luminance as converting to
        // HSL and back is expensive
        let bg_r = (bg_rgba >> 24) & 0xFF;
        let bg_g = (bg_rgba >> 16) & 0xFF;
        let bg_b = (bg_rgba >> 8) & 0xFF;
        let mut fg_r = (fg_rgba >> 24) & 0xFF;
        let mut fg_g = (fg_rgba >> 16) & 0xFF;
        let mut fg_b = (fg_rgba >> 8) & 0xFF;
        let mut cr = contrast_ratio(
            rgb::relative_luminance2(fg_r, fg_g, fg_b),
            rgb::relative_luminance2(bg_r, bg_g, bg_b),
        );
        while cr < ratio && (fg_r > 0 || fg_g > 0 || fg_b > 0) {
            // Reduce by 10% until the ratio is hit
            fg_r -= step_down(fg_r);
            fg_g -= step_down(fg_g);
            fg_b -= step_down(fg_b);
            cr = contrast_ratio(
                rgb::relative_luminance2(fg_r, fg_g, fg_b),
                rgb::relative_luminance2(bg_r, bg_g, bg_b),
            );
        }
        (fg_r << 24) | (fg_g << 16) | (fg_b << 8) | 0xFF
    }

    pub fn increase_luminance(bg_rgba: u32, fg_rgba: u32, ratio: f64) -> u32 {
        // This is a naive but fast approach to increasing luminance as converting to
        // HSL and back is expensive
        let bg_r = (bg_rgba >> 24) & 0xFF;
        let bg_g = (bg_rgba >> 16) & 0xFF;
        let bg_b = (bg_rgba >> 8) & 0xFF;
        let mut fg_r = (fg_rgba >> 24) & 0xFF;
        let mut fg_g = (fg_rgba >> 16) & 0xFF;
        let mut fg_b = (fg_rgba >> 8) & 0xFF;
        let mut cr = contrast_ratio(
            rgb::relative_luminance2(fg_r, fg_g, fg_b),
            rgb::relative_luminance2(bg_r, bg_g, bg_b),
        );
        while cr < ratio && (fg_r < 0xFF || fg_g < 0xFF || fg_b < 0xFF) {
            // Increase by 10% until the ratio is hit
            fg_r = 0xFF.min(fg_r + step_up(fg_r));
            fg_g = 0xFF.min(fg_g + step_up(fg_g));
            fg_b = 0xFF.min(fg_b + step_up(fg_b));
            cr = contrast_ratio(
                rgb::relative_luminance2(fg_r, fg_g, fg_b),
                rgb::relative_luminance2(bg_r, bg_g, bg_b),
            );
        }
        (fg_r << 24) | (fg_g << 16) | (fg_b << 8) | 0xFF
    }

    /// `Math.max(0, Math.ceil(c * 0.1))` for a channel 0..=255 (never more than `c`).
    fn step_down(c: u32) -> u32 {
        (f64::from(c) * 0.1).ceil() as u32
    }

    /// `Math.ceil((255 - c) * 0.1)` for a channel 0..=255.
    fn step_up(c: u32) -> u32 {
        (f64::from(255 - c.min(255)) * 0.1).ceil() as u32
    }

    pub fn to_channels(value: u32) -> [u32; 4] {
        [
            (value >> 24) & 0xFF,
            (value >> 16) & 0xFF,
            (value >> 8) & 0xFF,
            value & 0xFF,
        ]
    }
}

/// `c.toString(16)` padded to two digits.
pub fn to_padded_hex(c: u32) -> String {
    format!("{c:02x}")
}

/// Gets the contrast ratio between two relative luminance values.
/// See <https://www.w3.org/TR/WCAG20/#contrast-ratiodef>.
pub fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    if l1 < l2 {
        return (l2 + 0.05) / (l1 + 0.05);
    }
    (l1 + 0.05) / (l2 + 0.05)
}

#[cfg(test)]
#[path = "color_tests.rs"]
mod tests;
