//! Seeded random-operation no-panic test of the buffers (plan M5 step 8).
//!
//! Per seed: an `OptionsService` with a random scrollback, a `BufferService` at a random size,
//! and `OPS_PER_SEED` random operations over it. The operations are the ones the input handler
//! and the host perform and some they never do: printing text (wrapped lines, wide characters,
//! combining marks, url-linked curly underlines), resizing both ways (reflow), option changes
//! applied as the host would, scrolling in a scroll region, `scroll_lines`, cursor moves, raw
//! list `splice`/`trim_start`, cell edits at positions up to `cols + 3`, `copy_cells_from`,
//! markers, `OscLinkService`, buffer switches, wrapped-range and string translation queries and
//! the idle memory cleanup. Each seed runs under `catch_unwind`; a failure reports the seed and
//! the operation index only, never buffer content.
//!
//! Hard assertions hold after every operation (see `check_hard`); the soft invariants after a
//! resize hold only if the buffer was well formed before it (`Pre`): the raw list operations
//! deliberately leave `ybase`/`ydisp` inconsistent with the list now and then, and resizing such
//! a buffer promises nothing.
//!
//! Debug builds keep integer-overflow checks on, so arithmetic overflow in the port is a panic
//! here.
//!
//! `LUNE_VT_BUFFER_FUZZ_SEEDS=<n>` runs seeds `0..n` (default 300); `LUNE_VT_BUFFER_FUZZ_SEED=<s>`
//! replays one.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};

use lune_vt::buffer::attribute_data::{AttributeData, ExtendedAttrs};
use lune_vt::buffer::buffer::{Buffer, MAX_BUFFER_SIZE};
use lune_vt::buffer::buffer_line::BufferLine;
use lune_vt::buffer::cell_data::CellData;
use lune_vt::buffer::constants::{BgFlags, Content, UnderlineStyle};
use lune_vt::buffer::marker::MarkerId;
use lune_vt::services::buffer_service::BufferService;
use lune_vt::services::options_service::{OptionValue, OptionsService, PtyBackendKind, WindowsPty};
use lune_vt::services::osc_link_service::{OscLinkData, OscLinkService};

const DEFAULT_SEEDS: u64 = 300;
const OPS_PER_SEED: usize = 200;

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

    /// Uniform-ish in `0..n` (`n > 0`).
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// Uniform-ish in `0..n` (`n > 0`).
    fn usize_below(&mut self, n: usize) -> usize {
        usize::try_from(self.below(n as u64)).unwrap_or(0)
    }

    /// Uniform-ish in `lo..=hi`.
    fn between(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.usize_below(hi.saturating_sub(lo) + 1)
    }

    /// `true` with probability `percent`/100.
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn u32(&mut self) -> u32 {
        u32::try_from(self.next_u64() & 0xFFFF_FFFF).unwrap_or(0)
    }
}

// ---- the operations ----

#[derive(Clone, Copy)]
enum Op {
    Print,
    Linefeed,
    Resize,
    OptionChange,
    Scroll,
    ScrollLines,
    Cursor,
    Splice,
    TrimStart,
    CellEdit,
    CopyCells,
    Marker,
    Osc,
    Activate,
    WrappedRange,
    Translate,
    Cleanup,
    Tabs,
}

const OPS: [(Op, u64); 18] = [
    (Op::Print, 22),
    (Op::Linefeed, 5),
    (Op::Resize, 9),
    (Op::OptionChange, 5),
    (Op::Scroll, 8),
    (Op::ScrollLines, 4),
    (Op::Cursor, 5),
    (Op::Splice, 5),
    (Op::TrimStart, 2),
    (Op::CellEdit, 14),
    (Op::CopyCells, 4),
    (Op::Marker, 6),
    (Op::Osc, 5),
    (Op::Activate, 3),
    (Op::WrappedRange, 3),
    (Op::Translate, 3),
    (Op::Cleanup, 2),
    (Op::Tabs, 2),
];

fn pick_op(rng: &mut Rng) -> usize {
    let total: u64 = OPS.iter().map(|(_, w)| w).sum();
    let mut roll = rng.below(total);
    for (index, (_, weight)) in OPS.iter().enumerate() {
        if roll < *weight {
            return index;
        }
        roll -= weight;
    }
    0
}

/// A buffer's state before a resize (what decides which invariants must hold after it).
#[derive(Clone, Copy)]
struct Pre {
    non_empty: bool,
    well_formed: bool,
    /// Numbers for failure messages: ybase, ydisp, y, x, len, max_length, cols.
    nums: [usize; 7],
}

struct State {
    options: OptionsService,
    svc: BufferService,
    osc: OscLinkService,
    /// An upper bound on the markers created (the `OscLinkService` operations count one each).
    created: usize,
    disposed_seen: HashSet<MarkerId>,
    /// Link ids handed out, for `urlId`s and `add_line_to_link`.
    links: Vec<u32>,
    max_cols: usize,
    /// Raw list operations that leave the state a host can reach are allowed.
    chaos: bool,
    // Statistics for the generator test.
    op_counts: [u32; OPS.len()],
    shrinks: u32,
    grows: u32,
    max_ybase: usize,
    wrapped_peak: usize,
    disposed_total: usize,
    soft_checked: u32,
    width_checked: u32,
    links_freed: u32,
}

