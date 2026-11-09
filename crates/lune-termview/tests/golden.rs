//! Golden PNG tests for `TermView` (plan step 6).
//!
//! Each scene is rendered offscreen with the committed test-font subsets (`tests/fonts`, no
//! system fonts) and compared with `tests/goldens/<scene>@<s>x.png`. The grid sits 8 logical px
//! (16 px at 2x) inside a stand-in "glass" target, so ink outside the grid shows up.
//!
//! * Compare rule: fail if any channel differs by more than 16, or if more than 0.1% of the
//!   pixels differ by more than 2. A failure writes `target/termview-goldens/<name>.actual.png`
//!   and `.diff.png`.
//! * `LUNE_TERMVIEW_BLESS=1` writes the goldens instead of comparing. A missing golden fails.
//! * Without an adapter the GPU tests print `SKIPPED`; `LUNE_REQUIRE_GPU=1` makes that a failure.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{FontSystem, fontdb};
use lune_termview::{
    Attrs, Cell, CellMetrics, Color, Cursor, CursorShape, Grid, SelectionKind, SelectionRange,
    TermFonts, TermTheme, TermView, UnderlineStyle, ViewState, cell_metrics,
};
use lune_ui::custom::{CustomDraw, CustomFrame};

const REGULAR: &[u8] = include_bytes!("fonts/RznMono-Regular.subset.ttf");
const BOLD: &[u8] = include_bytes!("fonts/RznMono-Bold.subset.ttf");
const ITALIC: &[u8] = include_bytes!("fonts/RznMono-Italic.subset.ttf");
const BOLD_ITALIC: &[u8] = include_bytes!("fonts/RznMono-BoldItalic.subset.ttf");
// Private-use icons only (U+F000-F02F, U+F07B): the app's fallback face for the Nerd icons
// RznMono does not have. A Latin character missing from the RznMono subset shows as tofu here and
// cannot pass quietly through this face.
const NERD_REGULAR: &[u8] = include_bytes!("fonts/RobotoMonoNerdFontMono-Regular.pua.subset.ttf");
const NERD_BOLD: &[u8] = include_bytes!("fonts/RobotoMonoNerdFontMono-Bold.pua.subset.ttf");
const SYMBOLS: &[u8] = include_bytes!("fonts/NotoSansSymbols2.subset.ttf");
const TWEMOJI: &[u8] = include_bytes!("fonts/TwemojiLunegit.subset.ttf");

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// A glass stand-in, distinct from the theme background (#1E1E1E).
const STAND_IN: [u8; 4] = [0x2A, 0x2D, 0x33, 0xFF];
const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];
/// Logical px of clear space around every grid (16 px at 2x).
const MARGIN: f32 = 8.0;

// ---------------------------------------------------------------- images and comparison

#[derive(Clone)]
struct Image {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

fn actual_dir() -> PathBuf {
    // <target dir>/termview-goldens; CARGO_TARGET_TMPDIR is <target dir>/tmp and honours CARGO_TARGET_DIR
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("../termview-goldens")
}

fn bless_mode() -> bool {
    std::env::var("LUNE_TERMVIEW_BLESS").is_ok_and(|v| v == "1")
}

fn read_png(path: &std::path::Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{}: expected RGBA8, got {:?} {:?}",
            path.display(),
            info.color_type,
            info.bit_depth
        ));
    }
    buf.truncate(info.buffer_size());
    Ok(Image {
        w: info.width,
        h: info.height,
        rgba: buf,
    })
}

struct Mismatch {
    message: String,
    diff: Option<Image>,
}

/// The compare rule. `Ok` when within tolerance.
fn compare(golden: &Image, actual: &Image) -> Result<(), Mismatch> {
    if (golden.w, golden.h) != (actual.w, actual.h) {
        return Err(Mismatch {
            message: format!(
                "size differs: golden {}x{}, actual {}x{}",
                golden.w, golden.h, actual.w, actual.h
            ),
            diff: None,
        });
    }
    let total = (golden.w as usize) * (golden.h as usize);
    let mut max_delta = 0u8;
    let mut over2 = 0usize;
    let mut diff = vec![0u8; golden.rgba.len()];
    for (i, (g, a)) in golden
        .rgba
        .chunks_exact(4)
        .zip(actual.rgba.chunks_exact(4))
        .enumerate()
    {
        let d = (0..4).map(|k| g[k].abs_diff(a[k])).max().unwrap_or(0);
        max_delta = max_delta.max(d);
        if d > 2 {
            over2 += 1;
        }
        // black = identical (the actual image dimmed to a quarter), red ramp = difference
        let o = &mut diff[i * 4..i * 4 + 4];
        if d == 0 {
            o.copy_from_slice(&[a[0] / 4, a[1] / 4, a[2] / 4, 255]);
        } else {
            o.copy_from_slice(&[(80 + u32::from(d) * 3).min(255) as u8, 0, 0, 255]);
        }
    }
    let frac = over2 as f64 / total.max(1) as f64;
    if max_delta > 16 || frac > 0.001 {
        return Err(Mismatch {
            message: format!(
                "max channel delta {max_delta} (limit 16), {over2} of {total} pixels differ by \
                 more than 2 ({:.4}%, limit 0.1%)",
                frac * 100.0
            ),
            diff: Some(Image {
                w: golden.w,
                h: golden.h,
                rgba: diff,
            }),
        });
    }
    Ok(())
}

fn write_actual(name: &str, actual: &Image, diff: Option<&Image>) -> Result<(), String> {
    let dir = actual_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    lune_ui::dump::write_png(
        &dir.join(format!("{name}.actual.png")),
        (actual.w, actual.h),
        &actual.rgba,
    )?;
    if let Some(d) = diff {
        lune_ui::dump::write_png(&dir.join(format!("{name}.diff.png")), (d.w, d.h), &d.rgba)?;
    }
    Ok(())
}

