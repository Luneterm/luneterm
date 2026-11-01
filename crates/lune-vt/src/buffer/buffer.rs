//! Port of xterm.js src/common/buffer/Buffer.ts @ f447274 (tests: Buffer.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1, D3, D5, D9):
//!  - Upstream reads `IOptionsService.rawOptions` live and `IBufferService.cols/rows`. Here the
//!    methods that read options take `opts: &TerminalOptions` (a live read at the call), and the
//!    buffer keeps its own `cols`/`rows`, set from the constructor arguments and updated at the
//!    end of [`Buffer::resize`]. `get_blank_line` outside `resize` uses `self.cols`, which is what
//!    upstream's `_bufferService.cols` holds there (`BufferService.resize` sets it before it
//!    resizes the buffers).
//!  - `lines` is a `CircularList<BufferLine, MarkerSet>`: `Buffer.markers` and the three listeners
//!    of `addMarker` live in the [`MarkerSet`] observer, so markers move synchronously with the
//!    list. `Marker.onDispose` is the disposal log ([`Buffer::take_disposed_markers`]). Marker
//!    lines and `add_marker`/`clear_markers` arguments are `i64` (a trimmed marker goes below 0).
//!  - `Buffer.clear` replaces `lines` with a new list. Upstream's markers stay in `this.markers`
//!    but their listeners stay on the dropped list, so they never move again; here the set moves to
//!    the new list after [`MarkerSet::detach_all`], and markers added later follow the new list.
//!  - `tabs` is a `BTreeSet<usize>` of the columns that hold a stop (upstream: a sparse object of
//!    `true`). Its loops run in `i64` where JS goes negative.
//!  - The idle task queue is replaced (D5): [`Buffer::resize`] sets a pending flag
//!    ([`Buffer::memory_cleanup_pending`]) where upstream enqueues `_batchedMemoryCleanup`, and
//!    the host calls [`Buffer::batched_memory_cleanup`] while it is idle until it returns `false`.
//!  - `getNullCell`/`getWhitespaceCell` return an owned [`CellData`] (upstream hands out its one
//!    mutable cell); the result is the same for every caller that reads it before the next call.
//!  - `cols >= 2` and `rows >= 1` are the caller's preconditions (D8: `CoreTerminal` clamps, as
//!    does `BufferService::new`). Upstream goes to `-1` for `scrollBottom`/`x` at 0 columns or
//!    rows; here those subtractions saturate at 0 so a violation cannot panic.
//!  - `tab_stop_width == 0` (rejected by `OptionsService`) would loop forever upstream; the
//!    step is clamped to 1 here. `adapted:` no valid input is affected.
//!  - `get_wrapped_range_for_line` past the end, or over an empty slot, treats the missing line
//!    as not wrapped (upstream dereferences `undefined` and throws). `adapted:`.
//!  - Reflow (`_reflowLarger`, `_reflowSmaller`) takes each wrapped group out of the list, rewrites
//!    it as owned lines and puts it back (see `buffer_reflow.rs`); the list events upstream fires
//!    directly are the `notify_*` calls.
//!  - `_reflowSmaller`'s `lines.pop()` only shortens upstream's length and leaves the popped line
//!    in its ring slot, where the same loop can read it again (`lines.get(y)` past the length).
//!    Here the loop keeps its own length and leaves the slots alone; the final rearrangement
//!    drops everything past that length, as upstream's does.
//!  - In that rearrangement upstream writes new lines that fall below index 0 (trimmed) with
//!    `lines.set(-k)`, which on a ring whose start index is above 0 overwrites the slot of the
//!    last line. The list here has no ring (plan D2), so those writes go nowhere. `adapted:`.
//!  - Reads upstream would make of a missing line in reflow (then a throw) end the copy loop or
//!    the group (`buffer_reflow.rs`). A wrapped first line with no content gives
//!    `linesToAdd = -1` and can move `savedY` to -1 upstream; here it stops at 0. `adapted:`.

use std::collections::BTreeSet;

