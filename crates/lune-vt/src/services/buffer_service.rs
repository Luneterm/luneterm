//! Port of xterm.js src/common/services/BufferService.ts @ f447274. Upstream has no test file;
//! `buffer_service_tests.rs` is written from the TypeScript behaviour.
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1, D3, D8):
//!  - `onResize` and `onScroll` become return values: [`BufferService::resize`] returns the
//!    [`BufferResizeEvent`], [`BufferService::scroll`] the `ydisp` upstream emits, and
//!    [`BufferService::scroll_lines`] `Some(ydisp)` exactly when upstream fires. Upstream also
//!    fires `onScroll(activeBuffer.ydisp)` whenever the buffer set activates a buffer: the host
//!    forwards [`BufferActivate::active_ydisp`](crate::buffer::buffer_set::BufferActivate) from
//!    `buffers.activate_*`, and [`BufferService::reset`] returns that value.
//!  - Options are read live upstream; the methods that need them take `opts: &TerminalOptions`.
//!  - `resize` does not clamp, as upstream: `CoreTerminal.resize` clamps to
//!    [`MINIMUM_COLS`]/[`MINIMUM_ROWS`] before calling it (`CoreTerminal.ts:175-176`), and the
//!    caller must do the same (D8). Only [`BufferService::new`] clamps.
//!  - `scroll`: upstream's `recycle()` cannot fail when the list is full and its oldest slot is
//!    set. If the slot is empty (a list built by hand) the line is pushed instead, which has the
//!    same effect on the list. `shift_elements` fails only for an inconsistent scroll region, where
//!    upstream throws; the error is ignored here and the vacated row is still replaced. `adapted:`.

use crate::buffer::attribute_data::AttributeData;
use crate::buffer::buffer::Buffer;
use crate::buffer::buffer_line::BufferLine;
use crate::buffer::buffer_set::BufferSet;
use crate::services::options_service::TerminalOptions;

/// Less than 2 can mess with wide chars.
pub const MINIMUM_COLS: usize = 2;
pub const MINIMUM_ROWS: usize = 1;

/// Upstream `IBufferResizeEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferResizeEvent {
    pub cols: usize,
    pub rows: usize,
    pub cols_changed: bool,
    pub rows_changed: bool,
}

/// The terminal size, the two buffers and the scroll position.
#[derive(Debug)]
pub struct BufferService {
    pub cols: usize,
    pub rows: usize,
    pub buffers: BufferSet,
    /// Whether the user is scrolling (locks the scroll position).
    pub is_user_scrolling: bool,
    /// A line to clone/copy from for new blank lines.
    cached_blank_line: Option<BufferLine>,
}

impl BufferService {
    /// A service sized from the options, clamped to [`MINIMUM_COLS`] x [`MINIMUM_ROWS`].
    pub fn new(opts: &TerminalOptions) -> Self {
        let cols = opts.cols.max(MINIMUM_COLS);
        let rows = opts.rows.max(MINIMUM_ROWS);
        BufferService {
            cols,
            rows,
            buffers: BufferSet::new(opts, cols, rows),
            is_user_scrolling: false,
            cached_blank_line: None,
        }
    }

    /// The active buffer.
    pub fn buffer(&self) -> &Buffer {
        self.buffers.active()
    }

