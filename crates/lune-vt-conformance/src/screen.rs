//! Screen-dump runner: drives a fresh `lune_vt::Terminal` (Unicode 11 active, default options,
//! as the oracle's `createTerminal`) and renders the normalised dump of FORMAT.md ("Screen dump")
//! or Lunegit's grid ("Grid"), so a run can be compared line by line with a golden's `expect`.
//!
//! The dump reads the terminal only through its public/read API (`Terminal`, `core()`
//! accessors); it mutates nothing beyond what the oracle does (writes, resize, event drain).
//!
//! Depends only on `lune-vt`, `crate::escape` and `crate::trace`. The engine fuzz test
//! (`lune-vt/tests/engine_fuzz.rs`) includes this file by path next to `trace.rs` and
//! `escape.rs`, because `lune-vt` cannot dev-depend on this crate.
#![allow(unreachable_pub, dead_code)]

use lune_vt::HostEvent;
use lune_vt::Terminal;
use lune_vt::buffer::buffer::Buffer;
use lune_vt::buffer::cell_data::CellData;
use lune_vt::buffer::constants::{Attributes, BgFlags, FgFlags};
use lune_vt::services::options_service::{CursorStyle, OptionValue};

use crate::escape::escape_str;

/// The oracle's default size.
pub const DEFAULT_SIZE: (usize, usize) = (80, 24);

/// A terminal as the oracle builds it (`cols` x `rows`, Unicode 11 active), plus what its
/// events said so far.
#[derive(Debug)]
pub struct Run {
    pub term: Terminal,
    /// `onData` strings, in order.
    pub responses: Vec<String>,
    /// The last `onTitleChange` value, `""` if none.
    pub title: String,
}

impl Run {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut term = Terminal::new([OptionValue::Cols(cols), OptionValue::Rows(rows)]);
        // The oracle activates "11"; the name is registered by `Terminal::new`.
        let _ = term.unicode_mut().set_active_version("11");
        Run {
            term,
            responses: Vec::new(),
            title: String::new(),
        }
    }

    /// Moves the queued events into `responses` and `title`.
    pub fn drain(&mut self) {
        for event in self.term.drain_events() {
            match event {
                HostEvent::Response(bytes) => {
                    self.responses
                        .push(String::from_utf8_lossy(&bytes).into_owned());
                }
                HostEvent::Title(title) => self.title = title,
                _ => {}
            }
        }
    }

    /// One write, then the event drain.
    pub fn write(&mut self, chunk: &[u8]) {
        self.term.write(chunk);
        self.drain();
    }
}

/// The oracle's `runScreen`: size, every chunk written, then the optional resize.
pub fn run<'a, I>(size: Option<(usize, usize)>, resize: Option<(usize, usize)>, chunks: I) -> Run
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let (cols, rows) = size.unwrap_or(DEFAULT_SIZE);
    let mut run = Run::new(cols, rows);
    for chunk in chunks {
        run.write(chunk);
    }
    if let Some((cols, rows)) = resize {
        run.term.resize(cols, rows);
        run.drain();
    }
    run
}

/// The screen dump lines of FORMAT.md for a run of `chunks`.
pub fn dump<'a, I>(
    size: Option<(usize, usize)>,
    resize: Option<(usize, usize)>,
    chunks: I,
) -> Vec<String>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    dump_run(&run(size, resize, chunks))
}

/// Lunegit's grid (FORMAT.md "Grid") for a run of `chunks`, one escaped string per row.
pub fn grid<'a, I>(
    size: Option<(usize, usize)>,
    resize: Option<(usize, usize)>,
    chunks: I,
) -> Vec<String>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    grid_rows(&run(size, resize, chunks).term)
}

fn mode_bool(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

fn cursor_style_name(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Block => "block",
        CursorStyle::Underline => "underline",
        CursorStyle::Bar => "bar",
    }
}

