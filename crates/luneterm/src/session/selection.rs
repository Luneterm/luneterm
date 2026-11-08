//! Selection on the engine thread (plan M10 step 3, D2): the model, the word, line and wrapped-line
//! rules, the text, and the commands the UI drives them with.
//!
//! Ported from xterm.js 6.0.0 (commit f447274f430fd22513f6adbf9862d19524471c04):
//!  - `src/browser/selection/SelectionModel.ts` (and its `SelectionModel.test.ts`), as
//!    [`SelectionModel`];
//!  - `src/browser/services/SelectionService.ts`: `_getWordAt`, `_selectWordAt`, `_selectToWordAt`,
//!    `_selectLineAt`, `_convertViewportColToCharacterIndex`, `selectionText`, `handleMouseDown`
//!    and its single, double, triple and incremental clicks, `_handleMouseMove`, `_dragScroll`,
//!    `_isCharWordSeparator`. Its tests that need no DOM are ported on a real `Terminal` in the
//!    test module.
//!
//! Why here and not in `lune-vt`: the model reads the buffer, which only this thread may touch
//! (M9 I1: the UI never holds a `Terminal`), and it is browser-layer code, outside the `src/common`
//! scope the porting rules keep for the engine. The UI sends [`SelectCmd`]s with viewport cells; an
//! [`Update`](super::Update) carries the result ([`SelectionView`], the copied text).
//!
//! Differences from upstream, each marked `// LUNETERM:`:
//!  - the anchor of the selection is followed by a marker (the engine's marker API, which follows
//!    trim, insert and delete); when the anchor line is trimmed away the selection clears (upstream
//!    clamps the start to the first line and keeps the rest);
//!  - upstream clears on user input and on a vertical resize (`SelectionService.ts:139-143`,
//!    `:158-162`); both are kept and are driven from the engine, a horizontal-only resize keeps the
//!    selection as upstream does;
//!  - the copied text is bounded ([`MAX_COPY_BYTES`]) and its line break is the platform's;
//!  - no link-under-cursor rule yet (step 4), no select-all, no Alt-click cursor move.

use std::time::Duration;

use lune_ui::clipboard::MAX_PASTE_BYTES;
use lune_vt::Terminal;
use lune_vt::buffer::buffer::Buffer;
use lune_vt::buffer::buffer_line::BufferLine;
use lune_vt::buffer::marker::MarkerId;

/// The most bytes a copy puts on the clipboard: the toolkit's own cap, so the adapter never has to
/// refuse it.
pub const MAX_COPY_BYTES: usize = MAX_PASTE_BYTES;

/// How often a drag outside the viewport scrolls it (`DRAG_SCROLL_INTERVAL`).
pub const DRAG_SCROLL_INTERVAL: Duration = Duration::from_millis(50);
/// The pointer this many logical px beyond the viewport scrolls at full speed
/// (`DRAG_SCROLL_MAX_THRESHOLD`).
pub const DRAG_SCROLL_MAX_THRESHOLD: f32 = 50.0;
/// The most lines one drag scroll step moves (`DRAG_SCROLL_MAX_SPEED`).
pub const DRAG_SCROLL_MAX_SPEED: f32 = 15.0;

/// The line break of copied text.
#[cfg(windows)]
const EOL: &str = "\r\n";
#[cfg(not(windows))]
const EOL: &str = "\n";

/// `[x, y]` as upstream's arrays: x a column (the end is exclusive), y a buffer line. `y` can be
/// negative for a moment while a trim is applied.
pub type Pos = [i64; 2];

// -------------------------------------------------------------------------- SelectionModel

/// Represents a selection within the buffer. It only cares about column and row coordinates, not
/// wide characters (`SelectionModel.ts`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SelectionModel {
    /// Whether select all is currently active.
    pub is_select_all_active: bool,
    /// The minimal length of the selection from the start position: a double click selects a word,
    /// which makes the selection start at its start and this its length.
    pub selection_start_length: i64,
    /// The position the selection starts at.
    pub selection_start: Option<Pos>,
    /// The position the selection ends at.
    pub selection_end: Option<Pos>,
}

impl SelectionModel {
    pub fn clear_selection(&mut self) {
        *self = SelectionModel::default();
    }

    /// The final selection start, taking select all into consideration.
    pub fn final_selection_start(&self) -> Option<Pos> {
        if self.is_select_all_active {
            return Some([0, 0]);
        }
        match (self.selection_start, self.selection_end) {
            (Some(start), Some(end)) => Some(if self.are_selection_values_reversed() {
                end
            } else {
                start
            }),
            (start, _) => start,
        }
    }