    /// The active buffer, mutably.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffers.active_mut()
    }

    /// Resizes the terminal and both buffers. The caller clamps `cols >= MINIMUM_COLS` and
    /// `rows >= MINIMUM_ROWS` (module notes).
    pub fn resize(
        &mut self,
        cols: usize,
        rows: usize,
        opts: &TerminalOptions,
    ) -> BufferResizeEvent {
        let cols_changed = self.cols != cols;
        let rows_changed = self.rows != rows;
        self.cols = cols;
        self.rows = rows;
        self.buffers.resize(cols, rows, opts);
        BufferResizeEvent {
            cols,
            rows,
            cols_changed,
            rows_changed,
        }
    }

    /// Replaces the buffers with fresh ones and stops user scrolling. Returns the `onScroll`
    /// payload upstream fires through the buffer activation (the new active buffer's `ydisp`).
    pub fn reset(&mut self, opts: &TerminalOptions) -> usize {
        let event = self.buffers.reset(opts, self.cols, self.rows);
        self.is_user_scrolling = false;
        event.active_ydisp
    }

    /// Scrolls the terminal down 1 row, creating a blank line. `erase_attr` is the attribute data
    /// for the blank line; `is_wrapped` whether the new line is wrapped from the previous line.
    /// Returns the `ydisp` upstream emits as `onScroll`.
    pub fn scroll(&mut self, erase_attr: &AttributeData, is_wrapped: bool) -> usize {
        let cols = self.cols;
        let is_user_scrolling = self.is_user_scrolling;
        let buffer = self.buffers.active_mut();

        let reusable = self.cached_blank_line.take().filter(|line| {
            line.len() == cols && line.get_fg(0) == erase_attr.fg && line.get_bg(0) == erase_attr.bg
        });
        let mut new_line = match reusable {
            Some(line) => line,
            None => buffer.get_blank_line(erase_attr, is_wrapped),
        };
        new_line.is_wrapped = is_wrapped;

        let top_row = buffer.ybase + buffer.scroll_top;
        let bottom_row = buffer.ybase + buffer.scroll_bottom;

        if buffer.scroll_top == 0 {
            // Determine whether the buffer is going to be trimmed after insertion.
            let will_buffer_be_trimmed = buffer.lines.is_full();

            // Insert the line using the fastest method
            if bottom_row + 1 == buffer.lines.len() {
                if will_buffer_be_trimmed {
                    match buffer.lines.recycle() {
                        Ok(line) => line.copy_from(&new_line),
                        Err(_) => buffer.lines.push(new_line.clone()),
                    }
                } else {
                    buffer.lines.push(new_line.clone());
                }
            } else {
                buffer
                    .lines
                    .splice(bottom_row + 1, 0, vec![new_line.clone()]);
            }

            // Only adjust ybase and ydisp when the buffer is not trimmed
            if !will_buffer_be_trimmed {
                buffer.ybase += 1;
                // Only scroll the ydisp with ybase if the user has not scrolled up
                if !is_user_scrolling {
                    buffer.ydisp += 1;
                }
            } else if is_user_scrolling {
                // When the buffer is full and the user has scrolled up, keep the text stable
                // unless ydisp is right at the top
                buffer.ydisp = buffer.ydisp.saturating_sub(1);
            }
        } else {
            // scrollTop is non-zero which means no line will be going to the scrollback, instead
            // we can just shift them in-place.
            let scroll_region_height = bottom_row.saturating_sub(top_row) + 1; // zero-based
            let _ = buffer.lines.shift_elements(
                to_isize(top_row + 1),
                to_isize(scroll_region_height - 1),
                -1,
            );
            buffer.lines.set(bottom_row, Some(new_line.clone()));
        }

        // Move the viewport to the bottom of the buffer unless the user is scrolling.
        if !is_user_scrolling {
            buffer.ydisp = buffer.ybase;
        }
        let ydisp = buffer.ydisp;
        self.cached_blank_line = Some(new_line);
        ydisp
    }

    /// Scrolls the display by `disp` lines (negative scrolls up). Returns `Some(ydisp)` when
    /// upstream fires `onScroll`: the position changed and `suppress_scroll_event` (set when the
    /// viewport triggered the scroll itself) is off.
    pub fn scroll_lines(&mut self, disp: i64, suppress_scroll_event: bool) -> Option<usize> {
        let buffer = self.buffers.active_mut();
        let ydisp = i64::try_from(buffer.ydisp).unwrap_or(i64::MAX);
        let ybase = i64::try_from(buffer.ybase).unwrap_or(i64::MAX);
        if disp < 0 {
            if buffer.ydisp == 0 {
                return None;
            }
            self.is_user_scrolling = true;
        } else if disp.saturating_add(ydisp) >= ybase {
            self.is_user_scrolling = false;
        }

        let old_ydisp = buffer.ydisp;
        let new_ydisp = ydisp.saturating_add(disp).min(ybase).max(0);
        buffer.ydisp = usize::try_from(new_ydisp).unwrap_or(0);

        // No change occurred, don't trigger scroll/refresh
        if old_ydisp == buffer.ydisp {
            return None;
        }
        (!suppress_scroll_event).then_some(buffer.ydisp)
    }
}

fn to_isize(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

#[cfg(test)]
#[path = "buffer_service_tests.rs"]
mod tests;