/// `modes <key>=<value> ...`, sorted by key.
fn modes_line(term: &Terminal) -> String {
    let services = term.core().input_handler().services();
    let dec = &services.core.dec_private_modes;
    let protocol = services.mouse.active_protocol();
    let public_mouse = match protocol {
        "X10" => "x10",
        "VT200" => "vt200",
        "DRAG" => "drag",
        "ANY" => "any",
        _ => "none",
    };
    let mut modes: Vec<(String, String)> = vec![
        // public `term.modes`
        (
            "public.applicationCursorKeysMode".into(),
            mode_bool(dec.application_cursor_keys),
        ),
        (
            "public.applicationKeypadMode".into(),
            mode_bool(dec.application_keypad),
        ),
        (
            "public.bracketedPasteMode".into(),
            mode_bool(dec.bracketed_paste_mode),
        ),
        (
            "public.insertMode".into(),
            mode_bool(services.core.modes.insert_mode),
        ),
        ("public.mouseTrackingMode".into(), public_mouse.into()),
        ("public.originMode".into(), mode_bool(dec.origin)),
        (
            "public.reverseWraparoundMode".into(),
            mode_bool(dec.reverse_wraparound),
        ),
        ("public.sendFocusMode".into(), mode_bool(dec.send_focus)),
        (
            "public.synchronizedOutputMode".into(),
            mode_bool(dec.synchronized_output),
        ),
        ("public.wraparoundMode".into(), mode_bool(dec.wraparound)),
        // `coreService.decPrivateModes`
        (
            "dec.applicationCursorKeys".into(),
            mode_bool(dec.application_cursor_keys),
        ),
        (
            "dec.applicationKeypad".into(),
            mode_bool(dec.application_keypad),
        ),
        (
            "dec.bracketedPasteMode".into(),
            mode_bool(dec.bracketed_paste_mode),
        ),
        (
            "dec.cursorBlink".into(),
            dec.cursor_blink.map_or("-".into(), mode_bool),
        ),
        (
            "dec.cursorStyle".into(),
            dec.cursor_style
                .map_or("-".into(), |s| cursor_style_name(s).to_string()),
        ),
        ("dec.origin".into(), mode_bool(dec.origin)),
        (
            "dec.reverseWraparound".into(),
            mode_bool(dec.reverse_wraparound),
        ),
        ("dec.sendFocus".into(), mode_bool(dec.send_focus)),
        (
            "dec.synchronizedOutput".into(),
            mode_bool(dec.synchronized_output),
        ),
        ("dec.wraparound".into(), mode_bool(dec.wraparound)),
        // `coreService.modes`
        (
            "core.insertMode".into(),
            mode_bool(services.core.modes.insert_mode),
        ),
        ("mouse.protocol".into(), protocol.to_string()),
        (
            "mouse.encoding".into(),
            services.mouse.active_encoding().to_string(),
        ),
    ];
    modes.sort();
    let body: Vec<String> = modes.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("modes {}", body.join(" "))
}

/// `p16:<n>`, `p256:<n>`, `rgb:<rrggbb>`; `None` for the default colour.
fn colour(mode: u32, value: u32) -> Option<String> {
    match mode {
        Attributes::CM_P16 => Some(format!("p16:{}", value & 0xFF)),
        Attributes::CM_P256 => Some(format!("p256:{}", value & 0xFF)),
        Attributes::CM_RGB => Some(format!("rgb:{:06x}", value & 0xFF_FFFF)),
        _ => None,
    }
}

/// The style tokens of one cell, in the order of FORMAT.md, and its url id.
fn cell_tokens(cell: &CellData) -> (String, u32) {
    let mut tokens: Vec<String> = Vec::new();
    if let Some(c) = colour(cell.fg & Attributes::CM_MASK, cell.fg) {
        tokens.push(format!("fg={c}"));
    }
    if let Some(c) = colour(cell.bg & Attributes::CM_MASK, cell.bg) {
        tokens.push(format!("bg={c}"));
    }
    if cell.is_bold() != 0 {
        tokens.push("b".into());
    }
    if cell.is_dim() != 0 {
        tokens.push("d".into());
    }
    if cell.is_italic() != 0 {
        tokens.push("i".into());
    }
    // The raw underline bits and underline colour word, without the url id's DASHED override.
    let (ubits, ucword, url) = if cell.bg & BgFlags::HAS_EXTENDED != 0 {
        let url = cell.extended.url_id();
        let mut raw = cell.extended;
        raw.set_url_id(0);
        (raw.underline_style(), raw.ext(), url)
    } else {
        (0, 0, 0)
    };
    if cell.fg & FgFlags::UNDERLINE != 0 || ubits != 0 {
        tokens.push(format!("u={ubits}"));
    }
    if let Some(c) = colour(ucword & Attributes::CM_MASK, ucword) {
        tokens.push(format!("uc={c}"));
    }
    if cell.is_blink() != 0 {
        tokens.push("bl".into());
    }
    if cell.is_inverse() != 0 {
        tokens.push("inv".into());
    }
    if cell.is_invisible() != 0 {
        tokens.push("invis".into());
    }
    if cell.is_strikethrough() != 0 {
        tokens.push("s".into());
    }
    if cell.is_overline() != 0 {
        tokens.push("o".into());
    }
    if cell.get_width() != 1 {
        tokens.push(format!("w={}", cell.get_width()));
    }
    if url != 0 {
        tokens.push(format!("url={url}"));
    }
    (tokens.join(" "), url)
}

fn active_buffer(term: &Terminal) -> &Buffer {
    term.core().input_handler().services().buffer.buffer()
}

