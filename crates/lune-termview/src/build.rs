//! CPU instance builder (plan step 5): grid rows -> per-row instance vectors, view state ->
//! overlay rects. No GPU here, so everything is unit-testable; [`crate::render::TermView`]
//! uploads what this builds.
//!
//! Dirty tracking. [`Builder::update`] rebuilds only [`Grid::dirty_rows`] (plus rows holding
//! BLINK cells when the blink phase flips) and queues them for upload; it rebuilds and queues
//! every row when the grid size, scale, origin, metrics, theme, fonts or the atlas epoch changed.
//! The atlas protocol (see [`crate::atlas`]): a `Full` page is reset at most once per update and
//! the whole grid is rebuilt, because the reset killed every UV of that page; a glyph that still
//! does not fit afterwards is dropped for this frame and the grid is rebuilt again next update.

use bytemuck::{Pod, Zeroable};
use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache};
use lune_ui::color::Rgba;

use crate::atlas::{AtlasCore, AtlasKey, CustomKey, Entry, Full, Page, SwashRaster};
use crate::contain::{base_advance, glyph_x, is_blank, next_cell_blank, place};
use crate::custom_glyphs::has_custom_char;
use crate::layout::{CellMetrics, GridLayout};
use crate::model::{Attrs, Cell, Cursor, CursorShape, Grid, UnderlineStyle, ViewState};
use crate::text::{ShapedGlyph, Shaper, TermFonts, cell_metrics};
use crate::theme::{TermTheme, resolve};

/// Most glyphs drawn for one cell (plan D11): a combining-mark flood stays bounded.
pub const MAX_GLYPHS_PER_CELL: usize = 8;

/// A filled rectangle (cell backgrounds, cursor, selection, link hover). Physical px,
/// premultiplied colour.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RectInst {
    /// x, y, w, h
    pub rect: [f32; 4],
    pub color: [f32; 4],
}

/// One glyph bitmap. `uv` is x, y, w, h in the atlas page's texels; `clip` is x0, y0, x1, y1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GlyphInst {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub clip: [f32; 4],
}

/// Decoration style codes understood by the shader.
pub const DECO_SOLID: f32 = 0.0;
pub const DECO_DOTTED: f32 = 1.0;
pub const DECO_DASHED: f32 = 2.0;
pub const DECO_CURLY: f32 = 3.0;

/// An underline, strikethrough or overline. `params` = style, line width, grid origin x, cell
/// width (the last two anchor the dotted, dashed and curly patterns to the grid).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DecoInst {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

/// Everything one grid row draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowInstances {
    pub bgs: Vec<RectInst>,
    /// Glyphs on the mask page.
    pub mask: Vec<GlyphInst>,
    /// Glyphs on the colour page.
    pub color: Vec<GlyphInst>,
    pub decos: Vec<DecoInst>,
}

/// Cursor, selection and link-hover rects, rebuilt every update.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    pub cursor: Vec<RectInst>,
    pub selection: Vec<RectInst>,
    pub link: Vec<RectInst>,
}

impl Overlay {
    /// All rects in paint order: cursor, selection, link hover (plan D9).
    pub fn rects(&self) -> impl Iterator<Item = &RectInst> {
        self.cursor.iter().chain(&self.selection).chain(&self.link)
    }

    pub fn len(&self) -> usize {
        self.cursor.len() + self.selection.len() + self.link.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Straight-alpha bytes -> premultiplied floats.
pub fn premul(c: Rgba) -> [f32; 4] {
    let a = f32::from(c[3]) / 255.0;
    [
        f32::from(c[0]) / 255.0 * a,
        f32::from(c[1]) / 255.0 * a,
        f32::from(c[2]) / 255.0 * a,
        a,
    ]
}

fn rect(x: i32, y: i32, w: i32, h: i32, color: [f32; 4]) -> RectInst {
    RectInst {
        rect: [x as f32, y as f32, w.max(0) as f32, h.max(0) as f32],
        color,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct BuildKey {
    cols: usize,
    rows: usize,
    origin_px: (i32, i32),
    metrics: CellMetrics,
    theme_rev: u64,
    fonts_rev: u64,
    epoch: u64,
}

/// What the row builder needs besides the mutable caches.
struct RowCtx<'a> {
    theme: &'a TermTheme,
    fonts: &'a TermFonts,
    m: CellMetrics,
    layout: GridLayout,
    blink_on: bool,
    cols: usize,
}

/// Atlas-protocol state shared by the rows of one update.
#[derive(Debug, Default)]
struct Protocol {
    /// Pages already reset in this update (index: mask, colour).
    reset: [bool; 2],
    /// A glyph was dropped because its page was full again after its reset.
    dropped: bool,
}

/// The CPU half of the renderer: caches, per-row instances and the upload queue.
pub struct Builder {
    fonts: TermFonts,
    theme: TermTheme,
    fonts_rev: u64,
    theme_rev: u64,
    shaper: Shaper,
    swash: SwashCache,
    metrics: Option<(u64, u32, CellMetrics)>,
    rows: Vec<RowInstances>,
    queued: Vec<bool>,
    overlay: Overlay,
    last: Option<BuildKey>,
    last_blink: Option<bool>,
    skip: bool,
}

impl std::fmt::Debug for Builder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Builder")
            .field("rows", &self.rows.len())
            .field("skip", &self.skip)
            .finish_non_exhaustive()
    }
}

impl Builder {
    pub fn new(fonts: TermFonts, theme: TermTheme) -> Builder {
        Builder {
            fonts,
            theme,
            fonts_rev: 0,
            theme_rev: 0,
            shaper: Shaper::new(),
            swash: SwashCache::new(),
            metrics: None,
            rows: Vec::new(),
            queued: Vec::new(),
            overlay: Overlay::default(),
            last: None,
            last_blink: None,
            skip: true,
        }
    }

