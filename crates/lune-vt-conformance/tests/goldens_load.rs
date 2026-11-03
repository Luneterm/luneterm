//! Every committed golden loads, the counts match the corpus, and the reader rejects malformed
//! input with a `FormatError` (never a panic).

use lune_vt_conformance::lunegit_input::{
    self, InputGolden, InputKind, InputSection, expand_spec, load_input, parse_input,
};
use lune_vt_conformance::{
    Golden, Kind, MAGIC, corpus_summary, crate_dir, escape_code_points, escape_str, load, parse,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn named<'a>(goldens: &'a [Golden], name: &str) -> Result<&'a Golden> {
    goldens
        .iter()
        .find(|g| g.name == name)
        .ok_or_else(|| format!("no golden named {name}").into())
}

#[test]
fn parser_goldens_match_the_corpus_counts() -> Result<()> {
    let corpus = corpus_summary("parser.cases")?;
    let goldens = load(Kind::Parser)?;
    assert!(
        corpus.names.len() >= 40,
        "hand-written parser cases: {}",
        corpus.names.len()
    );
    assert!(
        corpus.random >= 200,
        "random parser cases: {}",
        corpus.random
    );
    assert_eq!(goldens.len(), corpus.names.len() + corpus.random);
    for name in &corpus.names {
        named(&goldens, name)?;
    }
    for i in 1..=corpus.random {
        named(&goldens, &format!("rand_{i:04}"))?;
    }
    assert!(goldens.iter().all(|g| g.kind == Kind::Parser));
    Ok(())
}

#[test]
fn screen_goldens_match_the_corpus_counts() -> Result<()> {
    let screen = corpus_summary("screen.cases")?;
    let lunegit = corpus_summary("lunegit.cases")?;
    let goldens = load(Kind::Screen)?;
    assert!(
        screen.names.len() >= 150,
        "screen cases: {}",
        screen.names.len()
    );
    assert!(
        screen.random >= 100,
        "random screen cases: {}",
        screen.random
    );
    assert_eq!(
        goldens.len(),
        screen.names.len() + screen.random + lunegit.names.len()
    );
    for name in &screen.names {
        named(&goldens, name)?;
    }
    for i in 1..=screen.random {
        named(&goldens, &format!("srand_{i:04}"))?;
    }
    for name in &lunegit.names {
        named(&goldens, &format!("lunegit_{name}"))?;
    }
    for g in &goldens {
        assert!(
            g.expect.first().is_some_and(|l| l.starts_with("cursor ")),
            "{}",
            g.name
        );
        assert!(g.size.is_some(), "{} has no size", g.name);
    }
    Ok(())
}

#[test]
fn lunegit_grid_goldens_are_the_27_vectors() -> Result<()> {
    let lunegit = corpus_summary("lunegit.cases")?;
    let goldens = load(Kind::Grid)?;
    assert_eq!(lunegit.names.len(), 27);
    assert_eq!(goldens.len(), 27);
    for name in &lunegit.names {
        let g = named(&goldens, name)?;
        assert!(g.size.is_some(), "{name}");
        assert!(
            g.expect
                .iter()
                .all(|l| l.starts_with('"') && l.ends_with('"')),
            "{name}"
        );
    }
    let divergent: Vec<&str> = goldens
        .iter()
        .filter(|g| g.divergent.is_some())
        .map(|g| g.name.as_str())
        .collect();
    assert_eq!(
        divergent,
        [
            "shrink_wrapped",
            "grow_wrapped",
            "shrink_multiline",
            "grow_after_wrap",
            "shrink_then_text",
            "viewport_resize"
        ]
    );
    assert_eq!(named(&goldens, "shrink_wrapped")?.resize, Some((8, 4)));
    assert_eq!(
        named(&goldens, "wrap_basic")?.expect,
        ["\"0123456789\"", "\"AB\""]
    );
    Ok(())
}

#[test]
fn split_utf8_sequence_golden_expects_the_unsplit_trace() -> Result<()> {
    let goldens = load(Kind::Parser)?;
    let g = named(&goldens, "utf8_split_after_80")?;
    assert_eq!(
        g.divergent.as_deref(),
        Some("upstream Utf8ToUtf32 split-sequence bug (TextDecoder.ts:161)")
    );
    assert_eq!(g.chunks, [vec![0xE2, 0x80], vec![0x99, b'A']]);
    assert_eq!(g.expect, ["print \"\\u{2019}A\""]);
    Ok(())
}