/// Compares against the golden file; a missing or unreadable golden is an error.
fn check_against(name: &str, actual: &Image) -> Result<(), String> {
    let path = golden_dir().join(format!("{name}.png"));
    let golden = read_png(&path).map_err(|e| {
        format!("golden missing or unreadable ({e}); bless with LUNE_TERMVIEW_BLESS=1")
    })?;
    compare(&golden, actual).map_err(|m| {
        let written = write_actual(name, actual, m.diff.as_ref());
        format!(
            "{name}: {} (actual and diff in {}{})",
            m.message,
            actual_dir().display(),
            written
                .err()
                .map(|e| format!("; write failed: {e}"))
                .unwrap_or_default()
        )
    })
}

fn golden(name: &str, frame: &Frame) {
    println!(
        "{name}: image {}x{}, cell {}x{} (char_h {}, char_top {}, baseline {})",
        frame.image.w,
        frame.image.h,
        frame.metrics.cell_w,
        frame.metrics.cell_h,
        frame.metrics.char_h,
        frame.metrics.char_top,
        frame.metrics.baseline
    );
    if bless_mode() {
        let dir = golden_dir();
        std::fs::create_dir_all(&dir).expect("create goldens dir");
        lune_ui::dump::write_png(
            &dir.join(format!("{name}.png")),
            (frame.image.w, frame.image.h),
            &frame.image.rgba,
        )
        .expect("write golden");
        println!("{name}: BLESSED");
        return;
    }
    if let Err(e) = check_against(name, &frame.image) {
        panic!("{e}");
    }
}

// ---------------------------------------------------------------- fonts and rendering

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fs: FontSystem,
    fonts: TermFonts,
}

fn family_of(db: &fontdb::Database, id: fontdb::ID) -> String {
    db.face(id)
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
        .expect("test font has a family name")
}

/// Loads the test fonts into `db`; returns (mono family, emoji family).
fn load_test_fonts(db: &mut fontdb::Database) -> (String, String) {
    let mut load = |bytes: &'static [u8]| {
        let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
        ids[0]
    };
    let mono = load(REGULAR);
    load(BOLD);
    load(ITALIC);
    load(BOLD_ITALIC);
    load(NERD_REGULAR);
    load(NERD_BOLD);
    load(SYMBOLS);
    let emoji = load(TWEMOJI);
    (family_of(db, mono), family_of(db, emoji))
}

fn term_fonts(family: String, emoji: String) -> TermFonts {
    TermFonts {
        family,
        size: 13.0,
        line_height: 17.0,
        emoji_family: Some(emoji),
    }
}

/// Only the committed test fonts: results do not depend on what is installed.
fn test_fonts() -> (FontSystem, TermFonts) {
    let mut db = fontdb::Database::new();
    let (family, emoji) = load_test_fonts(&mut db);
    (
        FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
        term_fonts(family, emoji),
    )
}

/// System fonts plus the test fonts (for fallback and the containment invariant).
fn system_fonts() -> (FontSystem, TermFonts) {
    let mut fs = FontSystem::new();
    let (family, test_emoji) = load_test_fonts(fs.db_mut());
    let emoji = if cfg!(target_os = "macos") {
        "Apple Color Emoji".to_owned()
    } else {
        test_emoji
    };
    (fs, term_fonts(family, emoji))
}

fn ctx(fonts: fn() -> (FontSystem, TermFonts)) -> Option<Ctx> {
    let (device, queue, _) = common::device()?;
    let (fs, fonts) = fonts();
    Some(Ctx {
        device,
        queue,
        fs,
        fonts,
    })
}

struct Frame {
    image: Image,
    metrics: CellMetrics,
}

/// One scene: the grid plus what the app supplies per frame.
struct Scene {
    grid: Grid,
    cursor: Cursor,
    focused: bool,
    blink_on: bool,
    selection: Vec<SelectionRange>,
    hover_link: u32,
}

impl Scene {
    fn new(cols: usize, rows: usize) -> Scene {
        Scene {
            grid: Grid::new(cols, rows),
            cursor: Cursor {
                visible: false,
                ..Cursor::default()
            },
            focused: true,
            blink_on: true,
            selection: Vec::new(),
            hover_link: 0,
        }
    }
}

fn clear_colour(c: [u8; 4]) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(c[0]) / 255.0,
        g: f64::from(c[1]) / 255.0,
        b: f64::from(c[2]) / 255.0,
        a: f64::from(c[3]) / 255.0,
    }
}

/// Renders `scene` with the one-dark theme into a target cleared to `clear`.
fn render(cx: &mut Ctx, scene: &mut Scene, scale: f32, clear: [u8; 4]) -> Frame {
    let mut tv = TermView::new(
        &cx.device,
        FORMAT,
        cx.fonts.clone(),
        TermTheme::lunegit_one_dark(),
    );
    let view = ViewState {
        cursor: scene.cursor,
        focused: scene.focused,
        blink_on: scene.blink_on,
        selection: &scene.selection,
        hover_link: scene.hover_link,
    };
    tv.update(&mut cx.fs, &mut scene.grid, &view, (MARGIN, MARGIN), scale);
    let m = tv.metrics(&mut cx.fs, scale);
    let margin = (MARGIN * scale).round() as u32;
    let size = (
        scene.grid.cols() as u32 * m.cell_w + 2 * margin,
        scene.grid.rows() as u32 * m.cell_h + 2 * margin,
    );
    let target = lune_ui::dump::target(&cx.device, FORMAT, size);
    let frame = CustomFrame {
        size,
        scale,
        format: FORMAT,
    };
    tv.prepare(&cx.device, &cx.queue, &frame);
    let tex_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = cx.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("termview golden"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tex_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_colour(clear)),
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
    cx.queue.submit(Some(enc.finish()));
    let rgba = lune_ui::dump::read_rgba(&cx.device, &cx.queue, &target).expect("readback");
    Frame {
        image: Image {
            w: size.0,
            h: size.1,
            rgba,
        },
        metrics: m,
    }
}