    /// The final selection end, taking select all, double click word selection and triple click
    /// line selection into consideration. `cols` is the buffer's width and `last_row` its last
    /// row for select all (`ybase + rows - 1`).
    pub fn final_selection_end(&self, cols: i64, last_row: i64) -> Option<Pos> {
        if self.is_select_all_active {
            return Some([cols, last_row]);
        }
        let start = self.selection_start?;
        // Use the selection start + length if the end doesn't exist or they're reversed
        let end = match self.selection_end {
            Some(end) if !self.are_selection_values_reversed() => end,
            _ => {
                let start_plus_length = start[0] + self.selection_start_length;
                if start_plus_length > cols {
                    // Ensure the trailing EOL isn't included when the selection ends on the right
                    // edge
                    if start_plus_length % cols == 0 {
                        return Some([cols, start[1] + start_plus_length / cols - 1]);
                    }
                    return Some([
                        start_plus_length % cols,
                        start[1] + start_plus_length / cols,
                    ]);
                }
                return Some([start_plus_length, start[1]]);
            }
        };
        // Ensure the word/line is selected after a double/triple click
        if self.selection_start_length != 0 && end[1] == start[1] {
            // Select the larger of the two when start and end are on the same line. Keep the whole
            // wrapped word/line selected if the content wraps multiple lines.
            let start_plus_length = start[0] + self.selection_start_length;
            if start_plus_length > cols {
                return Some([
                    start_plus_length % cols,
                    start[1] + start_plus_length / cols,
                ]);
            }
            return Some([start_plus_length.max(end[0]), end[1]]);
        }
        Some(end)
    }

    /// Whether the selection start and end are reversed.
    pub fn are_selection_values_reversed(&self) -> bool {
        match (self.selection_start, self.selection_end) {
            (Some(start), Some(end)) => {
                start[1] > end[1] || (start[1] == end[1] && start[0] > end[0])
            }
            _ => false,
        }
    }

    /// The buffer was trimmed by `amount` lines: moves the selection up with the text. Returns
    /// whether a refresh is necessary.
    pub fn handle_trim(&mut self, amount: i64) -> bool {
        if let Some(start) = &mut self.selection_start {
            start[1] -= amount;
        }
        if let Some(end) = &mut self.selection_end {
            end[1] -= amount;
        }
        // The selection has moved off the buffer, clear it.
        if self.selection_end.is_some_and(|end| end[1] < 0) {
            self.clear_selection();
            return true;
        }
        // If the selection start is trimmed, ensure the start line is the first
        if let Some(start) = &mut self.selection_start
            && start[1] < 0
        {
            start[1] = 0;
        }
        false
    }
}

// ------------------------------------------------------------------------------- word rules

/// A word on a line: its first column (negative when it begins on a wrapped line above) and its
/// length in columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Word {
    start: i64,
    length: i64,
}

/// What the rules read: the buffer, its width and the word separators.
struct Ctx<'a> {
    buffer: &'a Buffer,
    cols: i64,
    separators: &'a str,
}

fn line_at(buffer: &Buffer, y: i64) -> Option<&BufferLine> {
    usize::try_from(y).ok().and_then(|y| buffer.lines.get(y))
}

fn cell_width(line: &BufferLine, x: i64) -> u32 {
    match usize::try_from(x) {
        Ok(x) if x < line.len() => line.get_width(x),
        // Past the line: a blank, one column wide.
        _ => 1,
    }
}

fn cell_chars(line: &BufferLine, x: i64) -> String {
    match usize::try_from(x) {
        Ok(x) if x < line.len() => line.get_string(x),
        _ => String::new(),
    }
}

fn utf16_len(s: &str) -> i64 {
    i64::try_from(s.encode_utf16().count()).unwrap_or(i64::MAX)
}

fn cell_code_point(line: &BufferLine, x: i64) -> u32 {
    match usize::try_from(x) {
        Ok(x) if x < line.len() => line.get_code_point(x),
        _ => 0,
    }
}

/// Whether the cell at `x` is a word separator (`_isCharWordSeparator`). A zero-width cell is
/// never one (it is always to the right of a wide character); a cell with no text is one (upstream
/// `indexOf('')` is 0).
fn is_char_word_separator(ctx: &Ctx<'_>, line: &BufferLine, x: i64) -> bool {
    if cell_width(line, x) == 0 {
        return false;
    }
    ctx.separators.contains(cell_chars(line, x).as_str())
}

/// Converts a viewport column to the character index on the line's string, which leaves out the
/// cells to the right of wide characters and counts every UTF-16 unit of a multi-unit cell
/// (`_convertViewportColToCharacterIndex`; JS strings are UTF-16, so the port counts UTF-16 units
/// to keep every index and length as upstream's).
fn convert_viewport_col_to_character_index(line: &BufferLine, x: i64) -> i64 {
    let mut char_index = x;
    let mut i = 0;
    while x >= i {
        let length = utf16_len(&cell_chars(line, i));
        if cell_width(line, i) == 0 {
            // Wide characters aren't included in the line string so decrement the index so the
            // index is back on the wide character.
            char_index -= 1;
        } else if length > 1 && x != i {
            // Emojis take up multiple characters, so adjust accordingly. For these we don't want
            // to include the character at the column as we're returning the start index in the
            // string, not the end index.
            char_index += length - 1;
        }
        i += 1;
    }
    char_index
}