#[test]
fn goldens_stay_within_one_mebibyte() -> Result<()> {
    let mut total = 0u64;
    for dir in ["parser", "screen", "lunegit", "lunegit-input"] {
        for entry in std::fs::read_dir(crate_dir().join("goldens").join(dir))? {
            total += entry?.metadata()?.len();
        }
    }
    assert!(total <= 1024 * 1024, "goldens are {total} bytes");
    Ok(())
}

fn record(body: &str) -> String {
    format!("{MAGIC}\n{body}")
}

const GOOD: &str = "kind parser\nname a_b1\nsource hand\nsize 80 24\nresize 40 12\n\
                    divergent why\nchunk 1b5b\nchunk -\nexpect\nprint \"x\"\nend\n";

#[test]
fn a_minimal_record_parses() -> Result<()> {
    let goldens = parse("mem", &record(GOOD))?;
    assert_eq!(goldens.len(), 1);
    let g = goldens.first().ok_or("no golden")?;
    assert_eq!(g.kind, Kind::Parser);
    assert_eq!(g.name, "a_b1");
    assert_eq!(g.source, "hand");
    assert_eq!(g.size, Some((80, 24)));
    assert_eq!(g.resize, Some((40, 12)));
    assert_eq!(g.divergent.as_deref(), Some("why"));
    assert_eq!(g.chunks, [vec![0x1b, 0x5b], vec![]]);
    assert_eq!(g.expect, ["print \"x\""]);
    Ok(())
}

#[test]
fn crlf_and_blank_lines_between_records_are_tolerated() -> Result<()> {
    let two = record(&format!("{GOOD}\n{}", GOOD.replace("a_b1", "second")));
    let lf = parse("mem", &two)?;
    let crlf = parse("mem", &two.replace('\n', "\r\n"))?;
    assert_eq!(lf.len(), 2);
    assert_eq!(lf, crlf);
    Ok(())
}

#[test]
fn an_empty_expect_block_is_allowed() -> Result<()> {
    let goldens = parse("mem", &record("kind grid\nname e\nchunk 41\nexpect\nend\n"))?;
    assert!(goldens.first().ok_or("no golden")?.expect.is_empty());
    Ok(())
}

fn rejects(text: &str, needle: &str) {
    match parse("mem", text) {
        Ok(g) => panic!(
            "expected an error containing '{needle}', parsed {} golden(s)",
            g.len()
        ),
        Err(e) => {
            assert!(
                e.message.contains(needle),
                "'{}' does not contain '{needle}'",
                e.message
            );
            assert_eq!(e.file, "mem");
            assert!(e.to_string().starts_with("mem:"));
        }
    }
}

#[test]
fn malformed_input_returns_format_errors() {
    rejects("", "empty file");
    rejects("lune-vt-golden 2\n", "expected");
    rejects("\n", "expected");
    rejects(
        &record("kind parser\nname a\nbogus 1\nchunk 41\nexpect\nend\n"),
        "unknown header key",
    );
    rejects(
        &record("kind parser\nname a\nchunk 41\nexpect\nprint \"x\"\n"),
        "no 'end'",
    );
    rejects(
        &record("kind parser\nname a\nchunk 4\nexpect\nend\n"),
        "even length",
    );
    rejects(
        &record("kind parser\nname a\nchunk 4g\nexpect\nend\n"),
        "lowercase hex",
    );
    rejects(
        &record("kind parser\nname a\nchunk 4A\nexpect\nend\n"),
        "lowercase hex",
    );
    rejects(
        &record("kind parser\nname a\nchunk \nexpect\nend\n"),
        "even length",
    );
    rejects(
        &record("kind parser\nname a\nchunk 41\nchunk\nexpect\nend\n"),
        "even length",
    );
    rejects(
        &record("kind parser\nname a\nchunk 41\nexpect\n\nend\n"),
        "blank line",
    );
    rejects(
        &record("kind parser\nname a\nchunk 41\nsize 1 1\nexpect\nend\n"),
        "after a chunk",
    );
    rejects(
        &record("kind parser\nname a\nkind screen\nchunk 41\nexpect\nend\n"),
        "duplicate",
    );
    rejects(&record("name a\nchunk 41\nexpect\nend\n"), "no 'kind'");
    rejects(&record("kind parser\nchunk 41\nexpect\nend\n"), "no 'name'");
    rejects(&record("kind parser\nname a\nexpect\nend\n"), "no 'chunk'");
    rejects(
        &record("kind parser\nname a\nchunk 41\n"),
        "before 'expect'",
    );
    rejects(
        &record("kind bogus\nname a\nchunk 41\nexpect\nend\n"),
        "unknown kind",
    );
    rejects(
        &record("kind parser\nname A\nchunk 41\nexpect\nend\n"),
        "bad name",
    );
    rejects(
        &record("kind parser\nname a\nsize 0 5\nchunk 41\nexpect\nend\n"),
        "bad dimension",
    );
    rejects(
        &record("kind parser\nname a\nsize 5\nchunk 41\nexpect\nend\n"),
        "cols",
    );
    rejects(
        &record("kind parser\nname a\ndivergent\nchunk 41\nexpect\nend\n"),
        "divergent",
    );
}