/// Tiles equally sized frames, `per_row` to a row, left to right then top to bottom.
fn tile(frames: &[Frame], per_row: usize) -> Frame {
    let first = &frames[0];
    let (w, h) = (first.image.w as usize, first.image.h as usize);
    let rows = frames.len().div_ceil(per_row);
    let (tw, th) = (w * per_row, h * rows);
    let mut rgba = vec![0u8; tw * th * 4];
    for (i, f) in frames.iter().enumerate() {
        let (ox, oy) = ((i % per_row) * w, (i / per_row) * h);
        for y in 0..h {
            let src = &f.image.rgba[y * w * 4..(y + 1) * w * 4];
            let dst = ((oy + y) * tw + ox) * 4;
            rgba[dst..dst + w * 4].copy_from_slice(src);
        }
    }
    Frame {
        image: Image {
            w: tw as u32,
            h: th as u32,
            rgba,
        },
        metrics: first.metrics,
    }
}

fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.w + x) * 4) as usize;
    [
        img.rgba[i],
        img.rgba[i + 1],
        img.rgba[i + 2],
        img.rgba[i + 3],
    ]
}

// ---------------------------------------------------------------- grid-building helpers

#[derive(Clone, Copy)]
struct Style {
    fg: Color,
    bg: Color,
    attrs: Attrs,
    ul: UnderlineStyle,
    ulc: Color,
    link: u32,
}

fn s() -> Style {
    Style {
        fg: Color::Default,
        bg: Color::Default,
        attrs: Attrs::NONE,
        ul: UnderlineStyle::None,
        ulc: Color::Default,
        link: 0,
    }
}

impl Style {
    fn fg(mut self, c: Color) -> Style {
        self.fg = c;
        self
    }
    fn bg(mut self, c: Color) -> Style {
        self.bg = c;
        self
    }
    fn attr(mut self, a: Attrs) -> Style {
        self.attrs |= a;
        self
    }
    fn ul(mut self, u: UnderlineStyle) -> Style {
        self.ul = u;
        self
    }
    fn ulc(mut self, c: Color) -> Style {
        self.ulc = c;
        self
    }
    fn link(mut self, id: u32) -> Style {
        self.link = id;
        self
    }
}

fn pal(n: u8) -> Color {
    Color::Palette(n)
}

fn mk(text: &str, width: u8, st: &Style) -> Cell {
    let mut c = Cell::default();
    c.set_text(text);
    c.width = width;
    c.fg = st.fg;
    c.bg = st.bg;
    c.attrs = st.attrs;
    c.underline = st.ul;
    c.underline_color = st.ulc;
    c.link = st.link;
    c
}

/// One cluster of `width` columns (2 = leading cell + spacer); returns the next column.
fn cluster(g: &mut Grid, row: usize, col: usize, text: &str, width: u8, st: Style) -> usize {
    assert!(
        g.set(row, col, mk(text, width, &st)),
        "scene cell ({row},{col}) out of grid"
    );
    if width == 2 {
        assert!(
            g.set(row, col + 1, mk("", 0, &st)),
            "spacer ({row},{}) out of grid",
            col + 1
        );
    }
    col + usize::from(width.max(1))
}

/// Each char in its own width-1 cell; returns the next column.
fn put(g: &mut Grid, row: usize, mut col: usize, text: &str, st: Style) -> usize {
    for ch in text.chars() {
        col = cluster(g, row, col, &ch.to_string(), 1, st);
    }
    col
}

/// `n` blank cells with the style's background.
fn blanks(g: &mut Grid, row: usize, col: usize, n: usize, st: Style) -> usize {
    for i in 0..n {
        assert!(
            g.set(row, col + i, mk("", 1, &st)),
            "blank ({row},{}) out of grid",
            col + i
        );
    }
    col + n
}

// ---------------------------------------------------------------- scenes

const SAMPLE: &str = "Quick fox 0Oo1lI gjpqy";
const LABEL_W: usize = 12;

fn line(g: &mut Grid, row: &mut usize, label: &str, st: Style) {
    put(g, *row, 0, label, s());
    put(g, *row, LABEL_W, SAMPLE, st);
    *row += 1;
}

