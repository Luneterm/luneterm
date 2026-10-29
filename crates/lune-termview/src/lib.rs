//! Renderer.
//!
//! GPU backend still to be chosen by measurement (binary size and frame time); see
//! `docs/architecture.md`. The paint order below is fixed now because it is what makes the
//! terminal match Lunegit (`TerminalView.cs` render pass).

/// Cells with this background are not painted, so the glass shows through.
pub const TRANSPARENT_CELL_BG: lune_ui::color::Rgba = [0x1E, 0x1E, 0x1E, 0xFF];

/// Paint passes, back to front. Each pass is a separate draw so new passes slot in by order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pass {
    /// App-drawn glass layers over the OS backdrop (`lune_ui::lunegit_dark`).
    Glass,
    /// Coalesced cell backgrounds; the default background is skipped so glass shows through.
    CellBackground,
    /// Glyph runs. Wide/emoji glyphs are contained horizontally (Lunegit glyph-containment fix)
    /// and clipped to their row (Lunegit left vertical overflow open; we fix it here).
    Text,
    Underline,
    Selection,
    Cursor,
    Scrollbar,
    LinkHover,
    /// Tab chrome outline and running-command animation.
    Chrome,
}

impl Pass {
    pub const ALL: [Pass; 9] = [
        Pass::Glass,
        Pass::CellBackground,
        Pass::Text,
        Pass::Underline,
        Pass::Selection,
        Pass::Cursor,
        Pass::Scrollbar,
        Pass::LinkHover,
        Pass::Chrome,
    ];
}

#[cfg(test)]
mod tests {
    use super::Pass;

    #[test]
    fn passes_are_listed_back_to_front() {
        assert!(Pass::ALL.windows(2).all(|w| w[0] < w[1]));
    }
}
