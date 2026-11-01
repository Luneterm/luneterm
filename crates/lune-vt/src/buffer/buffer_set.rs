//! Port of xterm.js src/common/buffer/BufferSet.ts @ f447274 (tests: BufferSet.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1, D3):
//!  - `onBufferActivate` becomes a return value: [`BufferSet::reset`],
//!    [`BufferSet::activate_normal_buffer`] and [`BufferSet::activate_alt_buffer`] return a
//!    [`BufferActivate`] exactly when upstream fires the event (`reset` always; the `activate_*`
//!    methods not when that buffer is already active). Upstream's `BufferService` listens and
//!    forwards the active buffer's `ydisp` as `onScroll`: the host does the same with
//!    [`BufferActivate::active_ydisp`].
//!  - `BufferSet` reads `bufferService.cols/rows` and `rawOptions` live. Here the constructor and
//!    `reset` take the sizes, and the methods that read options take `opts: &TerminalOptions`.
//!    `activate_normal_buffer` takes `opts` too, because clearing the alt buffer rebuilds its tab
//!    stops.
//!  - The two option listeners of the constructor are the host's job (M6): a change of
//!    `scrollback` must call [`BufferSet::resize`] with the current `cols`/`rows`, and a change of
//!    `tab_stop_width` must call [`BufferSet::setup_tab_stops`] with `None`
//!    (`OptionsService::set` reports the changed key).

use crate::buffer::attribute_data::AttributeData;
use crate::buffer::buffer::Buffer;
use crate::services::options_service::TerminalOptions;

/// The payload of upstream's `onBufferActivate` that the host needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferActivate {
    /// `activeBuffer.ydisp`, which `BufferService` forwards as `onScroll`.
    pub active_ydisp: usize,
    /// The alternate buffer is now the active one (`inactiveBuffer` is the other).
    pub alt_active: bool,
}

/// The two buffers used by xterm terminals (normal and alt) and utilities for working with them.
#[derive(Debug)]
pub struct BufferSet {
    normal: Buffer,
    alt: Buffer,
    alt_active: bool,
}

impl BufferSet {
    /// A set for a `cols x rows` terminal, with the normal buffer active and filled.
    pub fn new(opts: &TerminalOptions, cols: usize, rows: usize) -> Self {
        let (normal, alt) = Self::build(opts, cols, rows);
        let mut set = BufferSet {
            normal,
            alt,
            alt_active: false,
        };
        set.setup_tab_stops(None, opts);
        set
    }

    fn build(opts: &TerminalOptions, cols: usize, rows: usize) -> (Buffer, Buffer) {
        let mut normal = Buffer::new(true, cols, rows, opts);
        normal.fill_viewport_rows(None);

        // The alt buffer should never have scrollback.
        // See http://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-The-Alternate-Screen-Buffer
        let alt = Buffer::new(false, cols, rows, opts);
        (normal, alt)
    }

    /// Replaces both buffers with fresh ones and activates the normal buffer. Always fires
    /// `onBufferActivate` upstream, so always returns the event.
    pub fn reset(&mut self, opts: &TerminalOptions, cols: usize, rows: usize) -> BufferActivate {
        let (normal, alt) = Self::build(opts, cols, rows);
        self.normal = normal;
        self.alt = alt;
        self.alt_active = false;
        let event = self.activation();
        self.setup_tab_stops(None, opts);
        event
    }

    fn activation(&self) -> BufferActivate {
        BufferActivate {
            active_ydisp: self.active().ydisp,
            alt_active: self.alt_active,
        }
    }

    /// The alt buffer.
    pub fn alt(&self) -> &Buffer {
        &self.alt
    }

    /// The alt buffer, mutably.
    pub fn alt_mut(&mut self) -> &mut Buffer {
        &mut self.alt
    }

    /// The normal buffer.
    pub fn normal(&self) -> &Buffer {
        &self.normal
    }

    /// The normal buffer, mutably.
    pub fn normal_mut(&mut self) -> &mut Buffer {
        &mut self.normal
    }

    /// The currently active buffer.
    pub fn active(&self) -> &Buffer {
        if self.alt_active {
            &self.alt
        } else {
            &self.normal
        }
    }

    /// The currently active buffer, mutably.
    pub fn active_mut(&mut self) -> &mut Buffer {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.normal
        }
    }

    /// The alt buffer is the active one.
    pub fn is_alt_active(&self) -> bool {
        self.alt_active
    }

    /// Makes the normal buffer the active one. `None` (no event) if it already is. The alt
    /// buffer, its markers and its contents are cleared, to free memory: it is always new when
    /// activated.
    pub fn activate_normal_buffer(&mut self, opts: &TerminalOptions) -> Option<BufferActivate> {
        if !self.alt_active {
            return None;
        }
        self.normal.x = self.alt.x;
        self.normal.y = self.alt.y;
        self.alt.clear_all_markers();
        self.alt.clear(opts);
        self.alt_active = false;
        Some(self.activation())
    }

    /// Makes the alt buffer the active one. `None` (no event) if it already is. Since the alt
    /// buffer is always cleared when the normal buffer is activated, it is filled here.
    pub fn activate_alt_buffer(&mut self, fill: Option<&AttributeData>) -> Option<BufferActivate> {
        if self.alt_active {
            return None;
        }
        self.alt.fill_viewport_rows(fill);
        self.alt.x = self.normal.x;
        self.alt.y = self.normal.y;
        self.alt_active = true;
        Some(self.activation())
    }

    /// Resizes both the normal and the alt buffer, adjusting their data accordingly.
    ///
    /// `new_cols >= 2` and `new_rows >= 1`: see [`Buffer::resize`].
    pub fn resize(&mut self, new_cols: usize, new_rows: usize, opts: &TerminalOptions) {
        self.normal.resize(new_cols, new_rows, opts);
        self.alt.resize(new_cols, new_rows, opts);
        self.setup_tab_stops(Some(new_cols), opts);
    }

    /// Sets up the tab stops of both buffers, from column `i` or (`None`) entirely.
    pub fn setup_tab_stops(&mut self, i: Option<usize>, opts: &TerminalOptions) {
        self.normal.setup_tab_stops(i, opts);
        self.alt.setup_tab_stops(i, opts);
    }
}

#[cfg(test)]
#[path = "buffer_set_tests.rs"]
mod tests;