/// Positional information for the word at `coords` (`_getWordAt`).
fn get_word_at(
    ctx: &Ctx<'_>,
    coords: Pos,
    allow_whitespace_only: bool,
    follow_above: bool,
    follow_below: bool,
) -> Option<Word> {
    let cols = ctx.cols;
    // Ensure coords are within viewport (eg. not within scroll bar)
    if coords[0] >= cols || coords[0] < 0 {
        return None;
    }
    let buffer_line = line_at(ctx.buffer, coords[1])?;
    let line: Vec<u16> = ctx
        .buffer
        .translate_buffer_line_to_string(usize::try_from(coords[1]).ok()?, false, 0, None)
        .encode_utf16()
        .collect();
    let line_len = i64::try_from(line.len()).unwrap_or(i64::MAX);
    let char_at = |i: i64| -> u16 {
        usize::try_from(i)
            .ok()
            .and_then(|i| line.get(i).copied())
            .unwrap_or(0)
    };
    const SPACE: u16 = b' ' as u16;

    // Get actual index, taking into consideration wide characters
    let mut start_index = convert_viewport_col_to_character_index(buffer_line, coords[0]);
    let mut end_index = start_index;

    // Record offset to be used later
    let char_offset = coords[0] - start_index;
    let mut left_wide_char_count = 0;
    let mut right_wide_char_count = 0;
    let mut left_long_char_offset = 0;
    let mut right_long_char_offset = 0;

    if char_at(start_index) == SPACE {
        // Expand until non-whitespace is hit
        while start_index > 0 && char_at(start_index - 1) == SPACE {
            start_index -= 1;
        }
        while end_index < line_len && char_at(end_index + 1) == SPACE {
            end_index += 1;
        }
    } else {
        // Expand until whitespace is hit. This algorithm works by scanning left and right from the
        // starting position, keeping both the index format (line) and the column format
        // (buffer_line) in sync. When a wide character is hit, it is recorded and the column index
        // is adjusted.
        let mut start_col = coords[0];
        let mut end_col = coords[0];

        // Consider the initial position, skip it and increment the wide char variable
        if cell_width(buffer_line, start_col) == 0 {
            left_wide_char_count += 1;
            start_col -= 1;
        }
        if cell_width(buffer_line, end_col) == 2 {
            right_wide_char_count += 1;
            end_col += 1;
        }

        // Adjust the end index for characters whose length are > 1 (emojis)
        let length = utf16_len(&cell_chars(buffer_line, end_col));
        if length > 1 {
            right_long_char_offset += length - 1;
            end_index += length - 1;
        }

        // Expand the string in both directions until a space is hit
        while start_col > 0
            && start_index > 0
            && !is_char_word_separator(ctx, buffer_line, start_col - 1)
        {
            let length = utf16_len(&cell_chars(buffer_line, start_col - 1));
            if cell_width(buffer_line, start_col - 1) == 0 {
                // If the next character is a wide char, record it and skip the column
                left_wide_char_count += 1;
                start_col -= 1;
            } else if length > 1 {
                // If the next character's string is longer than 1 char (eg. emoji), adjust the
                // index
                left_long_char_offset += length - 1;
                start_index -= length - 1;
            }
            start_index -= 1;
            start_col -= 1;
        }
        let buffer_line_len = i64::try_from(buffer_line.len()).unwrap_or(i64::MAX);
        while end_col < buffer_line_len
            && end_index + 1 < line_len
            && !is_char_word_separator(ctx, buffer_line, end_col + 1)
        {
            let length = utf16_len(&cell_chars(buffer_line, end_col + 1));
            if cell_width(buffer_line, end_col + 1) == 2 {
                // If the next character is a wide char, record it and skip the column
                right_wide_char_count += 1;
                end_col += 1;
            } else if length > 1 {
                // If the next character's string is longer than 1 char (eg. emoji), adjust the
                // index
                right_long_char_offset += length - 1;
                end_index += length - 1;
            }
            end_index += 1;
            end_col += 1;
        }
    }

    // Increment the end index so it is at the start of the next character
    end_index += 1;

    // Calculate the start _column_, converting the string indexes back to column coordinates.
    let mut start = start_index // The index of the selection's start char in the line string
        + char_offset // The difference between the initial char's column and index
        - left_wide_char_count // The number of wide chars left of the initial char
        + left_long_char_offset; // The additional chars left of the initial char added by columns with strings longer than 1 (emojis)

    // Calculate the length in _columns_, converting the string indexes back to column coordinates.
    let mut length = cols.min(
        end_index // The index of the selection's end char in the line string
            - start_index // The index of the selection's start char in the line string
            + left_wide_char_count // The number of wide chars left of the initial char
            + right_wide_char_count // The number of wide chars right of the initial char (inclusive)
            - left_long_char_offset // The additional chars left of the initial char added by columns with strings longer than 1 (emojis)
            - right_long_char_offset, // The additional chars right of the initial char (inclusive) added by columns with strings longer than 1 (emojis)
    );

    if !allow_whitespace_only {
        let from = usize::try_from(start_index).unwrap_or(0).min(line.len());
        let to = usize::try_from(end_index).unwrap_or(0).min(line.len());
        let slice = line.get(from..to.max(from)).unwrap_or(&[]);
        if String::from_utf16_lossy(slice).trim().is_empty() {
            return None;
        }
    }

    // Recurse upwards if the line is wrapped and the word wraps to the above line
    if follow_above && start == 0 && buffer_line.get_code_point(0) != 32 {
        let previous = line_at(ctx.buffer, coords[1] - 1);
        if let Some(previous) = previous
            && buffer_line.is_wrapped
            && cell_code_point(previous, cols - 1) != 32
            && let Some(above) = get_word_at(ctx, [cols - 1, coords[1] - 1], false, true, false)
        {
            let offset = cols - above.start;
            start -= offset;
            length += offset;
        }
    }

    // Recurse downwards if the line is wrapped and the word wraps to the next line
    if follow_below && start + length == cols && cell_code_point(buffer_line, cols - 1) != 32 {
        let next = line_at(ctx.buffer, coords[1] + 1);
        if let Some(next) = next
            && next.is_wrapped
            && cell_code_point(next, 0) != 32
            && let Some(below) = get_word_at(ctx, [0, coords[1] + 1], false, false, true)
        {
            length += below.length;
        }
    }

    Some(Word { start, length })
}