#[test]
fn errors_name_the_line() {
    let text = record("kind parser\nname a\nchunk zz\nexpect\nend\n");
    match parse("f.golden", &text) {
        Ok(_) => panic!("expected an error"),
        Err(e) => {
            assert_eq!(e.line, 4);
            assert_eq!(e.to_string(), format!("f.golden:4: {}", e.message));
        }
    }
}

#[test]
fn escaping_follows_the_format() {
    assert_eq!(escape_str("abc xyz~"), "\"abc xyz~\"");
    assert_eq!(escape_str("\"\\"), "\"\\u{22}\\u{5C}\"");
    assert_eq!(
        escape_str("\u{1b}[é你\u{1F680}\u{7f}"),
        "\"\\u{1B}[\\u{E9}\\u{4F60}\\u{1F680}\\u{7F}\""
    );
    assert_eq!(escape_code_points([]), "\"\"");
    // Code points that are not scalar values (surrogates) are still escaped, never dropped.
    assert_eq!(escape_code_points([0xD800, 0x41]), "\"\\u{D800}A\"");
    assert_eq!(
        escape_code_points([0x20, 0x7E, 0x7F, 0x1F]),
        "\" ~\\u{7F}\\u{1F}\""
    );
}

// ---- Lunegit input goldens (goldens/lunegit-input, written by tools/lunegit-oracle) ----

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn section<'a>(golden: &'a InputGolden, name: &str) -> Result<&'a InputSection> {
    golden
        .section(name)
        .ok_or_else(|| format!("no section {name}").into())
}

fn distinct(section: &InputSection, column: &str) -> Result<std::collections::BTreeSet<String>> {
    let index = section
        .column(column)
        .ok_or_else(|| format!("no column {column}"))?;
    Ok(section
        .rows
        .iter()
        .filter_map(|row| row.get(index).cloned())
        .collect())
}

#[test]
fn lunegit_input_goldens_load_with_provenance() -> Result<()> {
    for kind in InputKind::ALL {
        let golden = load_input(kind)?;
        assert_eq!(golden.kind, kind);
        assert_eq!(golden.header("tool"), Some("lunegit-oracle 1"), "{kind:?}");
        let commit = golden.header("lunegit-commit").ok_or("no commit header")?;
        assert!(
            commit.len() >= 7 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "{kind:?}: commit {commit}"
        );
        for key in ["core-dll-sha256", "infrastructure-dll-sha256"] {
            let value = golden.header(key).ok_or("missing DLL hash header")?;
            assert!(sha256_hex(value), "{kind:?}: {key} = {value}");
        }
        assert!(!golden.sections.is_empty(), "{kind:?}");
        for s in &golden.sections {
            assert!(!s.rows.is_empty(), "{kind:?}/{}", s.name);
        }
    }
    // The five files were produced by one run over one pair of DLLs.
    let first = load_input(InputKind::Keys)?;
    for kind in InputKind::ALL {
        let other = load_input(kind)?;
        assert_eq!(other.header, first.header, "{kind:?}");
    }
    Ok(())
}

