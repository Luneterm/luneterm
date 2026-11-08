//! Seeded fuzz of the C ABI surface, driven through the exports only (M8 step 6).
//!
//! Per seed: an engine from a random (usually valid, sometimes hostile) config, then a list of
//! random operations: hostile and random writes (grammar-biased escape sequences with extreme
//! parameters, OSC/DCS payloads, raw byte soup, split into chunks), resizes in the caps, at the
//! cap edges, over them and below 2 x 1, scrolls with `i32::MIN` and `i32::MAX`, every encoder with
//! random field values including out-of-range enums and non-scalar code points, paste with random
//! (often invalid UTF-8) bytes, viewport, drain, damage and modes, NULL and misaligned out-pointers,
//! `(NULL, n)` inputs, `idle` and the two gates.
//!
//! Checked over the whole run:
//!  - no status is `ERR_PANIC`, `ERR_POISONED` or `ERR_BUSY` (a single thread, no panic probe), and
//!    every status is one of the defined codes;
//!  - after every viewport call: `cell_count == cols * rows`, `cursor_x <= cols + 1` (xterm.js 6.0.0
//!    itself leaves the cursor at `cols + 1` when a zero-width character follows a CSI at the wrap-pending
//!    column: seed 59, checked on the oracle), every `text_offset + text_len` is
//!    inside the text pool, every link range is inside `link_data`, links ascend by id, and the size
//!    matches `luneterm_engine_size`; after every drain, every event's data range is inside `data`
//!    (colour ranges inside `colors`);
//!  - on every non-OK status the out-parameters were zeroed (R1) and a refused misaligned
//!    out-parameter was never written;
//!  - the engine size follows the resize rules (clamped to 2 x 1, over a cap changes nothing) and
//!    never exceeds the caps.
//!
//! A failure reports the seed, the operation index and kind, and status numbers: never bytes.
//! `LUNETERM_FFI_FUZZ_SEEDS=<n>` runs seeds `0..n` (default 60); `LUNETERM_FFI_FUZZ_SEED=<s>`
//! replays one.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{dangling, null, null_mut};

use luneterm_ffi::*;

const DEFAULT_SEEDS: u64 = 60;
const OPS_PER_SEED: usize = 64;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[usize::try_from(self.below(items.len() as u64)).unwrap()]
    }

    fn u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }
}

// ------------------------------------------------------------------------------------------
// Input generator (a local, smaller copy of lune-vt's engine_fuzz grammar).

const EXTREME: [&str; 7] = [
    "65535",
    "65536",
    "2147483647",
    "4294967295",
    "4294967296",
    "99999999999999999999",
    "1048576",
];
const CSI_FINALS: [char; 40] = [
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'P', 'S', 'T', 'X', 'Z',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'l', 'm', 'n', 'p', 'q', 'r', 's', 't', 'u', 'z',
    '}', '~',
];
const DEC_MODES: [&str; 26] = [
    "1", "3", "5", "6", "7", "8", "9", "12", "25", "45", "47", "66", "69", "1000", "1002", "1003",
    "1004", "1005", "1006", "1015", "1016", "1047", "1048", "1049", "2004", "2026",
];
const TEXT: [&str; 16] = [
    "a",
    "hello",
    "0123456789",
    "The quick brown fox",
    "\u{4F60}\u{597D}",
    "\u{D55C}\u{AE00}",
    "\u{1F600}",
    "\u{1F44D}\u{1F3FD}",
    "\u{1F1EF}\u{1F1F5}",
    "e\u{301}",
    "a\u{301}\u{302}",
    "\u{301}",
    "\u{26A0}\u{FE0F}",
    "\u{FE0F}",
    "\u{3042}",
    "\u{200B}",
];
const OSC_IDS: [&str; 14] = [
    "0", "2", "4", "7", "8", "9", "10", "11", "12", "52", "104", "133", "777", "9999",
];

fn csi(rng: &mut Rng) -> String {
    let mut out = String::from("\x1b[");
    if rng.chance(30) {
        out.push(*rng.pick(&['?', '>', '=', '<']));
    }
    let count = match rng.below(10) {
        0 => rng.between(6, 30),
        1..=3 => 2,
        4..=7 => 1,
        _ => 0,
    };
    for i in 0..count {
        if i > 0 {
            out.push(if rng.chance(15) { ':' } else { ';' });
        }
        match rng.below(10) {
            0 => out.push_str(rng.pick::<&str>(&EXTREME)),
            1..=2 => out.push_str(rng.pick::<&str>(&DEC_MODES)),
            3..=4 => out.push_str(&rng.below(110).to_string()),
            _ => out.push_str(&rng.below(30).to_string()),
        }
    }
    if rng.chance(8) {
        out.push(*rng.pick(&[' ', '!', '"', '$', '\'']));
    }
    out.push(*rng.pick(&CSI_FINALS));
    out
}

