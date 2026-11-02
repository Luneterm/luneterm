//! Reader for the Lunegit input goldens (`goldens/lunegit-input/*.golden`): the real outputs of
//! Lunegit's key encoders, `Osc52Parser` and its three OSC scanners, written by
//! `tools/lunegit-oracle` (`scripts/regen-lunegit-goldens.sh`). The format is specified in
//! `FORMAT.md` ("Lunegit input goldens"); nothing here needs .NET.

use std::fs;

use crate::{FormatError, crate_dir, error, io_error};

/// Magic first line of every Lunegit input golden.
pub const MAGIC: &str = "lune-lunegit-golden 1";

/// No field of a row may pass this many characters (large inputs are repeat directives).
pub const MAX_FIELD_CHARS: usize = 256;

/// Largest byte string [`expand_spec`] will build (a malformed file must not allocate unbounded).
pub const MAX_EXPANDED_BYTES: usize = 8 * 1024 * 1024;

/// Which Lunegit surface a file records; the `kind` line and the file stem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// `TerminalKeyEncoder.Encode`, `GenerateKeyInput`, `GenerateCharInput`.
    Keys,
    /// `Osc52Parser.Parse`.
    Osc52,
    /// `OscCwdScanner` (OSC 7 and OSC 9;9).
    OscCwd,
    /// `Osc94ProgressScanner` (OSC 9;4).
    OscProgress,
    /// `TerminalCommandScanner` (OSC 133 and CSI ? 2026 h).
    OscCommand,
}

impl InputKind {
    /// Every kind, in a fixed order.
    pub const ALL: [InputKind; 5] = [
        InputKind::Keys,
        InputKind::Osc52,
        InputKind::OscCwd,
        InputKind::OscProgress,
        InputKind::OscCommand,
    ];

    /// The spelling of the `kind` line, also the file stem.
    pub fn keyword(self) -> &'static str {
        match self {
            InputKind::Keys => "keys",
            InputKind::Osc52 => "osc52",
            InputKind::OscCwd => "osc-cwd",
            InputKind::OscProgress => "osc-progress",
            InputKind::OscCommand => "osc-command",
        }
    }

    fn from_keyword(word: &str) -> Option<InputKind> {
        InputKind::ALL.into_iter().find(|k| k.keyword() == word)
    }
}

/// One `section`: named columns and the rows recorded under them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSection {
    /// Section name, unique within the file.
    pub name: String,
    /// Input column names (what was fed to Lunegit's code).
    pub inputs: Vec<String>,
    /// Output column names (what Lunegit's code returned).
    pub outputs: Vec<String>,
    /// Rows: one field per input column, then one per output column.
    pub rows: Vec<Vec<String>>,
}

impl InputSection {
    /// Index of a column by name, inputs first.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.inputs
            .iter()
            .chain(self.outputs.iter())
            .position(|c| c == name)
    }

    /// The field of `row` under column `name`.
    pub fn field<'a>(&self, row: &'a [String], name: &str) -> Option<&'a str> {
        row.get(self.column(name)?).map(String::as_str)
    }
}

/// One parsed Lunegit input golden file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputGolden {
    /// What the file records.
    pub kind: InputKind,
    /// `header <key> <value>` lines in file order (tool, Lunegit commit, DLL SHA-256s).
    pub header: Vec<(String, String)>,
    /// Sections in file order.
    pub sections: Vec<InputSection>,
}

