//! Every committed golden loads, the counts match the corpus, and the reader rejects malformed
//! input with a `FormatError` (never a panic).

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
    for dir in ["parser", "screen", "lunegit"] {
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