/// The dump of an existing run.
pub fn dump_run(run: &Run) -> Vec<String> {
    let term = &run.term;
    let services = term.core().input_handler().services();
    let buffer = active_buffer(term);
    let cols = term.cols();
    let mut out: Vec<String> = Vec::new();
    out.push(format!("cursor {} {}", buffer.x, buffer.y));
    out.push(format!(
        "buffer {}",
        if services.buffer.buffers.is_alt_active() {
            "alternate"
        } else {
            "normal"
        }
    ));
    out.push(format!("base-y {}", buffer.ybase));
    out.push(format!("title {}", escape_str(&run.title)));
    out.push(modes_line(term));
    for response in &run.responses {
        out.push(format!("response {}", escape_str(response)));
    }
    let mut url_ids: Vec<u32> = Vec::new();
    let mut runs: Vec<String> = Vec::new();
    for y in 0..buffer.lines.len() {
        let Some(line) = buffer.lines.get(y) else {
            out.push(format!("line {y} 0 \"<missing line>\""));
            continue;
        };
        let width = line.len().min(cols);
        let text = line.translate_to_string(true, Some(0), Some(width), None);
        out.push(format!(
            "line {y} {} {}",
            u8::from(line.is_wrapped),
            escape_str(&text)
        ));
        let mut run_start = 0;
        let mut run_tokens = String::new();
        for x in 0..width {
            // A fresh cell per read: `load_cell` leaves `extended` stale without the flag.
            let mut cell = CellData::new();
            line.load_cell(x, &mut cell);
            let (tokens, url) = cell_tokens(&cell);
            if url != 0 && !url_ids.contains(&url) {
                url_ids.push(url);
            }
            if tokens != run_tokens {
                if !run_tokens.is_empty() {
                    runs.push(format!(
                        "attr {y} {run_start} {} {run_tokens}",
                        x - run_start
                    ));
                }
                run_start = x;
                run_tokens = tokens;
            }
        }
        if !run_tokens.is_empty() {
            runs.push(format!(
                "attr {y} {run_start} {} {run_tokens}",
                width - run_start
            ));
        }
    }
    out.extend(runs);
    url_ids.sort_unstable();
    for id in url_ids {
        out.push(format!(
            "link {id} {}",
            escape_str(term.link_uri(id).unwrap_or(""))
        ));
    }
    out
}

/// Lunegit's grid of a terminal (VtConformanceSupport.cs `Normalize`): the visible rows of the
/// active buffer, a wide glyph's tail skipped, right-trimmed, trailing empty rows dropped.
pub fn grid_rows(term: &Terminal) -> Vec<String> {
    let buffer = active_buffer(term);
    let cols = term.cols();
    let mut rows: Vec<String> = Vec::new();
    let mut cell = CellData::new();
    for y in 0..term.rows() {
        let mut text = String::new();
        if let Some(line) = buffer.lines.get(buffer.ybase.saturating_add(y)) {
            for x in 0..line.len().min(cols) {
                line.load_cell(x, &mut cell);
                if cell.get_width() == 0 {
                    continue;
                }
                let chars = cell.get_chars();
                text.push_str(if chars.is_empty() { " " } else { &chars });
            }
        }
        rows.push(text.trim_end_matches([' ', '\t']).to_string());
    }
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows.iter().map(|row| escape_str(row)).collect()
}

/// A small deterministic generator (xorshift64*), for the seeded splits.
#[derive(Clone, Debug)]
pub struct SplitRng(u64);

impl SplitRng {
    pub fn new(seed: u64) -> Self {
        // Never zero: xorshift sticks there.
        SplitRng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..bound` (`bound > 0`; 0 when `bound` is 0).
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % bound as u64).unwrap_or(0)
    }
}

/// FNV-1a of a name, to seed a split per golden.
pub fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// `data` cut at 1 to 8 random offsets (empty chunks possible).
pub fn random_split(data: &[u8], seed: u64) -> Vec<Vec<u8>> {
    let mut rng = SplitRng::new(seed);
    let cuts = 1 + rng.below(8);
    let mut points: Vec<usize> = (0..cuts).map(|_| rng.below(data.len() + 1)).collect();
    points.sort_unstable();
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut prev = 0;
    for point in points {
        out.push(data.get(prev..point).unwrap_or_default().to_vec());
        prev = point;
    }
    out.push(data.get(prev..).unwrap_or_default().to_vec());
    out
}

/// The ways a golden's input is replayed: as recorded, unsplit, one byte per write, and three
/// seeded random splits. Each entry is a label and the chunks.
pub fn replays(name: &str, recorded: &[Vec<u8>]) -> Vec<(String, Vec<Vec<u8>>)> {
    let flat: Vec<u8> = recorded.iter().flatten().copied().collect();
    let mut out: Vec<(String, Vec<Vec<u8>>)> = vec![
        ("recorded".into(), recorded.to_vec()),
        ("unsplit".into(), vec![flat.clone()]),
        ("1-byte".into(), flat.iter().map(|b| vec![*b]).collect()),
    ];
    for n in 1..=3u64 {
        out.push((
            format!("split {n}"),
            random_split(&flat, seed_of(name) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        ));
    }
    out
}