/// Selects the word at `coords` (`_selectWordAt`).
fn select_word_at(
    model: &mut SelectionModel,
    ctx: &Ctx<'_>,
    coords: Pos,
    allow_whitespace_only: bool,
) {
    if let Some(mut word) = get_word_at(ctx, coords, allow_whitespace_only, true, true) {
        let mut y = coords[1];
        // Adjust negative start value
        while word.start < 0 {
            word.start += ctx.cols;
            y -= 1;
        }
        model.selection_start = Some([word.start, y]);
        model.selection_start_length = word.length;
    }
}

/// Sets the selection end to the word at `coords` (`_selectToWordAt`).
fn select_to_word_at(model: &mut SelectionModel, ctx: &Ctx<'_>, coords: Pos) {
    if let Some(mut word) = get_word_at(ctx, coords, true, true, true) {
        let mut end_row = coords[1];
        // Adjust negative start value
        while word.start < 0 {
            word.start += ctx.cols;
            end_row -= 1;
        }
        // Adjust wrapped length value, this only needs to happen when values are reversed as in
        // that case we're interested in the start of the word, not the end
        let reversed = model.are_selection_values_reversed();
        if !reversed {
            while word.start + word.length > ctx.cols {
                word.length -= ctx.cols;
                end_row += 1;
            }
        }
        model.selection_end = Some([
            if reversed {
                word.start
            } else {
                word.start + word.length
            },
            end_row,
        ]);
    }
}

/// Selects the (wrapped) line at `line` (`_selectLineAt`).
fn select_line_at(model: &mut SelectionModel, ctx: &Ctx<'_>, line: i64) {
    let (first, last) = ctx
        .buffer
        .get_wrapped_range_for_line(usize::try_from(line).unwrap_or(0));
    let (first, last) = (
        i64::try_from(first).unwrap_or(0),
        i64::try_from(last).unwrap_or(0),
    );
    model.selection_start = Some([0, first]);
    model.selection_end = None;
    // `getRangeLength` of the range [0, first] to [cols - 1, last].
    model.selection_start_length = ctx.cols * (last - first) + ctx.cols;
}

// ------------------------------------------------------------------------------------ text

/// Why a copy gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyError {
    /// The text is over [`MAX_COPY_BYTES`].
    TooLarge,
}

/// The text of a copy, or why none.
#[derive(Clone, PartialEq, Eq)]
pub enum CopyResult {
    Text(String),
    TooLarge,
}

impl std::fmt::Debug for CopyResult {
    /// Never the text (I16).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyResult::Text(t) => write!(f, "Text({} bytes)", t.len()),
            CopyResult::TooLarge => f.write_str("TooLarge"),
        }
    }
}

/// Collects lines under a byte cap.
struct Capped {
    lines: Vec<String>,
    bytes: usize,
    limit: usize,
}

impl Capped {
    fn push(&mut self, line: String, join_to_previous: bool) -> Result<(), CopyError> {
        // A line break is counted where one will be written: before every line but the first, and
        // not before a wrapped continuation.
        let eol = if join_to_previous || self.lines.is_empty() {
            0
        } else {
            EOL.len()
        };
        self.bytes = self.bytes.saturating_add(line.len()).saturating_add(eol);
        if self.bytes > self.limit {
            return Err(CopyError::TooLarge);
        }
        match (join_to_previous, self.lines.last_mut()) {
            (true, Some(last)) => last.push_str(&line),
            _ => self.lines.push(line),
        }
        Ok(())
    }
}

