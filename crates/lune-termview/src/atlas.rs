//! Glyph atlas (plan step 4): one mask page (R8Unorm) and one colour page (Rgba8Unorm,
//! premultiplied), keyed by cosmic-text's [`CacheKey`].
//!
//! [`AtlasCore`] is the CPU half (allocators, entry maps, staged uploads, generations) and needs
//! no GPU; [`Atlas`] wraps it with the two textures and their bind group.
//!
//! Reset/grow protocol. When a page is full, [`AtlasCore::get`] returns [`Full`]. The caller
//! (the renderer) calls [`AtlasCore::reset_grow`] at most once per page per update, which wipes
//! that page and bumps its generation; every UV previously handed out for that page is then dead,
//! so the caller re-places every row. A glyph that still does not fit is dropped for that frame
//! and counted with [`AtlasCore::count_dropped`]. Nothing here loops or panics.

use std::collections::HashMap;

use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent};
use etagere::{AtlasAllocator, size2};

/// Start size of each page in texels (square).
pub const START_DIM: u32 = 512;
/// Upper bound on a page's side, before the device limit is applied.
pub const MAX_DIM: u32 = 8192;
/// Empty texels kept around every glyph.
const PAD: u32 = 1;

/// Which page an entry lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    /// R8Unorm coverage.
    Mask,
    /// Rgba8Unorm, premultiplied alpha.
    Color,
}

impl Page {
    fn index(self) -> usize {
        match self {
            Page::Mask => 0,
            Page::Color => 1,
        }
    }

    /// Bytes per texel on this page.
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            Page::Mask => 1,
            Page::Color => 4,
        }
    }
}

/// A placed glyph. `uv` is `[x, y, w, h]` in integer texels (read with `textureLoad`);
/// `left`/`top` are the raster's offsets from the pen position (swash placement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub page: Page,
    pub uv: [u32; 4],
    pub left: i32,
    pub top: i32,
}

/// Returned by [`AtlasCore::get`] when `page` has no room left for the glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full(pub Page);

/// Kind of a rasterized image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// One coverage byte per pixel.
    Mask,
    /// Straight-alpha RGBA, four bytes per pixel.
    Color,
}

/// A rasterized glyph as handed to the atlas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub content: Content,
    pub w: u32,
    pub h: u32,
    pub left: i32,
    pub top: i32,
    pub data: Vec<u8>,
}

/// Produces glyph images. The real one is [`SwashRaster`]; tests use a fake.
pub trait Raster {
    fn raster(&mut self, key: CacheKey) -> Option<Image>;
}

/// [`Raster`] over cosmic-text's swash integration.
pub struct SwashRaster<'a> {
    pub cache: &'a mut SwashCache,
    pub fs: &'a mut FontSystem,
}

impl std::fmt::Debug for SwashRaster<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SwashRaster")
    }
}

impl Raster for SwashRaster<'_> {
    fn raster(&mut self, key: CacheKey) -> Option<Image> {
        // uncached: the atlas map is the cache
        let img = self.cache.get_image_uncached(self.fs, key)?;
        let (content, data) = match img.content {
            SwashContent::Mask => (Content::Mask, img.data),
            SwashContent::Color => (Content::Color, img.data),
            // never requested; if it shows up, keep the first channel as coverage
            SwashContent::SubpixelMask => (
                Content::Mask,
                img.data.chunks_exact(4).map(|px| px[0]).collect(),
            ),
        };
        Some(Image {
            content,
            w: img.placement.width,
            h: img.placement.height,
            left: img.placement.left,
            top: img.placement.top,
            data,
        })
    }
}

/// One pending texture write: `data` fills `[x, y, w, h]` of its page, tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

struct PageCore {
    alloc: AtlasAllocator,
    map: HashMap<CacheKey, Option<Entry>>,
    staging: Vec<Upload>,
    size: u32,
    generation: u64,
}

impl PageCore {
    fn new(size: u32) -> Self {
        PageCore {
            alloc: new_alloc(size),
            map: HashMap::new(),
            staging: Vec::new(),
            size,
            generation: 0,
        }
    }
}

