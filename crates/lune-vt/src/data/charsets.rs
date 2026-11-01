//! Port of xterm.js src/common/data/Charsets.ts @ f447274.
//!
//! The character sets supported by the terminal. These enable several languages to be
//! represented within the terminal with only 8-bit encoding. See ISO 2022 for a discussion on
//! character sets. Only VT100 character sets are supported.
//!
//! Upstream keeps a `CHARSETS` object keyed by designator; here [`charset`] is the lookup and
//! each set is a [`Charset`] static. The US set (`B`) and any unknown designator are `None`.

/// A character set: the printable ASCII characters it replaces and what they become.
///
/// Upstream's `ICharset` is a string-keyed object; entries are in upstream order.
#[derive(Debug, PartialEq, Eq)]
pub struct Charset {
    entries: &'static [(char, &'static str)],
}

impl Charset {
    /// The replacement for `ch`, or `None` if this set leaves it unchanged.
    pub fn get(&self, ch: char) -> Option<&'static str> {
        self.entries
            .iter()
            .find(|(key, _)| *key == ch)
            .map(|(_, value)| *value)
    }

    /// All `(character, replacement)` pairs, in upstream order.
    pub fn entries(&self) -> &'static [(char, &'static str)] {
        self.entries
    }
}

/// The default character set, US (ESC ( B): no replacements.
pub const DEFAULT_CHARSET: Option<&'static Charset> = None;

/// Look up a character set by its designator (the final byte of `ESC ( x`, `ESC ) x`, ...).
///
/// `B` (US) and every unknown designator give `None`, as in upstream's `CHARSETS`.
pub fn charset(designator: char) -> Option<&'static Charset> {
    match designator {
        '0' => Some(&DEC_SPECIAL),
        'A' => Some(&BRITISH),
        '4' => Some(&DUTCH),
        'C' | '5' => Some(&FINNISH),
        'R' => Some(&FRENCH),
        'Q' => Some(&FRENCH_CANADIAN),
        'K' => Some(&GERMAN),
        'Y' => Some(&ITALIAN),
        'E' | '6' => Some(&NORWEGIAN_DANISH),
        'Z' => Some(&SPANISH),
        'H' | '7' => Some(&SWEDISH),
        '=' => Some(&SWISS),
        _ => None,
    }
}

/// DEC Special Character and Line Drawing Set.
/// Reference: <http://vt100.net/docs/vt102-ug/table5-13.html>
/// A lot of curses apps use this if they see TERM=xterm.
/// testing: echo -e '\e(0a\e(B'
/// The xterm output sometimes seems to conflict with the
/// reference above. xterm seems in line with the reference
/// when running vttest however.
/// The table below now uses xterm's output from vttest.
static DEC_SPECIAL: Charset = Charset {
    entries: &[
        ('`', "\u{25c6}"), // ◆
        ('a', "\u{2592}"), // ▒
        ('b', "\u{2409}"), // ␉
        ('c', "\u{240c}"), // ␌
        ('d', "\u{240d}"), // ␍
        ('e', "\u{240a}"), // ␊
        ('f', "°"),
        ('g', "±"),
        ('h', "\u{2424}"), // ␤
        ('i', "\u{240b}"), // ␋
        ('j', "\u{2518}"), // ┘
        ('k', "\u{2510}"), // ┐
        ('l', "\u{250c}"), // ┌
        ('m', "\u{2514}"), // └
        ('n', "\u{253c}"), // ┼
        ('o', "\u{23ba}"), // ⎺
        ('p', "\u{23bb}"), // ⎻
        ('q', "\u{2500}"), // ─
        ('r', "\u{23bc}"), // ⎼
        ('s', "\u{23bd}"), // ⎽
        ('t', "\u{251c}"), // ├
        ('u', "\u{2524}"), // ┤
        ('v', "\u{2534}"), // ┴
        ('w', "\u{252c}"), // ┬
        ('x', "\u{2502}"), // │
        ('y', "\u{2264}"), // ≤
        ('z', "\u{2265}"), // ≥
        ('{', "\u{3c0}"),  // π
        ('|', "\u{2260}"), // ≠
        ('}', "£"),
        ('~', "·"),
    ],
};

/// British character set
/// ESC (A
/// Reference: <http://vt100.net/docs/vt220-rm/table2-5.html>
static BRITISH: Charset = Charset {
    entries: &[('#', "£")],
};

/// Dutch character set
/// ESC (4
/// Reference: <http://vt100.net/docs/vt220-rm/table2-6.html>
static DUTCH: Charset = Charset {
    entries: &[
        ('#', "£"),
        ('@', "¾"),
        ('[', "ij"),
        ('\\', "½"),
        (']', "|"),
        ('{', "¨"),
        ('|', "f"),
        ('}', "¼"),
        ('~', "´"),
    ],
};

/// Finnish character set
/// ESC (C or ESC (5
/// Reference: <http://vt100.net/docs/vt220-rm/table2-7.html>
static FINNISH: Charset = Charset {
    entries: &[
        ('[', "Ä"),
        ('\\', "Ö"),
        (']', "Å"),
        ('^', "Ü"),
        ('`', "é"),
        ('{', "ä"),
        ('|', "ö"),
        ('}', "å"),
        ('~', "ü"),
    ],
};

/// French character set
/// ESC (R
/// Reference: <http://vt100.net/docs/vt220-rm/table2-8.html>
static FRENCH: Charset = Charset {
    entries: &[
        ('#', "£"),
        ('@', "à"),
        ('[', "°"),
        ('\\', "ç"),
        (']', "§"),
        ('{', "é"),
        ('|', "ù"),
        ('}', "è"),
        ('~', "¨"),
    ],
};