impl InputGolden {
    /// The value of a header key.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// A section by name.
    pub fn section(&self, name: &str) -> Option<&InputSection> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// Loads `goldens/lunegit-input/<kind>.golden`; the file's `kind` line must equal `kind`.
pub fn load_input(kind: InputKind) -> Result<InputGolden, FormatError> {
    let path = crate_dir()
        .join("goldens")
        .join("lunegit-input")
        .join(format!("{}.golden", kind.keyword()));
    let label = format!("goldens/lunegit-input/{}.golden", kind.keyword());
    let text = fs::read_to_string(&path).map_err(|e| io_error(&label, &e))?;
    let golden = parse_input(&label, &text)?;
    if golden.kind != kind {
        return Err(error(
            &label,
            0,
            format!(
                "kind is {}, expected {}",
                golden.kind.keyword(),
                kind.keyword()
            ),
        ));
    }
    Ok(golden)
}

fn is_word(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Parses the text of one file. `file` is only used in error messages. Never panics.
pub fn parse_input(file: &str, text: &str) -> Result<InputGolden, FormatError> {
    if text.is_empty() {
        return Err(error(file, 0, "empty file".to_string()));
    }
    let mut lines = text
        .split('\n')
        .enumerate()
        .map(|(i, l)| (i + 1, l.strip_suffix('\r').unwrap_or(l)))
        .peekable();
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

    let mut header: Vec<(String, String)> = Vec::new();
    let mut kind: Option<InputKind> = None;
    let mut sections: Vec<InputSection> = Vec::new();

    while let Some((n, line)) = lines.next() {
        if line.is_empty() {
            continue;
        }
        if kind.is_none() {
            if let Some(rest) = line.strip_prefix("header ") {
                let (key, value) = rest
                    .split_once(' ')
                    .filter(|(k, v)| is_word(k) && !v.is_empty())
                    .ok_or_else(|| error(file, n, format!("bad header line '{line}'")))?;
                if header.iter().any(|(k, _)| k == key) {
                    return Err(error(file, n, format!("duplicate header '{key}'")));
                }
                header.push((key.to_string(), value.to_string()));
                continue;
            }
            let word = line.strip_prefix("kind ").ok_or_else(|| {
                error(
                    file,
                    n,
                    format!("expected 'header' or 'kind', found '{line}'"),
                )
            })?;
            kind = Some(
                InputKind::from_keyword(word)
                    .ok_or_else(|| error(file, n, format!("unknown kind '{word}'")))?,
            );
            continue;
        }

        let name = line
            .strip_prefix("section ")
            .ok_or_else(|| error(file, n, format!("expected 'section', found '{line}'")))?;
        if !is_word(name) {
            return Err(error(file, n, format!("bad section name '{name}'")));
        }
        if sections.iter().any(|s| s.name == name) {
            return Err(error(file, n, format!("duplicate section '{name}'")));
        }
        let inputs = columns(file, lines.next(), "inputs")?;
        let outputs = columns(file, lines.next(), "outputs")?;
        let width = inputs.len() + outputs.len();
        let mut rows: Vec<Vec<String>> = Vec::new();
        loop {
            let Some((rn, row)) = lines.next() else {
                return Err(error(file, 0, format!("section '{name}' has no 'end'")));
            };
            if row == "end" {
                break;
            }
            if row.is_empty() {
                let message = if lines.peek().is_none() {
                    format!("section '{name}' has no 'end'")
                } else {
                    "blank line in a section".to_string()
                };
                return Err(error(file, rn, message));
            }
            let fields: Vec<String> = row.split('\t').map(str::to_string).collect();
            if fields.len() != width {
                return Err(error(
                    file,
                    rn,
                    format!("row has {} fields, expected {width}", fields.len()),
                ));
            }
            if let Some(bad) = fields
                .iter()
                .find(|f| f.is_empty() || f.chars().count() > MAX_FIELD_CHARS)
            {
                return Err(error(
                    file,
                    rn,
                    format!(
                        "field of {} characters (empty is '-', the limit is {MAX_FIELD_CHARS})",
                        bad.chars().count()
                    ),
                ));
            }
            rows.push(fields);
        }
        sections.push(InputSection {
            name: name.to_string(),
            inputs,
            outputs,
            rows,
        });
    }

    let kind = kind.ok_or_else(|| error(file, 0, "no 'kind' line".to_string()))?;
    if sections.is_empty() {
        return Err(error(file, 0, "no sections".to_string()));
    }
    Ok(InputGolden {
        kind,
        header,
        sections,
    })
}

fn columns(
    file: &str,
    line: Option<(usize, &str)>,
    keyword: &str,
) -> Result<Vec<String>, FormatError> {
    let (n, text) = line.ok_or_else(|| error(file, 0, format!("expected '{keyword}'")))?;
    let list = text
        .strip_prefix(keyword)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or_else(|| {
            error(
                file,
                n,
                format!("expected '{keyword} <columns>', found '{text}'"),
            )
        })?;
    let names: Vec<String> = list.split(' ').map(str::to_string).collect();
    if names.iter().any(|c| !is_word(c)) {
        return Err(error(file, n, format!("bad column list '{list}'")));
    }
    Ok(names)
}

/// Expands a byte spec: `-` is empty; otherwise parts joined by `+`, each lowercase hex
/// (`1b5d`) or hex repeated (`61x4096`). Errors name the problem; the result never exceeds
/// [`MAX_EXPANDED_BYTES`].
pub fn expand_spec(spec: &str) -> Result<Vec<u8>, String> {
    if spec == "-" {
        return Ok(Vec::new());
    }
    let mut out: Vec<u8> = Vec::new();
    for part in spec.split('+') {
        let (hex, count) = match part.split_once('x') {
            Some((hex, count)) => {
                if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(format!("bad repeat count in '{part}'"));
                }
                let count: usize = count
                    .parse()
                    .map_err(|_| format!("repeat count too large in '{part}'"))?;
                (hex, count)
            }
            None => (part, 1),
        };
        let unit = decode_hex(hex).ok_or_else(|| format!("bad hex in '{part}'"))?;
        if unit.is_empty() {
            return Err(format!("empty part in '{spec}'"));
        }
        let total = unit
            .len()
            .checked_mul(count)
            .and_then(|n| n.checked_add(out.len()))
            .filter(|n| *n <= MAX_EXPANDED_BYTES)
            .ok_or_else(|| format!("spec expands past {MAX_EXPANDED_BYTES} bytes"))?;
        out.reserve(total - out.len());
        for _ in 0..count {
            out.extend_from_slice(&unit);
        }
    }
    Ok(out)
}

/// Decodes lowercase hex of even length; `None` otherwise.
pub fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            };
            match pair {
                [hi, lo] => Some(digit(*hi)? << 4 | digit(*lo)?),
                _ => None,
            }
        })
        .collect()
}