fn osc(rng: &mut Rng) -> String {
    let id = *rng.pick(&OSC_IDS);
    let payload = match id {
        "8" => format!(
            "id={};https://example.com/{}",
            rng.below(4),
            rng.below(1000)
        ),
        "52" => format!(
            "c;{}",
            ["aGk=", "?", "!!!", "", "aGVsbG8gd29ybGQ="][rng.below(5) as usize]
        ),
        "7" => "file://host/tmp/x".to_owned(),
        "133" => {
            ["A", "B", "C", "D", "D;0", "D;137", "D;99999999999"][rng.below(7) as usize].to_owned()
        }
        "9" => ["4;1;50", "4;3", "4;9;200", "9;/tmp", "x"][rng.below(5) as usize].to_owned(),
        "4" | "10" | "11" | "12" => {
            ["?", "rgb:ff/00/11", "#abc", "1;?;2;rgb:1/2/3"][rng.below(4) as usize].to_owned()
        }
        _ => {
            let len = if rng.chance(8) {
                rng.between(1000, 70_000)
            } else {
                rng.between(0, 24)
            };
            "p".repeat(usize::try_from(len).unwrap())
        }
    };
    let terminator = if rng.chance(50) { "\x07" } else { "\x1b\\" };
    format!("\x1b]{id};{payload}{terminator}")
}