use crate::buffer::attribute_data::{AttributeData, ExtendedAttrs};
use crate::buffer::buffer_line::{BufferLine, DEFAULT_ATTR_DATA};
use crate::buffer::buffer_reflow::{
    copy_cells_in_group, put_group, reflow_larger_apply_new_layout,
    reflow_larger_create_new_layout, reflow_larger_get_lines_to_remove,
    reflow_smaller_get_new_line_lengths, take_group, wrapped_line_trimmed_length,
};
use crate::buffer::cell_data::CellData;
use crate::buffer::constants::{
    NULL_CELL_CHAR, NULL_CELL_CODE, NULL_CELL_WIDTH, WHITESPACE_CELL_CHAR, WHITESPACE_CELL_CODE,
    WHITESPACE_CELL_WIDTH,
};
use crate::buffer::marker::{Marker, MarkerId, MarkerSet};
use crate::circular_list::CircularList;
use crate::data::charsets::{Charset, DEFAULT_CHARSET};
use crate::services::options_service::{PtyBackendKind, TerminalOptions};

/// `2^32 - 1`, the largest buffer length (`Buffer.ts:19`).
pub const MAX_BUFFER_SIZE: usize = u32::MAX as usize;

/// First `Windows` build whose ConPTY reflows its own buffer (`Buffer.ts:299`).
const CONPTY_REFLOW_BUILD: u32 = 21376;

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A terminal buffer: the text, the cursor and the scroll position.
#[derive(Debug)]
pub struct Buffer {
    pub lines: CircularList<BufferLine, MarkerSet>,
    pub ydisp: usize,
    pub ybase: usize,
    pub y: usize,
    pub x: usize,
    pub scroll_bottom: usize,
    pub scroll_top: usize,
    /// The columns that hold a tab stop.
    pub tabs: BTreeSet<usize>,
    pub saved_y: usize,
    pub saved_x: usize,
    pub saved_cur_attr_data: AttributeData,
    pub saved_charset: Option<&'static Charset>,
    null_cell: CellData,
    whitespace_cell: CellData,
    has_scrollback: bool,
    cols: usize,
    rows: usize,
    memory_cleanup_pending: bool,
    memory_cleanup_position: usize,
}

impl Buffer {
    /// A buffer of `cols x rows` over the options `opts` (upstream reads the sizes from the buffer
    /// service). The lines list starts empty: call [`Buffer::fill_viewport_rows`].
    pub fn new(has_scrollback: bool, cols: usize, rows: usize, opts: &TerminalOptions) -> Self {
        let mut buffer = Buffer {
            lines: CircularList::with_observer(
                Self::correct_buffer_length(has_scrollback, rows, opts),
                MarkerSet::new(),
            ),
            ydisp: 0,
            ybase: 0,
            y: 0,
            x: 0,
            scroll_bottom: rows.saturating_sub(1),
            scroll_top: 0,
            tabs: BTreeSet::new(),
            saved_y: 0,
            saved_x: 0,
            saved_cur_attr_data: DEFAULT_ATTR_DATA,
            saved_charset: DEFAULT_CHARSET,
            null_cell: CellData::from_char_data(&(
                0,
                NULL_CELL_CHAR.to_string(),
                NULL_CELL_WIDTH,
                NULL_CELL_CODE,
            )),
            whitespace_cell: CellData::from_char_data(&(
                0,
                WHITESPACE_CELL_CHAR.to_string(),
                WHITESPACE_CELL_WIDTH,
                WHITESPACE_CELL_CODE,
            )),
            has_scrollback,
            cols,
            rows,
            memory_cleanup_pending: false,
            memory_cleanup_position: 0,
        };
        buffer.setup_tab_stops(None, opts);
        buffer
    }

    /// Columns (upstream `_cols`).
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows (upstream `_rows`).
    pub fn rows(&self) -> usize {
        self.rows
    }

    fn cell_with(template: &CellData, attr: Option<&AttributeData>) -> CellData {
        let mut cell = template.clone();
        match attr {
            Some(attr) => {
                cell.fg = attr.fg;
                cell.bg = attr.bg;
                cell.extended = attr.extended;
            }
            None => {
                cell.fg = 0;
                cell.bg = 0;
                cell.extended = ExtendedAttrs::default();
            }
        }
        cell
    }

    /// The empty cell carrying `attr`'s colours and extended attributes (default: none).
    pub fn get_null_cell(&self, attr: Option<&AttributeData>) -> CellData {
        Self::cell_with(&self.null_cell, attr)
    }

