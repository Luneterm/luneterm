//! Conformance data for `lune-vt`: a reader for the committed goldens and the string-escape
//! helper shared with the trace runners.
//!
//! The formats are specified in `FORMAT.md` next to this crate's `Cargo.toml`; the goldens are
//! generated from the reference xterm.js by `scripts/regen-vt-goldens.sh` and committed, so
//! nothing here needs Node. [`trace`] is the parser-trace runner (`tests/parser_trace.rs`; the
//! `lune-vt` fuzz test includes the same file by path, see its header); [`screen`] renders the
//! screen dump and Lunegit's grid from a `lune_vt::Terminal` (`tests/screen_goldens.rs`,
//! `tests/lunegit_vectors.rs`); [`lunegit_input`] reads the Lunegit key-encoder and OSC-scanner
//! goldens (`goldens/lunegit-input/`, written by `tools/lunegit-oracle`).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub mod escape;
pub mod lunegit_input;
pub mod screen;
pub mod trace;

pub use escape::{escape_code_points, escape_str};

/// Magic first line of every golden file.
pub const MAGIC: &str = "lune-vt-golden 1";

/// What a golden's `expect` lines describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Parser event trace (`goldens/parser/`).
    Parser,
    /// Screen dump (`goldens/screen/`).
    Screen,
    /// Visible grid in Lunegit's normalisation (`goldens/lunegit/`).
    Grid,
}

impl Kind {
    /// The spelling used by the `kind` header line.
    pub fn keyword(self) -> &'static str {
        match self {
            Kind::Parser => "parser",
            Kind::Screen => "screen",
            Kind::Grid => "grid",
        }
    }

    fn from_keyword(word: &str) -> Option<Kind> {
        match word {
            "parser" => Some(Kind::Parser),
            "screen" => Some(Kind::Screen),
            "grid" => Some(Kind::Grid),
            _ => None,
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            Kind::Parser => "parser",
            Kind::Screen => "screen",
            Kind::Grid => "lunegit",
        }
    }
}

/// One golden record: an input, an optional geometry, and the expected result lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Golden {
    /// Unique within its kind.
    pub name: String,
    /// What `expect` holds.
    pub kind: Kind,
    /// Free text: corpus line or origin.
    pub source: String,
    /// Terminal size `(cols, rows)` when the case sets one.
    pub size: Option<(u32, u32)>,
    /// Size applied after all chunks.
    pub resize: Option<(u32, u32)>,
    /// Why `lune-vt` is not expected to equal the reference xterm.js for this case, if so.
    pub divergent: Option<String>,
    /// The input, one entry per write; state carries across chunks.
    pub chunks: Vec<Vec<u8>>,
    /// Expected lines, in order.
    pub expect: Vec<String>,
}

/// A malformed golden or corpus file. Never produced by a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    /// File (or a label for in-memory text).
    pub file: String,
    /// 1-based line; 0 when the whole file is at fault (unreadable, empty).
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for FormatError {}

/// The crate's root directory (where `goldens/` and `corpus/` live).
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Directory holding the goldens of `kind`.
pub fn golden_dir(kind: Kind) -> PathBuf {
    crate_dir().join("goldens").join(kind.dir_name())
}

/// Loads every `*.golden` file of `kind`, in file-name order. Errors carry file and line.
pub fn load(kind: Kind) -> Result<Vec<Golden>, FormatError> {
    let dir = golden_dir(kind);
    let label = dir.display().to_string();
    let entries = fs::read_dir(&dir).map_err(|e| io_error(&label, &e))?;
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| io_error(&label, &e))?.path();
        if path.extension().is_some_and(|ext| ext == "golden") {
            files.push(path);
        }
    }
    files.sort();
    let mut all: Vec<Golden> = Vec::new();
    for path in files {
        let label = relative_label(&path);
        let text = fs::read_to_string(&path).map_err(|e| io_error(&label, &e))?;
        for golden in parse(&label, &text)? {
            if golden.kind != kind {
                return Err(error(
                    &label,
                    0,
                    format!(
                        "golden '{}' has kind {}, expected {}",
                        golden.name,
                        golden.kind.keyword(),
                        kind.keyword()
                    ),
                ));
            }
            if all.iter().any(|g| g.name == golden.name) {
                return Err(error(
                    &label,
                    0,
                    format!("duplicate golden name '{}'", golden.name),
                ));
            }
            all.push(golden);
        }
    }
    Ok(all)
}