fn attributes_scene(blink_on: bool) -> Scene {
    use UnderlineStyle as U;
    let mut sc = Scene::new(38, 34);
    sc.blink_on = blink_on;
    let g = &mut sc.grid;
    let mut r = 0;
    // weights and slants
    line(g, &mut r, "normal", s());
    line(g, &mut r, "bold", s().attr(Attrs::BOLD));
    line(g, &mut r, "dim", s().attr(Attrs::DIM));
    line(g, &mut r, "italic", s().attr(Attrs::ITALIC));
    line(
        g,
        &mut r,
        "bold ital",
        s().attr(Attrs::BOLD | Attrs::ITALIC),
    );
    line(g, &mut r, "p7", s().fg(pal(7)));
    line(g, &mut r, "bold p7", s().fg(pal(7)).attr(Attrs::BOLD));
    r += 1;
    // the five underline styles
    line(g, &mut r, "single", s().ul(U::Single));
    line(g, &mut r, "double", s().ul(U::Double));
    line(g, &mut r, "curly", s().ul(U::Curly));
    line(g, &mut r, "dotted", s().ul(U::Dotted));
    line(g, &mut r, "dashed", s().ul(U::Dashed));
    r += 1;
    // underline colour
    line(
        g,
        &mut r,
        "ul red",
        s().ul(U::Single).ulc(Color::Rgb(255, 0, 0)),
    );
    line(
        g,
        &mut r,
        "ul grn cur",
        s().ul(U::Curly).ulc(Color::Rgb(0, 220, 0)),
    );
    line(
        g,
        &mut r,
        "ul blu dbl",
        s().fg(pal(3)).ul(U::Double).ulc(Color::Rgb(80, 120, 255)),
    );
    r += 1;
    // strikethrough and overline
    line(g, &mut r, "strike", s().attr(Attrs::STRIKETHROUGH));
    line(g, &mut r, "overline", s().attr(Attrs::OVERLINE));
    line(
        g,
        &mut r,
        "all three",
        s().attr(Attrs::STRIKETHROUGH | Attrs::OVERLINE)
            .ul(U::Single),
    );
    r += 1;
    // inverse
    line(g, &mut r, "inverse", s().attr(Attrs::INVERSE));
    line(
        g,
        &mut r,
        "inv colour",
        s().fg(pal(2)).bg(pal(4)).attr(Attrs::INVERSE),
    );
    line(
        g,
        &mut r,
        "inv bold",
        s().attr(Attrs::INVERSE | Attrs::BOLD),
    );
    line(g, &mut r, "bg only", s().bg(pal(1)));
    r += 1;
    // invisible and blink
    line(
        g,
        &mut r,
        "invisible",
        s().bg(pal(1)).attr(Attrs::INVISIBLE).ul(U::Single),
    );
    line(
        g,
        &mut r,
        "blink",
        s().attr(Attrs::BLINK).ul(U::Single).bg(pal(5)),
    );
    line(
        g,
        &mut r,
        "blink+inv",
        s().attr(Attrs::BLINK | Attrs::INVERSE),
    );
    r += 1;
    // combinations
    line(
        g,
        &mut r,
        "b+i+curly",
        s().fg(pal(1))
            .attr(Attrs::BOLD | Attrs::ITALIC | Attrs::STRIKETHROUGH)
            .ul(U::Curly)
            .ulc(Color::Rgb(255, 200, 0)),
    );
    line(
        g,
        &mut r,
        "dim+ul",
        s().attr(Attrs::DIM).ul(U::Double).fg(pal(6)),
    );
    line(
        g,
        &mut r,
        "inv+dim+st",
        s().attr(Attrs::INVERSE | Attrs::DIM | Attrs::STRIKETHROUGH)
            .fg(pal(3)),
    );
    sc
}

fn colours_scene() -> Scene {
    let mut sc = Scene::new(34, 32);
    let g = &mut sc.grid;
    // row 0: the 16 bare backgrounds (probed by the harness)
    for b in 0..16u8 {
        blanks(g, 0, usize::from(b), 1, s().bg(pal(b)));
    }
    // rows 2-17: 16 fg x 16 bg
    for f in 0..16u8 {
        for b in 0..16u8 {
            cluster(
                g,
                2 + usize::from(f),
                usize::from(b),
                "A",
                1,
                s().fg(pal(f)).bg(pal(b)),
            );
        }
    }
    // rows 19-26: the 256 colours as backgrounds
    for i in 0..=255u8 {
        blanks(
            g,
            19 + usize::from(i) / 32,
            usize::from(i) % 32,
            1,
            s().bg(pal(i)),
        );
    }
    // rows 28-29: RGB ramps
    for i in 0..32usize {
        let v = (i * 255 / 31) as u8;
        blanks(g, 28, i, 1, s().bg(Color::Rgb(v, 0, 255 - v)));
        blanks(g, 29, i, 1, s().bg(Color::Rgb(0, v, 255 - v)));
    }
    // row 31: default bg, explicit #1E1E1E (skipped), #1F1E1E (painted), palette 0
    put(g, 31, 0, "dflt", s());
    blanks(g, 31, 4, 4, s().bg(Color::Rgb(0x1E, 0x1E, 0x1E)));
    blanks(g, 31, 8, 4, s().bg(Color::Rgb(0x1F, 0x1E, 0x1E)));
    blanks(g, 31, 12, 4, s().bg(pal(0)));
    sc
}

/// Twelve one-row panels, three to a row (see the legend for the order).
fn cursor_panels() -> Vec<Scene> {
    let wide = |shape: CursorShape, focused: bool, col: usize| {
        let mut sc = Scene::new(6, 1);
        put(&mut sc.grid, 0, 0, "Wg", s());
        cluster(&mut sc.grid, 0, 2, "\u{1F3A8}", 2, s());
        put(&mut sc.grid, 0, 4, "ab", s());
        sc.cursor = Cursor {
            col,
            shape,
            visible: true,
            ..Cursor::default()
        };
        sc.focused = focused;
        sc
    };
    let plain = |shape: CursorShape, focused: bool, blink: bool, blink_on: bool| {
        let mut sc = Scene::new(6, 1);
        put(&mut sc.grid, 0, 0, "Wgqjab", s());
        sc.cursor = Cursor {
            col: 2,
            shape,
            blink,
            visible: true,
            ..Cursor::default()
        };
        sc.focused = focused;
        sc.blink_on = blink_on;
        sc
    };
    let mut on_bg = plain(CursorShape::Block, true, false, true);
    on_bg.grid.set(0, 2, mk("q", 1, &s().bg(pal(1))));
    vec![
        plain(CursorShape::Block, true, false, true),
        plain(CursorShape::Underline, true, false, true),
        plain(CursorShape::Bar, true, false, true),
        plain(CursorShape::Block, false, false, true),
        plain(CursorShape::Underline, false, false, true),
        plain(CursorShape::Bar, false, false, true),
        wide(CursorShape::Block, true, 2),
        wide(CursorShape::Block, false, 2),
        wide(CursorShape::Block, true, 3),
        plain(CursorShape::Block, true, true, false),
        plain(CursorShape::Block, true, true, true),
        on_bg,
    ]
}