fn gen_write(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.between(1, 10) {
        match rng.below(10) {
            0..=2 => out.extend_from_slice(rng.pick(&TEXT).as_bytes()),
            3..=5 => out.extend_from_slice(csi(rng).as_bytes()),
            6 => out.extend_from_slice(osc(rng).as_bytes()),
            7 => {
                out.extend_from_slice(b"\x1bP");
                for _ in 0..rng.between(0, 20) {
                    out.push(rng.byte());
                }
                out.extend_from_slice(b"\x1b\\");
            }
            8 => out.extend_from_slice(
                rng.pick(&[
                    "\r\n", "\n", "\t", "\x08", "\x07", "\x1bM", "\x1bc", "\x1b7", "\x1b8",
                ])
                .as_bytes(),
            ),
            _ => {
                for _ in 0..rng.between(1, 64) {
                    out.push(rng.byte());
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// Garbage out-parameters and zero checks (R1).

fn viewport_garbage() -> LunetermViewport {
    LunetermViewport {
        cols: 0xAAAA,
        rows: 0xAAAA,
        cells: dangling(),
        cell_count: 0xAAAA,
        text: dangling(),
        text_len: 1,
        links: dangling(),
        link_count: 1,
        link_data: dangling(),
        link_data_len: 1,
        title: dangling(),
        title_len: 1,
        viewport_top: 1,
        base_y: 1,
        total_lines: 1,
        cursor_x: 1,
        cursor_y: 1,
        cursor_visible: 1,
        cursor_style: 1,
        cursor_blink: 1,
        is_at_bottom: 1,
        alt_screen: 1,
        reserved: [1; 3],
    }
}

fn viewport_is_zero(v: &LunetermViewport) -> bool {
    v.cols == 0
        && v.rows == 0
        && v.cells.is_null()
        && v.cell_count == 0
        && v.text.is_null()
        && v.text_len == 0
        && v.links.is_null()
        && v.link_count == 0
        && v.link_data.is_null()
        && v.link_data_len == 0
        && v.title.is_null()
        && v.title_len == 0
        && (v.viewport_top, v.base_y, v.total_lines) == (0, 0, 0)
        && (v.cursor_x, v.cursor_y) == (0, 0)
        && [
            v.cursor_visible,
            v.cursor_style,
            v.cursor_blink,
            v.is_at_bottom,
            v.alt_screen,
        ] == [0; 5]
        && v.reserved == [0; 3]
}

fn events_garbage() -> LunetermEvents {
    LunetermEvents {
        events: dangling(),
        count: 1,
        data: dangling(),
        data_len: 1,
        colors: dangling(),
        color_count: 1,
    }
}

fn events_is_zero(e: &LunetermEvents) -> bool {
    e.events.is_null()
        && e.count == 0
        && e.data.is_null()
        && e.data_len == 0
        && e.colors.is_null()
        && e.color_count == 0
}

fn bytes_garbage() -> LunetermBytes {
    LunetermBytes {
        ptr: dangling(),
        len: 1,
    }
}

fn bytes_is_zero(b: &LunetermBytes) -> bool {
    b.ptr.is_null() && b.len == 0
}

// ------------------------------------------------------------------------------------------

struct Progress {
    op: usize,
    kind: &'static str,
}

#[derive(Default)]
struct Totals {
    ops: u64,
    statuses: BTreeMap<i32, u64>,
    event_kinds: BTreeMap<u32, u64>,
    encoder_nonempty: u64,
    viewports: u64,
}

fn is_defined(code: i32) -> bool {
    (LUNETERM_ERR_BUSY..=LUNETERM_OK).contains(&code)
}

struct Fuzz<'a> {
    rng: Rng,
    engine: *mut LunetermEngine,
    /// The size the resize rules say the engine has.
    size: (u32, u32),
    progress: &'a mut Progress,
    totals: &'a mut Totals,
    seed: u64,
}

impl Fuzz<'_> {
    /// Records a status and fails on a defined-but-unexpected one.
    fn status(&mut self, code: i32) -> i32 {
        *self.totals.statuses.entry(code).or_insert(0) += 1;
        assert!(
            is_defined(code),
            "seed {} op {} {}: undefined status {code}",
            self.seed,
            self.progress.op,
            self.progress.kind
        );
        assert!(
            code != LUNETERM_ERR_PANIC
                && code != LUNETERM_ERR_POISONED
                && code != LUNETERM_ERR_BUSY,
            "seed {} op {} {}: status {code}",
            self.seed,
            self.progress.op,
            self.progress.kind
        );
        code
    }

    fn fail(&self, what: &str) -> ! {
        panic!(
            "seed {} op {} {}: {what}",
            self.seed, self.progress.op, self.progress.kind
        )
    }

    fn expect_ok(&mut self, code: i32) {
        let code = self.status(code);
        if code != LUNETERM_OK {
            self.fail(&format!("expected OK, status {code}"));
        }
    }

    fn set_size_after_resize(&mut self, cols: u32, rows: u32) {
        self.size = (cols.max(2), rows.max(1));
    }

    fn check_size(&mut self) {
        let (mut cols, mut rows) = (u32::MAX, u32::MAX);
        // SAFETY: a live handle and two live out-params.
        let code = unsafe { luneterm_engine_size(self.engine, &mut cols, &mut rows) };
        self.expect_ok(code);
        if (cols, rows) != self.size
            || cols > LUNETERM_MAX_COLS
            || rows > LUNETERM_MAX_ROWS
            || cols < 2
            || rows < 1
        {
            self.fail(&format!(
                "size {cols}x{rows}, expected {}x{}",
                self.size.0, self.size.1
            ));
        }
    }

    fn write(&mut self, data: &[u8]) {
        // Split into one to four chunks: sequences and UTF-8 may be cut anywhere.
        let mut rest = data;
        let chunks = self.rng.between(1, 4);
        for i in 0..chunks {
            let take = if i + 1 == chunks {
                rest.len()
            } else {
                usize::try_from(self.rng.below(rest.len() as u64 + 1)).unwrap()
            };
            let (head, tail) = rest.split_at(take);
            // SAFETY: a live handle and a live slice.
            let code = unsafe { luneterm_engine_write(self.engine, head.as_ptr(), head.len()) };
            self.expect_ok(code);
            rest = tail;
        }
    }

    fn viewport(&mut self) {
        self.totals.viewports += 1;
        let mut vp = viewport_garbage();
        // SAFETY: a live handle and a live out-param.
        let code = unsafe { luneterm_engine_viewport(self.engine, &mut vp) };
        self.expect_ok(code);
        if (vp.cols, vp.rows) != self.size {
            self.fail(&format!(
                "viewport {}x{} but size {}x{}",
                vp.cols, vp.rows, self.size.0, self.size.1
            ));
        }
        if vp.cell_count != u64::from(vp.cols) * u64::from(vp.rows) {
            self.fail("cell_count != cols * rows");
        }
        if vp.reserved != [0; 3]
            || vp.cursor_x > vp.cols.saturating_add(1)
            || vp.cursor_y >= vp.rows.max(1)
        {
            self.fail(&format!(
                "cursor ({},{}) or reserved out of range for {}x{}",
                vp.cursor_x, vp.cursor_y, vp.cols, vp.rows
            ));
        }
        // SAFETY: the slot is live and untouched until the next viewport call.
        let cells = unsafe { slice(vp.cells, vp.cell_count) };
        let text_len = vp.text_len;
        let mut used = 0u64;
        for cell in cells {
            if u64::from(cell.text_offset) + u64::from(cell.text_len) > text_len {
                self.fail("a cell's text is outside the text pool");
            }
            if cell.text_len == 0 && cell.text_offset != 0 {
                self.fail("an empty cell has a nonzero offset");
            }
            if cell.width > 2 || cell.flags >> 8 != 0 || cell.underline > 5 {
                self.fail("a cell field is out of range");
            }
            for color in [cell.fg, cell.bg, cell.underline_color] {
                if color >> 24 > 2 {
                    self.fail("a colour kind is out of range");
                }
            }
            used += u64::from(cell.text_len);
        }
        if used != text_len {
            self.fail("the text pool holds bytes no cell uses");
        }
        // SAFETY: as above.
        let (links, text, link_data, title) = unsafe {
            (
                slice(vp.links, vp.link_count),
                slice(vp.text, vp.text_len),
                slice(vp.link_data, vp.link_data_len),
                slice(vp.title, vp.title_len),
            )
        };
        let mut previous = None;
        for link in links {
            if link
                .offset
                .checked_add(link.len)
                .is_none_or(|end| end > vp.link_data_len)
            {
                self.fail("a link's uri is outside the link pool");
            }
            if previous.is_some_and(|p| p >= link.id) || link.reserved != 0 {
                self.fail("links are not ascending or reserved is set");
            }
            previous = Some(link.id);
        }
        if std::str::from_utf8(text).is_err()
            || std::str::from_utf8(link_data).is_err()
            || std::str::from_utf8(title).is_err()
        {
            self.fail("an output string is not UTF-8");
        }
    }

    fn drain(&mut self) {
        let mut ev = events_garbage();
        // SAFETY: a live handle and a live out-param.
        let code = unsafe { luneterm_engine_drain_events(self.engine, &mut ev) };
        self.expect_ok(code);
        // SAFETY: the slot is live and untouched until the next drain.
        let events = unsafe { slice(ev.events, ev.count) };
        for event in events {
            *self.totals.event_kinds.entry(event.kind).or_insert(0) += 1;
            let in_data = event
                .offset
                .checked_add(event.len)
                .is_some_and(|end| end <= ev.data_len);
            let in_colors = event
                .offset
                .checked_add(event.len)
                .is_some_and(|end| end <= ev.color_count);
            let ok = match event.kind {
                LUNETERM_EVENT_RESPONSE
                | LUNETERM_EVENT_A11Y_TEXT
                | LUNETERM_EVENT_CLIPBOARD_WRITE
                | LUNETERM_EVENT_CWD_CHANGED => in_data,
                LUNETERM_EVENT_COLOR => in_colors,
                LUNETERM_EVENT_BELL
                | LUNETERM_EVENT_TITLE_CHANGED
                | LUNETERM_EVENT_FOCUS_REPORT_REQUESTED
                | LUNETERM_EVENT_WINDOW_REPORT
                | LUNETERM_EVENT_A11Y_TAB
                | LUNETERM_EVENT_PROGRESS
                | LUNETERM_EVENT_UNKNOWN_OSC
                | LUNETERM_EVENT_COMMAND_MARK
                | LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND => event.offset == 0 && event.len == 0,
                _ => false,
            };
            if !ok {
                self.fail(&format!("event kind {} has a bad range", event.kind));
            }
        }
        // SAFETY: as above.
        for request in unsafe { slice(ev.colors, ev.color_count) } {
            if request.kind > 3 || request.reserved != 0 || (request.index > 258) {
                self.fail("a colour request field is out of range");
            }
        }
    }

    /// Calls an encoder with garbage in the out-param and checks the zeroing rule.
    fn encode(&mut self, call: impl FnOnce(*mut LunetermEngine, *mut LunetermBytes) -> i32) {
        let mut out = bytes_garbage();
        let code = call(self.engine, &mut out);
        let code = self.status(code);
        if code != LUNETERM_OK && !bytes_is_zero(&out) {
            self.fail(&format!("encoder out-param not zeroed on status {code}"));
        }
        if code == LUNETERM_OK {
            if out.len > 0 {
                self.totals.encoder_nonempty += 1;
                if out.ptr.is_null() {
                    self.fail("non-empty encoder output with a NULL pointer");
                }
            } else if !out.ptr.is_null() {
                self.fail("empty encoder output with a non-NULL pointer");
            }
        }
    }

    fn resize(&mut self) {
        let (cols, rows) = match self.rng.below(10) {
            0 => (0, 0),
            1 => (1, 1),
            2 => (LUNETERM_MAX_COLS, self.rng.between(1, 3) as u32),
            3 => (self.rng.between(2, 3) as u32, LUNETERM_MAX_ROWS),
            4 => (LUNETERM_MAX_COLS + 1, 24),
            5 => (80, LUNETERM_MAX_ROWS + 1),
            6 => (u32::MAX, u32::MAX),
            _ => (
                self.rng.between(2, 160) as u32,
                self.rng.between(1, 50) as u32,
            ),
        };
        // SAFETY: a live handle.
        let code = unsafe { luneterm_engine_resize(self.engine, cols, rows) };
        let code = self.status(code);
        let over = cols > LUNETERM_MAX_COLS || rows > LUNETERM_MAX_ROWS;
        match (over, code) {
            (true, LUNETERM_ERR_INVALID_ARGUMENT) => {}
            (false, LUNETERM_OK) => self.set_size_after_resize(cols, rows),
            _ => self.fail(&format!("resize {cols}x{rows} gave status {code}")),
        }
        self.check_size();
    }

    fn one_op(&mut self) {
        let choice = self.rng.below(100);
        let (kind, _): (&'static str, ()) = match choice {
            0..=29 => ("write", ()),
            30..=36 => ("resize", ()),
            37..=40 => ("scroll", ()),
            41..=46 => ("viewport", ()),
            47..=52 => ("drain", ()),
            53..=55 => ("damage", ()),
            56..=58 => ("modes", ()),
            59..=64 => ("key_event", ()),
            65..=67 => ("key_input", ()),
            68..=70 => ("char_input", ()),
            71..=75 => ("mouse_event", ()),
            76..=77 => ("focus_event", ()),
            78..=83 => ("paste", ()),
            84..=87 => ("null_or_misaligned_out", ()),
            88..=91 => ("null_input", ()),
            92..=93 => ("idle", ()),
            94..=95 => ("gates", ()),
            _ => ("size", ()),
        };
        self.progress.kind = kind;
        self.totals.ops += 1;
        if std::env::var_os("LUNETERM_FFI_FUZZ_TRACE").is_some() {
            eprintln!(
                "op {} {} (size {}x{})",
                self.progress.op, kind, self.size.0, self.size.1
            );
        }
        match kind {
            "write" => {
                let data = gen_write(&mut self.rng);
                self.write(&data);
            }
            "resize" => self.resize(),
            "scroll" => {
                let delta = *self
                    .rng
                    .pick(&[i32::MIN, i32::MAX, -1, 1, -50, 50, 0, -1000]);
                // SAFETY: a live handle.
                let code = unsafe {
                    match self.rng.below(3) {
                        0 => luneterm_engine_scroll_lines(self.engine, delta),
                        1 => luneterm_engine_scroll_to_top(self.engine),
                        _ => luneterm_engine_scroll_to_bottom(self.engine),
                    }
                };
                self.expect_ok(code);
            }
            "viewport" => self.viewport(),
            "drain" => self.drain(),
            "damage" => {
                let mut damage = LunetermDamage {
                    first_row: 7,
                    last_row: 7,
                    ..LunetermDamage::default()
                };
                // SAFETY: a live handle and out-param.
                let code = unsafe { luneterm_engine_take_damage(self.engine, &mut damage) };
                self.expect_ok(code);
                if damage.has_rows > 1
                    || damage.full > 1
                    || damage.cursor_moved > 1
                    || damage.scrolled > 1
                {
                    self.fail("damage flags are not 0/1");
                }
                // After a shrink the row range may predate it; `full` then says everything is
                // redrawn.
                if damage.has_rows == 1
                    && damage.full == 0
                    && (damage.first_row > damage.last_row || damage.last_row >= self.size.1)
                {
                    self.fail(&format!(
                        "damage rows {}..={} outside {} viewport rows",
                        damage.first_row, damage.last_row, self.size.1
                    ));
                }
            }
            "modes" => {
                let mut modes = LunetermModes {
                    wraparound: 7,
                    ..LunetermModes::default()
                };
                // SAFETY: a live handle and out-param.
                let code = unsafe { luneterm_engine_modes(self.engine, &mut modes) };
                self.expect_ok(code);
                let flags = [
                    modes.application_cursor_keys,
                    modes.application_keypad,
                    modes.bracketed_paste,
                    modes.insert,
                    modes.origin,
                    modes.reverse_wraparound,
                    modes.send_focus,
                    modes.synchronized_output,
                    modes.wraparound,
                    modes.cursor_hidden,
                ];
                if flags.iter().any(|f| *f > 1)
                    || modes.mouse_tracking > 4
                    || modes.mouse_encoding > 2
                    || modes.modify_other_keys > 2
                {
                    self.fail("a mode field is out of range");
                }
            }
            "key_event" => {
                let key = if self.rng.chance(75) {
                    self.rng.below(28) as u32
                } else {
                    self.rng.u32()
                };
                let event = LunetermKeyEvent {
                    key,
                    codepoint: if self.rng.chance(60) {
                        self.rng.between(0x20, 0x7E) as u32
                    } else {
                        self.rng.u32()
                    },
                    shifted_codepoint: if self.rng.chance(50) {
                        0
                    } else {
                        self.rng.u32()
                    },
                    base_layout_codepoint: if self.rng.chance(50) {
                        0
                    } else {
                        self.rng.u32()
                    },
                    modifiers: self.rng.byte(),
                    is_keypad: self.rng.byte(),
                    alt_is_meta: self.rng.byte(),
                    reserved: self.rng.byte(),
                };
                let expect_invalid = key > 27;
                let before = self
                    .totals
                    .statuses
                    .get(&LUNETERM_ERR_INVALID_ARGUMENT)
                    .copied();
                // SAFETY: a live handle, event and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_key_event(engine, &event, out)
                });
                let after = self
                    .totals
                    .statuses
                    .get(&LUNETERM_ERR_INVALID_ARGUMENT)
                    .copied();
                if expect_invalid != (after != before) {
                    self.fail("key_event status does not match the key range");
                }
            }
            "key_input" => {
                let key = if self.rng.chance(80) {
                    self.rng.below(28) as u32
                } else {
                    self.rng.u32()
                };
                let modifiers = self.rng.byte();
                // SAFETY: a live handle and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_key_input(engine, key, modifiers, out)
                });
            }
            "char_input" => {
                let codepoint = *self.rng.pick(&[
                    0x61,
                    0x5A,
                    0x20,
                    0x1F600,
                    0xD800,
                    0xDFFF,
                    0x110000,
                    u32::MAX,
                    0,
                    0xE9,
                ]);
                let modifiers = self.rng.byte();
                // SAFETY: a live handle and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_char_input(engine, codepoint, modifiers, out)
                });
            }
            "mouse_event" => {
                let edge = |rng: &mut Rng| {
                    *rng.pick(&[0, 1, 5, 222, 223, 224, 4095, 2047, -1, i32::MIN, i32::MAX])
                };
                let event = LunetermMouseEvent {
                    button: if self.rng.chance(80) {
                        self.rng.below(6) as u32
                    } else {
                        self.rng.u32()
                    },
                    kind: if self.rng.chance(80) {
                        self.rng.below(6) as u32
                    } else {
                        self.rng.u32()
                    },
                    col: edge(&mut self.rng),
                    row: edge(&mut self.rng),
                    x: edge(&mut self.rng),
                    y: edge(&mut self.rng),
                    modifiers: self.rng.byte(),
                    reserved: [self.rng.byte(); 3],
                };
                // SAFETY: a live handle, event and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_mouse_event(engine, &event, out)
                });
            }
            "focus_event" => {
                let focused = self.rng.byte();
                // SAFETY: a live handle and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_focus_event(engine, focused, out)
                });
            }
            "paste" => {
                let mut text: Vec<u8> = Vec::new();
                match self.rng.below(4) {
                    0 => {
                        for _ in 0..self.rng.between(0, 200) {
                            text.push(self.rng.byte());
                        }
                    }
                    1 => text.extend_from_slice(b"a\x1b[201~b\r\nc\x1b[200~\x01\x7f"),
                    2 => text.extend_from_slice(self.rng.pick(&TEXT).as_bytes()),
                    _ => text.extend_from_slice(
                        "line\n"
                            .repeat(self.rng.between(0, 300) as usize)
                            .as_bytes(),
                    ),
                }
                let valid = std::str::from_utf8(&text).is_ok();
                let before = self
                    .totals
                    .statuses
                    .get(&LUNETERM_ERR_INVALID_UTF8)
                    .copied();
                // SAFETY: a live handle, a live slice and out-param.
                self.encode(|engine, out| unsafe {
                    luneterm_engine_paste(engine, text.as_ptr(), text.len(), out)
                });
                let after = self
                    .totals
                    .statuses
                    .get(&LUNETERM_ERR_INVALID_UTF8)
                    .copied();
                if valid != (after == before) {
                    self.fail("paste status does not match the UTF-8 validity");
                }
            }
            "null_or_misaligned_out" => self.null_or_misaligned_out(),
            "null_input" => {
                self.null_handle();
                let n = self.rng.between(1, 4096) as usize;
                // SAFETY: a live handle; NULL data with a length is refused before any read.
                let code = unsafe { luneterm_engine_write(self.engine, null(), n) };
                if self.status(code) != LUNETERM_ERR_NULL_POINTER {
                    self.fail("write(NULL, n) was not refused");
                }
                let mut out = bytes_garbage();
                // SAFETY: as above.
                let code = unsafe { luneterm_engine_paste(self.engine, null(), n, &mut out) };
                if self.status(code) != LUNETERM_ERR_NULL_POINTER || !bytes_is_zero(&out) {
                    self.fail("paste(NULL, n) was not refused or not zeroed");
                }
                // SAFETY: (NULL, 0) is a valid empty input.
                let code = unsafe { luneterm_engine_write(self.engine, null(), 0) };
                self.expect_ok(code);
                // A length above PTRDIFF_MAX is refused before the pointer is read.
                // SAFETY: a live handle; the pointer is never read.
                let code = unsafe {
                    luneterm_engine_write(self.engine, dangling(), (isize::MAX as usize) + 1)
                };
                if self.status(code) != LUNETERM_ERR_INVALID_ARGUMENT {
                    self.fail("a length above PTRDIFF_MAX was not refused");
                }
            }
            "idle" => {
                let mut more = 7u8;
                // SAFETY: a live handle and out-param.
                let code = unsafe { luneterm_engine_idle(self.engine, &mut more) };
                self.expect_ok(code);
                if more > 1 {
                    self.fail("idle's more flag is not 0/1");
                }
            }
            "gates" => {
                let flag = self.rng.byte();
                // SAFETY: a live handle.
                let code = unsafe {
                    if self.rng.chance(50) {
                        luneterm_engine_set_clipboard_write(self.engine, flag)
                    } else {
                        luneterm_engine_reset_keyboard_protocol(self.engine)
                    }
                };
                self.expect_ok(code);
            }
            _ => self.check_size(),
        }
    }

    /// A NULL handle on every output export, with garbage in the out-params: `ERR_NULL_POINTER`
    /// and every out-param zeroed (R1).
    fn null_handle(&mut self) {
        let mut vp = viewport_garbage();
        let mut ev = events_garbage();
        let mut damage = LunetermDamage {
            has_rows: 1,
            full: 1,
            cursor_moved: 1,
            scrolled: 1,
            first_row: 9,
            last_row: 9,
        };
        let mut modes = LunetermModes {
            wraparound: 1,
            bracketed_paste: 1,
            ..LunetermModes::default()
        };
        let mut bytes = bytes_garbage();
        let (mut cols, mut rows) = (9u32, 9u32);
        // SAFETY: a NULL handle is refused; every out-param is a live local.
        let codes = unsafe {
            [
                luneterm_engine_viewport(null_mut(), &mut vp),
                luneterm_engine_drain_events(null_mut(), &mut ev),
                luneterm_engine_take_damage(null_mut(), &mut damage),
                luneterm_engine_modes(null_mut(), &mut modes),
                luneterm_engine_key_input(null_mut(), 0, 0, &mut bytes),
                luneterm_engine_size(null_mut(), &mut cols, &mut rows),
            ]
        };
        for code in codes {
            if self.status(code) != LUNETERM_ERR_NULL_POINTER {
                self.fail("a NULL handle was not refused");
            }
        }
        if !viewport_is_zero(&vp)
            || !events_is_zero(&ev)
            || damage != LunetermDamage::default()
            || modes != LunetermModes::default()
            || !bytes_is_zero(&bytes)
            || (cols, rows) != (0, 0)
        {
            self.fail("an out-param was not zeroed on a NULL handle");
        }
    }

    /// NULL and misaligned out-parameters on every output export: refused, never written.
    fn null_or_misaligned_out(&mut self) {
        let mut buf = [0xA5A5_A5A5_A5A5_A5A5u64; 32];
        let bad = buf.as_mut_ptr().cast::<u8>().wrapping_add(1);
        let engine = self.engine;
        let null_case = self.rng.chance(50);
        let codes = {
            // SAFETY: a live handle; a NULL out-param is refused, and a misaligned one (an odd
            // address into `buf`) is refused before any write.
            unsafe {
                if null_case {
                    [
                        luneterm_engine_viewport(engine, null_mut()),
                        luneterm_engine_drain_events(engine, null_mut()),
                        luneterm_engine_take_damage(engine, null_mut()),
                        luneterm_engine_modes(engine, null_mut()),
                        luneterm_engine_key_input(engine, 0, 0, null_mut()),
                        luneterm_engine_char_input(engine, 97, 0, null_mut()),
                        luneterm_engine_focus_event(engine, 1, null_mut()),
                        luneterm_engine_paste(engine, b"x".as_ptr(), 1, null_mut()),
                        luneterm_engine_key_event(engine, &LunetermKeyEvent::default(), null_mut()),
                        luneterm_engine_mouse_event(
                            engine,
                            &LunetermMouseEvent::default(),
                            null_mut(),
                        ),
                        luneterm_engine_size(engine, null_mut(), null_mut()),
                        luneterm_engine_idle(engine, null_mut()),
                    ]
                } else {
                    [
                        luneterm_engine_viewport(engine, bad.cast()),
                        luneterm_engine_drain_events(engine, bad.cast()),
                        luneterm_engine_take_damage(engine, bad.cast()),
                        // LunetermModes has alignment 1: it cannot be misaligned; use the u32 pair.
                        luneterm_engine_size(engine, bad.cast(), bad.cast()),
                        luneterm_engine_key_input(engine, 0, 0, bad.cast()),
                        luneterm_engine_char_input(engine, 97, 0, bad.cast()),
                        luneterm_engine_focus_event(engine, 1, bad.cast()),
                        luneterm_engine_paste(engine, b"x".as_ptr(), 1, bad.cast()),
                        luneterm_engine_key_event(engine, &LunetermKeyEvent::default(), bad.cast()),
                        luneterm_engine_mouse_event(
                            engine,
                            &LunetermMouseEvent::default(),
                            bad.cast(),
                        ),
                        luneterm_engine_key_event(engine, bad.cast(), bad.cast()),
                        luneterm_engine_mouse_event(engine, bad.cast(), bad.cast()),
                    ]
                }
            }
        };
        let expected = if null_case {
            LUNETERM_ERR_NULL_POINTER
        } else {
            LUNETERM_ERR_INVALID_ARGUMENT
        };
        for code in codes {
            if self.status(code) != expected {
                self.fail(&format!(
                    "a bad out-pointer gave status {code}, expected {expected}"
                ));
            }
        }
        if buf != [0xA5A5_A5A5_A5A5_A5A5u64; 32] {
            self.fail("a refused out-pointer was written");
        }
    }
}