    pub fn theme(&self) -> &TermTheme {
        &self.theme
    }

    pub fn fonts(&self) -> &TermFonts {
        &self.fonts
    }

    /// New colours; every row is rebuilt on the next update.
    pub fn set_theme(&mut self, theme: TermTheme) {
        self.theme = theme;
        self.theme_rev += 1;
    }

    /// New fonts; clears the shaper and rebuilds every row on the next update. The caller wipes
    /// the atlas (its keys name the old faces).
    pub fn set_fonts(&mut self, fonts: TermFonts) {
        self.fonts = fonts;
        self.fonts_rev += 1;
        self.shaper.clear();
        self.metrics = None;
    }

    /// Cell metrics for the current fonts at `scale`, cached per (fonts, scale).
    pub fn metrics(&mut self, fs: &mut FontSystem, scale: f32) -> CellMetrics {
        let bits = scale.to_bits();
        if let Some((rev, b, m)) = &self.metrics
            && *rev == self.fonts_rev
            && *b == bits
        {
            return *m;
        }
        let m = cell_metrics(fs, &self.fonts, scale);
        self.metrics = Some((self.fonts_rev, bits, m));
        m
    }

    pub fn rows(&self) -> &[RowInstances] {
        &self.rows
    }

    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// True after an update with an unusable scale: nothing to draw.
    pub fn is_skipped(&self) -> bool {
        self.skip
    }

    /// Rows rebuilt since the last call, ascending; clears the queue.
    pub fn take_queued(&mut self) -> Vec<usize> {
        let rows = self
            .queued
            .iter()
            .enumerate()
            .filter_map(|(r, q)| q.then_some(r))
            .collect();
        self.queued.fill(false);
        rows
    }

    /// Queue every row (the GPU buffers were re-created).
    pub fn queue_all(&mut self) {
        self.queued.fill(true);
    }