/// French Canadian character set
/// ESC (Q
/// Reference: <http://vt100.net/docs/vt220-rm/table2-9.html>
static FRENCH_CANADIAN: Charset = Charset {
    entries: &[
        ('@', "à"),
        ('[', "â"),
        ('\\', "ç"),
        (']', "ê"),
        ('^', "î"),
        ('`', "ô"),
        ('{', "é"),
        ('|', "ù"),
        ('}', "è"),
        ('~', "û"),
    ],
};

/// German character set
/// ESC (K
/// Reference: <http://vt100.net/docs/vt220-rm/table2-10.html>
static GERMAN: Charset = Charset {
    entries: &[
        ('@', "§"),
        ('[', "Ä"),
        ('\\', "Ö"),
        (']', "Ü"),
        ('{', "ä"),
        ('|', "ö"),
        ('}', "ü"),
        ('~', "ß"),
    ],
};

/// Italian character set
/// ESC (Y
/// Reference: <http://vt100.net/docs/vt220-rm/table2-11.html>
static ITALIAN: Charset = Charset {
    entries: &[
        ('#', "£"),
        ('@', "§"),
        ('[', "°"),
        ('\\', "ç"),
        (']', "é"),
        ('`', "ù"),
        ('{', "à"),
        ('|', "ò"),
        ('}', "è"),
        ('~', "ì"),
    ],
};

/// Norwegian/Danish character set
/// ESC (E or ESC (6
/// Reference: <http://vt100.net/docs/vt220-rm/table2-12.html>
static NORWEGIAN_DANISH: Charset = Charset {
    entries: &[
        ('@', "Ä"),
        ('[', "Æ"),
        ('\\', "Ø"),
        (']', "Å"),
        ('^', "Ü"),
        ('`', "ä"),
        ('{', "æ"),
        ('|', "ø"),
        ('}', "å"),
        ('~', "ü"),
    ],
};

/// Spanish character set
/// ESC (Z
/// Reference: <http://vt100.net/docs/vt220-rm/table2-13.html>
static SPANISH: Charset = Charset {
    entries: &[
        ('#', "£"),
        ('@', "§"),
        ('[', "¡"),
        ('\\', "Ñ"),
        (']', "¿"),
        ('{', "°"),
        ('|', "ñ"),
        ('}', "ç"),
    ],
};

/// Swedish character set
/// ESC (H or ESC (7
/// Reference: <http://vt100.net/docs/vt220-rm/table2-14.html>
static SWEDISH: Charset = Charset {
    entries: &[
        ('@', "É"),
        ('[', "Ä"),
        ('\\', "Ö"),
        (']', "Å"),
        ('^', "Ü"),
        ('`', "é"),
        ('{', "ä"),
        ('|', "ö"),
        ('}', "å"),
        ('~', "ü"),
    ],
};

/// Swiss character set
/// ESC (=
/// Reference: <http://vt100.net/docs/vt220-rm/table2-15.html>
static SWISS: Charset = Charset {
    entries: &[
        ('#', "ù"),
        ('@', "à"),
        ('[', "é"),
        ('\\', "ç"),
        (']', "ê"),
        ('^', "î"),
        ('_', "è"),
        ('`', "ô"),
        ('{', "ä"),
        ('|', "ö"),
        ('}', "ü"),
        ('~', "û"),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Check two mappings (copied from Charsets.ts) and the entry count of one designator.
    fn check(designator: char, a: (char, &str), b: (char, &str), len: usize) {
        let set = charset(designator).expect("charset exists");
        assert_eq!(set.get(a.0), Some(a.1), "{designator} {:?}", a.0);
        assert_eq!(set.get(b.0), Some(b.1), "{designator} {:?}", b.0);
        assert_eq!(set.entries().len(), len, "{designator} entry count");
    }

    #[test]
    fn dec_special_character_and_line_drawing() {
        check('0', ('q', "\u{2500}"), ('`', "\u{25c6}"), 31);
        assert_eq!(charset('0').and_then(|set| set.get('~')), Some("\u{b7}"));
    }

    #[test]
    fn british() {
        check('A', ('#', "£"), ('#', "£"), 1);
        assert_eq!(charset('A').and_then(|set| set.get('$')), None);
    }

    #[test]
    fn us_and_unknown_designators_are_none() {
        assert!(charset('B').is_none());
        assert!(DEFAULT_CHARSET.is_none());
        assert!(charset('?').is_none());
        assert!(charset('\u{2500}').is_none());
    }

    #[test]
    fn dutch() {
        check('4', ('@', "¾"), ('[', "ij"), 9);
    }

    #[test]
    fn finnish() {
        check('C', ('[', "Ä"), ('~', "ü"), 9);
        assert_eq!(charset('5'), charset('C'));
    }

    #[test]
    fn french() {
        check('R', ('@', "à"), ('}', "è"), 9);
    }

    #[test]
    fn french_canadian() {
        check('Q', ('`', "ô"), ('~', "û"), 10);
    }

    #[test]
    fn german() {
        check('K', ('@', "§"), ('~', "ß"), 8);
    }

    #[test]
    fn italian() {
        check('Y', ('#', "£"), ('~', "ì"), 10);
    }

    #[test]
    fn norwegian_danish() {
        check('E', ('@', "Ä"), ('|', "ø"), 10);
        assert_eq!(charset('6'), charset('E'));
    }

    #[test]
    fn spanish() {
        check('Z', ('[', "¡"), ('}', "ç"), 8);
    }

    #[test]
    fn swedish() {
        check('H', ('@', "É"), ('`', "é"), 10);
        assert_eq!(charset('7'), charset('H'));
    }

    #[test]
    fn swiss() {
        check('=', ('_', "è"), ('#', "ù"), 12);
    }
}
