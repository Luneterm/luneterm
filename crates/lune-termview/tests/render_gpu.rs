//! GPU smoke test for `TermView` (plan step 5): a small grid rendered offscreen. Prints
//! `SKIPPED` without an adapter unless `LUNE_REQUIRE_GPU=1`.

mod common;

use std::sync::Arc;

use cosmic_text::{FontSystem, fontdb};
use lune_termview::{
    Attrs, Cell, Color, Cursor, Grid, TermFonts, TermTheme, TermView, UnderlineStyle, ViewState,
};
use lune_ui::custom::{CustomDraw, CustomFrame};

const REGULAR: &[u8] = include_bytes!("fonts/RznMono-Regular.subset.ttf");
const BOLD: &[u8] = include_bytes!("fonts/RznMono-Bold.subset.ttf");
const TWEMOJI: &[u8] = include_bytes!("fonts/TwemojiLunegit.subset.ttf");

const CLEAR: [u8; 4] = [10, 20, 30, 255];
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn test_fonts() -> (FontSystem, TermFonts) {
    let mut db = fontdb::Database::new();
    let mono = db.load_font_source(fontdb::Source::Binary(Arc::new(REGULAR)));
    db.load_font_source(fontdb::Source::Binary(Arc::new(BOLD)));
    let family = db.face(mono[0]).expect("face").families[0].0.clone();
    (
        FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
        TermFonts {
            family,
            size: 13.0,
            line_height: 17.0,
            emoji_family: None,
            icon_family: None,
        },
    )
}

fn cell(f: impl FnOnce(&mut Cell)) -> Cell {
    let mut c = Cell::default();
    f(&mut c);
    c
}

fn view() -> ViewState<'static> {
    ViewState {
        cursor: Cursor {
            visible: false,
            ..Cursor::default()
        },
        focused: true,
        blink_on: true,
        selection: &[],
        hover_link: 0,
    }
}

/// Renders `tv` into a fresh target of `size`; returns RGBA8 pixels.
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tv: &mut TermView,
    size: (u32, u32),
    scale: f32,
) -> Vec<u8> {
    let target = lune_ui::dump::target(device, FORMAT, size);
    let frame = CustomFrame {
        size,
        scale,
        format: FORMAT,
    };
    tv.prepare(device, queue, &frame);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("termview test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(CLEAR[0]) / 255.0,
                        g: f64::from(CLEAR[1]) / 255.0,
                        b: f64::from(CLEAR[2]) / 255.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        tv.render(&mut pass, &frame);
    }
    queue.submit(Some(enc.finish()));
    lune_ui::dump::read_rgba(device, queue, &target).expect("readback")
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1)
}

#[test]
fn background_cell_and_clear_colour() {
    let Some((device, queue, _)) = common::device() else {
        return;
    };
    let (mut fs, fonts) = test_fonts();
    let mut tv = TermView::new(&device, FORMAT, fonts, TermTheme::lunegit_one_dark());
    let mut grid = Grid::new(4, 2);
    grid.set(0, 0, cell(|c| c.bg = Color::Rgb(200, 0, 0)));
    tv.update(&mut fs, &mut grid, &view(), (0.0, 0.0), 2.0);
    let m = tv.metrics(&mut fs, 2.0);
    assert_eq!((m.cell_w, m.cell_h), (16, 34));
    let (w, h) = (4 * m.cell_w, 2 * m.cell_h);
    let img = draw(&device, &queue, &mut tv, (w, h), 2.0);
    let centre = |col: u32, row: u32| {
        px(
            &img,
            w,
            col * m.cell_w + m.cell_w / 2,
            row * m.cell_h + m.cell_h / 2,
        )
    };
    assert!(close(centre(0, 0), [200, 0, 0, 255]), "{:?}", centre(0, 0));
    assert!(close(centre(1, 0), CLEAR), "{:?}", centre(1, 0));
    assert!(close(centre(0, 1), CLEAR), "{:?}", centre(0, 1));
}

