//! GPU half of the glyph atlas: uploads land where the entries say, and a grown page gets a new
//! texture. Prints `SKIPPED` without an adapter unless `LUNE_REQUIRE_GPU=1`.

mod common;

use cosmic_text::{CacheKey, CacheKeyFlags, Weight, fontdb};
use lune_termview::atlas::{Atlas, Content, Image, Page, Raster, START_DIM};

/// One 2x1 straight-alpha red pixel pair per key.
struct Red;

impl Raster for Red {
    fn raster(&mut self, _: CacheKey) -> Option<Image> {
        Some(Image {
            content: Content::Color,
            w: 2,
            h: 1,
            left: 0,
            top: 0,
            data: [255, 0, 0, 128].repeat(2),
        })
    }
}

#[test]
fn atlas_flush_uploads_and_regrows() {
    let Some((device, queue, _)) = common::device() else {
        return;
    };
    let mut atlas = Atlas::new(&device);
    assert_eq!(atlas.core().size(Page::Color), START_DIM);
    let key = CacheKey::new(
        fontdb::ID::dummy(),
        7,
        13.0,
        (0.0, 0.0),
        Weight::NORMAL,
        CacheKeyFlags::empty(),
    )
    .0;
    let e = atlas
        .core_mut()
        .get(&mut Red, key)
        .expect("room")
        .expect("entry");
    atlas.flush(&queue);
    assert!(atlas.core().staged(Page::Color).is_empty());
    let px =
        lune_ui::dump::read_rgba(&device, &queue, atlas.texture(Page::Color)).expect("readback");
    let at = ((e.uv[1] * START_DIM + e.uv[0]) * 4) as usize;
    assert_eq!(px[at..at + 8], [128, 0, 0, 128, 128, 0, 0, 128]);

    atlas.core_mut().reset_grow(Page::Color);
    atlas.flush(&queue);
    assert_eq!(atlas.texture(Page::Color).width(), START_DIM * 2);
    assert_eq!(atlas.texture(Page::Mask).width(), START_DIM);
}