const PROSE: [&str; 3] = [
    "The quick brown fox jumps over",
    "the lazy dog 0123456789 and so",
    "on and on until the line ends.",
];

fn selection_scene() -> Scene {
    let mut sc = Scene::new(30, 10);
    let g = &mut sc.grid;
    for (i, t) in PROSE.iter().enumerate() {
        put(g, i, 0, t, s());
        put(g, 4 + i, 0, t, s());
    }
    // row 8: wide cells; row 9: backgrounds
    let mut c = put(g, 8, 0, "ab", s());
    for e in ["\u{1F3A8}", "\u{2705}", "\u{1F44D}\u{1F3FD}"] {
        c = cluster(g, 8, c, e, 2, s());
    }
    put(g, 8, c, "cd", s());
    put(g, 9, 0, "ab", s());
    blanks(g, 9, 2, 8, s().bg(pal(1)));
    put(g, 9, 10, "cd", s());
    let sel = |a, b, kind| SelectionRange {
        start: a,
        end: b,
        kind,
    };
    sc.selection = vec![
        sel((0, 8), (2, 10), SelectionKind::Linear),
        sel((6, 12), (4, 5), SelectionKind::Block),
        sel((8, 1), (8, 6), SelectionKind::Linear),
        sel((9, 3), (9, 8), SelectionKind::Linear),
    ];
    sc
}

fn emoji_scene() -> Scene {
    let mut sc = Scene::new(24, 8);
    let g = &mut sc.grid;
    let wide_row = |g: &mut Grid, row: usize, st: Style, label: &str| {
        put(g, row, 0, label, s());
        let mut c = 8;
        for e in [
            "\u{1F3A8}",
            "\u{2705}",
            "\u{1F44D}\u{1F3FD}",
            "\u{26A0}\u{FE0F}",
        ] {
            c = cluster(g, row, c, e, 2, st);
        }
        put(g, row, c, "|", s());
    };
    wide_row(g, 0, s(), "w2");
    // width 1: followed by a non-blank cell (no spill allowance) and by a blank one
    let narrow_row = |g: &mut Grid, row: usize, next: &str, st: Style, label: &str| {
        put(g, row, 0, label, s());
        let mut c = 8;
        for e in ["\u{23FA}", "\u{1F600}"] {
            c = cluster(g, row, c, e, 1, st);
            c = put(g, row, c, next, st);
        }
        put(g, row, c, "|", s());
    };
    narrow_row(g, 2, "x", s(), "w1 x");
    narrow_row(g, 3, " ", s(), "w1 _");
    wide_row(g, 5, s().bg(pal(4)), "w2 bg");
    narrow_row(g, 6, "x", s().bg(pal(4)), "w1 x bg");
    narrow_row(g, 7, " ", s().bg(pal(4)), "w1 _ bg");
    sc
}

fn combining_scene() -> Scene {
    let mut sc = Scene::new(20, 7);
    let g = &mut sc.grid;
    let rows: [(&str, String); 4] = [
        ("e+0301", "e\u{0301}".to_owned()),
        ("a+0308", "a\u{0308}".to_owned()),
        ("stack", "a\u{0301}\u{0303}\u{0309}\u{0323}".to_owned()),
        ("flood 200", format!("a{}", "\u{0301}".repeat(200))),
    ];
    for (i, (label, text)) in rows.iter().enumerate() {
        let row = i * 2;
        put(g, row, 0, label, s());
        let c = put(g, row, LABEL_W, "x", s());
        let c = cluster(g, row, c, text, 1, s());
        put(g, row, c, "x", s());
    }
    sc
}

fn box_drawing_scene() -> Scene {
    let mut sc = Scene::new(20, 14);
    let g = &mut sc.grid;
    for i in 0..128u32 {
        let ch = char::from_u32(0x2500 + i).unwrap_or(' ');
        cluster(
            g,
            (i / 16) as usize,
            (i % 16) as usize,
            &ch.to_string(),
            1,
            s(),
        );
    }
    // rounded box
    put(
        g,
        9,
        0,
        "\u{256D}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256E}",
        s(),
    );
    for r in 10..13 {
        put(g, r, 0, "\u{2502}", s());
        put(g, r, 7, "\u{2502}", s());
    }
    put(
        g,
        13,
        0,
        "\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256F}",
        s(),
    );
    // five stacked rows of verticals
    for (col, ch) in [
        (10, "\u{2502}"),
        (12, "\u{2503}"),
        (14, "\u{2551}"),
        (16, "\u{254E}"),
        (18, "\u{254F}"),
    ] {
        for r in 9..14 {
            put(g, r, col, ch, s());
        }
    }
    sc
}

fn blocks_scene() -> Scene {
    let mut sc = Scene::new(16, 11);
    let g = &mut sc.grid;
    for i in 0..32u32 {
        let ch = char::from_u32(0x2580 + i).unwrap_or(' ').to_string();
        cluster(g, (i / 16) as usize, (i % 16) as usize, &ch, 1, s());
        cluster(
            g,
            3 + (i / 16) as usize,
            (i % 16) as usize,
            &ch,
            1,
            s().fg(pal(1)).bg(pal(4)),
        );
    }
    for (col, ch) in ["\u{2588}", "\u{258C}", "\u{2590}", "\u{2580}", "\u{2584}"]
        .iter()
        .enumerate()
    {
        for r in 6..9 {
            put(g, r, col, ch, s().fg(pal(2)));
        }
    }
    // row 9 stays blank; row 10 is a horizontal run to measure seams across cell boundaries
    for col in 0..8 {
        put(g, 10, col, "\u{2588}", s());
    }
    sc
}