#[test]
fn text_stays_in_its_cell_and_dirty_rows_update() {
    let Some((device, queue, _)) = common::device() else {
        return;
    };
    let (mut fs, fonts) = test_fonts();
    let mut tv = TermView::new(&device, FORMAT, fonts, TermTheme::lunegit_one_dark());
    let mut grid = Grid::new(3, 2);
    grid.set(0, 1, cell(|c| c.set_char('W')));
    tv.update(&mut fs, &mut grid, &view(), (0.0, 0.0), 2.0);
    let m = tv.metrics(&mut fs, 2.0);
    let (w, h) = (3 * m.cell_w, 2 * m.cell_h);
    let img = draw(&device, &queue, &mut tv, (w, h), 2.0);
    let ink = |img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32| -> usize {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if px(img, w, x, y) != CLEAR {
                    n += 1;
                }
            }
        }
        n
    };
    // W is inked in cell (0,1); the spill cell (0,2) and everything else stay clear
    assert!(ink(&img, m.cell_w, 0, 2 * m.cell_w, m.cell_h) > 20);
    assert_eq!(ink(&img, 0, 0, m.cell_w, h), 0, "left of the glyph");
    assert_eq!(ink(&img, 0, m.cell_h, w, h), 0, "row below");

    // change only row 1: the row-0 glyph survives, the new glyph shows up
    grid.set(1, 0, cell(|c| c.set_char('i')));
    tv.update(&mut fs, &mut grid, &view(), (0.0, 0.0), 2.0);
    let img2 = draw(&device, &queue, &mut tv, (w, h), 2.0);
    assert_eq!(
        ink(&img2, 0, 0, w, m.cell_h),
        ink(&img, 0, 0, w, m.cell_h),
        "row 0 unchanged"
    );
    assert!(ink(&img2, 0, m.cell_h, m.cell_w, h) > 0, "row 1 drawn");
}

#[test]
fn decorations_cursor_and_selection_draw() {
    let Some((device, queue, _)) = common::device() else {
        return;
    };
    let (mut fs, fonts) = test_fonts();
    let mut theme = TermTheme::lunegit_one_dark();
    theme.cursor = [255, 255, 255, 255];
    let mut tv = TermView::new(&device, FORMAT, fonts, theme);
    let mut grid = Grid::new(4, 1);
    grid.set(
        0,
        0,
        cell(|c| {
            c.underline = UnderlineStyle::Curly;
            c.underline_color = Color::Rgb(255, 0, 255);
        }),
    );
    grid.set(0, 1, cell(|c| c.attrs.insert(Attrs::STRIKETHROUGH)));
    let mut v = view();
    v.cursor.visible = true;
    v.cursor.col = 3;
    tv.update(&mut fs, &mut grid, &v, (0.0, 0.0), 2.0);
    let m = tv.metrics(&mut fs, 2.0);
    let (w, h) = (4 * m.cell_w, m.cell_h);
    let img = draw(&device, &queue, &mut tv, (w, h), 2.0);
    // opaque white block cursor in the last cell
    assert!(close(
        px(&img, w, 3 * m.cell_w + 8, 17),
        [255, 255, 255, 255]
    ));
    // the curly underline put magenta-ish ink at the bottom of cell 0
    let magenta = (0..m.cell_w)
        .flat_map(|x| (m.cell_h - 4..m.cell_h).map(move |y| (x, y)))
        .any(|(x, y)| {
            let p = px(&img, w, x, y);
            p[0] > 100 && p[2] > 100 && p[1] < 60
        });
    assert!(magenta, "curly underline");
    // strike: ink across the middle of cell 1 (foreground, grey-blue)
    let mid = px(&img, w, m.cell_w + 8, m.char_top as u32 + m.char_h / 2);
    assert_ne!(mid, CLEAR, "strikethrough");
}

fn fill(grid: &mut Grid, text: &str, row: usize) {
    for (c, ch) in text.chars().enumerate() {
        grid.set(row, c, cell(|x| x.set_char(ch)));
    }
}