    /// The space cell carrying `attr`'s colours and extended attributes (default: none).
    pub fn get_whitespace_cell(&self, attr: Option<&AttributeData>) -> CellData {
        Self::cell_with(&self.whitespace_cell, attr)
    }

    /// A blank line of `self.cols()` null cells carrying `attr`.
    pub fn get_blank_line(&self, attr: &AttributeData, is_wrapped: bool) -> BufferLine {
        BufferLine::new(self.cols, Some(&self.get_null_cell(Some(attr))), is_wrapped)
    }

    /// This buffer is allowed scrollback and its list is longer than the viewport.
    pub fn has_scrollback(&self) -> bool {
        self.has_scrollback && self.lines.max_length() > self.rows
    }

    /// The cursor row is inside the viewport.
    pub fn is_cursor_in_viewport(&self) -> bool {
        let absolute_y = to_i64(self.ybase).saturating_add(to_i64(self.y));
        let relative_y = absolute_y.saturating_sub(to_i64(self.ydisp));
        relative_y >= 0 && relative_y < to_i64(self.rows)
    }

    /// The list length for `rows` rows: the viewport plus the scrollback, if this buffer has any,
    /// capped at [`MAX_BUFFER_SIZE`].
    fn correct_buffer_length(has_scrollback: bool, rows: usize, opts: &TerminalOptions) -> usize {
        if !has_scrollback {
            return rows;
        }
        rows.saturating_add(opts.scrollback).min(MAX_BUFFER_SIZE)
    }

    /// Fills the viewport with blank lines if the list is empty.
    pub fn fill_viewport_rows(&mut self, fill_attr: Option<&AttributeData>) {
        if self.lines.is_empty() {
            let fill_attr = fill_attr.unwrap_or(&DEFAULT_ATTR_DATA);
            for _ in 0..self.rows {
                let line = self.get_blank_line(fill_attr, false);
                self.lines.push(line);
            }
        }
    }

    /// Clears the buffer to its initial state, discarding all previous data. Existing markers
    /// stay in the set but no longer follow the (new) list.
    pub fn clear(&mut self, opts: &TerminalOptions) {
        self.ydisp = 0;
        self.ybase = 0;
        self.y = 0;
        self.x = 0;
        let mut markers = std::mem::take(self.lines.observer_mut());
        markers.detach_all();
        self.lines = CircularList::with_observer(
            Self::correct_buffer_length(self.has_scrollback, self.rows, opts),
            markers,
        );
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.setup_tab_stops(None, opts);
    }

