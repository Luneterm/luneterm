//! Port of xterm.js src/common/input/XParseColor.ts @ f447274.
//!
//! Parse X11 color specs (`rgb:r/g/b`, `#rgb`) to 8 bit per channel RGB and format them back
//! as `rgb:` strings (OSC 4/10/11 queries). See `man xparsecolor`.
//!
//! Rust notes (see `docs/porting-xterm.md`):
//!  - The two regular expressions become hand-written matchers with the same language: the
//!    input is lower-cased (ASCII only; non-ASCII can never match), every character must be an
//!    ASCII hex digit, and nothing is sliced before that is checked, so no `&str` byte offset
//!    ever lands inside a multi-byte character.
//!  - Colors are `[u8; 3]` (upstream: `[number, number, number]` that is always 0..=255).
//!  - `toRgbString`'s default `bits = 16` is [`DEFAULT_RGB_BITS`].

/// Upstream's default `bits` argument of `toRgbString`.
pub const DEFAULT_RGB_BITS: u32 = 16;

/// One hex run of the `rgb:` rule: 1..=4 ASCII hex digits.
fn hex_value(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `Math.round(value / base * 255)` (values are non-negative, so half-away == half-up).
fn scale(value: u32, base: u32) -> u8 {
    // Same operation order as upstream, in f64, so ties round identically. `value <= base`
    // for hex runs of the matching width, so the result is within 0..=255.
    (f64::from(value) / f64::from(base) * 255.0).round() as u8
}

/// Parse a color spec to RGB values (8 bit per channel).
///
/// Supported formats:
/// - `rgb:<red>/<green>/<blue>` with each component `h | hh | hhh | hhhh` (same width for all)
/// - `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB`
///
/// Anything else (`rgbi:`, named colors, float notation) is `None`. Upper case is accepted.
pub fn parse_color(data: &str) -> Option<[u8; 3]> {
    if data.is_empty() {
        return None;
    }
    // also handle uppercases
    let low = data.to_ascii_lowercase();
    if let Some(rest) = low.strip_prefix("rgb:") {
        // 'rgb:' specifier: r/g/b | rr/gg/bb | rrr/ggg/bbb | rrrr/gggg/bbbb
        let mut parts = rest.split('/');
        let (r, g, b) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let width = r.len();
        if !(1..=4).contains(&width) || g.len() != width || b.len() != width {
            return None;
        }
        let base = match width {
            1 => 15,
            2 => 255,
            3 => 4095,
            _ => 65535,
        };
        Some([
            scale(hex_value(r)?, base),
            scale(hex_value(g)?, base),
            scale(hex_value(b)?, base),
        ])
    } else if let Some(rest) = low.strip_prefix('#') {
        // '#' specifier: any hex digits, 3/6/9/12 of them
        if !rest.bytes().all(|b| b.is_ascii_hexdigit()) || ![3, 6, 9, 12].contains(&rest.len()) {
            return None;
        }
        let adv = rest.len() / 3;
        let mut result = [0u8; 3];
        for (i, out) in result.iter_mut().enumerate() {
            let c = hex_value(rest.get(adv * i..adv * i + adv)?)?;
            *out = match adv {
                1 => c << 4,
                2 => c,
                3 => c >> 4,
                _ => c >> 8,
            } as u8;
        }
        Some(result)
    } else {
        // Named colors are not supported (upstream: bundle size, xterm.js #3530).
        None
    }
}

/// Pad hex output to the requested bit width.
fn pad(n: u8, bits: u32) -> String {
    let s = format!("{n:x}");
    let s2 = if s.len() < 2 {
        format!("0{s}")
    } else {
        s.clone()
    };
    match bits {
        // upstream returns `s[0]`: the first digit of the *unpadded* hex string
        4 => s.chars().next().map(String::from).unwrap_or_default(),
        8 => s2,
        12 => format!("{s2}{s2}").chars().take(3).collect(),
        _ => format!("{s2}{s2}"),
    }
}

/// Convert a color to an `rgb:../../..` string of `bits` depth (4, 8, 12; anything else is 16).
pub fn to_rgb_string(color: [u8; 3], bits: u32) -> String {
    let [r, g, b] = color;
    format!("rgb:{}/{}/{}", pad(r, bits), pad(g, bits), pad(b, bits))
}

#[cfg(test)]
#[path = "x_parse_color_tests.rs"]
mod tests;
