//! Offscreen render targets and GPU readback to PNG: the frame-dump building blocks. Readback
//! copies a texture back through a mapped buffer (no window or screen capture), so dumps work
//! on a hidden window and are byte-comparable between runs.

use std::path::Path;

/// An offscreen colour target of `format` and `size` physical pixels that can be drawn into
/// and read back.
pub fn target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: (u32, u32),
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame dump"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Copies `texture` back to the CPU as tightly packed RGBA8.
pub fn read_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<Vec<u8>, String> {
    let (w, h) = (texture.width(), texture.height());
    let row =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame dump readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("poll: {e:?}"))?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| format!("map: {e:?}"))?;
    unpad_to_rgba(&data, w, h, row, texture.format())
}

/// Removes the row padding of a readback and converts to RGBA8, keeping alpha.
/// `Bgra8Unorm` and `Bgra8UnormSrgb` are swizzled; `Rgba8*` pass through.
fn unpad_to_rgba(
    data: &[u8],
    w: u32,
    h: u32,
    row: u32,
    format: wgpu::TextureFormat,
) -> Result<Vec<u8>, String> {
    use wgpu::TextureFormat as F;
    let bgra = match format {
        F::Bgra8Unorm | F::Bgra8UnormSrgb => true,
        F::Rgba8Unorm | F::Rgba8UnormSrgb => false,
        other => return Err(format!("unsupported dump format {other:?}")),
    };
    let (w, h, row) = (w as usize, h as usize, row as usize);
    if row < w * 4 || data.len() < row * h {
        return Err("readback buffer too small".to_owned());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for px in data[y * row..y * row + w * 4].chunks_exact(4) {
            if bgra {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            } else {
                rgba.extend_from_slice(px);
            }
        }
    }
    Ok(rgba)
}

pub fn write_png(path: &Path, size: (u32, u32), rgba: &[u8]) -> Result<(), String> {
    let file =
        std::fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpad_swizzles_bgra_and_keeps_alpha() {
        // 2x2 BGRA, rows padded to 256 bytes with 0xEE filler.
        let mut data = vec![0xEE; 512];
        data[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        data[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let out = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Bgra8Unorm)
            .expect("valid readback");
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]);
        let srgb = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("valid readback");
        assert_eq!(srgb, out);
    }

    #[test]
    fn unpad_passes_rgba_through() {
        let mut data = vec![0xEE; 512];
        data[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        data[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let out = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Rgba8Unorm)
            .expect("valid readback");
        assert_eq!(out, (1..=16).collect::<Vec<u8>>());
    }

    #[test]
    fn unpad_rejects_other_formats_and_short_buffers() {
        assert!(unpad_to_rgba(&[0; 512], 2, 2, 256, wgpu::TextureFormat::R8Unorm).is_err());
        assert!(unpad_to_rgba(&[0; 100], 2, 2, 256, wgpu::TextureFormat::Bgra8Unorm).is_err());
    }
}
