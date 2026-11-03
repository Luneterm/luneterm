//! Replays `goldens/lunegit-input/keys.golden`, the real outputs of Lunegit's key encoders
//! (`TerminalKeyEncoder.Encode`, `XtermVtInputEncoder.GenerateKeyInput` / `GenerateCharInput`),
//! against `lune-vt`'s `encode_key`, `legacy_key_input` and `legacy_char_input` (plan M7 step 5).
//! Every row must match; a mismatch is a bug in the port, never a golden edit. Failure messages
//! name the row and show only the key bytes.

use lune_vt::input::key_encoder::{KeyboardModes, encode_key};
use lune_vt::input::key_event::{TerminalKey, TerminalKeyEvent, TerminalModifiers};
use lune_vt::input::legacy_key_encoder::{legacy_char_input, legacy_key_input};
use lune_vt_conformance::lunegit_input::{
    InputKind, InputSection, decode_hex, expand_spec, load_input,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn section<'a>(
    golden: &'a lune_vt_conformance::lunegit_input::InputGolden,
    name: &str,
) -> Result<&'a InputSection> {
    golden
        .section(name)
        .ok_or_else(|| format!("no section {name}").into())
}

fn field<'a>(s: &InputSection, row: &'a [String], name: &str) -> Result<&'a str> {
    s.field(row, name)
        .ok_or_else(|| format!("no column {name}").into())
}

fn num(text: &str) -> Result<u32> {
    Ok(text.parse::<u32>()?)
}

fn hex(text: &str) -> Result<u32> {
    Ok(u32::from_str_radix(text, 16)?)
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn check(label: &str, row: &[String], got: &[u8], want: &[u8], failures: &mut Vec<String>) {
    if got != want {
        failures.push(format!(
            "{label} {}: want {} got {}",
            row.iter()
                .take(row.len().saturating_sub(1))
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            bytes_hex(want),
            bytes_hex(got)
        ));
    }
}

fn key(name: &str) -> Result<TerminalKey> {
    TerminalKey::from_name(name).ok_or_else(|| format!("unknown key {name}").into())
}

#[test]
fn the_header_names_the_lunegit_commit_and_dlls() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    assert_eq!(golden.header("lunegit-commit"), Some("33a1dd4"));
    for name in ["core-dll-sha256", "infrastructure-dll-sha256"] {
        let sha = golden.header(name).ok_or("no sha header")?;
        assert_eq!(sha.len(), 64, "{name}");
    }
    Ok(())
}