    /// Resizes the buffer, adjusting its data accordingly.
    ///
    /// `new_cols >= 2` and `new_rows >= 1` are the caller's preconditions (plan D8): upstream
    /// locks up in the column reflow at one column, and `CoreTerminal` clamps.
    pub fn resize(&mut self, new_cols: usize, new_rows: usize, opts: &TerminalOptions) {
        // store a null cell with default attrs
        let null_cell = self.get_null_cell(Some(&DEFAULT_ATTR_DATA));

        // count bufferlines with overly big memory to be cleaned afterwards
        let mut dirty_memory_lines = 0usize;

        // Increase max length if needed before adjustments to allow space to fill as required.
        let new_max_length = Self::correct_buffer_length(self.has_scrollback, new_rows, opts);
        if new_max_length > self.lines.max_length() {
            self.lines.set_max_length(new_max_length);
        }

        // The following adjustments should only happen if the buffer has been initialized/filled.
        if !self.lines.is_empty() {
            // Deal with columns increasing (reducing needs to happen after reflow)
            if self.cols < new_cols {
                for i in 0..self.lines.len() {
                    if let Some(line) = self.lines.get_mut(i) {
                        dirty_memory_lines += usize::from(line.resize(new_cols, &null_cell));
                    }
                }
            }

            // Resize rows in both directions as needed
            let mut add_to_y = 0usize;
            if self.rows < new_rows {
                for _ in self.rows..new_rows {
                    if self.lines.len() < new_rows.saturating_add(self.ybase) {
                        if opts.windows_mode
                            || opts.windows_pty.backend.is_some()
                            || opts.windows_pty.build_number.is_some()
                        {
                            // Just add the new missing rows on Windows as conpty reprints the
                            // screen with its view of the world. Once a line enters scrollback
                            // for conpty it remains there.
                            self.lines
                                .push(BufferLine::new(new_cols, Some(&null_cell), false));
                        } else if self.ybase > 0
                            && self.lines.len() <= self.ybase + self.y + add_to_y + 1
                        {
                            // There is room above the buffer and there are no empty elements
                            // below the line, scroll up
                            self.ybase -= 1;
                            add_to_y += 1;
                            if self.ydisp > 0 {
                                // Viewport is at the top of the buffer, must increase downwards
                                self.ydisp -= 1;
                            }
                        } else {
                            // Add a blank line if there is no buffer left at the top to scroll
                            // to, or if there are blank lines after the cursor
                            self.lines
                                .push(BufferLine::new(new_cols, Some(&null_cell), false));
                        }
                    }
                }
            } else {
                // (self.rows >= new_rows)
                for _ in new_rows..self.rows {
                    if self.lines.len() > new_rows.saturating_add(self.ybase) {
                        if self.lines.len() > self.ybase + self.y + 1 {
                            // The line is a blank line below the cursor, remove it
                            self.lines.pop();
                        } else {
                            // The line is the cursor, scroll down
                            self.ybase += 1;
                            self.ydisp += 1;
                        }
                    }
                }
            }

            // Reduce max length if needed after adjustments, this is done after as it would
            // otherwise cut data from the bottom of the buffer.
            if new_max_length < self.lines.max_length() {
                // Trim from the top of the buffer and adjust ybase and ydisp.
                let amount_to_trim = self.lines.len().saturating_sub(new_max_length);
                if amount_to_trim > 0 {
                    self.lines.trim_start(amount_to_trim);
                    self.ybase = self.ybase.saturating_sub(amount_to_trim);
                    self.ydisp = self.ydisp.saturating_sub(amount_to_trim);
                    self.saved_y = self.saved_y.saturating_sub(amount_to_trim);
                }
                self.lines.set_max_length(new_max_length);
            }

            // Make sure that the cursor stays on screen
            self.x = self.x.min(new_cols.saturating_sub(1));
            self.y = self.y.min(new_rows.saturating_sub(1));
            if add_to_y > 0 {
                self.y += add_to_y;
            }
            self.saved_x = self.saved_x.min(new_cols.saturating_sub(1));

            self.scroll_top = 0;
        }

        self.scroll_bottom = new_rows.saturating_sub(1);

        if self.is_reflow_enabled(opts) {
            self.reflow(new_cols, new_rows, opts);

            // Trim the end of the line off if cols shrunk
            if self.cols > new_cols {
                for i in 0..self.lines.len() {
                    if let Some(line) = self.lines.get_mut(i) {
                        dirty_memory_lines += usize::from(line.resize(new_cols, &null_cell));
                    }
                }
            }
        }

        self.cols = new_cols;
        self.rows = new_rows;

        // The queued idle task is replaced (D5): upstream clears the queue here, and schedules
        // the cleanup only if more than 10% of the lines are affected.
        self.memory_cleanup_pending = false;
        if dirty_memory_lines as f64 > 0.1 * self.lines.len() as f64 {
            self.memory_cleanup_position = 0;
            self.memory_cleanup_pending = true;
        }
    }

    /// A memory cleanup is waiting for [`Buffer::batched_memory_cleanup`] (upstream: the idle task
    /// is queued).
    pub fn memory_cleanup_pending(&self) -> bool {
        self.memory_cleanup_pending
    }

    /// One batch of the idle memory cleanup (`_batchedMemoryCleanup`): cleans up to about 100
    /// lines. Returns `true` while another call is needed.
    pub fn batched_memory_cleanup(&mut self) -> bool {
        let mut normal_run = true;
        if self.memory_cleanup_position >= self.lines.len() {
            // cleanup made it once through all lines, thus rescan in loop below to also catch
            // shifted lines, which should finish rather quick if there are no more cleanups
            // pending
            self.memory_cleanup_position = 0;
            normal_run = false;
        }
        let mut counted = 0u32;
        while self.memory_cleanup_position < self.lines.len() {
            let position = self.memory_cleanup_position;
            self.memory_cleanup_position += 1;
            if let Some(line) = self.lines.get_mut(position) {
                counted = counted.saturating_add(line.cleanup_memory());
            }
            // cleanup max 100 lines per batch
            if counted > 100 {
                self.memory_cleanup_pending = true;
                return true;
            }
        }
        // normal runs always need another rescan afterwards; if we made it here with
        // normal_run == false, we are in a final run and can end the cleanup task for sure
        self.memory_cleanup_pending = normal_run;
        normal_run
    }