#[test]
fn lunegit_key_goldens_cover_the_grid() -> Result<()> {
    let keys = load_input(InputKind::Keys)?;
    let encode_key = section(&keys, "encode-key")?;
    assert_eq!(
        distinct(encode_key, "key")?.len(),
        27,
        "every key but Character"
    );
    assert_eq!(distinct(encode_key, "mods")?.len(), 8);
    assert_eq!(distinct(encode_key, "alt")?.len(), 2);
    assert_eq!(distinct(encode_key, "keypad")?.len(), 2);
    assert_eq!(distinct(encode_key, "modes")?.len(), 8);
    let encode_char = section(&keys, "encode-char")?;
    assert_eq!(distinct(encode_char, "mods")?.len(), 8);
    assert_eq!(distinct(encode_char, "modes")?.len(), 8);
    assert!(distinct(encode_char, "cp")?.len() >= 100);
    assert!(distinct(encode_char, "shifted")?.len() > 1);
    assert!(distinct(encode_char, "base")?.len() > 1);
    let generate_key = section(&keys, "generate-key-input")?;
    assert_eq!(distinct(generate_key, "key")?.len(), 28, "every key");
    assert_eq!(generate_key.rows.len(), 28 * 8 * 2);
    let generate_char = section(&keys, "generate-char-input")?;
    assert!(distinct(generate_char, "char")?.len() >= 127);
    // Every output is hex bytes (or `-` for nothing sent).
    for s in &keys.sections {
        for row in &s.rows {
            let out = s.field(row, "out").ok_or("no out column")?;
            expand_spec(out)?;
        }
    }
    // A row from the first real run of the probe (Up with Ctrl under no protocol): ESC [ 1 ; 5 A.
    let up_ctrl = encode_key
        .rows
        .iter()
        .find(|r| {
            ["key", "mods", "alt", "keypad", "modes"]
                .iter()
                .zip(["Up", "4", "1", "0", "0"])
                .all(|(c, v)| encode_key.field(r, c) == Some(v))
        })
        .ok_or("no Up+Ctrl row")?;
    assert_eq!(encode_key.field(up_ctrl, "out"), Some("1b5b313b3541"));
    Ok(())
}

#[test]
fn lunegit_osc_goldens_have_expandable_inputs() -> Result<()> {
    let osc52 = load_input(InputKind::Osc52)?;
    let parse52 = section(&osc52, "parse")?;
    let outcomes = distinct(parse52, "outcome")?;
    for outcome in ["Copy", "Query", "Malformed", "Empty", "Oversize"] {
        assert!(outcomes.contains(outcome), "no {outcome} outcome recorded");
    }
    let mut largest = 0;
    for row in &parse52.rows {
        let payload = parse52.field(row, "payload").ok_or("no payload")?;
        if payload != "null" {
            largest = largest.max(expand_spec(payload)?.len());
        }
        if let (Some(len), Some(hex)) = (
            parse52.field(row, "text-bytes"),
            parse52.field(row, "text-hex"),
        ) {
            if hex != "-" {
                assert_eq!(len.parse::<usize>()?, expand_spec(hex)?.len());
            }
        }
    }
    assert!(
        largest > 1_398_104,
        "the size-cap cases are generated, not inline"
    );

    for (kind, outputs) in [
        (
            InputKind::OscCwd,
            ["cwd", "cwd-bytes", "cwd-sha256"].as_slice(),
        ),
        (InputKind::OscProgress, ["state", "percent"].as_slice()),
        (
            InputKind::OscCommand,
            ["running", "sync", "starts"].as_slice(),
        ),
    ] {
        let golden = load_input(kind)?;
        let feed = section(&golden, "feed")?;
        assert_eq!(feed.inputs, ["case", "step", "bytes"]);
        for column in outputs {
            assert!(feed.column(column).is_some(), "{kind:?} lacks {column}");
        }
        let mut max_feed = 0;
        for row in &feed.rows {
            max_feed =
                max_feed.max(expand_spec(feed.field(row, "bytes").ok_or("no bytes")?)?.len());
        }
        assert!(max_feed >= 64, "{kind:?}: the cap cases are present");
    }
    let cwd = load_input(InputKind::OscCwd)?;
    assert!(
        section(&cwd, "feed")?
            .rows
            .iter()
            .any(|r| r.first().map(String::as_str) == Some("body_4097")),
        "cap + 1 case"
    );
    Ok(())
}