fn new_alloc(size: u32) -> AtlasAllocator {
    let side = i32::try_from(size).unwrap_or(i32::MAX);
    AtlasAllocator::new(size2(side, side))
}

/// CPU half of the atlas. See the module docs for the reset/grow protocol.
pub struct AtlasCore {
    pages: [PageCore; 2],
    max_dim: u32,
    dropped: u64,
}

impl std::fmt::Debug for AtlasCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("AtlasCore");
        for (name, p) in ["mask", "color"].iter().zip(&self.pages) {
            d.field(
                name,
                &format_args!(
                    "size {} gen {} entries {} staged {}",
                    p.size,
                    p.generation,
                    p.map.len(),
                    p.staging.len()
                ),
            );
        }
        d.field("max_dim", &self.max_dim)
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl AtlasCore {
    /// Both pages start at `start` texels square (clamped to `1..=max_dim`); a page never grows
    /// past `max_dim` (clamped to `1..=65535`, etagere's coordinate range).
    pub fn new(start: u32, max_dim: u32) -> Self {
        let max_dim = max_dim.clamp(1, u32::from(u16::MAX));
        let start = start.clamp(1, max_dim);
        AtlasCore {
            pages: [PageCore::new(start), PageCore::new(start)],
            max_dim,
            dropped: 0,
        }
    }

    pub fn size(&self, page: Page) -> u32 {
        self.pages[page.index()].size
    }

    pub fn max_dim(&self) -> u32 {
        self.max_dim
    }

    /// Bumped by every [`reset_grow`](Self::reset_grow) of `page`.
    pub fn generation(&self, page: Page) -> u64 {
        self.pages[page.index()].generation
    }

    /// Sum of both pages' generations: changes whenever any UV handed out may have died.
    pub fn epoch(&self) -> u64 {
        self.pages[0].generation + self.pages[1].generation
    }

    /// Glyphs dropped so far (too big for `max_dim`, or counted by the caller).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Count a glyph the caller dropped (a page filled again after its one reset this update).
    pub fn count_dropped(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    /// Entries cached for `page`, including cached empties.
    pub fn len(&self, page: Page) -> usize {
        self.pages[page.index()].map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.iter().all(|p| p.map.is_empty())
    }

    /// Uploads waiting for [`Atlas::flush`].
    pub fn staged(&self, page: Page) -> &[Upload] {
        &self.pages[page.index()].staging
    }

    /// Takes `page`'s pending uploads, leaving its staging list empty.
    pub fn take_staged(&mut self, page: Page) -> Vec<Upload> {
        std::mem::take(&mut self.pages[page.index()].staging)
    }

    fn cached(&self, key: &CacheKey) -> Option<Option<Entry>> {
        self.pages.iter().find_map(|p| p.map.get(key).copied())
    }

    /// The entry for `key`, rasterizing and staging it on first use.
    ///
    /// - `Ok(None)`: nothing to draw. Empty or missing images are cached, so `raster` is not
    ///   called for them again; a glyph bigger than `max_dim` is not cached and is counted in
    ///   [`dropped`](Self::dropped).
    /// - `Err(Full(page))`: no room on `page` now; see the module docs.
    pub fn get(&mut self, raster: &mut dyn Raster, key: CacheKey) -> Result<Option<Entry>, Full> {
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let Some(img) = raster.raster(key) else {
            self.pages[Page::Mask.index()].map.insert(key, None);
            return Ok(None);
        };
        let page = match img.content {
            Content::Mask => Page::Mask,
            Content::Color => Page::Color,
        };
        let need = (img.w as usize)
            .checked_mul(img.h as usize)
            .and_then(|n| n.checked_mul(page.bytes_per_texel() as usize));
        let usable = img.w > 0 && img.h > 0 && need.is_some_and(|n| img.data.len() >= n);
        if !usable {
            self.pages[page.index()].map.insert(key, None);
            return Ok(None);
        }
        let padded_w = img.w.saturating_add(2 * PAD);
        let padded_h = img.h.saturating_add(2 * PAD);
        if padded_w > self.max_dim || padded_h > self.max_dim {
            self.count_dropped();
            return Ok(None);
        }
        let p = &mut self.pages[page.index()];
        // both fit in i32: bounded by max_dim <= u16::MAX
        let Some(alloc) = p.alloc.allocate(size2(padded_w as i32, padded_h as i32)) else {
            return Err(Full(page));
        };
        // Box2D min is inclusive; the glyph sits one pad inside it
        let x = alloc.rectangle.min.x.max(0) as u32 + PAD;
        let y = alloc.rectangle.min.y.max(0) as u32 + PAD;
        let n = need.unwrap_or(0);
        let mut data = img.data;
        data.truncate(n);
        if page == Page::Color {
            premultiply(&mut data);
        }
        p.staging.push(Upload {
            x,
            y,
            w: img.w,
            h: img.h,
            data,
        });
        let entry = Entry {
            page,
            uv: [x, y, img.w, img.h],
            left: img.left,
            top: img.top,
        };
        p.map.insert(key, Some(entry));
        Ok(Some(entry))
    }

    /// Wipes `page` at its current size: allocator, entries and staged uploads. Bumps its
    /// generation. Used when the fonts change (stale keys, nothing to grow for).
    pub fn reset(&mut self, page: Page) {
        let p = &mut self.pages[page.index()];
        p.alloc = new_alloc(p.size);
        p.map.clear();
        p.staging.clear();
        p.generation += 1;
    }

    /// Doubles `page` (capped at `max_dim`) and wipes it: allocator, entries and staged uploads.
    /// Bumps its generation; every entry previously returned for `page` is dead.
    pub fn reset_grow(&mut self, page: Page) {
        let max_dim = self.max_dim;
        let p = &mut self.pages[page.index()];
        p.size = p.size.saturating_mul(2).min(max_dim);
        p.alloc = new_alloc(p.size);
        p.map.clear();
        p.staging.clear();
        p.generation += 1;
    }
}

/// Straight → premultiplied alpha, rounded: `(c * a + 127) / 255`.
fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// GPU half: the two page textures and a bind group exposing them (binding 0 = mask,
/// binding 1 = colour), read with `textureLoad` at integer texel coordinates.
#[derive(Debug)]
pub struct Atlas {
    core: AtlasCore,
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    textures: [wgpu::Texture; 2],
    bind_group: wgpu::BindGroup,
}

const FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::R8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
];

