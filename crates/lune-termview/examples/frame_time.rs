//! Frame-time measurement for the termview renderer (plan step 8). Dev-only.
//!
//! `cargo run --release -p lune-termview --example frame_time`
//!
//! An offscreen 3840x2160 target at scale 2 holds a dense grid (random printable ASCII, palette
//! and RGB colours, 20% bold, 10% underlined) drawn from a fixed-seed xorshift. After 20 warm-up
//! frames, 200 frames are timed for each case: (a) every row dirty, (b) one dirty row,
//! (c) only the cursor blink toggling. Prints the median and p95 of `update` (CPU) and of
//! `prepare` + `render` + submit + `device.poll(wait)`. Without an adapter it says so and exits 0.

use std::path::Path;
use std::time::Instant;

use cosmic_text::FontSystem;
use lune_termview::{
    Attrs, Cell, Color, Cursor, Grid, TermFonts, TermTheme, TermView, UnderlineStyle, ViewState,
};
use lune_ui::custom::{CustomDraw, CustomFrame};

const SIZE: (u32, u32) = (3840, 2160);
const SCALE: f32 = 2.0;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const WARMUP: usize = 20;
const FRAMES: usize = 200;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn random_color(rng: &mut Rng) -> Color {
    let v = rng.next();
    if v & 1 == 0 {
        Color::Palette(((v >> 8) % 256) as u8)
    } else {
        Color::Rgb((v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8)
    }
}

fn fill_row(grid: &mut Grid, row: usize, rng: &mut Rng) {
    for col in 0..grid.cols() {
        let mut cell = Cell::default();
        cell.set_char(char::from(b' ' + (rng.next() % 95) as u8));
        cell.fg = random_color(rng);
        cell.bg = random_color(rng);
        if rng.next() % 5 == 0 {
            cell.attrs.insert(Attrs::BOLD);
        }
        if rng.next() % 10 == 0 {
            cell.underline = UnderlineStyle::Single;
        }
        grid.set(row, col, cell);
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn summarize(label: &str, mut update: Vec<f64>, mut gpu: Vec<f64>) {
    update.sort_by(f64::total_cmp);
    gpu.sort_by(f64::total_cmp);
    println!(
        "{label:<28} update  median {:7.3} ms  p95 {:7.3} ms | prepare+render+submit+poll  median {:7.3} ms  p95 {:7.3} ms",
        percentile(&update, 0.5),
        percentile(&update, 0.95),
        percentile(&gpu, 0.5),
        percentile(&gpu, 0.95),
    );
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        {
            Ok(a) => a,
            Err(e) => {
                println!("no wgpu adapter ({e}); nothing measured");
                return;
            }
        };
    let info = adapter.get_info();
    println!(
        "adapter: {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
            Ok(dq) => dq,
            Err(e) => {
                println!("no wgpu device ({e}); nothing measured");
                return;
            }
        };

    // The committed RobotoMono subset (it holds every glyph the grid uses), loaded by path.
    let font_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fonts/RobotoMonoNerdFontMono-Regular.subset.ttf");
    let mut fs = FontSystem::new();
    let ids = fs
        .db_mut()
        .load_font_source(cosmic_text::fontdb::Source::File(font_path));
    let family = ids
        .first()
        .and_then(|id| fs.db().face(*id))
        .map(|f| f.families[0].0.clone())
        .unwrap_or_else(|| "RobotoMono Nerd Font Mono".to_owned());
    let fonts = TermFonts {
        family,
        size: 13.0,
        line_height: 17.0,
        emoji_family: None,
    };

    let mut tv = TermView::new(&device, FORMAT, fonts, TermTheme::lunegit_one_dark());
    let m = tv.metrics(&mut fs, SCALE);
    let (cols, rows) = ((SIZE.0 / m.cell_w) as usize, (SIZE.1 / m.cell_h) as usize);
    println!(
        "target {}x{} @{SCALE}x, cell {}x{} px, grid {cols} x {rows} cells",
        SIZE.0, SIZE.1, m.cell_w, m.cell_h
    );

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut grid = Grid::new(cols, rows);
    for r in 0..rows {
        fill_row(&mut grid, r, &mut rng);
    }

    let target = lune_ui::dump::target(&device, FORMAT, SIZE);
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let frame = CustomFrame {
        size: SIZE,
        scale: SCALE,
        format: FORMAT,
    };
    let view_state = |blink_on: bool| ViewState {
        cursor: Cursor {
            row: rows / 2,
            col: cols / 2,
            blink: true,
            ..Cursor::default()
        },
        focused: true,
        blink_on,
        selection: &[],
        hover_link: 0,
    };

    // One frame: update (timed), then prepare + render + submit + poll (timed).
    let mut one = |grid: &mut Grid, tv: &mut TermView, blink_on: bool| -> (f64, f64) {
        let t0 = Instant::now();
        tv.update(&mut fs, grid, &view_state(blink_on), (0.0, 0.0), SCALE);
        let update_ms = t0.elapsed().as_secs_f64() * 1e3;

        let t1 = Instant::now();
        tv.prepare(&device, &queue, &frame);
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame_time"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        (update_ms, t1.elapsed().as_secs_f64() * 1e3)
    };

    // Prime every row (atlas fill, buffer sizing) before the cases.
    let mut blink = true;
    for _ in 0..WARMUP {
        grid.mark_all_dirty();
        one(&mut grid, &mut tv, blink);
    }

    type Case = (&'static str, fn(&mut Grid, &mut Rng, usize, usize));
    let cases: [Case; 3] = [
        ("(a) all rows dirty", |g, _, _, _| g.mark_all_dirty()),
        ("(b) one dirty row", |g, rng, rows, i| {
            fill_row(g, i % rows, rng);
        }),
        ("(c) cursor blink only", |_, _, _, _| {}),
    ];
    for (label, prep) in cases {
        for i in 0..WARMUP {
            prep(&mut grid, &mut rng, rows, i);
            blink = !blink;
            one(&mut grid, &mut tv, blink);
        }
        let (mut upd, mut gpu) = (Vec::new(), Vec::new());
        for i in 0..FRAMES {
            prep(&mut grid, &mut rng, rows, i);
            blink = !blink;
            let (u, g) = one(&mut grid, &mut tv, blink);
            upd.push(u);
            gpu.push(g);
        }
        summarize(label, upd, gpu);
    }
}