/// The text of the selection (`selectionText`), at most `limit` bytes: a longer text is refused
/// whole, never cut. Non-breaking spaces become spaces.
pub fn selection_text(
    buffer: &Buffer,
    start: Pos,
    end: Pos,
    column: bool,
    limit: usize,
) -> Result<String, CopyError> {
    let row = |y: i64| usize::try_from(y.max(0)).unwrap_or(0);
    let col = |x: i64| usize::try_from(x.max(0)).unwrap_or(0);
    let mut out = Capped {
        lines: Vec::new(),
        bytes: 0,
        limit,
    };
    if column {
        // Ignore zero width selections
        if start[0] == end[0] {
            return Ok(String::new());
        }
        // For column selection it's not enough to rely on final selection's swapping of reversed
        // values, it also needs the x coordinates to swap independently of the y coordinate
        let start_col = start[0].min(end[0]);
        let end_col = start[0].max(end[0]);
        for y in start[1]..=end[1] {
            let text = buffer.translate_buffer_line_to_string(
                row(y),
                true,
                col(start_col),
                Some(col(end_col)),
            );
            out.push(text, false)?;
        }
    } else {
        // Get first row
        let start_row_end_col = (start[1] == end[1]).then(|| col(end[0]));
        out.push(
            buffer.translate_buffer_line_to_string(
                row(start[1]),
                true,
                col(start[0]),
                start_row_end_col,
            ),
            false,
        )?;
        // Get middle rows
        for y in (start[1] + 1)..end[1] {
            let wrapped = buffer.lines.get(row(y)).is_some_and(|l| l.is_wrapped);
            let text = buffer.translate_buffer_line_to_string(row(y), true, 0, None);
            out.push(text, wrapped)?;
        }
        // Get final row
        if start[1] != end[1] {
            let wrapped = buffer.lines.get(row(end[1])).is_some_and(|l| l.is_wrapped);
            let text =
                buffer.translate_buffer_line_to_string(row(end[1]), true, 0, Some(col(end[0])));
            out.push(text, wrapped)?;
        }
    }
    // Format string by replacing non-breaking space chars with regular spaces and joining the
    // array into a multi-line string.
    let lines: Vec<String> = out
        .lines
        .into_iter()
        .map(|l| l.replace('\u{a0}', " "))
        .collect();
    Ok(lines.join(EOL))
}

// ----------------------------------------------------------------------------- commands, view

/// A selection mode: how the selection behaves on mouse move (`SelectionMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Normal,
    Word,
    Line,
    Column,
}

/// What the UI asks of the selection. Positions are viewport (row, column edge): the column is the
/// cell edge nearest the pointer, 0 to the grid's width ([`edge_of`]). `top` is the viewport's first buffer line at that time, so a viewport that moved since cannot
/// shift the click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectCmd {
    /// A press: `clicks` is 1, 2 or 3 (single, word, line); `extend` is an incremental click
    /// (Shift with the program not taking the mouse); `column` is Alt.
    Start {
        cell: (usize, usize),
        top: usize,
        clicks: u8,
        extend: bool,
        column: bool,
    },
    /// The pointer moved while the button is down; `scroll` is lines per [`DRAG_SCROLL_INTERVAL`]
    /// when it is beyond the viewport (positive: below), else 0 ([`drag_scroll_amount`]).
    Drag {
        cell: (usize, usize),
        top: usize,
        scroll: i32,
    },
    /// The button was released.
    End,
    Clear,
}

/// The selection as the UI draws it: buffer coordinates, `start` before `end`, `end`'s column
/// exclusive. `column` selects a block: the columns between the corners' x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionView {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub column: bool,
}

/// The cell edge nearest to `x` logical px from the left of a grid of `cols` columns of `cell_w`
/// px: 0 to `cols` (`getCoords`: `ceil((x + w / 2) / w) - 1`, clamped).
pub fn edge_of(x: f32, cell_w: f32, cols: usize) -> usize {
    if !(x.is_finite() && cell_w.is_finite() && cell_w > 0.0) || x <= 0.0 {
        return 0;
    }
    let edge = ((x + cell_w / 2.0) / cell_w).ceil() as usize;
    edge.saturating_sub(1).min(cols)
}

/// The scroll step of a drag whose pointer is `offset` logical px below the top of the viewport
/// (`_getMouseEventScrollAmount`): 0 inside it, else a signed line count from 1 up to the maximum
/// speed, more the farther out.
pub fn drag_scroll_amount(offset: f32, height: f32) -> i32 {
    if !(offset.is_finite() && height.is_finite()) || (0.0..=height).contains(&offset) {
        return 0;
    }
    let mut offset = offset;
    if offset > height {
        offset -= height;
    }
    let offset = offset.clamp(-DRAG_SCROLL_MAX_THRESHOLD, DRAG_SCROLL_MAX_THRESHOLD)
        / DRAG_SCROLL_MAX_THRESHOLD;
    // `offset / |offset|` is the sign; JS rounds half up.
    let sign = if offset < 0.0 { -1.0 } else { 1.0 };
    let scaled = (offset * (DRAG_SCROLL_MAX_SPEED - 1.0) + 0.5).floor();
    (sign + scaled) as i32
}