#[test]
fn encode_key_matches_every_row() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    let s = section(&golden, "encode-key")?;
    assert_eq!(s.rows.len(), 4160);
    let mut failures = Vec::new();
    for row in &s.rows {
        let mut event = TerminalKeyEvent::for_key(
            key(field(s, row, "key")?)?,
            TerminalModifiers::from_bits(u8::try_from(num(field(s, row, "mods")?)?)?),
        );
        event.is_keypad = field(s, row, "keypad")? == "1";
        event.alt_is_meta = field(s, row, "alt")? == "1";
        let modes = KeyboardModes::unpack(num(field(s, row, "modes")?)?);
        let want = expand_spec(field(s, row, "out")?)?;
        check(
            "encode-key",
            row,
            &encode_key(&event, modes),
            &want,
            &mut failures,
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn encode_char_matches_every_row() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    let s = section(&golden, "encode-char")?;
    assert_eq!(s.rows.len(), 5616);
    let mut failures = Vec::new();
    for row in &s.rows {
        let mut event = TerminalKeyEvent::for_key(
            TerminalKey::Character,
            TerminalModifiers::from_bits(u8::try_from(num(field(s, row, "mods")?)?)?),
        );
        event.codepoint = hex(field(s, row, "cp")?)?;
        event.shifted_codepoint = hex(field(s, row, "shifted")?)?;
        event.base_layout_codepoint = hex(field(s, row, "base")?)?;
        event.is_keypad = field(s, row, "keypad")? == "1";
        event.alt_is_meta = field(s, row, "alt")? == "1";
        let modes = KeyboardModes::unpack(num(field(s, row, "modes")?)?);
        let want = expand_spec(field(s, row, "out")?)?;
        check(
            "encode-char",
            row,
            &encode_key(&event, modes),
            &want,
            &mut failures,
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn legacy_key_input_matches_every_row() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    let s = section(&golden, "generate-key-input")?;
    assert_eq!(s.rows.len(), 448);
    let mut failures = Vec::new();
    for row in &s.rows {
        let mods = TerminalModifiers::from_bits(u8::try_from(num(field(s, row, "mods")?)?)?);
        let decckm = field(s, row, "decckm")? == "1";
        let want = expand_spec(field(s, row, "out")?)?;
        let got = legacy_key_input(key(field(s, row, "key")?)?, mods, decckm);
        check(
            "generate-key-input",
            row,
            got.as_bytes(),
            &want,
            &mut failures,
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn legacy_char_input_matches_every_row() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    let s = section(&golden, "generate-char-input")?;
    assert_eq!(s.rows.len(), 1064);
    let mut failures = Vec::new();
    for row in &s.rows {
        let mods = TerminalModifiers::from_bits(u8::try_from(num(field(s, row, "mods")?)?)?);
        let c = char::from_u32(hex(field(s, row, "char")?)?).ok_or("char is not a scalar")?;
        let want = expand_spec(field(s, row, "out")?)?;
        let got = legacy_char_input(c, mods);
        check(
            "generate-char-input",
            row,
            got.as_bytes(),
            &want,
            &mut failures,
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

/// The invalid-code-point rows (Lunegit sends nothing) are in the golden and are empty there.
#[test]
fn the_golden_covers_the_invalid_code_points_with_empty_output() -> Result<()> {
    let golden = load_input(InputKind::Keys)?;
    let s = section(&golden, "encode-char")?;
    let mut seen = 0;
    for row in &s.rows {
        let cp = hex(field(s, row, "cp")?)?;
        if cp == 0xD800 || cp == 0x110000 || cp == 0xFFFF_FFFF {
            assert_eq!(field(s, row, "out")?, "-");
            seen += 1;
        }
    }
    // 3 code points x 8 modifier sets x 3 modes.
    assert_eq!(seen, 72);
    assert_eq!(decode_hex("1b5b").as_deref(), Some(&[0x1b, 0x5b][..]));
    Ok(())
}

// --- OSC 52 (plan M7 step 7) --------------------------------------------------------------------

mod osc52 {
    use lune_vt::ext::osc52::{Osc52Outcome, parse_osc52};
    use lune_vt::{HostEvent, StandardExtensions, Terminal};
    use lune_vt_conformance::lunegit_input::{InputKind, expand_spec, load_input};

    use super::{Result, bytes_hex, field, num, section};

    /// SHA-256 (FIPS 180-4), so the golden's text digests can be checked without a dependency.
    pub(super) fn sha256(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut message = data.to_vec();
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
        for block in message.chunks(64) {
            let mut w = [0u32; 64];
            for (i, word) in block.chunks(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
                let t1 = v[7]
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v = [
                    t1.wrapping_add(t2),
                    v[0],
                    v[1],
                    v[2],
                    v[3].wrapping_add(t1),
                    v[4],
                    v[5],
                    v[6],
                ];
            }
            for (a, b) in h.iter_mut().zip(v) {
                *a = a.wrapping_add(b);
            }
        }
        h.iter().map(|word| format!("{word:08x}")).collect()
    }

    fn outcome_of(name: &str) -> Result<Osc52Outcome> {
        Ok(match name {
            "Copy" => Osc52Outcome::Copy,
            "Query" => Osc52Outcome::Query,
            "Malformed" => Osc52Outcome::Malformed,
            "Empty" => Osc52Outcome::Empty,
            "Oversize" => Osc52Outcome::Oversize,
            other => return Err(format!("unknown outcome {other}").into()),
        })
    }

    /// The instrument check: the digest function agrees with two known answers (the empty
    /// string and "hello", the latter also recorded by the golden's own rows).
    #[test]
    fn the_sha256_instrument_matches_known_answers() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            sha256(&[b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    /// Every real `Osc52Parser.Parse` outcome (the row `null` has no `&str` form).
    #[test]
    fn parse_osc52_matches_every_golden_row() -> Result<()> {
        let golden = load_input(InputKind::Osc52)?;
        let s = section(&golden, "parse")?;
        let mut failures = Vec::new();
        let mut checked = 0;
        for row in &s.rows {
            let name = field(s, row, "name")?;
            if field(s, row, "payload")? == "null" {
                continue;
            }
            let payload = String::from_utf8(expand_spec(field(s, row, "payload")?)?)?;
            let got = parse_osc52(&payload);
            let want = outcome_of(field(s, row, "outcome")?)?;
            let want_len = num(field(s, row, "length")?)? as usize;
            let mut bad = Vec::new();
            if got.outcome != want {
                bad.push(format!("outcome {:?} want {want:?}", got.outcome));
            }
            if got.payload_length != want_len {
                bad.push(format!("length {} want {want_len}", got.payload_length));
            }
            match (&got.text, want == Osc52Outcome::Copy) {
                (Some(text), true) => {
                    let want_bytes = num(field(s, row, "text-bytes")?)? as usize;
                    if text.len() != want_bytes {
                        bad.push(format!("text bytes {} want {want_bytes}", text.len()));
                    }
                    let hex = field(s, row, "text-hex")?;
                    if hex != "-" && bytes_hex(text.as_bytes()) != hex {
                        bad.push("text differs".to_owned());
                    }
                    let sha = field(s, row, "text-sha256")?;
                    if sha256(text.as_bytes()) != sha {
                        bad.push("text sha256 differs".to_owned());
                    }
                }
                (None, false) => {}
                (text, _) => bad.push(format!("text present: {}", text.is_some())),
            }
            if !bad.is_empty() {
                failures.push(format!("{name}: {}", bad.join("; ")));
            }
            checked += 1;
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert_eq!(checked, s.rows.len() - 1);
        Ok(())
    }

    /// The C0 controls the parser drops inside an OSC string before any handler sees them
    /// (everything below 0x20 but BEL, ESC, CAN and SUB, which end or abort the string).
    fn dropped_by_the_parser(byte: u8) -> bool {
        byte < 0x20 && !matches!(byte, 0x07 | 0x18 | 0x1a | 0x1b)
    }

    /// The same rows through `Terminal::write` with the gate on. The parser's framing differs
    /// from a bare string in one way the golden cannot show: it drops C0 controls (TAB, CR, LF,
    /// VT, FF, NUL, FS, ...) before the handler, so the expectation is the golden's rule applied
    /// to the payload with those removed. A C1 control (U+0080..U+009F, from UTF-8 `C2 80..9F`)
    /// aborts the OSC in the parser table, which the golden's NEL rows show: they are Malformed
    /// there and queue nothing here. Query/Malformed/Empty/Oversize queue nothing.
    #[test]
    fn the_engine_handler_agrees_with_the_parser_on_every_golden_row() -> Result<()> {
        let golden = load_input(InputKind::Osc52)?;
        let s = section(&golden, "parse")?;
        let mut failures = Vec::new();
        let mut copies = 0;
        for row in &s.rows {
            let name = field(s, row, "name")?;
            if field(s, row, "payload")? == "null" {
                continue;
            }
            let payload = expand_spec(field(s, row, "payload")?)?;
            if payload
                .iter()
                .any(|b| matches!(b, 0x07 | 0x18 | 0x1a | 0x1b))
            {
                return Err(format!("{name}: payload holds a terminator").into());
            }
            let seen: Vec<u8> = payload
                .iter()
                .copied()
                .filter(|b| !dropped_by_the_parser(*b))
                .collect();
            let want = parse_osc52(&String::from_utf8(seen)?);
            let mut t = Terminal::new([]);
            t.install_standard_extensions(&StandardExtensions {
                clipboard_write: true,
                ..StandardExtensions::default()
            })?;
            t.write(b"\x1b]52;");
            t.write(&payload);
            t.write(b"\x07");
            let events = t.drain_events();
            let got: Vec<&String> = events
                .iter()
                .filter_map(|e| match e {
                    HostEvent::ClipboardWrite(text) => Some(text),
                    _ => None,
                })
                .collect();
            let ok = match (&want.text, got.as_slice()) {
                (Some(text), [one]) => *one == text,
                (None, []) => true,
                _ => false,
            };
            let unknown = events
                .iter()
                .any(|e| matches!(e, HostEvent::UnknownOsc { .. }));
            if !ok || unknown {
                failures.push(format!(
                    "{name}: want outcome {:?}, got {} clipboard events, unknown-osc {unknown}",
                    want.outcome,
                    got.len()
                ));
            }
            copies += usize::from(want.text.is_some());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        // the instrument saw real copies (a loop that checks nothing would pass the rest)
        assert!(copies > 40, "{copies}");
        Ok(())
    }
}

// --- OSC 7 / 9;9 / 9;4 / 133 and the 2026 latch (plan M7 step 8) ----------------------------------
//
// The three scanner goldens are replayed through `Terminal::write`, step by step, one terminal per
// case (the scanner state carries across the steps of a case, so does the terminal). The golden
// columns are the scanner's state after each step; the terminal reports events, so each file has a
// small model that folds the events into the same state. Differences from Lunegit, all listed:
//  - framing (plan M7 D3): the parser's OSC framing is used, not a byte scanner's. No golden row
//    depends on it: none has an ESC, CAN, SUB or C1 control inside an OSC body, a payload over the
//    parser's limit, or a C0 control the parser would drop;
//  - the control-character tightening: a cwd with a C0, DEL or C1 control after decoding is
//    dropped (row `osc7_nul_escape`: Lunegit gives "/" + NUL, here: no event);
//  - `sync_intermediate` (`CSI ? 2026 SP h`): Lunegit's scanner ignores intermediates and latches;
//    with an intermediate the sequence is a different function in the parser, so the latch does
//    not set and nothing is queued;
//  - the latch is visible to the host only while a command runs (`SyncUpdateInCommand`), so the
//    golden's `sync` column is compared as `sync && running`;
//  - a lone ESC inside an OSC ends it at the ESC (the parser's table, as upstream), where
//    Lunegit's scanner waits for the `\` of ST. A step that ends with ESC and is followed by a
//    step that is `5c` is therefore compared with the state after the `5c` step.

mod osc_scanners {
    use lune_vt::{CommandMarkKind, HostEvent, ProgressState, StandardExtensions, Terminal};
    use lune_vt_conformance::lunegit_input::{InputKind, InputSection, expand_spec, load_input};

    use super::osc52::sha256;
    use super::{Result, field, num, section};

    /// The golden row to compare step `i` against: the next one when step `i` ends with ESC and
    /// the next step is the `\` of ST (see the header).
    fn expected_row<'a>(
        s: &InputSection,
        steps: &[&'a Vec<String>],
        i: usize,
    ) -> Result<&'a Vec<String>> {
        let row = steps.get(i).ok_or("no step")?;
        let ends_with_esc = field(s, row, "bytes")?.ends_with("1b");
        if let (true, Some(next)) = (ends_with_esc, steps.get(i + 1)) {
            if field(s, next, "bytes")? == "5c" {
                return Ok(next);
            }
        }
        Ok(row)
    }

    /// A case name and its rows, one per step.
    type Case<'a> = (&'a str, Vec<&'a Vec<String>>);

    /// The rows of a section grouped by `case`, steps in file order.
    fn cases(s: &InputSection) -> Result<Vec<Case<'_>>> {
        let mut out: Vec<Case<'_>> = Vec::new();
        for row in &s.rows {
            let case = field(s, row, "case")?;
            let step: usize = field(s, row, "step")?.parse()?;
            match out.last_mut() {
                Some((name, steps)) if *name == case => {
                    if steps.len() != step {
                        return Err(format!("{case}: step {step} out of order").into());
                    }
                    steps.push(row);
                }
                _ => {
                    if step != 0 {
                        return Err(format!("{case}: first step is {step}").into());
                    }
                    out.push((case, vec![row]));
                }
            }
        }
        Ok(out)
    }

    fn terminal() -> Result<Terminal> {
        let mut t = Terminal::new([]);
        t.install_standard_extensions(&StandardExtensions::default())?;
        Ok(t)
    }

    #[test]
    fn osc_cwd_golden_rows_are_reproduced() -> Result<()> {
        let golden = load_input(InputKind::OscCwd)?;
        let s = section(&golden, "feed")?;
        let mut failures = Vec::new();
        let (mut steps_checked, mut tightened) = (0, 0);
        for (case, steps) in cases(s)? {
            let mut t = terminal()?;
            let mut latest: Option<Vec<u8>> = None;
            let mut want_latest: Option<(Option<Vec<u8>>, usize, String)> = None;
            for (i, row) in steps.iter().enumerate() {
                t.write(&expand_spec(field(s, row, "bytes")?)?);
                for event in t.drain_events() {
                    if let HostEvent::CwdChanged(path) = event {
                        latest = Some(path.into_bytes());
                    }
                }
                let want_row = expected_row(s, &steps, i)?;
                // the value is in `cwd-hex`, or, past 256 characters, only its length and digest
                let golden_cwd: Option<(Option<Vec<u8>>, usize, String)> = match (
                    field(s, want_row, "cwd-hex")?,
                    field(s, want_row, "cwd-bytes")?,
                ) {
                    ("-", "-") => None,
                    ("-", len) => Some((
                        None,
                        len.parse()?,
                        field(s, want_row, "cwd-sha256")?.to_owned(),
                    )),
                    (hex, len) => Some((
                        Some(expand_spec(hex)?),
                        len.parse()?,
                        field(s, want_row, "cwd-sha256")?.to_owned(),
                    )),
                };
                let golden_cwd = match golden_cwd {
                    Some((Some(bytes), _, _)) if bytes.iter().any(|b| *b < 0x20 || *b == 0x7f) => {
                        // the tightening: dropped, so the state does not move
                        tightened += 1;
                        want_latest.clone()
                    }
                    other => other,
                };
                want_latest = golden_cwd;
                let same = match (&latest, &want_latest) {
                    (None, None) => true,
                    (Some(got), Some((hex, len, sha))) => {
                        got.len() == *len
                            && hex.as_ref().is_none_or(|bytes| bytes == got)
                            && sha256(got) == *sha
                    }
                    _ => false,
                };
                if !same {
                    failures.push(format!(
                        "{case} step {}: latest {} bytes, want {} bytes",
                        field(s, row, "step")?,
                        latest.as_ref().map_or(0, Vec::len),
                        want_latest.as_ref().map_or(0, |(_, len, _)| *len)
                    ));
                }
                steps_checked += 1;
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert_eq!(tightened, 1, "the one row with a control character");
        assert_eq!(steps_checked, s.rows.len());
        Ok(())
    }

    #[test]
    fn osc_progress_golden_rows_are_reproduced() -> Result<()> {
        let golden = load_input(InputKind::OscProgress)?;
        let s = section(&golden, "feed")?;
        let mut failures = Vec::new();
        for (case, steps) in cases(s)? {
            let mut t = terminal()?;
            let mut latest = (ProgressState::None, 0u8);
            for (i, row) in steps.iter().enumerate() {
                t.write(&expand_spec(field(s, row, "bytes")?)?);
                for event in t.drain_events() {
                    if let HostEvent::Progress(p) = event {
                        latest = (p.state, p.percent);
                    }
                }
                let row = expected_row(s, &steps, i)?;
                let state = match field(s, row, "state")? {
                    "None" => ProgressState::None,
                    "Normal" => ProgressState::Normal,
                    "Error" => ProgressState::Error,
                    "Indeterminate" => ProgressState::Indeterminate,
                    "Warning" => ProgressState::Warning,
                    other => return Err(format!("unknown state {other}").into()),
                };
                let want = (state, u8::try_from(num(field(s, row, "percent")?)?)?);
                if latest != want {
                    failures.push(format!(
                        "{case} step {}: got {:?} want {want:?}",
                        field(s, row, "step")?,
                        latest
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }

    #[test]
    fn osc_command_golden_rows_are_reproduced() -> Result<()> {
        let golden = load_input(InputKind::OscCommand)?;
        let s = section(&golden, "feed")?;
        let mut failures = Vec::new();
        for (case, steps) in cases(s)? {
            let mut t = terminal()?;
            let (mut running, mut starts, mut sync) = (false, 0u32, false);
            for (i, row) in steps.iter().enumerate() {
                t.write(&expand_spec(field(s, row, "bytes")?)?);
                let row = expected_row(s, &steps, i)?;
                for event in t.drain_events() {
                    match event {
                        HostEvent::CommandMark(mark) => match mark.kind {
                            CommandMarkKind::OutputStart => {
                                running = true;
                                starts += 1;
                                sync = false;
                            }
                            CommandMarkKind::PromptStart | CommandMarkKind::CommandFinished => {
                                running = false;
                                sync = false;
                            }
                            CommandMarkKind::PromptEnd => {}
                        },
                        HostEvent::SyncUpdateInCommand => sync = true,
                        _ => {}
                    }
                }
                let want_running = field(s, row, "running")? == "1";
                let want_starts = num(field(s, row, "starts")?)?;
                // the host sees the latch only while a command runs
                let mut want_sync = field(s, row, "sync")? == "1" && want_running;
                if case == "sync_intermediate" && field(s, row, "step")? != "0" {
                    want_sync = false;
                }
                if (running, starts, sync) != (want_running, want_starts, want_sync) {
                    failures.push(format!(
                        "{case} step {}: got running {running} starts {starts} sync {sync}, \
                         want {want_running} {want_starts} {want_sync}",
                        field(s, row, "step")?
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}