struct Report {
    digest: u64,
    op_counts: [u32; OPS.len()],
    shrinks: u32,
    grows: u32,
    max_ybase: usize,
    wrapped_peak: usize,
    disposed_total: usize,
    soft_checked: u32,
    width_checked: u32,
    links_freed: u32,
}

impl State {
    fn opts(&self) -> lune_vt::services::options_service::TerminalOptions {
        self.options.options().clone()
    }

    fn buffer(&mut self, alt: bool) -> &mut Buffer {
        if alt {
            self.svc.buffers.alt_mut()
        } else {
            self.svc.buffers.normal_mut()
        }
    }

    /// Mostly the active buffer, now and then the other one.
    fn pick_alt(&self, rng: &mut Rng) -> bool {
        let active = self.svc.buffers.is_alt_active();
        if rng.chance(75) { active } else { !active }
    }
}

fn blank_attr(rng: &mut Rng, links: &[u32]) -> AttributeData {
    if rng.chance(70) {
        return AttributeData::default();
    }
    random_attr(rng, links)
}

fn random_attr(rng: &mut Rng, links: &[u32]) -> AttributeData {
    let mut attr = AttributeData::default();
    if rng.chance(50) {
        attr.fg = rng.u32();
    }
    if rng.chance(50) {
        attr.bg = rng.u32() & !BgFlags::HAS_EXTENDED;
    }
    if rng.chance(25) {
        // A curly underline, linked half of the time.
        let mut ext = ExtendedAttrs::default();
        ext.set_underline_style(UnderlineStyle::CURLY);
        ext.set_underline_color(rng.u32() & 0x03FF_FFFF);
        if !links.is_empty() && rng.chance(60) {
            ext.set_url_id(links[rng.usize_below(links.len())]);
        }
        attr.extended = ext;
        attr.bg |= BgFlags::HAS_EXTENDED;
    }
    attr
}

/// A code point and its width as `print` stores it.
fn pick_char(rng: &mut Rng) -> (u32, u32) {
    match rng.below(100) {
        0..=69 => (0x21 + rng.u32() % 94, 1),
        70..=77 => (0x20, 1),
        78..=87 => (0x4E00 + rng.u32() % 0x100, 2),
        88..=93 => (0x1F600 + rng.u32() % 0x40, 2),
        _ => (0xE9, 1),
    }
}

// ---- invariants ----

/// A state the host can reach: the list holds exactly the scrollback and the viewport, its
/// maximum length is the one the options give, and the cursor and the viewport are in range.
/// Raw list operations (`chaos` seeds) leave this state; resizing from there promises nothing.
fn well_formed(b: &Buffer, alt: bool, scrollback: usize) -> bool {
    let max = if alt {
        b.rows()
    } else {
        (b.rows() + scrollback).min(MAX_BUFFER_SIZE)
    };
    b.lines.len() == b.ybase + b.rows()
        && b.lines.max_length() == max
        && b.y < b.rows()
        && b.x <= b.cols()
        && b.ydisp <= b.ybase
}

fn snapshot(st: &mut State) -> [Pre; 2] {
    let mut pre = [Pre {
        non_empty: false,
        well_formed: false,
        nums: [0; 7],
    }; 2];
    let scrollback = st.options.options().scrollback;
    for (i, alt) in [false, true].into_iter().enumerate() {
        let b = st.buffer(alt);
        pre[i] = Pre {
            non_empty: !b.lines.is_empty(),
            well_formed: well_formed(b, alt, scrollback),
            nums: [
                b.ybase,
                b.ydisp,
                b.y,
                b.x,
                b.lines.len(),
                b.lines.max_length(),
                b.cols(),
            ],
        };
    }
    pre
}