/// Persistent buffers: after row edits, buffer growth, a resize and an atlas reset, the
/// incrementally updated view must draw exactly what a fresh view draws for the same grid.
#[test]
fn incremental_updates_match_a_fresh_view() {
    let Some((device, queue, _)) = common::device() else {
        return;
    };
    let (mut fs, fonts) = test_fonts();
    let theme = TermTheme::lunegit_one_dark();
    let mut tv = TermView::new(&device, FORMAT, fonts.clone(), theme.clone());
    let mut grid = Grid::new(24, 4);
    let metrics = tv.metrics(&mut fs, 2.0);
    let size = (30 * metrics.cell_w, 6 * metrics.cell_h);
    let fresh = |fs: &mut FontSystem, grid: &mut Grid, v: &ViewState<'_>| {
        let mut other = TermView::new(&device, FORMAT, fonts.clone(), theme.clone());
        other.update(fs, grid, v, (4.0, 2.0), 2.0);
        // `update` cleared the dirty flags; the caller's view must still see its own changes
        draw(&device, &queue, &mut other, size, 2.0)
    };
    let v = view();
    fill(&mut grid, "abc", 0);
    tv.update(&mut fs, &mut grid, &v, (4.0, 2.0), 2.0);
    draw(&device, &queue, &mut tv, size, 2.0);

    // a long row: more instances than the initial per-row slot
    fill(&mut grid, "The quick brown fox jumps", 1);
    grid.set(1, 3, cell(|c| c.bg = Color::Rgb(9, 90, 200)));
    tv.update(&mut fs, &mut grid, &v, (4.0, 2.0), 2.0);
    let a = draw(&device, &queue, &mut tv, size, 2.0);
    let mut copy = grid.clone();
    copy.mark_all_dirty();
    assert_eq!(a, fresh(&mut fs, &mut copy, &v), "after growth");

    // edit one row only
    fill(&mut grid, "xyz", 2);
    tv.update(&mut fs, &mut grid, &v, (4.0, 2.0), 2.0);
    let b = draw(&device, &queue, &mut tv, size, 2.0);
    let mut copy = grid.clone();
    copy.mark_all_dirty();
    assert_eq!(b, fresh(&mut fs, &mut copy, &v), "after one dirty row");

    // an atlas reset kills every UV: the next update must re-place all rows
    tv.atlas_mut()
        .core_mut()
        .reset_grow(lune_termview::Page::Mask);
    tv.update(&mut fs, &mut grid, &v, (4.0, 2.0), 2.0);
    let c = draw(&device, &queue, &mut tv, size, 2.0);
    assert_eq!(b, c, "after an atlas reset");

    // resize
    grid.resize(26, 5);
    fill(&mut grid, "0123456789012345678901234", 4);
    tv.update(&mut fs, &mut grid, &v, (4.0, 2.0), 2.0);
    let d = draw(&device, &queue, &mut tv, size, 2.0);
    let mut copy = grid.clone();
    copy.mark_all_dirty();
    assert_eq!(d, fresh(&mut fs, &mut copy, &v), "after resize");
}

/// A grid with a background, a glyph, an underline and a cursor, so every pipeline draws: the
/// rects, the mask glyphs, the decorations and the overlay.
fn busy_grid() -> (Grid, ViewState<'static>) {
    let mut grid = Grid::new(4, 2);
    grid.set(0, 0, cell(|c| c.bg = Color::Rgb(200, 0, 0)));
    grid.set(
        0,
        1,
        cell(|c| {
            c.set_char('W');
            c.underline = UnderlineStyle::Curly;
            c.underline_color = Color::Rgb(255, 0, 255);
        }),
    );
    let mut v = view();
    v.cursor = Cursor {
        visible: true,
        row: 1,
        col: 2,
        ..Cursor::default()
    };
    (grid, v)
}

/// The grid at `opacity`, drawn over the clear colour.
fn drawn_at(opacity: Option<f32>) -> (Vec<u8>, u32) {
    let (device, queue, _) = common::device().expect("a device (checked by the caller)");
    let (mut fs, fonts) = test_fonts();
    let mut tv = TermView::new(&device, FORMAT, fonts, TermTheme::lunegit_one_dark());
    if let Some(o) = opacity {
        tv.set_opacity(o);
    }
    let (mut grid, v) = busy_grid();
    tv.update(&mut fs, &mut grid, &v, (0.0, 0.0), 2.0);
    let m = tv.metrics(&mut fs, 2.0);
    let (w, h) = (4 * m.cell_w, 2 * m.cell_h);
    (draw(&device, &queue, &mut tv, (w, h), 2.0), w)
}