fn nerd_scene() -> Scene {
    let mut sc = Scene::new(24, 9);
    let g = &mut sc.grid;
    let spaced = |g: &mut Grid, row: usize, range: std::ops::RangeInclusive<u32>, st: Style| {
        let mut c = 0;
        for cp in range {
            let ch = char::from_u32(cp).unwrap_or(' ').to_string();
            c = cluster(g, row, c, &ch, 1, st);
            c = cluster(g, row, c, " ", 1, st);
        }
    };
    spaced(g, 0, 0xE0A0..=0xE0A3, s());
    spaced(g, 2, 0xE0B0..=0xE0B3, s());
    spaced(g, 4, 0xE0B0..=0xE0B3, s().fg(pal(4)).bg(pal(2)));
    let mut c = 0;
    for cp in 0xF000..=0xF00Fu32 {
        let ch = char::from_u32(cp).unwrap_or(' ').to_string();
        c = cluster(g, 6, c, &ch, 1, s());
    }
    // a status-line mock: ~/p > main >
    let c = put(g, 8, 0, " ~/p ", s().fg(pal(0)).bg(pal(4)));
    let c = cluster(g, 8, c, "\u{E0B0}", 1, s().fg(pal(4)).bg(pal(2)));
    let c = put(g, 8, c, " \u{E0A0} main ", s().fg(pal(0)).bg(pal(2)));
    cluster(g, 8, c, "\u{E0B0}", 1, s().fg(pal(2)));
    sc
}

/// The fallback proof (M10 step 11, D9a): Nerd icons are not in RznMono and draw from the
/// bundled RobotoMono Nerd face. `A`, icon, `B` have non-blank neighbours, so an icon that
/// overflowed its cell would show as ink on the letters or past the slot.
fn nerd_fallback_scene() -> Scene {
    let mut sc = Scene::new(14, 5);
    let g = &mut sc.grid;
    for (row, st) in [(0, s()), (2, s().attr(Attrs::BOLD))] {
        let mut c = 0;
        for text in [
            "A", "\u{F015}", "B", " ", "A", "\u{F07B}", "B", " ", "A", "\u{F013}", "B",
        ] {
            c = cluster(g, row, c, text, 1, st);
        }
    }
    // the same icons with blanks between
    let mut c = 0;
    for text in ["\u{F015}", " ", "\u{F07B}", " ", "\u{F013}"] {
        c = cluster(g, 4, c, text, 1, s());
    }
    sc
}

/// RznMono's italics overhang the 600 cell (capitals `W` to 658, `X` from -31): the capitals that
/// overhang most, so the cut at the cell edge can be seen. Rows 1 and 3 have non-blank neighbours
/// (no spill allowance), row 5 a blank between each letter. Clip and `fit_scale` are unchanged.
fn italic_overhang_scene() -> Scene {
    const LETTERS: &str = "WTVXK7M";
    let mut sc = Scene::new(16, 7);
    let g = &mut sc.grid;
    put(g, 1, 0, LETTERS, s().attr(Attrs::ITALIC));
    put(g, 3, 0, LETTERS, s().attr(Attrs::BOLD | Attrs::ITALIC));
    let mut c = 0;
    for ch in LETTERS.chars() {
        c = cluster(g, 5, c, &ch.to_string(), 1, s().attr(Attrs::ITALIC));
        c = cluster(g, 5, c, " ", 1, s());
    }
    sc
}

fn links_scene() -> Scene {
    let mut sc = Scene::new(40, 5);
    let g = &mut sc.grid;
    put(g, 0, 0, "link 1 (hover) ", s());
    put(g, 0, 15, "https://example.com/one", s().link(1));
    put(g, 2, 0, "link 2 (none)  ", s());
    put(g, 2, 15, "https://example.com/two", s().link(2));
    put(g, 4, 0, "link 1 again   ", s());
    put(g, 4, 15, "wrapped/path", s().link(1));
    sc.hover_link = 1;
    sc
}

fn overflow_scene() -> Scene {
    let mut sc = Scene::new(20, 8);
    let g = &mut sc.grid;
    let row = |g: &mut Grid, r: usize, next: &str, st: Style, label: &str| {
        put(g, r, 0, label, s());
        let mut c = 6;
        for e in ["\u{2733}", "\u{2605}", "\u{2733}", "\u{2605}"] {
            c = cluster(g, r, c, e, 1, s());
            c = put(g, r, c, next, st);
            c += 1;
        }
    };
    row(g, 0, "x", s(), "nb");
    row(g, 1, " ", s(), "bl");
    row(g, 3, "x", s().bg(pal(4)), "nb bg");
    row(g, 4, " ", s().bg(pal(4)), "bl bg");
    // the last column has no next cell
    put(g, 6, 0, "last column", s());
    cluster(g, 6, 19, "\u{2733}", 1, s());
    put(g, 7, 0, "last column", s());
    cluster(g, 7, 19, "\u{2605}", 1, s());
    sc
}

#[cfg(target_os = "macos")]
fn system_fallback_scene() -> Scene {
    let mut sc = Scene::new(24, 9);
    let g = &mut sc.grid;
    put(g, 0, 0, "cjk", s());
    let mut c = 6;
    for e in ["你", "好", "世", "界"] {
        c = cluster(g, 0, c, e, 2, s());
    }
    put(g, 0, c, "|", s());
    put(g, 2, 0, "hangul", s());
    let c = cluster(g, 2, 6, "가", 2, s());
    put(g, 2, c, "|", s());
    put(g, 4, 0, "emoji", s());
    let mut c = 6;
    for e in ["\u{1F3A8}", "\u{1F44D}\u{1F3FD}", "\u{26A0}\u{FE0F}"] {
        c = cluster(g, 4, c, e, 2, s());
    }
    put(g, 4, c, "|", s());
    for (row, next, label) in [(6, "x", "sym x"), (7, " ", "sym _")] {
        put(g, row, 0, label, s());
        let mut c = 6;
        for e in ["\u{2318}", "\u{2713}", "\u{2605}"] {
            c = cluster(g, row, c, e, 1, s());
            c = put(g, row, c, next, s());
            c += 1;
        }
    }
    sc
}

