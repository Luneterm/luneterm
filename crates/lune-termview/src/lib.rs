//! Renderer: an engine-agnostic cell grid drawn inside a `lune-ui` frame.
//!
//! The paint order in [`Pass`] is what makes the terminal match Lunegit (`TerminalView.cs`
//! render pass). Glass, scrollbar and chrome are drawn by the app around the grid.

pub mod atlas;
pub mod contain;
pub mod layout;
pub mod model;
pub mod text;
pub mod theme;

pub use atlas::{Atlas, AtlasCore, Entry, Full, Page, Raster, SwashRaster};
pub use contain::{Containment, Placement};
pub use layout::{CellMetrics, GridLayout};
pub use model::{
    Attrs, Cell, Color, Cursor, CursorShape, Grid, SelectionKind, SelectionRange, UnderlineStyle,
    ViewState,
};
pub use text::{Shaped, ShapedGlyph, Shaper, TermFonts, cell_metrics};
pub use theme::{Resolved, TermTheme, resolve};

/// Paint passes inside the grid, back to front (plan decision D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pass {
    /// Coalesced cell backgrounds; default and theme-background cells are skipped so glass
    /// shows through.
    CellBackground,
    /// Glyphs, mask then colour. Contained horizontally in their slot (Lunegit glyph-containment
    /// fix) and clipped to their row (Lunegit left vertical overflow open; fixed here).
    Text,
    /// Underlines (five styles), strikethrough, overline.
    Decoration,
    Cursor,
    Selection,
    LinkHover,
}

impl Pass {
    pub const ALL: [Pass; 6] = [
        Pass::CellBackground,
        Pass::Text,
        Pass::Decoration,
        Pass::Cursor,
        Pass::Selection,
        Pass::LinkHover,
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