#[test]
fn lunegit_input_golden_fields_stay_short() -> Result<()> {
    for kind in InputKind::ALL {
        let golden = load_input(kind)?;
        for s in &golden.sections {
            for row in &s.rows {
                for field in row {
                    assert!(
                        field.chars().count() <= lunegit_input::MAX_FIELD_CHARS,
                        "{kind:?}/{}: {} characters",
                        s.name,
                        field.chars().count()
                    );
                }
            }
        }
    }
    Ok(())
}

const INPUT_GOOD: &str = "lune-lunegit-golden 1\nheader tool t 1\nkind osc52\nsection parse\n\
                          inputs name payload\noutputs outcome\na\t61\tCopy\nend\n";

#[test]
fn a_minimal_lunegit_input_file_parses() -> Result<()> {
    let golden = parse_input("mem", INPUT_GOOD)?;
    assert_eq!(golden.kind, InputKind::Osc52);
    assert_eq!(golden.header("tool"), Some("t 1"));
    let s = section(&golden, "parse")?;
    assert_eq!(s.rows, [["a", "61", "Copy"]]);
    let row = s.rows.first().ok_or("no row")?;
    assert_eq!(s.field(row, "outcome"), Some("Copy"));
    assert_eq!(s.field(row, "bogus"), None);
    let crlf = parse_input("mem", &INPUT_GOOD.replace('\n', "\r\n"))?;
    assert_eq!(crlf, golden);
    Ok(())
}

fn input_rejects(text: &str, needle: &str) {
    match parse_input("mem", text) {
        Ok(_) => panic!("expected an error containing '{needle}'"),
        Err(e) => assert!(
            e.message.contains(needle),
            "'{}' does not contain '{needle}'",
            e.message
        ),
    }
}

#[test]
fn malformed_lunegit_input_returns_format_errors() {
    input_rejects("", "empty file");
    input_rejects("lune-vt-golden 1\n", "expected");
    input_rejects("lune-lunegit-golden 1\nkind nope\n", "unknown kind");
    input_rejects("lune-lunegit-golden 1\n", "no 'kind'");
    input_rejects("lune-lunegit-golden 1\nkind keys\n", "no sections");
    input_rejects(
        &INPUT_GOOD.replace("header tool t 1\n", "header tool\n"),
        "bad header",
    );
    input_rejects(
        &INPUT_GOOD.replace("header tool t 1\n", "header tool a\nheader tool b\n"),
        "duplicate header",
    );
    input_rejects(&INPUT_GOOD.replace("a\t61\tCopy", "a\t61"), "2 fields");
    input_rejects(
        &INPUT_GOOD.replace("a\t61\tCopy", "a\t\tCopy"),
        "empty is '-'",
    );
    input_rejects(
        &INPUT_GOOD.replace("a\t61", &format!("a\t{}", "6".repeat(257))),
        "257 characters",
    );
    input_rejects(&INPUT_GOOD.replace("end\n", ""), "no 'end'");
    input_rejects(
        &INPUT_GOOD.replace("inputs name payload", "inputs Name payload"),
        "bad column list",
    );
    input_rejects(
        &INPUT_GOOD.replace("outputs outcome\n", ""),
        "expected 'outputs",
    );
    input_rejects(
        &format!("{INPUT_GOOD}section parse\ninputs a\noutputs b\nend\n"),
        "duplicate section",
    );
    input_rejects(&format!("{INPUT_GOOD}stray\n"), "expected 'section'");
}

#[test]
fn byte_specs_expand_and_reject_malformed_input() -> Result<()> {
    assert_eq!(expand_spec("-")?, Vec::<u8>::new());
    assert_eq!(expand_spec("1b5d")?, [0x1b, 0x5d]);
    assert_eq!(expand_spec("1b+61x3+07")?, [0x1b, 0x61, 0x61, 0x61, 0x07]);
    assert_eq!(expand_spec("6162x2")?, [0x61, 0x62, 0x61, 0x62]);
    assert_eq!(expand_spec("61x1398104")?.len(), 1_398_104);
    for bad in [
        "",
        "zz",
        "1B",
        "6",
        "61x",
        "x3",
        "61x-1",
        "61x99999999999999999999",
        "61x9999999",
        "1b++07",
        "1b+",
        "+1b",
    ] {
        assert!(expand_spec(bad).is_err(), "'{bad}' should be rejected");
    }
    Ok(())
}
