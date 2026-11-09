//! CI probes (roadmap P4 and P6): what each OS gives us. They print one line each and pass,
//! except that the emoji probe insists on a colour bitmap on macOS.

mod common;

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};

#[test]
fn gpu_adapter_probe() {
    if let Some((_gpu, _device, _queue, info)) = common::device() {
        println!("P6 adapter: {info}");
    }
}

#[test]
fn emoji_raster_probe() {
    let mut fs = FontSystem::new();
    let mut buffer = Buffer::new(&mut fs, Metrics::new(26.0, 32.0));
    buffer.set_text("\u{1F3A8}", &Attrs::new(), Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut fs, false);
    let glyph = buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .next()
        .map(|g| (g.font_id, g.physical((0.0, 0.0), 1.0).cache_key));
    let Some((font_id, key)) = glyph else {
        println!("P4 emoji: font=none content=none 0x0");
        if cfg!(target_os = "macos") {
            panic!("no glyph shaped for U+1F3A8");
        }
        return;
    };
    let family = fs
        .db()
        .face(font_id)
        .and_then(|f| f.families.first().map(|(name, _)| name.clone()))
        .unwrap_or_else(|| "?".to_owned());
    let mut swash = SwashCache::new();
    let (content, w, h) = match swash.get_image_uncached(&mut fs, key) {
        Some(img) => {
            let c = match img.content {
                SwashContent::Mask => "Mask",
                SwashContent::Color => "Color",
                SwashContent::SubpixelMask => "SubpixelMask",
            };
            (c, img.placement.width, img.placement.height)
        }
        None => ("none", 0, 0),
    };
    println!("P4 emoji: font={family} content={content} {w}x{h}");
    if cfg!(target_os = "macos") {
        assert_eq!(content, "Color", "macOS must rasterize emoji in colour");
    }
}