/// A selection mapped to the viewport rows starting at buffer line `top`: the corners for the
/// termview (`end` inclusive), clipped to the `rows` x `cols` viewport; `None` when none of it is
/// in view.
pub fn view_range(
    view: &SelectionView,
    top: usize,
    rows: usize,
    cols: usize,
) -> Option<lune_termview::SelectionRange> {
    use lune_termview::{SelectionKind, SelectionRange};
    if rows == 0 || cols == 0 {
        return None;
    }
    let bottom = top.saturating_add(rows); // exclusive
    let (sx, sy) = view.start;
    let (ex, ey) = view.end;
    if view.column {
        let (y0, y1) = (sy.min(ey), sy.max(ey));
        if y1 < top || y0 >= bottom {
            return None;
        }
        let (c0, c1) = (sx.min(ex), sx.max(ex));
        if c0 == c1 {
            return None;
        }
        let r0 = y0.max(top) - top;
        let r1 = y1.min(bottom - 1) - top;
        return Some(SelectionRange {
            start: (r0, c0.min(cols - 1)),
            end: (r1, (c1 - 1).min(cols - 1)),
            kind: SelectionKind::Block,
        });
    }
    // The end as an inclusive cell: column 0 of a line means the line before it, to its end.
    let (end_y, end_x) = if ex == 0 {
        (ey.checked_sub(1)?, cols - 1)
    } else {
        (ey, (ex - 1).min(cols - 1))
    };
    if (end_y, end_x) < (sy, sx) || end_y < top || sy >= bottom {
        return None;
    }
    let start = if sy < top {
        (0, 0)
    } else {
        (sy - top, sx.min(cols - 1))
    };
    let end = if end_y >= bottom {
        (rows - 1, cols - 1)
    } else {
        (end_y - top, end_x)
    };
    Some(SelectionRange {
        start,
        end,
        kind: SelectionKind::Linear,
    })
}

// --------------------------------------------------------------------------- the engine state

/// The selection of one session: the model, the mode, the drag state and the marker that follows
/// the anchor line through trims.
#[derive(Debug, Default)]
pub struct Selection {
    model: SelectionModel,
    mode: SelectionMode,
    /// The mouse button is down: moves extend the selection.
    selecting: bool,
    /// Lines per step while a drag is beyond the viewport.
    drag_amount: i32,
    /// When the next drag scroll step is due.
    next_drag: Option<Duration>,
    /// The marker on the anchor line and the line it was made for.
    anchor: Option<(MarkerId, i64)>,
    /// The selection was made on the alternate buffer.
    on_alt: bool,
}

impl Selection {
    pub fn new() -> Selection {
        Selection::default()
    }

    /// The width and the last row (`ybase + rows - 1`) of the engine's active buffer.
    fn dims(term: &Terminal) -> (i64, i64) {
        let cols = i64::try_from(term.cols()).unwrap_or(i64::MAX);
        let buffer = term.buffers().active();
        let last = i64::try_from(buffer.ybase.saturating_add(term.rows())).unwrap_or(i64::MAX) - 1;
        (cols, last)
    }

    /// Whether anything is selected: a start and an end that differ (`hasSelection`).
    pub fn has_selection(&self, term: &Terminal) -> bool {
        self.view(term).is_some()
    }