// ---------------------------------------------------------------- tests

fn golden_scene(name: &str, scale: f32, scene: &mut Scene) {
    let Some(mut cx) = ctx(test_fonts) else {
        return;
    };
    let frame = render(&mut cx, scene, scale, STAND_IN);
    golden(&format!("{name}@{}x", scale as u32), &frame);
}

#[test]
fn attributes() {
    golden_scene("attributes", 2.0, &mut attributes_scene(true));
}

#[test]
fn attributes_1x() {
    golden_scene("attributes", 1.0, &mut attributes_scene(true));
}

#[test]
fn attributes_blink_off() {
    golden_scene("attributes-blink-off", 2.0, &mut attributes_scene(false));
}

#[test]
fn colours() {
    let Some(mut cx) = ctx(test_fonts) else {
        return;
    };
    let frame = render(&mut cx, &mut colours_scene(), 2.0, STAND_IN);
    golden("colours@2x", &frame);

    // colour probe: the centre of each bare background cell is the palette value, and an
    // explicit #1E1E1E background paints nothing (the stand-in shows through)
    let m = frame.metrics;
    let margin = (MARGIN * 2.0) as u32;
    let theme = TermTheme::lunegit_one_dark();
    let centre = |col: u32, row: u32| {
        px(
            &frame.image,
            margin + col * m.cell_w + m.cell_w / 2,
            margin + row * m.cell_h + m.cell_h / 2,
        )
    };
    let near = |a: [u8; 4], b: [u8; 4], tol: u8| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol);
    for b in 0..16u32 {
        let want = theme.palette[b as usize];
        let got = centre(b, 0);
        println!("colour probe bg {b:2}: got {got:02X?} want {want:02X?}");
        assert!(near(got, want, 1), "bg {b}: got {got:?} want {want:?}");
    }
    let skipped = centre(5, 31);
    println!("colour probe explicit #1E1E1E: {skipped:02X?}");
    assert_eq!(
        skipped, STAND_IN,
        "explicit #1E1E1E background must not paint"
    );
    let painted = centre(9, 31);
    assert!(
        near(painted, [0x1F, 0x1E, 0x1E, 0xFF], 1),
        "#1F1E1E paints: {painted:?}"
    );
}

#[test]
fn cursors() {
    let Some(mut cx) = ctx(test_fonts) else {
        return;
    };
    let frames: Vec<Frame> = cursor_panels()
        .iter_mut()
        .map(|p| render(&mut cx, p, 2.0, STAND_IN))
        .collect();
    golden("cursors@2x", &tile(&frames, 3));
}

#[test]
fn selection() {
    golden_scene("selection", 2.0, &mut selection_scene());
}

#[test]
fn emoji_sbix() {
    golden_scene("emoji_sbix", 2.0, &mut emoji_scene());
}

#[test]
fn combining() {
    golden_scene("combining", 2.0, &mut combining_scene());
}

/// The 200-mark flood draws its base letter: ink inside the flood cell's slot (batch 1 measured
/// none, because the whole cluster was shrunk to fit its summed advance).
#[test]
fn combining_flood_has_ink() {
    let Some(mut cx) = ctx(test_fonts) else {
        return;
    };
    let f = render(&mut cx, &mut combining_scene(), 2.0, TRANSPARENT);
    let m = f.metrics;
    let margin = (MARGIN * 2.0).round() as u32;
    // the flood is the scene's 4th row (grid row 6), in the cell after the label and one "x"
    let (col, row) = (LABEL_W as u32 + 1, 6);
    let (x0, y0) = (margin + col * m.cell_w, margin + row * m.cell_h);
    let mut ink = 0;
    for y in y0..y0 + m.cell_h {
        for x in x0..x0 + m.cell_w {
            if px(&f.image, x, y)[3] > 0 {
                ink += 1;
            }
        }
    }
    println!("flood cell: {ink} px with alpha > 0");
    assert!(ink > 0, "the flood cell draws nothing");
}

#[test]
fn box_drawing() {
    golden_scene("box_drawing", 2.0, &mut box_drawing_scene());
}

#[test]
fn blocks() {
    golden_scene("blocks", 2.0, &mut blocks_scene());
}

#[test]
fn nerd_powerline() {
    golden_scene("nerd_powerline", 2.0, &mut nerd_scene());
}

#[test]
fn nerd_fallback() {
    golden_scene("nerd_fallback", 2.0, &mut nerd_fallback_scene());
}

#[test]
fn italic_overhang() {
    golden_scene("italic_overhang", 2.0, &mut italic_overhang_scene());
}

#[test]
fn italic_overhang_1x() {
    golden_scene("italic_overhang", 1.0, &mut italic_overhang_scene());
}

#[test]
fn links() {
    golden_scene("links", 2.0, &mut links_scene());
}

#[test]
fn overflow() {
    golden_scene("overflow", 2.0, &mut overflow_scene());
}

#[cfg(target_os = "macos")]
#[test]
fn system_fallback() {
    let Some(mut cx) = ctx(system_fonts) else {
        return;
    };
    let frame = render(&mut cx, &mut system_fallback_scene(), 2.0, STAND_IN);
    golden("system_fallback-macos@2x", &frame);
}