/// An array from an output struct: NULL means empty.
///
/// # Safety
///
/// `ptr` is NULL or valid for `len` elements.
unsafe fn slice<'a, T>(ptr: *const T, len: u64) -> &'a [T] {
    if len == 0 || ptr.is_null() {
        return &[];
    }
    // SAFETY: by the caller's contract.
    unsafe { std::slice::from_raw_parts(ptr, usize::try_from(len).unwrap()) }
}

fn run_seed(seed: u64, progress: &mut Progress, totals: &mut Totals) {
    let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) ^ 0x5851_F42D_4C95_7F2D);
    // Config: mostly valid and small, now and then hostile.
    let hostile = rng.chance(15);
    let cols = if hostile {
        *rng.pick(&[0, 1, LUNETERM_MAX_COLS, LUNETERM_MAX_COLS + 1, u32::MAX])
    } else {
        rng.between(1, 160) as u32
    };
    let rows = if hostile {
        *rng.pick(&[0, 1, 3, LUNETERM_MAX_ROWS + 1, u32::MAX])
    } else {
        rng.between(1, 50) as u32
    };
    let scrollback = if hostile {
        *rng.pick(&[
            0,
            LUNETERM_MAX_SCROLLBACK,
            LUNETERM_MAX_SCROLLBACK + 1,
            u32::MAX,
        ])
    } else {
        *rng.pick(&[0, 10, 100, 5000])
    };
    let unicode = if hostile && rng.chance(50) {
        *rng.pick(&[3, 99, u32::MAX])
    } else {
        rng.below(3) as u32
    };
    let name = *rng.pick(&["luneterm", "x", "a b", "caf\u{e9}", "n\0m"]);
    let (xtversion, xtversion_len) = match rng.below(4) {
        0 => (null(), 0),
        1 => (name.as_ptr(), name.len() as u64),
        2 => (name.as_ptr(), 0),
        _ => (null(), 3),
    };
    let config = LunetermConfig {
        cols,
        rows,
        scrollback,
        unicode,
        xtversion,
        xtversion_len,
        clipboard_write: rng.byte(),
        reserved: [rng.byte(); 7],
    };
    progress.op = 0;
    progress.kind = "new";
    let mut engine: *mut LunetermEngine = dangling_mut_engine();
    // SAFETY: a live config and out-param; the xtversion pointer is NULL or a live string.
    let code = unsafe { luneterm_engine_new(&config, &mut engine) };
    *totals.statuses.entry(code).or_insert(0) += 1;
    assert!(
        is_defined(code) && code != LUNETERM_ERR_PANIC,
        "seed {seed} new: status {code}"
    );
    let must_fail = cols > LUNETERM_MAX_COLS
        || rows > LUNETERM_MAX_ROWS
        || scrollback > LUNETERM_MAX_SCROLLBACK
        || unicode > 2
        || (xtversion.is_null() && xtversion_len > 0);
    if code != LUNETERM_OK {
        assert!(
            engine.is_null(),
            "seed {seed} new: *out not NULL on status {code}"
        );
        return;
    }
    assert!(!must_fail, "seed {seed} new: accepted an invalid config");
    assert!(!engine.is_null());

    let mut fuzz = Fuzz {
        rng,
        engine,
        size: (cols.max(2), rows.max(1)),
        progress,
        totals,
        seed,
    };
    fuzz.check_size();
    for op in 0..OPS_PER_SEED {
        fuzz.progress.op = op + 1;
        fuzz.one_op();
    }
    fuzz.progress.kind = "final";
    fuzz.drain();
    fuzz.viewport();
    fuzz.check_size();
    // SAFETY: a live handle, used for the last time.
    let code = unsafe { luneterm_engine_free(engine) };
    assert_eq!(code, LUNETERM_OK, "seed {seed} free");
}