impl Atlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let max_dim = device.limits().max_texture_dimension_2d.min(MAX_DIM);
        let core = AtlasCore::new(START_DIM, max_dim);
        let entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("termview atlas layout"),
            entries: &[entry(0), entry(1)],
        });
        let textures = [
            page_texture(device, Page::Mask, core.size(Page::Mask)),
            page_texture(device, Page::Color, core.size(Page::Color)),
        ];
        let bind_group = bind_group(device, &layout, &textures);
        Atlas {
            core,
            device: device.clone(),
            layout,
            textures,
            bind_group,
        }
    }

    pub fn core(&self) -> &AtlasCore {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut AtlasCore {
        &mut self.core
    }

    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// The page's current texture (re-created when the page grows).
    pub fn texture(&self, page: Page) -> &wgpu::Texture {
        &self.textures[page.index()]
    }

    /// Valid until the next [`flush`](Self::flush) that grows a page.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Re-creates any page texture whose size no longer matches the core (and the bind group with
    /// it), then writes every staged upload and empties the staging lists.
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        let mut regrown = false;
        for page in [Page::Mask, Page::Color] {
            let size = self.core.size(page);
            let tex = &mut self.textures[page.index()];
            if tex.width() != size || tex.height() != size {
                *tex = page_texture(&self.device, page, size);
                regrown = true;
            }
        }
        if regrown {
            self.bind_group = bind_group(&self.device, &self.layout, &self.textures);
        }
        for page in [Page::Mask, Page::Color] {
            let tex = &self.textures[page.index()];
            for up in self.core.take_staged(page) {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: up.x,
                            y: up.y,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &up.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(up.w * page.bytes_per_texel()),
                        rows_per_image: Some(up.h),
                    },
                    wgpu::Extent3d {
                        width: up.w,
                        height: up.h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}

fn page_texture(device: &wgpu::Device, page: Page, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(match page {
            Page::Mask => "termview atlas mask",
            Page::Color => "termview atlas colour",
        }),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMATS[page.index()],
        // COPY_SRC: readback in tests and golden debugging
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    textures: &[wgpu::Texture; 2],
) -> wgpu::BindGroup {
    let views = textures
        .each_ref()
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("termview atlas"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&views[1]),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::{CacheKeyFlags, fontdb};

    /// Fixed-size images; glyph ids at or above `COLOR_FROM` are colour, id 0 is empty.
    struct Fake {
        calls: usize,
        w: u32,
        h: u32,
        rgba: [u8; 4],
    }

    const COLOR_FROM: u16 = 1000;

    impl Fake {
        fn new(w: u32, h: u32) -> Self {
            Fake {
                calls: 0,
                w,
                h,
                rgba: [255, 0, 0, 128],
            }
        }
    }

    impl Raster for Fake {
        fn raster(&mut self, key: CacheKey) -> Option<Image> {
            self.calls += 1;
            if key.glyph_id == 0 {
                return Some(Image {
                    content: Content::Mask,
                    w: 0,
                    h: 0,
                    left: 0,
                    top: 0,
                    data: Vec::new(),
                });
            }
            let n = (self.w * self.h) as usize;
            Some(if key.glyph_id >= COLOR_FROM {
                Image {
                    content: Content::Color,
                    w: self.w,
                    h: self.h,
                    left: 1,
                    top: 2,
                    data: self.rgba.repeat(n),
                }
            } else {
                Image {
                    content: Content::Mask,
                    w: self.w,
                    h: self.h,
                    left: 1,
                    top: 2,
                    data: vec![200; n],
                }
            })
        }
    }

    fn key(glyph: u16) -> CacheKey {
        CacheKey::new(
            fontdb::ID::dummy(),
            glyph,
            13.0,
            (0.0, 0.0),
            cosmic_text::Weight::NORMAL,
            CacheKeyFlags::empty(),
        )
        .0
    }

    #[test]
    fn same_key_rasterizes_once() {
        let mut core = AtlasCore::new(64, 64);
        let mut r = Fake::new(10, 10);
        let a = core.get(&mut r, key(1)).expect("room");
        let b = core.get(&mut r, key(1)).expect("room");
        assert_eq!(r.calls, 1);
        assert!(a.is_some());
        assert_eq!(a, b);
        assert_eq!(core.staged(Page::Mask).len(), 1);
        let e = a.expect("entry");
        assert_eq!((e.uv[2], e.uv[3], e.left, e.top), (10, 10, 1, 2));
        // padded by one texel inside the allocation
        assert!(e.uv[0] >= 1 && e.uv[1] >= 1);
    }

    #[test]
    fn empty_image_is_cached_as_none() {
        let mut core = AtlasCore::new(64, 64);
        let mut r = Fake::new(10, 10);
        assert_eq!(core.get(&mut r, key(0)), Ok(None));
        assert_eq!(core.get(&mut r, key(0)), Ok(None));
        assert_eq!(r.calls, 1);
        assert!(core.staged(Page::Mask).is_empty());
    }

    #[test]
    fn full_page_then_reset_grow_doubles_and_wipes() {
        let mut core = AtlasCore::new(64, 512);
        let mut r = Fake::new(20, 20);
        let mut placed = 0;
        let err = (1..100u16)
            .find_map(|g| match core.get(&mut r, key(g)) {
                Ok(_) => {
                    placed += 1;
                    None
                }
                Err(e) => Some(e),
            })
            .expect("fills up");
        assert_eq!(err, Full(Page::Mask));
        // 22x22 padded slots: 2 x 2 fit in 64
        assert_eq!(placed, 4);
        let gen0 = core.generation(Page::Mask);
        core.reset_grow(Page::Mask);
        assert_eq!(core.size(Page::Mask), 128);
        assert_eq!(core.generation(Page::Mask), gen0 + 1);
        assert_eq!(core.len(Page::Mask), 0);
        assert!(core.staged(Page::Mask).is_empty());
        // the colour page is untouched
        assert_eq!(core.size(Page::Color), 64);
        assert_eq!(core.generation(Page::Color), 0);
        // a wiped key is rasterized again and fits now
        let calls = r.calls;
        assert!(matches!(core.get(&mut r, key(1)), Ok(Some(_))));
        assert_eq!(r.calls, calls + 1);
    }

    #[test]
    fn at_max_dim_reset_keeps_size_and_big_glyphs_drop() {
        let mut core = AtlasCore::new(64, 64);
        let mut r = Fake::new(20, 20);
        assert!((1..100u16).any(|g| core.get(&mut r, key(g)).is_err()));
        core.reset_grow(Page::Mask);
        assert_eq!(core.size(Page::Mask), 64);
        assert_eq!(core.generation(Page::Mask), 1);
        assert_eq!(core.epoch(), 1);

        let mut big = Fake::new(100, 100);
        assert_eq!(core.get(&mut big, key(500)), Ok(None));
        assert_eq!(core.dropped(), 1);
        // not cached: tried again next time, and counted again
        assert_eq!(core.get(&mut big, key(500)), Ok(None));
        assert_eq!(big.calls, 2);
        assert_eq!(core.dropped(), 2);
        assert!(core.staged(Page::Mask).is_empty());
    }

    #[test]
    fn colour_is_premultiplied_on_staging() {
        let mut core = AtlasCore::new(64, 64);
        let mut r = Fake::new(2, 1);
        let e = core.get(&mut r, key(COLOR_FROM)).expect("room");
        assert_eq!(e.map(|e| e.page), Some(Page::Color));
        let up = &core.staged(Page::Color)[0];
        assert_eq!(up.data, vec![128, 0, 0, 128, 128, 0, 0, 128]);
    }

    #[test]
    fn mask_and_colour_land_on_different_pages() {
        let mut core = AtlasCore::new(64, 64);
        let mut r = Fake::new(4, 4);
        let m = core.get(&mut r, key(1)).expect("room").expect("entry");
        let c = core
            .get(&mut r, key(COLOR_FROM))
            .expect("room")
            .expect("entry");
        assert_eq!((m.page, c.page), (Page::Mask, Page::Color));
        assert_eq!(core.staged(Page::Mask).len(), 1);
        assert_eq!(core.staged(Page::Color).len(), 1);
        assert_eq!(core.staged(Page::Color)[0].data.len(), 4 * 4 * 4);
        assert_eq!(core.staged(Page::Mask)[0].data.len(), 4 * 4);
    }

    #[test]
    fn reset_keeps_size_and_bumps_generation() {
        let mut core = AtlasCore::new(64, 128);
        let mut r = Fake::new(4, 4);
        core.get(&mut r, key(1)).expect("room");
        core.reset(Page::Mask);
        assert_eq!(core.size(Page::Mask), 64);
        assert_eq!(core.generation(Page::Mask), 1);
        assert!(core.is_empty());
        assert!(core.staged(Page::Mask).is_empty());
    }

    #[test]
    fn short_image_data_is_treated_as_empty() {
        struct Short;
        impl Raster for Short {
            fn raster(&mut self, _: CacheKey) -> Option<Image> {
                Some(Image {
                    content: Content::Color,
                    w: 4,
                    h: 4,
                    left: 0,
                    top: 0,
                    data: vec![0; 10],
                })
            }
        }
        let mut core = AtlasCore::new(64, 64);
        assert_eq!(core.get(&mut Short, key(1)), Ok(None));
        assert!(core.staged(Page::Color).is_empty());
    }
}