    fn is_reflow_enabled(&self, opts: &TerminalOptions) -> bool {
        let windows_pty = &opts.windows_pty;
        if let Some(build_number) = windows_pty.build_number.filter(|n| *n != 0) {
            return self.has_scrollback
                && windows_pty.backend == Some(PtyBackendKind::Conpty)
                && build_number >= CONPTY_REFLOW_BUILD;
        }
        self.has_scrollback && !opts.windows_mode
    }

    fn reflow(&mut self, new_cols: usize, new_rows: usize, opts: &TerminalOptions) {
        if self.cols == new_cols {
            return;
        }

        // Iterate through rows, ignore the last one as it cannot be wrapped
        if new_cols > self.cols {
            self.reflow_larger(new_cols, new_rows, opts);
        } else {
            self.reflow_smaller(new_cols, new_rows, opts);
        }
    }

    fn reflow_larger(&mut self, new_cols: usize, new_rows: usize, opts: &TerminalOptions) {
        let reflow_cursor_line = opts.reflow_cursor_line;
        let null_cell = self.get_null_cell(Some(&DEFAULT_ATTR_DATA));
        let to_remove = reflow_larger_get_lines_to_remove(
            &mut self.lines,
            self.cols,
            new_cols,
            self.ybase + self.y,
            &null_cell,
            reflow_cursor_line,
        );
        if !to_remove.is_empty() {
            let new_layout_result = reflow_larger_create_new_layout(&mut self.lines, &to_remove);
            reflow_larger_apply_new_layout(&mut self.lines, &new_layout_result.layout);
            self.reflow_larger_adjust_viewport(new_cols, new_rows, new_layout_result.count_removed);
        }
    }

    fn reflow_larger_adjust_viewport(
        &mut self,
        new_cols: usize,
        new_rows: usize,
        count_removed: usize,
    ) {
        let null_cell = self.get_null_cell(Some(&DEFAULT_ATTR_DATA));
        // Adjust viewport based on number of items removed
        for _ in 0..count_removed {
            if self.ybase == 0 {
                if self.y > 0 {
                    self.y -= 1;
                }
                if self.lines.len() < new_rows {
                    // Add an extra row at the bottom of the viewport
                    self.lines
                        .push(BufferLine::new(new_cols, Some(&null_cell), false));
                }
            } else {
                if self.ydisp == self.ybase {
                    self.ydisp = self.ydisp.saturating_sub(1);
                }
                self.ybase -= 1;
            }
        }
        self.saved_y = self.saved_y.saturating_sub(count_removed);
    }