/// After a resize (or a scrollback change followed by `buffers.resize`).
fn check_after_resize(st: &mut State, pre: [Pre; 2], old_rows: usize, op: usize) {
    let (cols, rows) = (st.svc.cols, st.svc.rows);
    let opts = st.opts();
    let conpty_reflow = opts
        .windows_pty
        .build_number
        .is_some_and(|n| n != 0 && opts.windows_pty.backend == Some(PtyBackendKind::Conpty));
    let (mut width_checked, mut soft_checked) = (0, 0);
    for (i, alt) in [false, true].into_iter().enumerate() {
        let scrollback = opts.scrollback;
        let b = st.buffer(alt);
        assert_eq!((b.cols(), b.rows()), (cols, rows), "op {op}: buffer size");
        let expected = if alt {
            rows
        } else {
            (rows + scrollback).min(MAX_BUFFER_SIZE)
        };
        if pre[i].non_empty {
            assert_eq!(b.lines.max_length(), expected, "op {op}: max_length");
        } else {
            assert!(b.lines.max_length() >= expected, "op {op}: max_length");
        }
        // Reflow certainly ran on the normal buffer unless windows mode or a pty version
        // switches it off; only then the lines were trimmed to `cols`.
        let reflowed = !alt
            && !opts.windows_mode
            && opts.windows_pty.build_number.is_none_or(|n| n == 0)
            && rows + scrollback > old_rows
            && pre[i].nums[6] != cols
            && !conpty_reflow;
        for line_index in 0..b.lines.len() {
            if let Some(line) = b.lines.get(line_index) {
                assert!(
                    line.len() >= cols,
                    "op {op}: line {line_index} shorter than cols"
                );
                if reflowed {
                    width_checked += 1;
                    assert_eq!(
                        line.len(),
                        cols,
                        "op {op}: line {line_index} width (alt {alt}, cols {} -> {cols}, rows {old_rows} -> {rows}, scrollback {scrollback}, windows_mode {}, pty {:?}, pre {:?})",
                        pre[i].nums[6],
                        opts.windows_mode,
                        opts.windows_pty,
                        pre[i].nums
                    );
                }
            }
        }
        if pre[i].well_formed {
            soft_checked += 1;
            // Soft invariants: may be upstream behaviour, not a port bug.
            assert!(b.y < rows, "SOFT op {op}: y {} >= rows {rows}", b.y);
            assert!(b.x < cols, "SOFT op {op}: x {} >= cols {cols}", b.x);
            assert!(
                b.ydisp <= b.ybase,
                "SOFT op {op}: ydisp {} > ybase {}",
                b.ydisp,
                b.ybase
            );
            // Without scrollback (`max_length == rows`) upstream's reflow can leave `ybase`
            // above what the list holds: `_reflowSmaller` bumps `ybase` per added line while
            // the list is capped at `rows` (real xterm.js 6.0.0 gives baseY 1, length 15 for
            // 97x9 -> 23x15 with `scrollback: 0`; see `reflow_without_scrollback_matches_*`).
            assert!(
                b.lines.max_length() <= rows || b.ybase + rows <= b.lines.len(),
                "SOFT op {op}: ybase {} + rows {rows} > len {} (alt {alt}, cols {} -> {cols}, rows {old_rows} -> {rows}, scrollback {scrollback}, windows_mode {}, pty {:?}, pre [ybase, ydisp, y, x, len, max, cols] {:?})",
                b.ybase,
                b.lines.len(),
                pre[i].nums[6],
                opts.windows_mode,
                opts.windows_pty,
                pre[i].nums
            );
        }
    }
    st.width_checked += width_checked;
    st.soft_checked += soft_checked;
}

/// Holds after every operation.
fn check_hard(st: &mut State, op: usize) {
    let bound = 4 * st.max_cols;
    for alt in [false, true] {
        let b = st.buffer(alt);
        assert!(
            b.lines.len() <= b.lines.max_length(),
            "op {op}: list longer than its max length"
        );
        for line_index in 0..b.lines.len() {
            if let Some(line) = b.lines.get(line_index) {
                let held = line.combined_len() + line.extended_attrs_len();
                assert!(
                    held <= bound,
                    "op {op}: line {line_index} holds {held} side-table entries (bound {bound})"
                );
            }
        }
    }
    assert!(
        st.disposed_seen.len() <= st.created,
        "op {op}: disposed > created"
    );
    assert!(
        st.osc.marker_count() <= st.created,
        "op {op}: osc markers > created"
    );
    assert!(
        st.osc.len() <= st.osc.marker_count(),
        "op {op}: link without marker"
    );
}

/// The host duty: drain both buffers' disposal logs into the `OscLinkService`.
fn forward(st: &mut State, op: usize) {
    let links_before = st.osc.len();
    for alt in [false, true] {
        let ids = st.buffer(alt).take_disposed_markers();
        for id in &ids {
            assert!(
                st.disposed_seen.insert(*id),
                "op {op}: marker disposed twice"
            );
        }
        st.disposed_total += ids.len();
        st.osc.on_markers_disposed(&ids);
    }
    assert!(
        st.disposed_seen.len() <= st.created,
        "op {op}: disposed > created"
    );
    st.links_freed += u32::from(st.osc.len() < links_before);
    // Every marker an entry holds is live in the buffer that holds it, and no entry is empty.
    let ids: Vec<u32> = st.osc.link_ids().collect();
    for link_id in ids {
        let markers = st.osc.link_markers(link_id).unwrap().to_vec();
        assert!(!markers.is_empty(), "op {op}: link without markers");
        for m in markers {
            assert!(
                st.buffer(m.alt).marker(m.id).is_some(),
                "op {op}: link holds a disposed marker"
            );
        }
    }
}

// ---- operations ----