    /// Rebuilds what changed, then the overlay, then clears the grid's dirty flags.
    pub fn update(
        &mut self,
        core: &mut AtlasCore,
        fs: &mut FontSystem,
        grid: &mut Grid,
        view: &ViewState<'_>,
        origin: (f32, f32),
        scale: f32,
    ) {
        if !(scale.is_finite() && scale > 0.0) {
            self.skip = true;
            self.rows.clear();
            self.queued.clear();
            self.overlay = Overlay::default();
            self.last = None;
            grid.clear_dirty();
            return;
        }
        self.skip = false;
        let m = self.metrics(fs, scale);
        let layout = GridLayout::new(origin, m);
        let (cols, nrows) = (grid.cols(), grid.rows());
        let key = BuildKey {
            cols,
            rows: nrows,
            origin_px: layout.origin_px,
            metrics: m,
            theme_rev: self.theme_rev,
            fonts_rev: self.fonts_rev,
            epoch: core.epoch(),
        };
        let all = self.last.as_ref() != Some(&key);
        let mut todo: Vec<usize> = if all {
            self.rows = vec![RowInstances::default(); nrows];
            self.queued = vec![false; nrows];
            (0..nrows).collect()
        } else {
            let mut v: Vec<usize> = grid.dirty_rows().collect();
            if self.last_blink != Some(view.blink_on) {
                v.extend((0..nrows).filter(|&r| {
                    grid.row(r)
                        .is_some_and(|row| row.iter().any(|c| c.attrs.contains(Attrs::BLINK)))
                }));
                v.sort_unstable();
                v.dedup();
            }
            v
        };

        let mut proto = Protocol::default();
        loop {
            let done = {
                let Builder {
                    fonts,
                    theme,
                    shaper,
                    swash,
                    rows,
                    queued,
                    ..
                } = &mut *self;
                let ctx = RowCtx {
                    theme,
                    fonts,
                    m,
                    layout,
                    blink_on: view.blink_on,
                    cols,
                };
                let mut done = true;
                for &r in &todo {
                    let Some(row) = grid.row(r) else { continue };
                    match build_row(&ctx, shaper, swash, fs, core, &mut proto, row, r) {
                        Ok(inst) => {
                            rows[r] = inst;
                            queued[r] = true;
                        }
                        Err(Restart) => {
                            done = false;
                            break;
                        }
                    }
                }
                done
            };
            if done {
                break;
            }
            // A page was reset: every UV of it is dead. At most one reset per page, so this
            // loop runs at most three times.
            todo = (0..nrows).collect();
        }

        self.last_blink = Some(view.blink_on);
        self.last = (!proto.dropped).then_some(BuildKey {
            epoch: core.epoch(),
            ..key
        });
        self.overlay = build_overlay(grid, view, &layout, &self.theme);
        grid.clear_dirty();
    }
}

/// An atlas page was reset mid-build: rebuild everything.
struct Restart;

fn page_index(p: Page) -> usize {
    match p {
        Page::Mask => 0,
        Page::Color => 1,
    }
}

fn fetch(
    core: &mut AtlasCore,
    raster: &mut SwashRaster<'_>,
    proto: &mut Protocol,
    key: AtlasKey,
) -> Result<Option<Entry>, Restart> {
    match core.get(raster, key) {
        Ok(e) => Ok(e),
        Err(Full(page)) => {
            let i = page_index(page);
            if proto.reset[i] {
                core.count_dropped();
                proto.dropped = true;
                Ok(None)
            } else {
                proto.reset[i] = true;
                core.reset_grow(page);
                Err(Restart)
            }
        }
    }
}

/// The character of a cell whose text is exactly one character with a procedural definition.
fn custom_char(text: &str) -> Option<char> {
    let mut it = text.chars();
    let (Some(c), None) = (it.next(), it.next()) else {
        return None;
    };
    has_custom_char(c).then_some(c)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[allow(clippy::too_many_arguments)]
fn build_row(
    ctx: &RowCtx<'_>,
    shaper: &mut Shaper,
    swash: &mut SwashCache,
    fs: &mut FontSystem,
    core: &mut AtlasCore,
    proto: &mut Protocol,
    row: &[Cell],
    r: usize,
) -> Result<RowInstances, Restart> {
    let m = &ctx.m;
    let scale = m.scale;
    let mut out = RowInstances::default();
    // open background run: (first col, end col, colour)
    let mut run: Option<(usize, usize, Rgba)> = None;
    let flush = |run: &mut Option<(usize, usize, Rgba)>, out: &mut RowInstances| {
        if let Some((c0, c1, color)) = run.take() {
            let [x, y, _, h] = ctx.layout.cell_rect_px(r, c0, c1 - c0);
            let w = clamp_i32((c1 - c0) as i64 * i64::from(m.cell_w));
            out.bgs.push(rect(x, y, w, h, premul(color)));
        }
    };

    for (c, cell) in row.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        let span = usize::from(cell.width)
            .min(ctx.cols.saturating_sub(c))
            .max(1);
        let res = resolve(cell, ctx.theme, ctx.blink_on);

        match res.bg {
            Some(color) => match &mut run {
                Some((_, end, rc)) if *end == c && *rc == color => *end = c + span,
                _ => {
                    flush(&mut run, &mut out);
                    run = Some((c, c + span, color));
                }
            },
            None => flush(&mut run, &mut out),
        }

        if !res.show_fg {
            continue;
        }
        let [cell_x, row_top, _, _] = ctx.layout.cell_rect_px(r, c, span);

        let text = cell.text();
        if let Some(ch) = custom_char(text) {
            // Procedural glyph (box drawing, block, powerline): one cell-sized mask image, never
            // shaped, so bold and italic don't change it. A width-2 cell gets the one-cell image
            // at its leading cell, as xterm.js draws it.
            let mut raster = SwashRaster { cache: swash, fs };
            let key = AtlasKey::Custom(CustomKey {
                ch,
                cell_w: m.cell_w,
                cell_h: m.cell_h,
                scale_bits: m.scale.to_bits(),
                font_size_bits: m.font_size.to_bits(),
            });
            if let Some(e) = fetch(core, &mut raster, proto, key)? {
                let (cw, ch_) = (m.cell_w as f32, m.cell_h as f32);
                out.mask.push(GlyphInst {
                    rect: [
                        cell_x as f32,
                        row_top as f32,
                        e.uv[2] as f32,
                        e.uv[3] as f32,
                    ],
                    uv: e.uv.map(|v| v as f32),
                    color: premul(res.fg),
                    clip: [
                        cell_x as f32,
                        row_top as f32,
                        cell_x as f32 + cw,
                        row_top as f32 + ch_,
                    ],
                });
            }
        } else if !is_blank(text) {
            let bold = cell.attrs.contains(Attrs::BOLD);
            let italic = cell.attrs.contains(Attrs::ITALIC);
            let shaped = shaper.shape(fs, ctx.fonts, text, bold, italic);
            if !shaped.glyphs.is_empty() {
                let advance_px = base_advance(&shaped.glyphs) * scale;
                let next_blank = next_cell_blank(row, c);
                let mut raster = SwashRaster { cache: swash, fs };
                let key_of = |i: usize, pl: &crate::contain::Placement| {
                    let g: &ShapedGlyph = &shaped.glyphs[i];
                    let rs = pl.raster_scale;
                    let flags = if g.italic {
                        CacheKeyFlags::FAKE_ITALIC
                    } else {
                        CacheKeyFlags::empty()
                    };
                    let (key, xi, yi) = CacheKey::new(
                        g.font_id,
                        g.glyph_id,
                        m.font_size * scale * rs,
                        (
                            pl.origin_x + glyph_x(&shaped.glyphs, i) * scale * rs,
                            (pl.baseline_y as f32 + g.y * scale * rs).trunc(),
                        ),
                        g.weight,
                        flags,
                    );
                    (AtlasKey::Font(key), xi, yi)
                };
                // Whether the glyph is colour is known only once it is rasterized: probe the
                // first glyph as a mask glyph, then re-place if it turns out to be colour.
                let pl0 = place(
                    cell_x, row_top, m, cell.width, advance_px, false, next_blank,
                );
                let (k0, _, _) = key_of(0, &pl0);
                let is_color =
                    fetch(core, &mut raster, proto, k0)?.is_some_and(|e| e.page == Page::Color);
                let pl = if is_color {
                    place(cell_x, row_top, m, cell.width, advance_px, true, next_blank)
                } else {
                    pl0
                };
                let clip = pl.clip.map(|v| v as f32);
                let color = premul(res.fg);
                for i in 0..shaped.glyphs.len().min(MAX_GLYPHS_PER_CELL) {
                    let (key, xi, yi) = key_of(i, &pl);
                    let Some(e) = fetch(core, &mut raster, proto, key)? else {
                        continue;
                    };
                    let inst = GlyphInst {
                        rect: [
                            (xi + e.left) as f32,
                            (yi - e.top) as f32,
                            e.uv[2] as f32,
                            e.uv[3] as f32,
                        ],
                        uv: e.uv.map(|v| v as f32),
                        color,
                        clip,
                    };
                    match e.page {
                        Page::Mask => out.mask.push(inst),
                        Page::Color => out.color.push(inst),
                    }
                }
            }
        }

        decorations(ctx, cell, &res, r, c, span, &mut out.decos);
    }
    flush(&mut run, &mut out);
    Ok(out)
}

fn line_width(m: &CellMetrics, div: f32) -> i32 {
    ((m.font_size * m.scale / div).floor() as i32).max(1)
}

/// Underline, strikethrough and overline of one cell (geometry after xterm.js
/// `TextureAtlas.ts:532-730`, on whole pixels; see the plan's step 5 builder rules).
fn decorations(
    ctx: &RowCtx<'_>,
    cell: &Cell,
    res: &crate::theme::Resolved,
    r: usize,
    c: usize,
    span: usize,
    out: &mut Vec<DecoInst>,
) {
    let m = &ctx.m;
    let [x, row_top, w, cell_h] = ctx.layout.cell_rect_px(r, c, span);
    let origin_x = ctx.layout.origin_px.0 as f32;
    let char_top = m.char_top;
    let char_h = m.char_h as i32;
    // rows relative to the row top, clamped into the cell
    let mut push = |y0: i32, h: i32, color: Rgba, style: f32, lw: i32| {
        let y0 = y0.clamp(0, cell_h.max(0));
        let y1 = y0.saturating_add(h).clamp(0, cell_h.max(0));
        if y1 > y0 {
            out.push(DecoInst {
                rect: [x as f32, (row_top + y0) as f32, w as f32, (y1 - y0) as f32],
                color: premul(color),
                params: [style, lw as f32, origin_x, m.cell_w as f32],
            });
        }
    };

    if cell.underline != UnderlineStyle::None {
        let lw = line_width(m, 15.0);
        // the band's bottom sits on the character box's bottom, moved up to stay inside the cell
        let bottom = (char_top + char_h).min(cell_h);
        let col = res.decoration;
        match cell.underline {
            UnderlineStyle::None => {}
            UnderlineStyle::Single => push(bottom - lw, lw, col, DECO_SOLID, lw),
            UnderlineStyle::Dotted => push(bottom - lw, lw, col, DECO_DOTTED, lw),
            UnderlineStyle::Dashed => push(bottom - lw, lw, col, DECO_DASHED, lw),
            UnderlineStyle::Double => {
                let top = bottom - 3 * lw;
                push(top, lw, col, DECO_SOLID, lw);
                push(top + 2 * lw, lw, col, DECO_SOLID, lw);
            }
            UnderlineStyle::Curly => push(bottom - 3 * lw, 3 * lw, col, DECO_CURLY, lw),
        }
    }
    if cell.attrs.contains(Attrs::STRIKETHROUGH) {
        let lw = line_width(m, 10.0);
        let centre = char_top + char_h / 2;
        push(centre - lw / 2, lw, res.fg, DECO_SOLID, lw);
    }
    if cell.attrs.contains(Attrs::OVERLINE) {
        let lw = line_width(m, 15.0);
        push(char_top, lw, res.fg, DECO_SOLID, lw);
    }
}

/// Cursor, selection and link hover for this frame.
fn build_overlay(
    grid: &Grid,
    view: &ViewState<'_>,
    layout: &GridLayout,
    theme: &TermTheme,
) -> Overlay {
    let mut o = Overlay::default();
    let m = &layout.metrics;
    let (cols, rows) = (grid.cols(), grid.rows());
    let px = |v: f32| (v.round() as i32).max(1);
    let thin = px(m.scale);

    let cur: &Cursor = &view.cursor;
    if cur.visible && (!cur.blink || view.blink_on) && cur.row < rows && cur.col < cols {
        let mut col = cur.col;
        if grid.get(cur.row, col).is_some_and(|c| c.width == 0) && col > 0 {
            col -= 1;
        }
        let wide = grid.get(cur.row, col).is_some_and(|c| c.width >= 2);
        let span = if wide { 2.min(cols - col) } else { 1 };
        let [x, y, w, h] = layout.cell_rect_px(cur.row, col, span);
        let color = premul(theme.cursor);
        if view.focused {
            match cur.shape {
                CursorShape::Block => o.cursor.push(rect(x, y, w, h, color)),
                CursorShape::Underline => {
                    let t = thin.min(h);
                    o.cursor.push(rect(x, y + h - t, w, t, color));
                }
                CursorShape::Bar => o.cursor.push(rect(x, y, thin.min(w), h, color)),
            }
        } else {
            // outline; the side edges stop at the top and bottom edges so a translucent cursor
            // does not double up in the corners
            let t = thin.min(h / 2).min(w / 2).max(1);
            o.cursor.push(rect(x, y, w, t, color));
            o.cursor.push(rect(x, y + h - t, w, t, color));
            o.cursor.push(rect(x, y + t, t, h - 2 * t, color));
            o.cursor.push(rect(x + w - t, y + t, t, h - 2 * t, color));
        }
    }

    let sel = premul(theme.selection);
    for range in view.selection {
        for r in 0..rows {
            if let Some(cols_sel) = range.cols_in_row(r, cols) {
                let [x, y, w, h] = layout.cell_rect_px(r, cols_sel.start, cols_sel.len());
                o.selection.push(rect(x, y, w, h, sel));
            }
        }
    }

    if view.hover_link != 0 {
        let color = premul(theme.link_hover);
        let t = px(2.0 * m.scale);
        for r in 0..rows {
            let Some(row) = grid.row(r) else { continue };
            let mut run: Option<(usize, usize)> = None;
            let mut flush = |run: &mut Option<(usize, usize)>| {
                if let Some((c0, c1)) = run.take() {
                    let [x, y, w, h] = layout.cell_rect_px(r, c0, c1 - c0);
                    let t = t.min(h);
                    o.link.push(rect(x, y + h - t, w, t, color));
                }
            };
            for (c, cell) in row.iter().enumerate() {
                if cell.width == 0 {
                    continue;
                }
                let span = usize::from(cell.width).min(cols - c);
                if cell.link == view.hover_link {
                    match &mut run {
                        Some((_, end)) if *end == c => *end = c + span,
                        _ => {
                            flush(&mut run);
                            run = Some((c, c + span));
                        }
                    }
                } else {
                    flush(&mut run);
                }
            }
            flush(&mut run);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Color, SelectionKind, SelectionRange};
    use cosmic_text::fontdb;
    use std::sync::Arc;

    const REGULAR: &[u8] = include_bytes!("../tests/fonts/RznMono-Regular.subset.ttf");
    const BOLD: &[u8] = include_bytes!("../tests/fonts/RznMono-Bold.subset.ttf");
    const SYMBOLS: &[u8] = include_bytes!("../tests/fonts/NotoSansSymbols2.subset.ttf");
    const TWEMOJI: &[u8] = include_bytes!("../tests/fonts/TwemojiLunegit.subset.ttf");

    struct Rig {
        b: Builder,
        fs: FontSystem,
        core: AtlasCore,
        grid: Grid,
    }

    fn rig(cols: usize, rows: usize) -> Rig {
        let mut db = fontdb::Database::new();
        let mono = db.load_font_source(fontdb::Source::Binary(Arc::new(REGULAR)));
        db.load_font_source(fontdb::Source::Binary(Arc::new(BOLD)));
        db.load_font_source(fontdb::Source::Binary(Arc::new(SYMBOLS)));
        db.load_font_source(fontdb::Source::Binary(Arc::new(TWEMOJI)));
        let family = db.face(mono[0]).expect("face").families[0].0.clone();
        let fonts = TermFonts {
            family,
            size: 13.0,
            line_height: 17.0,
            emoji_family: None,
        };
        Rig {
            b: Builder::new(fonts, TermTheme::lunegit_one_dark()),
            fs: FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
            core: AtlasCore::new(512, 8192),
            grid: Grid::new(cols, rows),
        }
    }

    fn view(selection: &[SelectionRange]) -> ViewState<'_> {
        ViewState {
            cursor: Cursor {
                visible: false,
                ..Cursor::default()
            },
            focused: true,
            blink_on: true,
            selection,
            hover_link: 0,
        }
    }

    impl Rig {
        fn update_with(&mut self, v: &ViewState<'_>, scale: f32) {
            self.b.update(
                &mut self.core,
                &mut self.fs,
                &mut self.grid,
                v,
                (0.0, 0.0),
                scale,
            );
        }
        fn update(&mut self) {
            self.update_with(&view(&[]), 2.0);
        }
    }

    fn bg(r: u8, g: u8, b: u8) -> Color {
        Color::Rgb(r, g, b)
    }

    fn set_bg(rig: &mut Rig, row: usize, col: usize, c: Color) {
        let mut cell = Cell::default();
        cell.bg = c;
        assert!(rig.grid.set(row, col, cell));
    }

    #[test]
    fn background_runs_coalesce() {
        let mut r = rig(5, 1);
        let (a, b2) = (bg(1, 2, 3), bg(9, 9, 9));
        for (c, col) in [(0, a), (1, a), (2, b2), (4, a)] {
            set_bg(&mut r, 0, c, col);
        }
        r.update();
        let bgs = &r.b.rows()[0].bgs;
        let xs: Vec<[f32; 2]> = bgs.iter().map(|b| [b.rect[0], b.rect[2]]).collect();
        assert_eq!(xs, vec![[0.0, 32.0], [32.0, 16.0], [64.0, 16.0]], "{bgs:?}");
        assert!(bgs.iter().all(|b| b.rect[3] == 34.0));
    }

    #[test]
    fn theme_background_cell_paints_nothing() {
        let mut r = rig(2, 1);
        set_bg(&mut r, 0, 0, bg(0x1E, 0x1E, 0x1E));
        set_bg(&mut r, 0, 1, bg(0x1E, 0x1E, 0x1F));
        r.update();
        let bgs = &r.b.rows()[0].bgs;
        assert_eq!(bgs.len(), 1);
        assert_eq!(bgs[0].rect[0], 16.0);
    }

    #[test]
    fn wide_cell_background_is_two_cells() {
        let mut r = rig(4, 1);
        let mut lead = Cell::default();
        lead.width = 2;
        lead.bg = bg(1, 2, 3);
        let mut spacer = Cell::default();
        spacer.width = 0;
        spacer.bg = bg(1, 2, 3);
        r.grid.set(0, 1, lead);
        r.grid.set(0, 2, spacer);
        r.update();
        let bgs = &r.b.rows()[0].bgs;
        assert_eq!(bgs.len(), 1, "{bgs:?}");
        assert_eq!((bgs[0].rect[0], bgs[0].rect[2]), (16.0, 32.0));
    }

    #[test]
    fn underline_and_strike_line_widths() {
        let mut r = rig(2, 1);
        let mut u = Cell::default();
        u.set_char('a');
        u.underline = UnderlineStyle::Single;
        let mut s = Cell::default();
        s.set_char('b');
        s.attrs.insert(Attrs::STRIKETHROUGH);
        r.grid.set(0, 0, u);
        r.grid.set(0, 1, s);
        r.update();
        let d = &r.b.rows()[0].decos;
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].rect[3], 1.0, "underline lw: {d:?}");
        assert_eq!(d[0].params[0], DECO_SOLID);
        // inside the 34 px cell
        assert!(d[0].rect[1] + d[0].rect[3] <= 34.0);
        assert_eq!(d[1].rect[3], 2.0, "strike lw: {d:?}");
    }

    #[test]
    fn underline_styles_emit_their_codes() {
        let mut r = rig(5, 1);
        let styles = [
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];
        for (c, st) in styles.into_iter().enumerate() {
            let mut cell = Cell::default();
            cell.underline = st;
            r.grid.set(0, c, cell);
        }
        r.update();
        let codes: Vec<f32> = r.b.rows()[0].decos.iter().map(|d| d.params[0]).collect();
        assert_eq!(
            codes,
            vec![DECO_SOLID, DECO_SOLID, DECO_CURLY, DECO_DOTTED, DECO_DASHED]
        );
        let d = &r.b.rows()[0].decos;
        // double: two lines, a line width apart
        assert_eq!(d[1].rect[1] - d[0].rect[1], 2.0);
    }

    #[test]
    fn focused_cursor_shapes() {
        let mut r = rig(4, 2);
        let mut v = view(&[]);
        v.cursor = Cursor {
            row: 1,
            col: 2,
            shape: CursorShape::Block,
            blink: false,
            visible: true,
        };
        r.update_with(&v, 2.0);
        let c = r.b.overlay().cursor.clone();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].rect, [32.0, 34.0, 16.0, 34.0]);
        assert!((c[0].color[3] - 128.0 / 255.0).abs() < 1e-6);

        v.cursor.shape = CursorShape::Bar;
        r.update_with(&v, 2.0);
        assert_eq!(r.b.overlay().cursor[0].rect, [32.0, 34.0, 2.0, 34.0]);

        v.cursor.shape = CursorShape::Underline;
        r.update_with(&v, 2.0);
        assert_eq!(r.b.overlay().cursor[0].rect, [32.0, 66.0, 16.0, 2.0]);
    }

    #[test]
    fn unfocused_cursor_is_four_edges() {
        let mut r = rig(4, 2);
        let mut v = view(&[]);
        v.focused = false;
        v.cursor.visible = true;
        for shape in [CursorShape::Block, CursorShape::Bar, CursorShape::Underline] {
            v.cursor.shape = shape;
            r.update_with(&v, 2.0);
            assert_eq!(r.b.overlay().cursor.len(), 4, "{shape:?}");
        }
    }

    #[test]
    fn blinking_cursor_hides_in_the_off_phase() {
        let mut r = rig(2, 1);
        let mut v = view(&[]);
        v.cursor.visible = true;
        v.cursor.blink = true;
        v.blink_on = false;
        r.update_with(&v, 2.0);
        assert!(r.b.overlay().cursor.is_empty());
        v.blink_on = true;
        r.update_with(&v, 2.0);
        assert_eq!(r.b.overlay().cursor.len(), 1);
        // outside the grid: nothing
        v.cursor.row = 5;
        r.update_with(&v, 2.0);
        assert!(r.b.overlay().cursor.is_empty());
    }

    #[test]
    fn cursor_on_spacer_moves_to_the_wide_cell() {
        let mut r = rig(4, 1);
        let mut lead = Cell::default();
        lead.width = 2;
        let mut spacer = Cell::default();
        spacer.width = 0;
        r.grid.set(0, 1, lead);
        r.grid.set(0, 2, spacer);
        let mut v = view(&[]);
        v.cursor.visible = true;
        v.cursor.col = 2;
        r.update_with(&v, 2.0);
        assert_eq!(r.b.overlay().cursor[0].rect, [16.0, 0.0, 32.0, 34.0]);
    }

    #[test]
    fn linear_selection_over_three_rows_is_three_rects() {
        let mut r = rig(10, 5);
        let sel = [SelectionRange {
            start: (1, 4),
            end: (3, 2),
            kind: SelectionKind::Linear,
        }];
        r.update_with(&view(&sel), 2.0);
        let s = &r.b.overlay().selection;
        assert_eq!(s.len(), 3, "{s:?}");
        assert_eq!(s[0].rect, [64.0, 34.0, 96.0, 34.0]);
        assert_eq!(s[1].rect, [0.0, 68.0, 160.0, 34.0]);
        assert_eq!(s[2].rect, [0.0, 102.0, 48.0, 34.0]);
    }

    #[test]
    fn link_hover_underlines_the_hovered_run() {
        let mut r = rig(6, 1);
        for c in 1..4 {
            let mut cell = Cell::default();
            cell.link = 7;
            r.grid.set(0, c, cell);
        }
        let mut v = view(&[]);
        v.hover_link = 7;
        r.update_with(&v, 2.0);
        let l = &r.b.overlay().link;
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].rect, [16.0, 30.0, 48.0, 4.0]);
    }

    #[test]
    fn dirty_tracking() {
        let mut r = rig(4, 3);
        r.update();
        assert_eq!(r.b.take_queued(), vec![0, 1, 2], "first update");
        r.update();
        assert!(r.b.take_queued().is_empty(), "nothing dirty");
        let mut c = Cell::default();
        c.set_char('x');
        r.grid.set(1, 0, c);
        r.update();
        assert_eq!(r.b.take_queued(), vec![1], "one dirty row");
        r.grid.resize(5, 3);
        r.update();
        assert_eq!(r.b.take_queued(), vec![0, 1, 2], "resize");
        r.update();
        assert!(r.b.take_queued().is_empty());
        r.core.reset_grow(Page::Mask);
        r.update();
        assert_eq!(r.b.take_queued(), vec![0, 1, 2], "atlas generation bump");
        r.update_with(&view(&[]), 1.0);
        assert_eq!(r.b.take_queued(), vec![0, 1, 2], "scale change");
        r.b.set_theme(TermTheme::lunegit_one_dark());
        r.update_with(&view(&[]), 1.0);
        assert_eq!(r.b.take_queued(), vec![0, 1, 2], "theme change");
    }

    #[test]
    fn blink_rows_rebuild_when_the_phase_flips() {
        let mut r = rig(3, 3);
        let mut c = Cell::default();
        c.set_char('b');
        c.attrs.insert(Attrs::BLINK);
        r.grid.set(1, 1, c);
        r.update();
        r.b.take_queued();
        let glyphs_on = r.b.rows()[1].mask.len();
        assert_eq!(glyphs_on, 1);
        let mut v = view(&[]);
        v.blink_on = false;
        r.update_with(&v, 2.0);
        assert_eq!(r.b.take_queued(), vec![1]);
        assert_eq!(r.b.rows()[1].mask.len(), 0, "hidden in the off phase");
    }

    #[test]
    fn combining_mark_flood_is_bounded() {
        let mut r = rig(2, 1);
        let mut c = Cell::default();
        c.set_text(&format!("a{}", "\u{301}".repeat(200)));
        r.grid.set(0, 0, c);
        r.update();
        let row = &r.b.rows()[0];
        // (the shaped cluster is hundreds of glyphs wide and is scaled to its slot, so most of
        // them rasterize to nothing; the point is the bound)
        assert!(row.mask.len() + row.color.len() <= MAX_GLYPHS_PER_CELL);
        // a modest stack still draws
        let mut c = Cell::default();
        c.set_text("a\u{301}\u{301}");
        r.grid.set(0, 1, c);
        r.update();
        assert!(!r.b.rows()[0].mask.is_empty());
    }

    #[test]
    fn glyph_lands_in_its_cell_and_row() {
        let mut r = rig(3, 2);
        let mut c = Cell::default();
        c.set_char('W');
        r.grid.set(1, 1, c);
        r.update();
        let g = r.b.rows()[1].mask[0];
        assert_eq!(g.clip, [16.0, 34.0, 48.0, 68.0], "slot + spill cell, row");
        assert!(
            g.rect[0] >= 16.0 && g.rect[0] + g.rect[2] <= 32.0 + 1.0,
            "{g:?}"
        );
        assert!(r.b.rows()[0].mask.is_empty());
    }

    #[test]
    fn single_box_char_is_one_cell_sized_mask_instance() {
        let mut r = rig(4, 2);
        let mut c = Cell::default();
        c.set_char('─');
        r.grid.set(1, 2, c);
        r.update();
        let row = &r.b.rows()[1];
        assert_eq!(row.mask.len(), 1, "{row:?}");
        assert!(row.color.is_empty());
        let g = row.mask[0];
        // cell (row 1, col 2) at 2x: x = 2*16, y = 1*34, 16 x 34
        assert_eq!(g.rect, [32.0, 34.0, 16.0, 34.0]);
        assert_eq!(g.clip, [32.0, 34.0, 48.0, 68.0]);
        assert_eq!((g.uv[2], g.uv[3]), (16.0, 34.0));
        assert!(r.b.rows()[0].mask.is_empty());

        // bold does not change a procedural glyph
        let mut bold = Cell::default();
        bold.set_char('─');
        bold.attrs.insert(Attrs::BOLD);
        r.grid.set(1, 3, bold);
        r.update();
        let row = &r.b.rows()[1];
        assert_eq!(row.mask.len(), 2);
        assert_eq!(row.mask[1].uv, row.mask[0].uv);
        assert_eq!(row.mask[1].rect, [48.0, 34.0, 16.0, 34.0]);
    }

    #[test]
    fn box_char_with_a_combining_mark_takes_the_font_path() {
        let mut r = rig(3, 1);
        let mut c = Cell::default();
        c.set_char('─');
        r.grid.set(0, 0, c);
        let mut c = Cell::default();
        c.set_text("─\u{301}");
        r.grid.set(0, 1, c);
        r.update();
        let row = &r.b.rows()[0];
        let plain = row.mask[0];
        assert_eq!(plain.rect[2..], [16.0, 34.0]);
        assert!(
            row.mask.len() >= 2,
            "the mark cell draws through the font: {row:?}"
        );
        // whatever the font path produced, it is not the cell-sized procedural image
        assert!(row.mask.iter().skip(1).all(|g| g.uv != plain.uv), "{row:?}");
    }

    #[test]
    fn wide_cell_box_char_draws_one_cell_at_the_leading_column() {
        let mut r = rig(4, 1);
        let mut c = Cell::default();
        c.set_char('█');
        c.width = 2;
        r.grid.set(0, 1, c);
        r.update();
        let row = &r.b.rows()[0];
        assert_eq!(row.mask.len(), 1);
        assert_eq!(row.mask[0].rect, [16.0, 0.0, 16.0, 34.0]);
        assert_eq!(row.mask[0].clip, [16.0, 0.0, 32.0, 34.0]);
    }

    fn put_text(r: &mut Rig, row: usize, col: usize, text: &str) {
        let mut c = Cell::default();
        c.set_text(text);
        assert!(r.grid.set(row, col, c));
    }

    #[test]
    fn mark_stack_is_fitted_on_the_base_glyph() {
        // row 0: plain "a"; row 1: "a" with four marks; non-blank neighbours, so no spill cell
        let mut r = rig(3, 2);
        for row in 0..2 {
            put_text(&mut r, row, 0, "x");
            put_text(&mut r, row, 2, "x");
        }
        // (the font composes a + U+0301 into its "a acute" glyph, so the base is U+00E1)
        put_text(&mut r, 0, 1, "\u{e1}");
        put_text(&mut r, 1, 1, "a\u{301}\u{303}\u{309}\u{323}");
        r.update();
        let plain = r.b.rows()[0].mask[1];
        let stack = &r.b.rows()[1].mask;
        // the first instance is the base: same size and place as the plain letter (no ScaleToFit)
        assert_eq!(stack[1].uv[2..], plain.uv[2..], "{stack:?}");
        assert_eq!(
            stack[1].rect[1] - 34.0,
            plain.rect[1],
            "same height on its row"
        );
        assert_eq!(stack[1].rect[2..], plain.rect[2..]);
        assert_eq!(stack[1].rect[0], plain.rect[0]);
        // every glyph of the cell starts within a cell of its slot
        let cell_x = 16.0;
        for g in &stack[1..stack.len() - 1] {
            assert!(
                g.rect[0] >= cell_x - 16.0 && g.rect[0] <= cell_x + 16.0,
                "{g:?}"
            );
        }
        assert!(stack.len() >= 4, "marks are drawn: {stack:?}");
    }

    #[test]
    fn mark_flood_keeps_the_base_at_full_size_and_is_bounded() {
        let mut r = rig(3, 2);
        put_text(&mut r, 0, 1, "\u{e1}");
        put_text(&mut r, 1, 1, &format!("a{}", "\u{301}".repeat(200)));
        r.update();
        let plain = r.b.rows()[0].mask[0];
        let flood = &r.b.rows()[1].mask;
        assert!(flood.len() <= MAX_GLYPHS_PER_CELL, "{}", flood.len());
        // the base is the plain letter, one row lower (row 1: y and clip move by one cell height)
        assert_eq!(flood[0].uv[2..], plain.uv[2..]);
        assert_eq!(flood[0].rect[2..], plain.rect[2..]);
        assert_eq!(flood[0].rect[0], plain.rect[0]);
        assert_eq!(flood[0].rect[1] - 34.0, plain.rect[1]);
    }

    #[test]
    fn bad_scale_draws_nothing() {
        let mut r = rig(3, 2);
        for s in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            r.update_with(&view(&[]), s);
            assert!(r.b.is_skipped());
            assert!(r.b.rows().is_empty());
            assert!(r.b.take_queued().is_empty());
        }
        r.update();
        assert!(!r.b.is_skipped());
        assert_eq!(r.b.take_queued(), vec![0, 1]);
    }

    #[test]
    fn empty_grid_is_fine() {
        let mut r = rig(0, 0);
        let sel = [SelectionRange {
            start: (0, 0),
            end: (3, 3),
            kind: SelectionKind::Block,
        }];
        let mut v = view(&sel);
        v.cursor.visible = true;
        v.hover_link = 1;
        r.update_with(&v, 2.0);
        assert!(r.b.rows().is_empty() && r.b.overlay().is_empty());
    }

    #[test]
    fn atlas_full_resets_once_and_rebuilds_everything() {
        let mut r = rig(8, 4);
        // a page too small for the grid's glyphs
        r.core = AtlasCore::new(16, 64);
        for row in 0..4 {
            for col in 0..8 {
                let mut c = Cell::default();
                c.set_char(char::from(b'A' + (row * 8 + col) as u8));
                r.grid.set(row, col, c);
            }
        }
        r.update();
        // reset happened, every row was rebuilt after it, nothing panicked
        assert!(r.core.epoch() >= 1);
        assert_eq!(r.b.take_queued(), vec![0, 1, 2, 3]);
        for _ in 0..8 {
            r.update();
        }
    }
}
