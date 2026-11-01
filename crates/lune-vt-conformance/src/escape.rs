//! The golden string escaping (FORMAT.md, "String escaping"). Dependency-free: `lune-vt`'s
//! fuzz test includes this file by path next to `trace.rs`.

#![allow(unreachable_pub, dead_code)]

/// Quotes code points with the golden string escaping: 0x20 to 0x7E except `"` and `\` verbatim,
/// everything else as `\u{X}` (uppercase hex, no padding).
pub fn escape_code_points<I: IntoIterator<Item = u32>>(code_points: I) -> String {
    let mut out = String::from("\"");
    for cp in code_points {
        match char::from_u32(cp) {
            Some(c) if (' '..='~').contains(&c) && c != '"' && c != '\\' => out.push(c),
            _ => out.push_str(&format!("\\u{{{cp:X}}}")),
        }
    }
    out.push('"');
    out
}

/// [`escape_code_points`] for a `&str`.
pub fn escape_str(s: &str) -> String {
    escape_code_points(s.chars().map(u32::from))
}