/// Parses the text of one golden file. `file` is only used in error messages.
pub fn parse(file: &str, text: &str) -> Result<Vec<Golden>, FormatError> {
    let mut lines = Lines::new(text);
    match lines.next() {
        Some((_, first)) if first == MAGIC => {}
        Some((n, other)) => {
            return Err(error(
                file,
                n,
                format!("expected '{MAGIC}', found '{other}'"),
            ));
        }
        None => return Err(error(file, 0, "empty file".to_string())),
    }
    let mut goldens: Vec<Golden> = Vec::new();
    while let Some((n, line)) = lines.next() {
        if line.is_empty() {
            continue;
        }
        goldens.push(parse_record(file, n, line, &mut lines)?);
    }
    Ok(goldens)
}

/// Line iterator: 1-based numbers, `\n` separators, one trailing `\r` stripped per line.
struct Lines<'a> {
    inner: std::str::Split<'a, char>,
    number: usize,
    done: bool,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        Lines {
            inner: text.split('\n'),
            number: 0,
            done: false,
        }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let line = self.inner.next()?;
        self.number += 1;
        // The piece after a final '\n' is empty and is not a line.
        if line.is_empty() {
            let mut rest = self.inner.clone();
            if rest.next().is_none() {
                self.done = true;
                return None;
            }
        }
        Some((self.number, line.strip_suffix('\r').unwrap_or(line)))
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Header,
    Chunks,
}

fn parse_record(
    file: &str,
    first_line: usize,
    first: &str,
    lines: &mut Lines<'_>,
) -> Result<Golden, FormatError> {
    let mut kind: Option<Kind> = None;
    let mut name: Option<String> = None;
    let mut source: Option<String> = None;
    let mut size: Option<(u32, u32)> = None;
    let mut resize: Option<(u32, u32)> = None;
    let mut divergent: Option<String> = None;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut phase = Phase::Header;
    let mut current = Some((first_line, first));
    let mut last_line = first_line;

    // Header and chunk lines, up to `expect`.
    loop {
        let (n, line) = match current.take().or_else(|| lines.next()) {
            Some(pair) => pair,
            None => {
                return Err(error(
                    file,
                    last_line,
                    "record ends before 'expect'".to_string(),
                ));
            }
        };
        last_line = n;
        if line == "expect" {
            break;
        }
        let (key, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, r),
            None => (line, ""),
        };
        match key {
            "chunk" => {
                phase = Phase::Chunks;
                chunks.push(decode_hex(file, n, rest)?);
            }
            "kind" | "name" | "source" | "size" | "resize" | "divergent" => {
                if phase > Phase::Header {
                    return Err(error(file, n, format!("header '{key}' after a chunk line")));
                }
                match key {
                    "kind" => {
                        set_once(file, n, key, &mut kind, || {
                            Kind::from_keyword(rest).ok_or_else(|| format!("unknown kind '{rest}'"))
                        })?;
                    }
                    "name" => {
                        set_once(file, n, key, &mut name, || {
                            if !rest.is_empty()
                                && rest.bytes().all(|b| {
                                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
                                })
                            {
                                Ok(rest.to_string())
                            } else {
                                Err(format!("bad name '{rest}'"))
                            }
                        })?;
                    }
                    "source" => set_once(file, n, key, &mut source, || Ok(rest.to_string()))?,
                    "size" => set_once(file, n, key, &mut size, || parse_pair(rest))?,
                    "resize" => set_once(file, n, key, &mut resize, || parse_pair(rest))?,
                    _ => set_once(file, n, key, &mut divergent, || {
                        if rest.is_empty() {
                            Err("empty divergent reason".to_string())
                        } else {
                            Ok(rest.to_string())
                        }
                    })?,
                }
            }
            other => return Err(error(file, n, format!("unknown header key '{other}'"))),
        }
    }

    let kind = kind.ok_or_else(|| error(file, first_line, "record has no 'kind'".to_string()))?;
    let name = name.ok_or_else(|| error(file, first_line, "record has no 'name'".to_string()))?;
    if chunks.is_empty() {
        return Err(error(
            file,
            last_line,
            format!("golden '{name}' has no 'chunk' line"),
        ));
    }

    // Expected lines, up to `end`.
    let mut expect: Vec<String> = Vec::new();
    loop {
        match lines.next() {
            None => {
                return Err(error(
                    file,
                    last_line,
                    format!("golden '{name}' has no 'end' line"),
                ));
            }
            Some((_, "end")) => break,
            Some((n, "")) => {
                return Err(error(
                    file,
                    n,
                    "blank line in the expected lines".to_string(),
                ));
            }
            Some((n, line)) => {
                last_line = n;
                expect.push(line.to_string());
            }
        }
    }

    Ok(Golden {
        name,
        kind,
        source: source.unwrap_or_default(),
        size,
        resize,
        divergent,
        chunks,
        expect,
    })
}