fn print_one(st: &mut State, rng: &mut Rng, attr: &AttributeData) {
    let (cp, width) = pick_char(rng);
    let cols = st.svc.cols;
    if st.svc.buffer().x + width as usize > cols {
        // Wrap, as `InputHandler.print`.
        st.svc.buffer_mut().x = 0;
        let b = st.svc.buffer_mut();
        if b.y == b.scroll_bottom {
            st.svc.scroll(&AttributeData::default(), true);
        } else if b.y + 1 < b.rows() {
            b.y += 1;
            let at = b.ybase + b.y;
            if let Some(line) = b.lines.get_mut(at) {
                line.is_wrapped = true;
            }
        }
    }
    let b = st.svc.buffer_mut();
    let (at, x) = (b.ybase + b.y, b.x);
    if let Some(line) = b.lines.get_mut(at) {
        line.set_cell_from_codepoint(x, cp, width, attr);
        if width == 2 {
            line.set_cell_from_codepoint(x + 1, 0, 0, attr);
        }
        if rng.chance(6) {
            line.add_codepoint_to_cell(x, 0x301, 0);
        }
    }
    b.x += width as usize;
}

fn linefeed(st: &mut State) {
    let b = st.svc.buffer_mut();
    b.x = 0;
    if b.y == b.scroll_bottom {
        st.svc.scroll(&AttributeData::default(), false);
    } else if b.y + 1 < b.rows() {
        b.y += 1;
    }
}

fn op_resize(st: &mut State, rng: &mut Rng, op: usize) {
    let (old_cols, old_rows) = (st.svc.cols, st.svc.rows);
    let cols = match rng.below(10) {
        0 => old_cols,
        1..=5 => {
            let delta = rng.between(1, 20);
            if rng.chance(50) {
                old_cols.saturating_sub(delta).max(2)
            } else {
                (old_cols + delta).min(120)
            }
        }
        _ => rng.between(2, 120),
    };
    let rows = match rng.below(10) {
        0 => old_rows,
        1..=5 => {
            let delta = rng.between(1, 10);
            if rng.chance(50) {
                old_rows.saturating_sub(delta).max(1)
            } else {
                (old_rows + delta).min(50)
            }
        }
        _ => rng.between(1, 50),
    };
    st.max_cols = st.max_cols.max(cols);
    st.shrinks += u32::from(cols < old_cols);
    st.grows += u32::from(cols > old_cols);
    let pre = snapshot(st);
    let opts = st.opts();
    st.svc.resize(cols, rows, &opts);
    check_after_resize(st, pre, old_rows, op);
    forward(st, op);
    let wrapped = count_wrapped(st);
    st.wrapped_peak = st.wrapped_peak.max(wrapped);
}

fn count_wrapped(st: &mut State) -> usize {
    let mut n = 0;
    for alt in [false, true] {
        let b = st.buffer(alt);
        n += (0..b.lines.len())
            .filter(|&i| b.lines.get(i).is_some_and(|l| l.is_wrapped))
            .count();
    }
    n
}

fn op_option_change(st: &mut State, rng: &mut Rng, op: usize) {
    let value = match rng.below(6) {
        0 | 1 => OptionValue::Scrollback(rng.between(0, 300)),
        2 => OptionValue::TabStopWidth(rng.between(1, 16)),
        3 => OptionValue::ReflowCursorLine(rng.chance(50)),
        4 => OptionValue::WindowsMode(rng.chance(30)),
        _ => OptionValue::WindowsPty(match rng.below(4) {
            0 => None,
            1 => Some(WindowsPty {
                backend: Some(PtyBackendKind::Conpty),
                build_number: Some(21375),
            }),
            2 => Some(WindowsPty {
                backend: Some(PtyBackendKind::Conpty),
                build_number: Some(21376),
            }),
            _ => Some(WindowsPty {
                backend: Some(PtyBackendKind::Winpty),
                build_number: Some(19045),
            }),
        }),
    };
    let key = match st.options.set(value.clone()) {
        Ok(Some(key)) => key,
        _ => return,
    };
    // Applied as the host would (the listeners of `BufferSet`'s constructor).
    let opts = st.opts();
    match value {
        OptionValue::Scrollback(_) => {
            let (cols, rows, old_rows) = (st.svc.cols, st.svc.rows, st.svc.rows);
            let pre = snapshot(st);
            st.svc.buffers.resize(cols, rows, &opts);
            check_after_resize(st, pre, old_rows, op);
            forward(st, op);
        }
        OptionValue::TabStopWidth(_) => st.svc.buffers.setup_tab_stops(None, &opts),
        _ => {}
    }
    let _ = key;
}

fn op_scroll(st: &mut State, rng: &mut Rng) {
    let rows = st.svc.rows;
    for _ in 0..rng.between(1, 6) {
        let top = if rng.chance(70) {
            0
        } else {
            rng.usize_below(rows)
        };
        let bottom = rng.between(top, rows - 1);
        let b = st.svc.buffer_mut();
        b.scroll_top = top;
        b.scroll_bottom = bottom;
        let attr = blank_attr(rng, &[]);
        st.svc.scroll(&attr, rng.chance(20));
    }
}

fn op_scroll_lines(st: &mut State, rng: &mut Rng) {
    let n = rng.between(0, 60) as i64 - 30;
    st.svc.scroll_lines(n, rng.chance(30));
}

fn op_cursor(st: &mut State, rng: &mut Rng) {
    let (cols, rows) = (st.svc.cols, st.svc.rows);
    let b = st.svc.buffer_mut();
    b.x = rng.between(0, cols);
    b.y = rng.usize_below(rows);
    if rng.chance(20) {
        b.saved_y = rng.usize_below(rows + 5);
        b.saved_x = rng.between(0, cols);
    }
}