    fn reflow_smaller(&mut self, new_cols: usize, new_rows: usize, opts: &TerminalOptions) {
        let reflow_cursor_line = opts.reflow_cursor_line;
        let null_cell = self.get_null_cell(Some(&DEFAULT_ATTR_DATA));
        let max_length = to_i64(self.lines.max_length());
        let new_rows_i = to_i64(new_rows);
        // Gather all BufferLines that need to be inserted into the Buffer here so that they can
        // be batched up and only committed once
        let mut to_insert: Vec<(i64, Vec<BufferLine>)> = Vec::new();
        let mut count_to_insert = 0usize;
        // Upstream's `lines.length`: `pop` below only shortens it and leaves the popped lines in
        // their slots, where later reads in this loop still find them (module doc).
        let mut lines_len = self.lines.len();
        // Go backwards as many lines may be trimmed and this will avoid considering them
        let mut y = to_i64(self.lines.len()) - 1;
        while y >= 0 {
            let Ok(end) = usize::try_from(y) else { break };
            // Check whether this line is a problem
            let skip = match self.lines.get(end) {
                None => true,
                Some(line) => !line.is_wrapped && line.get_trimmed_length() <= new_cols,
            };
            if skip {
                y -= 1;
                continue;
            }

            // Gather wrapped lines and adjust y to be the starting line
            let mut start = end;
            while start > 0
                && self.lines.get(start).is_some_and(|line| line.is_wrapped)
                && self.lines.get(start - 1).is_some()
            {
                start -= 1;
            }
            let group_len = end - start + 1;

            if !reflow_cursor_line {
                // If these lines contain the cursor don't touch them, the program will handle
                // fixing up wrapped lines with the cursor
                let absolute_y = self.ybase + self.y;
                if absolute_y >= start && absolute_y < start + group_len {
                    y = to_i64(start) - 1;
                    continue;
                }
            }

            let (last_line_length, dest_line_lengths) = {
                let refs: Vec<&BufferLine> =
                    (start..=end).filter_map(|i| self.lines.get(i)).collect();
                let last = refs.last().map_or(0, |line| line.get_trimmed_length());
                (
                    last,
                    reflow_smaller_get_new_line_lengths(&refs, self.cols, new_cols),
                )
            };
            let lines_to_add = to_i64(dest_line_lengths.len()) - to_i64(group_len);
            let trimmed_lines = if self.ybase == 0 && to_i64(self.y) != to_i64(lines_len) - 1 {
                // If the top section of the buffer is not yet filled
                (to_i64(self.y) - max_length + lines_to_add).max(0)
            } else {
                (to_i64(lines_len) - max_length + lines_to_add).max(0)
            };

            // Add the new lines (`getBlankLine` reads the buffer service's columns, already the
            // new ones during a resize)
            let blank = self.get_null_cell(Some(&DEFAULT_ATTR_DATA));
            let new_line_count = usize::try_from(lines_to_add).unwrap_or(0);
            let mut wrapped_lines = take_group(&mut self.lines, start, group_len);
            wrapped_lines.extend(
                std::iter::repeat_with(|| BufferLine::new(new_cols, Some(&blank), true))
                    .take(new_line_count),
            );

            // Copy buffer data to new locations, this needs to happen backwards to do in-place.
            // Index -1 is upstream's `undefined` line, which ends the copy.
            let length_at = |i: i64| -> usize {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| dest_line_lengths.get(i).copied())
                    .unwrap_or(0)
            };
            let mut dest_line_index = to_i64(dest_line_lengths.len()) - 1;
            let mut dest_col = length_at(dest_line_index);
            if dest_col == 0 {
                dest_line_index -= 1;
                dest_col = length_at(dest_line_index);
            }
            let mut src_line_index = to_i64(wrapped_lines.len()) - lines_to_add - 1;
            let mut src_col = last_line_length;
            while src_line_index >= 0 {
                let cells_to_copy = src_col.min(dest_col);
                let (Ok(dest), Ok(src)) = (
                    usize::try_from(dest_line_index),
                    usize::try_from(src_line_index),
                ) else {
                    // Sanity check that the line exists, this has been known to fail for an
                    // unknown reason which would stop the reflow from happening if an exception
                    // would throw.
                    break;
                };
                if dest >= wrapped_lines.len() || src >= wrapped_lines.len() {
                    // the same sanity check; a missing source line throws upstream
                    break;
                }
                copy_cells_in_group(
                    &mut wrapped_lines,
                    dest,
                    src,
                    src_col - cells_to_copy,
                    dest_col - cells_to_copy,
                    cells_to_copy,
                    true,
                );
                dest_col -= cells_to_copy;
                if dest_col == 0 {
                    dest_line_index -= 1;
                    dest_col = length_at(dest_line_index);
                }
                src_col -= cells_to_copy;
                if src_col == 0 {
                    src_line_index -= 1;
                    let wrapped_lines_index = usize::try_from(src_line_index).unwrap_or(0);
                    src_col =
                        wrapped_line_trimmed_length(&wrapped_lines, wrapped_lines_index, self.cols);
                }
            }

            // Null out the end of the line ends if a wide character wrapped to the following line
            for (line, &length) in wrapped_lines.iter_mut().zip(&dest_line_lengths) {
                if length < new_cols {
                    line.set_cell(length, &null_cell);
                }
            }

            let new_lines = wrapped_lines.split_off(group_len.min(wrapped_lines.len()));
            put_group(&mut self.lines, start, wrapped_lines);
            if !new_lines.is_empty() {
                // count_to_insert here gets the actual index, taking into account other inserted
                // items. using this we can iterate through the list forwards
                let insert_start = to_i64(start + group_len + count_to_insert);
                count_to_insert += new_lines.len();
                to_insert.push((insert_start, new_lines));
            }

            // Adjust viewport as needed
            let mut viewport_adjustments = lines_to_add - trimmed_lines;
            while viewport_adjustments > 0 {
                viewport_adjustments -= 1;
                if self.ybase == 0 {
                    if to_i64(self.y) < new_rows_i - 1 {
                        self.y += 1;
                        // `this.lines.pop()`
                        lines_len = lines_len.saturating_sub(1);
                    } else {
                        self.ybase += 1;
                        self.ydisp += 1;
                    }
                } else {
                    // Ensure ybase does not exceed its maximum value
                    let limit =
                        max_length.min(to_i64(lines_len) + to_i64(count_to_insert)) - new_rows_i;
                    if to_i64(self.ybase) < limit {
                        if self.ybase == self.ydisp {
                            self.ydisp += 1;
                        }
                        self.ybase += 1;
                    }
                }
            }
            let saved_y =
                (to_i64(self.saved_y) + lines_to_add).min(to_i64(self.ybase) + new_rows_i - 1);
            self.saved_y = usize::try_from(saved_y).unwrap_or(0);

            // the loop's `y--` from the group's first line
            y = to_i64(start) - 1;
        }