fn dangling_mut_engine() -> *mut LunetermEngine {
    std::ptr::dangling_mut()
}

#[test]
fn the_abi_survives_random_and_hostile_calls() {
    let single = std::env::var("LUNETERM_FFI_FUZZ_SEED")
        .ok()
        .map(|s| s.parse::<u64>().expect("LUNETERM_FFI_FUZZ_SEED"));
    let count = std::env::var("LUNETERM_FFI_FUZZ_SEEDS")
        .ok()
        .map(|s| s.parse::<u64>().expect("LUNETERM_FFI_FUZZ_SEEDS"))
        .unwrap_or(DEFAULT_SEEDS);
    let seeds: Vec<u64> = match single {
        Some(seed) => vec![seed],
        None => (0..count).collect(),
    };
    let mut totals = Totals::default();
    for &seed in &seeds {
        let mut progress = Progress { op: 0, kind: "new" };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run_seed(seed, &mut progress, &mut totals)
        }));
        if let Err(payload) = outcome {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "non-string panic".to_owned());
            panic!(
                "ffi fuzz failed: seed {seed}, op {} ({}): {message} (replay: LUNETERM_FFI_FUZZ_SEED={seed})",
                progress.op, progress.kind
            );
        }
    }
    eprintln!(
        "ffi fuzz: {} seeds, {} ops, {} viewports, {} non-empty encoder outputs, statuses {:?}, event kinds {:?}",
        seeds.len(),
        totals.ops,
        totals.viewports,
        totals.encoder_nonempty,
        totals.statuses,
        totals.event_kinds
    );
    if single.is_none() && seeds.len() >= 20 {
        // Instrument check: the run reached what it claims to fuzz.
        for code in [
            LUNETERM_OK,
            LUNETERM_ERR_NULL_POINTER,
            LUNETERM_ERR_INVALID_ARGUMENT,
            LUNETERM_ERR_INVALID_UTF8,
        ] {
            assert!(
                totals.statuses.get(&code).copied().unwrap_or(0) > 0,
                "status {code} never occurred"
            );
        }
        assert!(
            totals.encoder_nonempty > 20,
            "encoders rarely produced output"
        );
        assert!(
            totals.event_kinds.len() >= 6,
            "only {} event kinds seen",
            totals.event_kinds.len()
        );
        assert!(totals.viewports > 100);
    }
}
