//! The JSON writer (plan M10 step 5, D17): the other half of the toolkit's bounded reader.
//!
//! luneui-rs reads JSON (`lune_ui::json::parse`) and writes none, so the app writes its own, small
//! and closed: null, booleans, integers, strings, arrays and objects. Everything it writes reads
//! back through `json::parse` to the same tree, or is refused: [`Json::to_text`] checks the tree
//! against the reader's limits (depth, string and key length, members, values, total size) and
//! then parses its own output and compares, so a document that would not round-trip is never
//! written.
//!
//! Strings are escaped as RFC 8259 asks, and a little more: `"` and `\`, every control character
//! (including DEL) as `\u00XX` or its short form, and U+2028 and U+2029, so no string can end its
//! line or its quotes early. Non-ASCII text goes out as UTF-8.

use std::fmt::Write as _;

use lune_ui::json::{
    self, MAX_DEPTH, MAX_INPUT, MAX_KEY, MAX_MEMBERS, MAX_STRING, MAX_VALUES, Value,
};

/// A JSON tree the writer can write.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// An integer the reader holds exactly (magnitude at most 2^53).
    Int(i64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// Why a tree is not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// A string, key, container or the whole document is over a reader limit.
    OverLimit,
    /// An integer the reader would hold inexactly.
    Number,
    /// An object names a key twice.
    DuplicateKey,
    /// The text did not read back to the tree (a writer bug; nothing is written).
    RoundTrip,
}

/// The integers a reader holds exactly.
const EXACT_INT_MAX: i64 = 1 << 53;

impl Json {
    /// The tree of a parsed value (to carry unknown fields through a rewrite). A number that is
    /// not an exact integer becomes `Null`: the writer has no floats.
    pub fn from_value(value: &Value) -> Json {
        match value {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Number(n) => {
                if n.is_exact_int() {
                    Json::Int(n.value() as i64)
                } else {
                    Json::Null
                }
            }
            Value::String(s) => Json::Str(s.clone()),
            Value::Array(items) => Json::Array(items.iter().map(Json::from_value).collect()),
            Value::Object(members) => Json::Object(
                members
                    .iter()
                    .map(|(k, v)| (k.clone(), Json::from_value(v)))
                    .collect(),
            ),
        }
    }

    /// The text of the tree, compact, with a trailing newline. Checked: within the reader's
    /// limits, and read back by `json::parse` to the same tree.
    pub fn to_text(&self) -> Result<String, WriteError> {
        let mut counts = Counts::default();
        self.check(0, &mut counts)?;
        let mut out = String::new();
        self.write(&mut out);
        out.push('\n');
        if out.len() > MAX_INPUT {
            return Err(WriteError::OverLimit);
        }
        match json::parse(out.as_bytes()) {
            Ok(read) if same(self, &read) => Ok(out),
            _ => Err(WriteError::RoundTrip),
        }
    }

