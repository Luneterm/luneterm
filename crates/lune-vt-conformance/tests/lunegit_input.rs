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