fn op_splice(st: &mut State, rng: &mut Rng) {
    let alt = st.pick_alt(rng);
    let attr = blank_attr(rng, &[]);
    let chaos = st.chaos;
    let b = st.buffer(alt);
    let len = b.lines.len();
    let start = if chaos && rng.chance(5) {
        len + rng.between(1, 5)
    } else {
        rng.between(0, len)
    };
    let delete = rng.between(0, 5);
    let insert = if !chaos || rng.chance(50) {
        delete
    } else {
        rng.between(0, 5)
    };
    let items: Vec<BufferLine> = (0..insert)
        .map(|_| b.get_blank_line(&attr, rng.chance(30)))
        .collect();
    b.lines.splice(start, delete, items);
}

fn op_trim_start(st: &mut State, rng: &mut Rng) {
    if !st.chaos {
        return;
    }
    let alt = st.pick_alt(rng);
    let b = st.buffer(alt);
    let count = rng.between(0, b.lines.len() + 3);
    b.lines.trim_start(count);
}

/// A random line (`None` for an empty list or an empty slot) and a copy of another one.
fn pick_line(st: &mut State, rng: &mut Rng) -> (bool, usize) {
    let alt = st.pick_alt(rng);
    let len = st.buffer(alt).lines.len();
    (alt, rng.usize_below(len.max(1)))
}

fn op_cell_edit(st: &mut State, rng: &mut Rng) {
    let (alt, index) = pick_line(st, rng);
    let cols = st.svc.cols;
    let links = st.links.clone();
    let attr = random_attr(rng, &links);
    let fill = {
        let b = st.buffer(alt);
        b.get_null_cell(Some(&attr))
    };
    let Some(line) = st.buffer(alt).lines.get_mut(index) else {
        return;
    };
    let pos = if rng.chance(5) {
        cols + rng.between(0, 3)
    } else {
        rng.usize_below(cols)
    };
    match rng.below(12) {
        0 => {
            // A cell, as `print` writes it.
            let (cp, width) = pick_char(rng);
            line.set_cell_from_codepoint(pos, cp, width, &attr);
        }
        1 => {
            // A wide character and its zero-width follower.
            let (cp, _) = pick_char(rng);
            line.set_cell_from_codepoint(pos, cp, 2, &attr);
            line.set_cell_from_codepoint(pos + 1, 0, 0, &attr);
        }
        2 => line.add_codepoint_to_cell(pos, 0x301, 0),
        3 => {
            // A combined cell through `set_cell`.
            let mut cell = CellData::new();
            cell.content = Content::IS_COMBINED_MASK | (1 << Content::WIDTH_SHIFT);
            cell.combined_data = "e\u{301}".to_string();
            cell.fg = attr.fg;
            cell.bg = attr.bg;
            cell.extended = attr.extended;
            line.set_cell(pos, &cell);
        }
        4 => {
            // A curly underline with a url id.
            let mut ext = ExtendedAttrs::default();
            ext.set_underline_style(UnderlineStyle::CURLY);
            ext.set_url_id(rng.u32() % 20);
            let mut cell = fill.clone();
            cell.bg |= BgFlags::HAS_EXTENDED;
            cell.extended = ext;
            line.set_cell(pos, &cell);
        }
        5 => line.insert_cells(pos, rng.between(0, cols + 3), &fill),
        6 => line.delete_cells(pos, rng.between(0, cols + 3), &fill),
        7 => {
            let end = rng.between(0, cols + 3);
            line.replace_cells(pos, end, &fill, rng.chance(30));
        }
        8 => line.is_wrapped = !line.is_wrapped,
        9 => line.fill(&fill, rng.chance(30)),
        10 => {
            // Same width: the host only resizes lines through the buffer.
            line.resize(cols, &fill);
            line.cleanup_memory();
        }
        _ => {
            let width = line.get_width(pos);
            let _ = (
                line.get_trimmed_length(),
                line.get_no_bg_trimmed_length(),
                width,
            );
        }
    }
}

fn op_copy_cells(st: &mut State, rng: &mut Rng) {
    let cols = st.svc.cols;
    let (src_alt, src_index) = pick_line(st, rng);
    let (alt, index) = pick_line(st, rng);
    let src = st.buffer(src_alt).lines.get(src_index).cloned();
    let (src_col, dest_col, len) = if rng.chance(85) {
        // In range, as reflow calls it.
        let len = rng.between(0, cols);
        (
            rng.between(0, cols - len.min(cols)),
            rng.between(0, cols - len.min(cols)),
            len,
        )
    } else {
        (
            rng.between(0, cols + 3),
            rng.between(0, cols + 3),
            rng.between(0, cols + 3),
        )
    };
    let reverse = rng.chance(50);
    let Some(dest) = st.buffer(alt).lines.get_mut(index) else {
        return;
    };
    if rng.chance(25) {
        dest.copy_cells_within(src_col, dest_col, len, reverse);
    } else if let Some(src) = src {
        dest.copy_cells_from(&src, src_col, dest_col, len, reverse);
    }
}