    fn check(&self, depth: usize, counts: &mut Counts) -> Result<(), WriteError> {
        counts.values += 1;
        if counts.values > MAX_VALUES {
            return Err(WriteError::OverLimit);
        }
        match self {
            Json::Null | Json::Bool(_) => Ok(()),
            Json::Int(n) if (-EXACT_INT_MAX..=EXACT_INT_MAX).contains(n) => Ok(()),
            Json::Int(_) => Err(WriteError::Number),
            Json::Str(s) if s.len() <= MAX_STRING => Ok(()),
            Json::Str(_) => Err(WriteError::OverLimit),
            Json::Array(items) => {
                if depth >= MAX_DEPTH || items.len() > MAX_MEMBERS {
                    return Err(WriteError::OverLimit);
                }
                items.iter().try_for_each(|i| i.check(depth + 1, counts))
            }
            Json::Object(members) => {
                if depth >= MAX_DEPTH || members.len() > MAX_MEMBERS {
                    return Err(WriteError::OverLimit);
                }
                let mut keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
                if keys.iter().any(|k| k.len() > MAX_KEY) {
                    return Err(WriteError::OverLimit);
                }
                keys.sort_unstable();
                if keys.windows(2).any(|w| w[0] == w[1]) {
                    return Err(WriteError::DuplicateKey);
                }
                members
                    .iter()
                    .try_for_each(|(_, v)| v.check(depth + 1, counts))
            }
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => write_string(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

#[derive(Default)]
struct Counts {
    values: usize,
}

/// Writes `s` as a JSON string literal.
pub fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
                // Every control character (C0, DEL, C1) and the two line separators.
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether `read` is the tree `written` was written from.
fn same(written: &Json, read: &Value) -> bool {
    match (written, read) {
        (Json::Null, Value::Null) => true,
        (Json::Bool(a), Value::Bool(b)) => a == b,
        (Json::Int(a), Value::Number(n)) => n.is_exact_int() && n.value() as i64 == *a,
        (Json::Str(a), Value::String(b)) => a == b,
        (Json::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Json::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(j: &Json) -> String {
        j.to_text().expect("writes")
    }

    fn round(j: &Json) -> Value {
        json::parse(text(j).as_bytes()).expect("reads back")
    }

    #[test]
    fn every_kind_round_trips() {
        let j = Json::Object(vec![
            ("null".into(), Json::Null),
            ("yes".into(), Json::Bool(true)),
            ("no".into(), Json::Bool(false)),
            ("n".into(), Json::Int(-42)),
            ("big".into(), Json::Int(1 << 53)),
            ("s".into(), Json::Str("héllo 日本 🎉".into())),
            (
                "list".into(),
                Json::Array(vec![
                    Json::Int(1),
                    Json::Str("a".into()),
                    Json::Array(vec![]),
                ]),
            ),
            ("empty".into(), Json::Object(vec![])),
        ]);
        let t = text(&j);
        assert!(
            t.ends_with('\n') && !t[..t.len() - 1].contains('\n'),
            "one compact line"
        );
        assert!(same(&j, &round(&j)));
        // The same tree through `from_value` and back.
        assert_eq!(Json::from_value(&round(&j)), j);
    }

    #[test]
    fn quotes_backslashes_and_controls_are_escaped_and_read_back() {
        let nasty =
            "a\"b\\c\nd\re\tf\u{8}\u{c}\u{0}\u{1}\u{1f}\u{7f}\u{85}\u{2028}\u{2029}</script>'";
        let t = text(&Json::Str(nasty.into()));
        // No raw control character, quote or separator survives into the text, but the final
        // newline and the two quotes.
        let body = &t[1..t.len() - 2];
        assert!(
            !body
                .chars()
                .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        );
        assert!(!body.replace("\\\"", "").replace("\\\\", "").contains('"'));
        assert_eq!(round(&Json::Str(nasty.into())), Value::String(nasty.into()));
        // Every ASCII character and a spread of others, one string.
        let all: String = (0u32..0x250).filter_map(char::from_u32).collect();
        assert_eq!(round(&Json::Str(all.clone())), Value::String(all));
    }

    #[test]
    fn strings_at_the_reader_limit_round_trip_whatever_the_escaping_does_to_them() {
        // The limit is on the decoded string, so escaping grows the text, not the string: a
        // string of 4096 quotes (12 KiB written) and one of 4096 control characters read back.
        for c in ['"', '\\', '\n', '\u{1}', '\u{2028}', 'a', 'é'] {
            let max = MAX_STRING / c.len_utf8();
            let s: String = std::iter::repeat_n(c, max).collect();
            assert!(s.len() <= MAX_STRING);
            assert_eq!(round(&Json::Str(s.clone())), Value::String(s), "{c:?}");
        }
        // One byte more is refused, never written.
        let over = "a".repeat(MAX_STRING + 1);
        assert_eq!(Json::Str(over).to_text(), Err(WriteError::OverLimit));
        let over = "é".repeat(MAX_STRING / 2 + 1);
        assert_eq!(Json::Str(over).to_text(), Err(WriteError::OverLimit));
    }

    #[test]
    fn keys_depth_members_and_integers_are_held_to_the_readers_limits() {
        let key = "k".repeat(MAX_KEY);
        assert!(Json::Object(vec![(key, Json::Null)]).to_text().is_ok());
        let long_key = "k".repeat(MAX_KEY + 1);
        assert_eq!(
            Json::Object(vec![(long_key, Json::Null)]).to_text(),
            Err(WriteError::OverLimit)
        );
        // Nesting: 16 containers deep is the reader's limit.
        let nest = |n: usize| (0..n).fold(Json::Null, |inner, _| Json::Array(vec![inner]));
        assert!(nest(MAX_DEPTH).to_text().is_ok());
        assert_eq!(nest(MAX_DEPTH + 1).to_text(), Err(WriteError::OverLimit));
        // Members.
        let many = Json::Array(vec![Json::Null; MAX_MEMBERS]);
        assert!(many.to_text().is_ok());
        assert_eq!(
            Json::Array(vec![Json::Null; MAX_MEMBERS + 1]).to_text(),
            Err(WriteError::OverLimit)
        );
        // Integers beyond 2^53 are inexact for the reader.
        assert!(Json::Int(EXACT_INT_MAX).to_text().is_ok());
        assert_eq!(
            Json::Int(EXACT_INT_MAX + 1).to_text(),
            Err(WriteError::Number)
        );
        assert_eq!(Json::Int(i64::MIN).to_text(), Err(WriteError::Number));
        // A repeated key.
        assert_eq!(
            Json::Object(vec![("a".into(), Json::Null), ("a".into(), Json::Null)]).to_text(),
            Err(WriteError::DuplicateKey)
        );
    }

    #[test]
    fn the_whole_document_stays_under_the_input_limit() {
        // 300 strings of 4000 bytes: 1.2 MB, over the reader's 1 MiB.
        let big = Json::Array((0..300).map(|_| Json::Str("x".repeat(4000))).collect());
        assert_eq!(big.to_text(), Err(WriteError::OverLimit));
    }

    #[test]
    fn a_seeded_fuzz_of_trees_writes_and_reads_back_or_is_refused() {
        // A small xorshift: reproducible, no dependency.
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0
            }
            fn below(&mut self, n: u64) -> u64 {
                self.next() % n
            }
        }
        fn string(r: &mut Rng) -> String {
            let pool = [
                'a', 'Z', '"', '\\', '\n', '\u{0}', '\u{7f}', 'é', '日', '🎉', '\u{2028}', ' ', '/',
            ];
            (0..r.below(40))
                .map(|_| pool[r.below(pool.len() as u64) as usize])
                .collect()
        }
        fn tree(r: &mut Rng, depth: usize) -> Json {
            match r.below(if depth > 4 { 4 } else { 6 }) {
                0 => Json::Null,
                1 => Json::Bool(r.below(2) == 0),
                2 => Json::Int(r.below(1 << 40) as i64 - (1 << 39)),
                3 => Json::Str(string(r)),
                4 => Json::Array((0..r.below(5)).map(|_| tree(r, depth + 1)).collect()),
                _ => Json::Object(
                    (0..r.below(5))
                        .map(|i| (format!("k{i}{}", string(r)), tree(r, depth + 1)))
                        .collect(),
                ),
            }
        }
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut written = 0;
        for _ in 0..2000 {
            let t = tree(&mut rng, 0);
            match t.to_text() {
                Ok(text) => {
                    written += 1;
                    let read = json::parse(text.as_bytes()).expect("what is written reads");
                    assert!(same(&t, &read));
                }
                // A tree the writer refuses (a duplicate key made of the random tail).
                Err(e) => assert!(
                    matches!(e, WriteError::DuplicateKey | WriteError::OverLimit),
                    "{e:?}"
                ),
            }
        }
        assert!(written > 1500, "most trees write: {written}");
    }
}