    /// The selection to draw, `None` when nothing is selected.
    pub fn view(&self, term: &Terminal) -> Option<SelectionView> {
        let (cols, last) = Self::dims(term);
        let start = self.model.final_selection_start()?;
        let end = self.model.final_selection_end(cols, last)?;
        if start == end {
            return None;
        }
        let to = |p: Pos| {
            (
                usize::try_from(p[0].max(0)).unwrap_or(0),
                usize::try_from(p[1].max(0)).unwrap_or(0),
            )
        };
        Some(SelectionView {
            start: to(start),
            end: to(end),
            column: self.mode == SelectionMode::Column,
        })
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn text(&self, term: &Terminal, limit: usize) -> Option<Result<String, CopyError>> {
        let (cols, last) = Self::dims(term);
        let start = self.model.final_selection_start()?;
        let end = self.model.final_selection_end(cols, last)?;
        Some(selection_text(
            term.buffers().active(),
            start,
            end,
            self.mode == SelectionMode::Column,
            limit,
        ))
    }

    /// Clears the selection and everything that belonged to it.
    pub fn clear(&mut self, term: &mut Terminal) {
        self.model.clear_selection();
        self.selecting = false;
        self.drag_amount = 0;
        self.next_drag = None;
        self.release_anchor(term);
    }

    fn release_anchor(&mut self, term: &mut Terminal) {
        if let Some((id, _)) = self.anchor.take() {
            term.dispose_marker(id);
        }
    }

    /// The selection's anchor line moved or is new: follow it with a marker (the normal buffer
    /// only, which is the only one with a scrollback to trim).
    fn follow_anchor(&mut self, term: &mut Terminal) {
        let Some(start) = self.model.selection_start else {
            self.release_anchor(term);
            return;
        };
        if self.anchor.is_some_and(|(_, y)| y == start[1]) {
            return;
        }
        self.release_anchor(term);
        if term.buffers().is_alt_active() {
            return;
        }
        let buffer = term.buffers().active();
        let cursor_line = i64::try_from(buffer.ybase.saturating_add(buffer.y)).unwrap_or(0);
        if let Some(id) = term.add_marker(start[1] - cursor_line) {
            self.anchor = Some((id, start[1]));
        }
    }

    /// Brings the selection up to date with what the engine did since the last call: lines
    /// trimmed (or inserted or deleted) above the anchor, the alternate buffer entered or left.
    /// What changed shows in [`Selection::view`]; the engine compares it around the call.
    pub fn sync(&mut self, term: &mut Terminal) {
        if self.model.selection_start.is_none() {
            return;
        }
        // Clears on a buffer switch (`_handleBufferActivate`).
        if term.buffers().is_alt_active() != self.on_alt {
            self.clear(term);
            return;
        }
        let Some((id, y)) = self.anchor else {
            return;
        };
        // LUNETERM: upstream hooks `lines.onTrim`; this engine has no such event, so the anchor
        // line's marker (which the buffer moves on trim) tells the amount. A marker the buffer
        // disposed was trimmed away: the selection clears (upstream would clamp its start to the
        // first line).
        let Some(marker) = term.buffers().normal().marker(id) else {
            self.anchor = None;
            self.clear(term);
            return;
        };
        let moved = y - marker.line;
        if moved == 0 {
            return;
        }
        if moved > 0 {
            // Trimmed: `handleTrim`.
            let cleared = self.model.handle_trim(moved);
            if cleared {
                self.clear(term);
                return;
            }
        } else {
            // Lines inserted above (a scroll region): the text moved down with its marker.
            for p in [
                &mut self.model.selection_start,
                &mut self.model.selection_end,
            ]
            .into_iter()
            .flatten()
            {
                p[1] -= moved;
            }
        }
        self.anchor = Some((id, marker.line));
    }

    /// A vertical resize clears the selection (`SelectionService.ts:158-162`, xterm.js#5300).
    pub fn rows_changed(&mut self, term: &mut Terminal) {
        self.clear(term);
    }

    /// User input clears the selection (`SelectionService.ts:139-143`).
    pub fn user_input(&mut self, term: &mut Terminal) {
        if self.model.selection_start.is_some() {
            self.clear(term);
        }
    }

    /// The buffer position of a viewport (row, column). The column is a cell *edge*, 0 to `cols`
    /// (`getCoords` with `isSelection`: the left half of a cell is its left edge, the right half
    /// the next cell's); a selection ends before the cell its end edge is on the left of.
    fn coords(term: &Terminal, cell: (usize, usize), top: usize) -> Pos {
        let cols = term.cols();
        let rows = term.rows().max(1);
        let x = cell.1.min(cols);
        let row = cell.0.min(rows - 1);
        [
            i64::try_from(x).unwrap_or(0),
            i64::try_from(top.saturating_add(row)).unwrap_or(i64::MAX),
        ]
    }

    /// One command from the UI. `now` is the engine's clock.
    pub fn apply(&mut self, term: &mut Terminal, cmd: SelectCmd, now: Duration) {
        match cmd {
            SelectCmd::Start {
                cell,
                top,
                clicks,
                extend,
                column,
            } => self.start(term, cell, top, clicks, extend, column),
            SelectCmd::Drag { cell, top, scroll } => self.drag(term, cell, top, scroll, now),
            SelectCmd::End => {
                self.selecting = false;
                self.drag_amount = 0;
                self.next_drag = None;
            }
            SelectCmd::Clear => self.clear(term),
        }
        self.follow_anchor(term);
    }

    /// `handleMouseDown`'s three clicks and the incremental one.
    fn start(
        &mut self,
        term: &mut Terminal,
        cell: (usize, usize),
        top: usize,
        clicks: u8,
        extend: bool,
        column: bool,
    ) {
        let coords = Self::coords(term, cell, top);
        // Reset drag scroll state
        self.drag_amount = 0;
        self.next_drag = None;
        self.selecting = true;
        if extend {
            // `_handleIncrementalClick`
            if self.model.selection_start.is_some() {
                self.model.selection_end = Some(coords);
            }
            return;
        }
        let (cols, _) = Self::dims(term);
        let separators = term.options().word_separator.clone();
        let ctx = Ctx {
            buffer: term.buffers().active(),
            cols,
            separators: &separators,
        };
        self.on_alt = term.buffers().is_alt_active();
        match clicks {
            1 => {
                // `_handleSingleClick`
                self.model.selection_start_length = 0;
                self.model.is_select_all_active = false;
                self.mode = if column {
                    SelectionMode::Column
                } else {
                    SelectionMode::Normal
                };
                // Initialize the new selection
                let mut start = coords;
                self.model.selection_end = None;
                // Ensure the line exists, and return early if the click is not in the buffer
                if let Some(line) = line_at(ctx.buffer, start[1])
                    && i64::try_from(line.len()).ok() != Some(start[0])
                {
                    // If the pointer is over the second half of a wide character, adjust the
                    // selection to cover the whole character
                    if cell_width(line, start[0]) == 0 {
                        start[0] += 1;
                    }
                }
                self.model.selection_start = Some(start);
            }
            2 => {
                // `_handleDoubleClick` (the link under the pointer is step 4's)
                self.mode = SelectionMode::Word;
                select_word_at(&mut self.model, &ctx, coords, true);
                self.model.selection_end = None;
            }
            3 => {
                // `_handleTripleClick`
                self.mode = SelectionMode::Line;
                select_line_at(&mut self.model, &ctx, coords[1]);
            }
            _ => {}
        }
    }

    /// `_handleMouseMove`.
    fn drag(
        &mut self,
        term: &mut Terminal,
        cell: (usize, usize),
        top: usize,
        scroll: i32,
        now: Duration,
    ) {
        // Do nothing if the button is not down or there is no selection start (the first click
        // may have been an incremental click)
        if !self.selecting || self.model.selection_start.is_none() {
            return;
        }
        let mut end = Self::coords(term, cell, top);
        let start = self.model.selection_start.unwrap_or(end);
        let (cols, _) = Self::dims(term);
        let separators = term.options().word_separator.clone();
        self.model.selection_end = Some(end);
        let ctx = Ctx {
            buffer: term.buffers().active(),
            cols,
            separators: &separators,
        };

        // Select the entire line if line select mode is active.
        match self.mode {
            SelectionMode::Line => {
                end[0] = if end[1] < start[1] { 0 } else { cols };
                self.model.selection_end = Some(end);
            }
            SelectionMode::Word => select_to_word_at(&mut self.model, &ctx, end),
            _ => {}
        }
        let mut end = self.model.selection_end.unwrap_or(end);

        // The amount of scrolling that will happen.
        self.drag_amount = scroll;
        self.next_drag = match (scroll != 0, self.next_drag) {
            (true, None) => Some(now + DRAG_SCROLL_INTERVAL),
            (true, keep) => keep,
            (false, _) => None,
        };

        // If the cursor was above or below the viewport, make sure it's at the start or end of the
        // viewport respectively. This should only happen when NOT in column select mode.
        if self.mode != SelectionMode::Column {
            if scroll > 0 {
                end[0] = cols;
            } else if scroll < 0 {
                end[0] = 0;
            }
        }

        // If the character is a wide character include the cell to the right in the selection.
        // Note that selections at the very end of the line will never have a character.
        if let Some(line) = line_at(ctx.buffer, end[1])
            && let Ok(x) = usize::try_from(end[0])
            && x < line.len()
            && line.has_width(x) == 0
            && end[0] < cols
        {
            end[0] += 1;
        }
        self.model.selection_end = Some(end);
    }

    /// A drag scroll step is due (the pointer is held beyond the viewport).
    pub fn drag_due(&self, now: Duration) -> bool {
        self.next_drag.is_some_and(|t| now >= t)
    }

    /// When the next drag scroll step is due, for the engine's wait.
    pub fn drag_deadline(&self) -> Option<Duration> {
        self.next_drag
    }

    /// `_dragScroll`: scrolls the viewport and moves the selection's end to its edge.
    pub fn drag_scroll(&mut self, term: &mut Terminal, now: Duration) {
        self.next_drag = Some(now + DRAG_SCROLL_INTERVAL);
        if self.model.selection_start.is_none() || self.model.selection_end.is_none() {
            return;
        }
        if self.drag_amount == 0 {
            self.next_drag = None;
            return;
        }
        term.scroll_lines(i64::from(self.drag_amount));
        // Re-evaluate the selection: if the cursor was above or below the viewport, make sure it's
        // at the start or end of the viewport respectively (not in column mode).
        let (cols, _) = Self::dims(term);
        let buffer = term.buffers().active();
        let ydisp = i64::try_from(buffer.ydisp).unwrap_or(0);
        let rows = i64::try_from(term.rows()).unwrap_or(0);
        let len = i64::try_from(buffer.lines.len()).unwrap_or(0);
        let column = self.mode == SelectionMode::Column;
        if let Some(end) = &mut self.model.selection_end {
            if self.drag_amount > 0 {
                if !column {
                    end[0] = cols;
                }
                end[1] = (ydisp + rows).min(len - 1);
            } else {
                if !column {
                    end[0] = 0;
                }
                end[1] = ydisp;
            }
        }
    }
}

#[cfg(test)]
mod tests;