fn op_marker(st: &mut State, rng: &mut Rng, op: usize) {
    let alt = st.pick_alt(rng);
    match rng.below(10) {
        0..=4 => {
            let len = st.buffer(alt).lines.len();
            let line = rng.between(0, len + 2) as i64 - 1;
            st.buffer(alt).add_marker(line);
            st.created += 1;
        }
        5..=6 => {
            let ids: Vec<MarkerId> = st.buffer(alt).markers().iter().map(|m| m.id).collect();
            if !ids.is_empty() {
                let id = ids[rng.usize_below(ids.len())];
                st.buffer(alt).lines.observer_mut().dispose(id);
            }
        }
        7..=8 => {
            let len = st.buffer(alt).lines.len();
            let line = rng.between(0, len + 2) as i64 - 1;
            st.buffer(alt).clear_markers(line);
        }
        _ => st.buffer(alt).clear_all_markers(),
    }
    if rng.chance(70) {
        forward(st, op);
    }
}

fn op_osc(st: &mut State, rng: &mut Rng, op: usize) {
    // The service is told of every disposal before it is used again.
    forward(st, op);
    if rng.chance(75) || st.links.is_empty() {
        let data = OscLinkData {
            id: match rng.below(3) {
                0 => None,
                1 => Some("a".to_string()),
                _ => Some("b".to_string()),
            },
            uri: format!("u{}", rng.below(3)),
        };
        let link = st.osc.register_link(&mut st.svc, data);
        assert_ne!(link, 0, "op {op}: link id 0");
        st.created += 1;
        if !st.links.contains(&link) {
            st.links.push(link);
            if st.links.len() > 40 {
                st.links.remove(0);
            }
        }
        // Store the url id in a cell, as the OSC 8 handler's attribute does.
        let mut attr = AttributeData::default();
        let mut ext = ExtendedAttrs::default();
        ext.set_underline_style(UnderlineStyle::CURLY);
        ext.set_url_id(link);
        attr.extended = ext;
        attr.bg |= BgFlags::HAS_EXTENDED;
        let b = st.svc.buffer_mut();
        let (at, x) = (b.ybase + b.y, b.x);
        if let Some(line) = b.lines.get_mut(at) {
            line.set_cell_from_codepoint(x, 0x6C, 1, &attr);
        }
    } else {
        let link = st.links[rng.usize_below(st.links.len())];
        let len = st.svc.buffer().lines.len();
        let y = rng.between(0, len + 1);
        st.osc.add_line_to_link(&mut st.svc, link, y);
        st.created += 1;
    }
    forward(st, op);
}

fn op_activate(st: &mut State, rng: &mut Rng, op: usize) {
    if rng.chance(50) {
        let attr = blank_attr(rng, &[]);
        st.svc.buffers.activate_alt_buffer(Some(&attr));
    } else {
        let opts = st.opts();
        st.svc.buffers.activate_normal_buffer(&opts);
    }
    forward(st, op);
}

fn op_wrapped_range(st: &mut State, rng: &mut Rng, op: usize) {
    let alt = st.pick_alt(rng);
    let b = st.buffer(alt);
    let y = rng.between(0, b.lines.len() + 2);
    let (first, last) = b.get_wrapped_range_for_line(y);
    assert!(first <= y && y <= last, "op {op}: wrapped range");
    for i in first + 1..=last {
        assert!(
            b.lines.get(i).is_some_and(|l| l.is_wrapped),
            "op {op}: wrapped range holds an unwrapped line"
        );
    }
}

fn op_translate(st: &mut State, rng: &mut Rng, op: usize) {
    let cols = st.svc.cols;
    let alt = st.pick_alt(rng);
    let b = st.buffer(alt);
    let index = rng.between(0, b.lines.len() + 2);
    let start = rng.between(0, cols + 3);
    let end = rng.chance(60).then(|| rng.between(0, cols + 3));
    let trim = rng.chance(50);
    let text = b.translate_buffer_line_to_string(index, trim, start, end);
    if let Some(line) = b.lines.get(index) {
        let mut out = Vec::new();
        let text2 = line.translate_to_string(trim, Some(start), end, Some(&mut out));
        assert_eq!(text, text2, "op {op}: translate");
        assert_eq!(
            out.len(),
            text2.encode_utf16().count() + 1,
            "op {op}: out_columns length"
        );
    } else {
        assert!(text.is_empty(), "op {op}: translate of an absent line");
    }
}

fn op_cleanup(st: &mut State, rng: &mut Rng, op: usize) {
    let alt = st.pick_alt(rng);
    let b = st.buffer(alt);
    let allowed = b.lines.len() + 10;
    let mut calls = 0;
    while b.batched_memory_cleanup() {
        calls += 1;
        assert!(calls <= allowed, "op {op}: memory cleanup does not finish");
    }
}