#[test]
fn opacity_scales_every_pipeline_and_a_default_view_is_opaque() {
    if common::device().is_none() {
        return;
    }
    let (full, w) = drawn_at(None);
    let (one, _) = drawn_at(Some(1.0));
    assert_eq!(full, one, "the default is opacity 1");
    // Something other than the clear colour is drawn, so the comparisons below mean something.
    let inked = full
        .chunks_exact(4)
        .filter(|p| p[..3] != CLEAR[..3])
        .count();
    assert!(inked > 200, "{inked} inked pixels");
    // Half: every pixel is the clear colour moved half way to the opaque one (premultiplied
    // colours scale with the opacity, so the blend is linear in it).
    let (half, _) = drawn_at(Some(0.5));
    let mut worst = 0u8;
    for (i, (h, f)) in half.chunks_exact(4).zip(full.chunks_exact(4)).enumerate() {
        for k in 0..3 {
            let want = CLEAR[k] as f32 + (f[k] as f32 - CLEAR[k] as f32) * 0.5;
            let d = (h[k] as f32 - want).abs().round() as u8;
            worst = worst.max(d);
            assert!(d <= 2, "pixel {i} channel {k}: {} vs {want}", h[k]);
        }
    }
    assert!(worst <= 2);
    // The red cell's centre, by hand: 200 * 0.5 + 10 * 0.5 = 105.
    let c = px(&half, w, 8, 17);
    assert!(
        c.iter()
            .zip([105u8, 10, 15, 255])
            .all(|(a, b)| a.abs_diff(b) <= 2),
        "{c:?}"
    );
    // Zero draws nothing at all; out of range and NaN are the clamp's.
    let (none, _) = drawn_at(Some(0.0));
    assert!(none.chunks_exact(4).all(|p| p == CLEAR), "opacity 0");
    let (over, _) = drawn_at(Some(3.0));
    assert_eq!(over, full, "above 1 is 1");
    let (nan, _) = drawn_at(Some(f32::NAN));
    assert_eq!(nan, full, "NaN is 1");
}

/// A rig of its own: the other cases' fallback stays the mono faces only.
fn colour_fonts() -> (FontSystem, TermFonts) {
    let (mut fs, mut fonts) = test_fonts();
    let id = fs
        .db_mut()
        .load_font_source(fontdb::Source::Binary(Arc::new(TWEMOJI)));
    let family = fs.db().face(id[0]).expect("face").families[0].0.clone();
    fonts.emoji_family = Some(family);
    (fs, fonts)
}

/// One colour glyph, alone in the grid, at `opacity`.
fn colour_glyph_at(opacity: f32) -> Vec<u8> {
    let (device, queue, _) = common::device().expect("a device (checked by the caller)");
    let (mut fs, fonts) = colour_fonts();
    let mut tv = TermView::new(&device, FORMAT, fonts, TermTheme::lunegit_one_dark());
    tv.set_opacity(opacity);
    let mut grid = Grid::new(4, 1);
    grid.set(
        0,
        0,
        cell(|c| {
            c.set_char('\u{1F600}');
            c.width = 2;
        }),
    );
    grid.set(0, 1, cell(|c| c.width = 0));
    tv.update(&mut fs, &mut grid, &view(), (0.0, 0.0), 2.0);
    let m = tv.metrics(&mut fs, 2.0);
    draw(&device, &queue, &mut tv, (4 * m.cell_w, m.cell_h), 2.0)
}

/// The colour-glyph path (`fs_glyph_color`): the page already holds premultiplied colour, so the
/// opacity is applied in its own place. A colour glyph has channels a mask glyph's single tint
/// cannot, which is what proves this path drew it.
#[test]
fn opacity_scales_a_colour_glyph() {
    if common::device().is_none() {
        return;
    }
    let full = colour_glyph_at(1.0);
    let inked = full
        .chunks_exact(4)
        .filter(|p| {
            let spread = |a: u8, b: u8| a.abs_diff(b) > 40;
            p[..3] != CLEAR[..3] && (spread(p[0], p[2]) || spread(p[1], p[2]))
        })
        .count();
    assert!(inked > 50, "{inked} coloured pixels: no colour glyph drawn");
    let half = colour_glyph_at(0.5);
    for (i, (h, f)) in half.chunks_exact(4).zip(full.chunks_exact(4)).enumerate() {
        for k in 0..3 {
            let want = CLEAR[k] as f32 + (f[k] as f32 - CLEAR[k] as f32) * 0.5;
            assert!(
                (h[k] as f32 - want).abs() <= 2.0,
                "pixel {i} channel {k}: {} vs {want}",
                h[k]
            );
        }
    }
    let none = colour_glyph_at(0.0);
    assert!(none.chunks_exact(4).all(|p| p == CLEAR), "opacity 0");
}