fn set_once<T>(
    file: &str,
    line: usize,
    key: &str,
    slot: &mut Option<T>,
    make: impl FnOnce() -> Result<T, String>,
) -> Result<(), FormatError> {
    if slot.is_some() {
        return Err(error(file, line, format!("duplicate header '{key}'")));
    }
    *slot = Some(make().map_err(|message| error(file, line, message))?);
    Ok(())
}

fn parse_pair(text: &str) -> Result<(u32, u32), String> {
    let (a, b) = text
        .split_once(' ')
        .ok_or_else(|| format!("expected '<cols> <rows>', found '{text}'"))?;
    let number = |s: &str| match s.parse::<u32>() {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(format!("bad dimension '{s}'")),
    };
    Ok((number(a)?, number(b)?))
}

fn decode_hex(file: &str, line: usize, text: &str) -> Result<Vec<u8>, FormatError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    let digits = text.as_bytes();
    if digits.is_empty() || digits.len() % 2 != 0 {
        return Err(error(
            file,
            line,
            format!("chunk hex must be non-empty with an even length: '{text}'"),
        ));
    }
    let nibble = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let (Some(hi), Some(lo)) = (
            pair.first().and_then(|b| nibble(*b)),
            pair.get(1).and_then(|b| nibble(*b)),
        ) else {
            return Err(error(
                file,
                line,
                format!("chunk is not lowercase hex: '{text}'"),
            ));
        };
        out.push(hi << 4 | lo);
    }
    Ok(out)
}

fn error(file: &str, line: usize, message: String) -> FormatError {
    FormatError {
        file: file.to_string(),
        line,
        message,
    }
}

fn io_error(file: &str, e: &std::io::Error) -> FormatError {
    error(file, 0, format!("cannot read: {e}"))
}

fn relative_label(path: &Path) -> String {
    path.strip_prefix(crate_dir())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The case names and generated-case count declared by a corpus file (`corpus/<file>`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CorpusSummary {
    /// `case <name>` lines, in order.
    pub names: Vec<String>,
    /// Count from a `random <count> seed <n>` line, 0 when absent.
    pub random: usize,
}

/// Reads the `case` and `random` lines of a corpus file; used to check golden counts.
pub fn corpus_summary(file: &str) -> Result<CorpusSummary, FormatError> {
    let path = crate_dir().join("corpus").join(file);
    let text = fs::read_to_string(&path).map_err(|e| io_error(file, &e))?;
    let mut summary = CorpusSummary::default();
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(name) = line.strip_prefix("case ") {
            summary.names.push(name.to_string());
        } else if let Some(rest) = line.strip_prefix("random ") {
            let count = rest.split(' ').next().and_then(|c| c.parse::<usize>().ok());
            summary.random =
                count.ok_or_else(|| error(file, index + 1, format!("bad random line '{line}'")))?;
        }
    }
    Ok(summary)
}