fn op_tabs(st: &mut State, rng: &mut Rng, op: usize) {
    let cols = st.svc.cols;
    let alt = st.pick_alt(rng);
    let x = rng.between(0, cols + 3);
    let b = st.buffer(alt);
    assert!(b.next_stop(Some(x)) < cols, "op {op}: next_stop");
    assert!(b.prev_stop(Some(x)) < cols, "op {op}: prev_stop");
}

fn digest(st: &mut State) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (st.svc.cols, st.svc.rows).hash(&mut h);
    for alt in [false, true] {
        let b = st.buffer(alt);
        (
            b.ybase,
            b.ydisp,
            b.x,
            b.y,
            b.lines.len(),
            b.scroll_top,
            b.scroll_bottom,
        )
            .hash(&mut h);
        for i in 0..b.lines.len() {
            match b.lines.get(i) {
                Some(line) => (
                    line.is_wrapped,
                    line.len(),
                    line.translate_to_string(false, None, None, None),
                    line.combined_len(),
                    line.extended_attrs_len(),
                )
                    .hash(&mut h),
                None => 0u8.hash(&mut h),
            }
        }
        for m in b.markers().iter() {
            m.line.hash(&mut h);
        }
    }
    (st.osc.len(), st.osc.marker_count(), st.created).hash(&mut h);
    h.finish()
}

fn run_seed(seed: u64) -> Report {
    let mut rng = Rng(seed ^ 0xB0FF_E125);
    let cols = rng.between(2, 120);
    let rows = rng.between(1, 50);
    let mut initial = vec![
        OptionValue::Cols(cols),
        OptionValue::Rows(rows),
        OptionValue::Scrollback(rng.between(0, 300)),
        OptionValue::ReflowCursorLine(rng.chance(30)),
    ];
    if rng.chance(10) {
        initial.push(OptionValue::WindowsMode(true));
    }
    if rng.chance(8) {
        initial.push(OptionValue::TabStopWidth(rng.between(1, 16)));
    }
    let options = OptionsService::new(initial);
    let svc = BufferService::new(options.options());
    let mut st = State {
        options,
        svc,
        osc: OscLinkService::new(),
        created: 0,
        disposed_seen: HashSet::new(),
        links: Vec::new(),
        max_cols: cols,
        chaos: rng.chance(25),
        op_counts: [0; OPS.len()],
        shrinks: 0,
        grows: 0,
        max_ybase: 0,
        wrapped_peak: 0,
        disposed_total: 0,
        soft_checked: 0,
        width_checked: 0,
        links_freed: 0,
    };

    for op in 0..OPS_PER_SEED {
        let which = pick_op(&mut rng);
        st.op_counts[which] += 1;
        match OPS[which].0 {
            Op::Print => {
                let n = rng.between(1, st.svc.cols * 3);
                let links = st.links.clone();
                let attr = if rng.chance(70) {
                    AttributeData::default()
                } else {
                    random_attr(&mut rng, &links)
                };
                for _ in 0..n {
                    if rng.chance(4) {
                        linefeed(&mut st);
                    } else {
                        print_one(&mut st, &mut rng, &attr);
                    }
                }
            }
            Op::Linefeed => linefeed(&mut st),
            Op::Resize => op_resize(&mut st, &mut rng, op),
            Op::OptionChange => op_option_change(&mut st, &mut rng, op),
            Op::Scroll => op_scroll(&mut st, &mut rng),
            Op::ScrollLines => op_scroll_lines(&mut st, &mut rng),
            Op::Cursor => op_cursor(&mut st, &mut rng),
            Op::Splice => op_splice(&mut st, &mut rng),
            Op::TrimStart => op_trim_start(&mut st, &mut rng),
            Op::CellEdit => op_cell_edit(&mut st, &mut rng),
            Op::CopyCells => op_copy_cells(&mut st, &mut rng),
            Op::Marker => op_marker(&mut st, &mut rng, op),
            Op::Osc => op_osc(&mut st, &mut rng, op),
            Op::Activate => op_activate(&mut st, &mut rng, op),
            Op::WrappedRange => op_wrapped_range(&mut st, &mut rng, op),
            Op::Translate => op_translate(&mut st, &mut rng, op),
            Op::Cleanup => op_cleanup(&mut st, &mut rng, op),
            Op::Tabs => op_tabs(&mut st, &mut rng, op),
        }
        let ybase = st.svc.buffers.normal().ybase;
        st.max_ybase = st.max_ybase.max(ybase);
        check_hard(&mut st, op);
    }
    forward(&mut st, OPS_PER_SEED);
    Report {
        digest: digest(&mut st),
        op_counts: st.op_counts,
        shrinks: st.shrinks,
        grows: st.grows,
        max_ybase: st.max_ybase,
        wrapped_peak: st.wrapped_peak,
        disposed_total: st.disposed_total,
        soft_checked: st.soft_checked,
        width_checked: st.width_checked,
        links_freed: st.links_freed,
    }
}

// ---- harness ----

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<non-string panic>".to_string()
    }
}