/// D3 numbers for 13 px / line 17 on the RznMono subset (no GPU needed).
#[test]
fn cell_metrics_match_decision_d3() {
    let (mut fs, fonts) = test_fonts();
    let at = |fs: &mut FontSystem, scale| {
        let m = cell_metrics(fs, &fonts, scale);
        (m.cell_w, m.cell_h, m.char_h, m.char_top, m.baseline)
    };
    assert_eq!(at(&mut fs, 2.0), (16, 34, 35, 0, 27));
    assert_eq!(at(&mut fs, 1.0), (8, 17, 18, 0, 14));
}

/// No ink outside a cluster's slot or its row, with non-blank neighbours (so no spill
/// allowance) and system fonts allowed (ledger 1 and 2). Font independent: it measures the
/// pixels, not the placement.
#[test]
fn containment_no_spill() {
    let Some(mut cx) = ctx(system_fonts) else {
        return;
    };
    let flood = format!("a{}", "\u{0301}".repeat(200));
    let clusters = [
        "你",
        "가",
        "\u{1F3A8}",
        "\u{1F44D}\u{1F3FD}",
        "\u{26A0}\u{FE0F}",
        "\u{23FA}",
        "\u{2733}",
        "\u{2605}",
        "\u{2318}",
        "\u{F07B}",
        // procedural glyphs (plan step 6a)
        "\u{2500}",
        "\u{2588}",
        "\u{256D}",
        "\u{E0B0}",
        // marks on a base (plan step 6b)
        "a\u{0301}\u{0303}\u{0309}\u{0323}",
        flood.as_str(),
    ];
    // present in the committed test fonts on every OS, so they must draw something
    let must_ink = [
        "\u{1F3A8}",
        "\u{1F44D}\u{1F3FD}",
        "\u{26A0}\u{FE0F}",
        "\u{23FA}",
        "\u{2733}",
        "\u{2605}",
        "\u{2500}",
        "\u{2588}",
        "\u{256D}",
        "\u{E0B0}",
        "a\u{0301}\u{0303}\u{0309}\u{0323}",
        flood.as_str(),
    ];
    let hidden = s().attr(Attrs::INVISIBLE);
    for scale in [1.0f32, 2.0] {
        for text in clusters {
            for width in [1u8, 2] {
                let mut sc = Scene::new(7, 3);
                for r in 0..3 {
                    for c in 0..7 {
                        sc.grid.set(r, c, mk("X", 1, &hidden));
                    }
                }
                cluster(
                    &mut sc.grid,
                    1,
                    2,
                    text,
                    width,
                    s().fg(pal(2)).bg(Color::Default),
                );
                let f = render(&mut cx, &mut sc, scale, TRANSPARENT);
                let m = f.metrics;
                let margin = (MARGIN * scale).round() as u32;
                let (x0, x1) = (
                    margin + 2 * m.cell_w,
                    margin + (2 + u32::from(width)) * m.cell_w,
                );
                let (y0, y1) = (margin + m.cell_h, margin + 2 * m.cell_h);
                let (mut inside, mut outside) = (0usize, Vec::new());
                for y in 0..f.image.h {
                    for x in 0..f.image.w {
                        if px(&f.image, x, y)[3] == 0 {
                            continue;
                        }
                        if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                            inside += 1;
                        } else if outside.len() < 5 {
                            outside.push((x, y));
                        }
                    }
                }
                println!("containment {text:?} w{width} @{scale}x: {inside} px inside");
                assert!(
                    outside.is_empty(),
                    "{text:?} w{width} @{scale}x: ink outside slot x {x0}..{x1} y {y0}..{y1}, e.g. {outside:?}"
                );
                if must_ink.contains(&text) {
                    assert!(inside > 0, "{text:?} w{width} @{scale}x drew nothing");
                }
            }
        }
    }
}

/// Instrument check: a golden with a 10x10 block altered by +50 must fail the compare and
/// write `.actual.png` and `.diff.png`; a missing golden must fail too.
#[test]
fn compare_detects_tampering_and_missing_goldens() {
    if bless_mode() {
        return; // goldens are being written by other tests right now
    }
    let name = "attributes@2x";
    let path = golden_dir().join(format!("{name}.png"));
    let golden = read_png(&path).expect("golden attributes@2x exists (bless first)");
    assert!(
        compare(&golden, &golden).is_ok(),
        "identical images compare equal"
    );

    let mut tampered = golden.clone();
    for y in 40..50u32 {
        for x in 60..70u32 {
            let i = ((y * tampered.w + x) * 4) as usize;
            for k in 0..3 {
                tampered.rgba[i + k] = tampered.rgba[i + k].saturating_add(50);
            }
        }
    }
    // the 10x10 block is 100 px, far under 0.1%: the per-channel limit is what catches it
    let err = check_against_path_for_test(&golden, &tampered, "instrument-check@2x")
        .expect_err("a +50 block must fail");
    println!("instrument check reported: {err}");
    let dir = actual_dir();
    assert!(dir.join("instrument-check@2x.actual.png").exists());
    assert!(dir.join("instrument-check@2x.diff.png").exists());

    // a few pixels off by 2 stay within tolerance
    let mut slight = golden.clone();
    for i in 0..1000usize {
        slight.rgba[i * 4] = slight.rgba[i * 4].saturating_add(2);
    }
    assert!(
        compare(&golden, &slight).is_ok(),
        "a delta of 2 is tolerated"
    );

    let missing = check_against("no-such-scene@2x", &golden)
        .expect_err("a missing golden must fail, never auto-bless");
    assert!(missing.contains("missing"), "{missing}");
    assert!(!golden_dir().join("no-such-scene@2x.png").exists());
}

fn check_against_path_for_test(golden: &Image, actual: &Image, name: &str) -> Result<(), String> {
    compare(golden, actual).map_err(|m| {
        write_actual(name, actual, m.diff.as_ref()).expect("write actual and diff");
        m.message
    })
}