        // Rearrange lines in the buffer if there are any insertions, this is done at the end
        // rather than earlier so that it's a single O(n) pass through the buffer, instead of
        // O(n^2) from many costly calls to CircularList.splice.
        if !to_insert.is_empty() {
            self.reflow_smaller_insert(to_insert, count_to_insert, lines_len);
        } else {
            self.lines.set_len(lines_len);
        }
    }

    /// The end of `_reflowSmaller`: rebuilds the list with the new lines in place, then fires the
    /// insert events backwards and one trim event.
    fn reflow_smaller_insert(
        &mut self,
        to_insert: Vec<(i64, Vec<BufferLine>)>,
        count_to_insert: usize,
        original_lines_length: usize,
    ) {
        // Record buffer insert events and then play them back backwards so that the indexes are
        // correct
        let mut insert_events: Vec<(i64, usize)> = Vec::new();

        // Record original lines so they don't get overridden when we rearrange the list (the
        // popped lines past `original_lines_length` are dropped)
        let mut original_lines = self.lines.take_all();
        original_lines.truncate(original_lines_length);
        let max_length = self.lines.max_length();

        let mut original_line_index = to_i64(original_lines_length) - 1;
        let new_len = max_length.min(original_lines_length.saturating_add(count_to_insert));
        let mut out: Vec<Option<BufferLine>> = Vec::new();
        out.resize_with(new_len, || None);
        let set = |out: &mut Vec<Option<BufferLine>>, i: i64, line: Option<BufferLine>| {
            // a negative index writes nowhere visible upstream
            if let Some(slot) = usize::try_from(i).ok().and_then(|i| out.get_mut(i)) {
                *slot = line;
            }
        };
        let mut pending = to_insert.into_iter().peekable();
        let mut count_inserted_so_far = 0usize;
        let mut i = to_i64(new_len) - 1;
        while i >= 0 {
            let insert_here = pending.peek().is_some_and(|(start, _)| {
                *start > original_line_index + to_i64(count_inserted_so_far)
            });
            if insert_here {
                if let Some((_, new_lines)) = pending.next() {
                    let amount = new_lines.len();
                    // Insert extra lines here, adjusting i as needed
                    for line in new_lines.into_iter().rev() {
                        set(&mut out, i, Some(line));
                        i -= 1;
                    }
                    i += 1;

                    // Create insert events for later
                    insert_events.push((original_line_index + 1, amount));

                    count_inserted_so_far += amount;
                }
            } else {
                let line = usize::try_from(original_line_index)
                    .ok()
                    .and_then(|k| original_lines.get_mut(k))
                    .and_then(Option::take);
                original_line_index -= 1;
                set(&mut out, i, line);
            }
            i -= 1;
        }
        self.lines.replace_all(out);

        // Update markers
        let mut insert_count_emitted = 0usize;
        for &(index, amount) in insert_events.iter().rev() {
            let index = usize::try_from(index).unwrap_or(0) + insert_count_emitted;
            self.lines.notify_insert(index, amount);
            insert_count_emitted += amount;
        }
        let amount_to_trim = (original_lines_length + count_to_insert).saturating_sub(max_length);
        if amount_to_trim > 0 {
            self.lines.notify_trim(amount_to_trim);
        }
    }

    /// Translates a buffer line to a string, with optional start and end columns. Wide
    /// characters count as two columns in the resulting string. Useful for getting the actual
    /// text underneath the raw selection position.
    pub fn translate_buffer_line_to_string(
        &self,
        line_index: usize,
        trim_right: bool,
        start_col: usize,
        end_col: Option<usize>,
    ) -> String {
        match self.lines.get(line_index) {
            Some(line) => line.translate_to_string(trim_right, Some(start_col), end_col, None),
            None => String::new(),
        }
    }

    /// The first and last rows of the wrapped line that contains row `y`.
    pub fn get_wrapped_range_for_line(&self, y: usize) -> (usize, usize) {
        let is_wrapped = |i: usize| self.lines.get(i).is_some_and(|line| line.is_wrapped);
        let mut first = y;
        let mut last = y;
        // Scan upwards for wrapped lines
        while first > 0 && is_wrapped(first) {
            first -= 1;
        }
        // Scan downwards for wrapped lines
        while last.saturating_add(1) < self.lines.len() && is_wrapped(last + 1) {
            last += 1;
        }
        (first, last)
    }

    /// Sets up the tab stops. `None` rebuilds all of them; `Some(i)` fills from the stop at or
    /// before `i`.
    pub fn setup_tab_stops(&mut self, i: Option<usize>, opts: &TerminalOptions) {
        let start = match i {
            Some(i) => {
                if self.tabs.contains(&i) {
                    i
                } else {
                    self.prev_stop(Some(i))
                }
            }
            None => {
                self.tabs.clear();
                0
            }
        };
        let step = opts.tab_stop_width.max(1);
        let mut column = start;
        while column < self.cols {
            self.tabs.insert(column);
            match column.checked_add(step) {
                Some(next) => column = next,
                None => break,
            }
        }
    }

    /// The tab stop before `x` (default: the cursor column), clamped to the viewport.
    pub fn prev_stop(&self, x: Option<usize>) -> usize {
        let mut x = to_i64(x.unwrap_or(self.x));
        loop {
            x -= 1;
            if usize::try_from(x).is_ok_and(|column| self.tabs.contains(&column)) || x <= 0 {
                break;
            }
        }
        self.clamp_stop(x)
    }

    /// The tab stop after `x` (default: the cursor column), clamped to the viewport.
    pub fn next_stop(&self, x: Option<usize>) -> usize {
        let mut x = to_i64(x.unwrap_or(self.x));
        let cols = to_i64(self.cols);
        loop {
            x = x.saturating_add(1);
            if usize::try_from(x).is_ok_and(|column| self.tabs.contains(&column)) || x >= cols {
                break;
            }
        }
        self.clamp_stop(x)
    }

    fn clamp_stop(&self, x: i64) -> usize {
        if x >= to_i64(self.cols) {
            self.cols.saturating_sub(1)
        } else {
            usize::try_from(x).unwrap_or(0)
        }
    }

    /// Disposes the markers on line `y`.
    pub fn clear_markers(&mut self, y: i64) {
        self.lines.observer_mut().clear_line(y);
    }

    /// Disposes all markers.
    pub fn clear_all_markers(&mut self) {
        self.lines.observer_mut().clear_all();
    }

    /// Adds a marker on line `y` that follows its line as the list trims and changes.
    pub fn add_marker(&mut self, y: i64) -> MarkerId {
        self.lines.observer_mut().add(y)
    }

    /// Disposes one marker (upstream `IMarker.dispose`); logged like every disposal. Unknown or
    /// already disposed ids are ignored.
    pub fn dispose_marker(&mut self, id: MarkerId) {
        self.lines.observer_mut().dispose(id);
    }

    /// The live marker, `None` once disposed.
    pub fn marker(&self, id: MarkerId) -> Option<&Marker> {
        self.lines.observer().get(id)
    }

    /// The live markers (upstream `markers`).
    pub fn markers(&self) -> &MarkerSet {
        self.lines.observer()
    }

    /// The markers disposed since the last call, each once, in disposal order (upstream
    /// `Marker.onDispose`).
    pub fn take_disposed_markers(&mut self) -> Vec<MarkerId> {
        self.lines.observer_mut().take_disposed()
    }
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