#[test]
fn random_buffer_operations_never_panic_and_keep_the_invariants() {
    let seeds: Vec<u64> = match env_u64("LUNE_VT_BUFFER_FUZZ_SEED") {
        Some(one) => vec![one],
        None => (0..env_u64("LUNE_VT_BUFFER_FUZZ_SEEDS").unwrap_or(DEFAULT_SEEDS)).collect(),
    };
    let mut failures: Vec<String> = Vec::new();
    for &seed in &seeds {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| run_seed(seed))) {
            failures.push(format!("seed {seed}: {}", panic_text(payload.as_ref())));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} seeds failed (replay with LUNE_VT_BUFFER_FUZZ_SEED=<seed>):\n{}",
        failures.len(),
        seeds.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The generator itself: deterministic per seed, and it reaches the states the fuzz relies on
/// (an instrument check: a generator that never wrapped a line or never scrolled would pass
/// everything).
#[test]
fn generator_is_deterministic_and_varied() {
    let a = run_seed(7);
    let b = run_seed(7);
    let c = run_seed(8);
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.op_counts, b.op_counts);
    assert_ne!(a.digest, c.digest);

    let mut counts = [0u32; OPS.len()];
    let (mut shrinks, mut grows, mut disposed) = (0, 0, 0);
    let (mut scrolled, mut wrapped) = (0, 0);
    let (mut soft, mut width, mut freed) = (0, 0, 0);
    for seed in 0..60 {
        let r = run_seed(seed);
        for (total, n) in counts.iter_mut().zip(r.op_counts) {
            *total += n;
        }
        shrinks += r.shrinks;
        grows += r.grows;
        disposed += r.disposed_total;
        scrolled += usize::from(r.max_ybase > 0);
        wrapped += usize::from(r.wrapped_peak > 0);
        soft += r.soft_checked;
        width += r.width_checked;
        freed += r.links_freed;
    }
    assert!(soft > 600, "the soft invariants ran {soft} times");
    assert!(width > 5000, "the line widths were checked {width} times");
    assert!(freed > 50, "OscLinkService freed links {freed} times");
    assert!(
        counts.iter().all(|&n| n >= 20),
        "an operation is rare: {counts:?}"
    );
    assert!(
        shrinks > 50 && grows > 50,
        "{shrinks} shrinks, {grows} grows"
    );
    assert!(disposed > 50, "{disposed} markers disposed");
    assert!(
        scrolled > 30,
        "only {scrolled} of 60 seeds scrolled into scrollback"
    );
    assert!(
        wrapped > 30,
        "only {wrapped} of 60 seeds held wrapped lines at a resize"
    );
}

/// Fills lines `0..lengths.len()` with `lengths[i]` times `a` (not wrapped), puts the cursor at
/// the end of the last one and resizes, like writing `a` x n + CRLF per line to a terminal.
/// Returns `(ybase, len, y, x, ydisp)`.
fn fill_and_resize(
    (cols, rows, scrollback): (usize, usize, usize),
    lengths: &[usize],
    (new_cols, new_rows): (usize, usize),
) -> (usize, usize, usize, usize, usize) {
    let options = OptionsService::new([
        OptionValue::Cols(cols),
        OptionValue::Rows(rows),
        OptionValue::Scrollback(scrollback),
    ]);
    let mut svc = BufferService::new(options.options());
    let attr = AttributeData::default();
    let b = svc.buffer_mut();
    for (i, &n) in lengths.iter().enumerate() {
        let line = b.lines.get_mut(i).unwrap();
        for c in 0..n {
            line.set_cell_from_codepoint(c, u32::from('a'), 1, &attr);
        }
    }
    b.y = lengths.len() - 1;
    b.x = *lengths.last().unwrap();
    svc.resize(new_cols, new_rows, options.options());
    let b = svc.buffer();
    (b.ybase, b.lines.len(), b.y, b.x, b.ydisp)
}

/// The soft invariant `ybase + rows <= len` does not hold upstream without scrollback: the
/// reflow bumps `ybase` for every line it adds while the list is capped at `rows`. The expected
/// values come from real @xterm/headless 6.0.0 (`tools/xterm-oracle`, `scrollback: 0`; the
/// terminal written with `a` x n + CRLF per line, cursor at the end of the last line, then
/// `resize`), so the port keeps the quirk and the fuzz exempts it.
#[test]
fn reflow_without_scrollback_matches_xterm_js() {
    // 97x9 -> 23x15, cursor on the last row: baseY 1, length 15, y 14, x 10, viewportY 1.
    let mut lengths = vec![97; 8];
    lengths.push(10);
    assert_eq!(
        fill_and_resize((97, 9, 0), &lengths, (23, 15)),
        (1, 15, 14, 10, 1)
    );
    // 120x41 -> 109x36, cursor on row 33: baseY 1, length 36, y 35, x 6, viewportY 1.
    let mut lengths = vec![120; 33];
    lengths.push(6);
    assert_eq!(
        fill_and_resize((120, 41, 0), &lengths, (109, 36)),
        (1, 36, 35, 6, 1)
    );
    // The same with 10 lines of scrollback: baseY 10, length 46, y 35, x 6, viewportY 10.
    assert_eq!(
        fill_and_resize((120, 41, 10), &lengths, (109, 36)),
        (10, 46, 35, 6, 10)
    );
}
