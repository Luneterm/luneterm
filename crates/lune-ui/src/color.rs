//! Colour type and helpers shared by the toolkit and its apps.

/// RGBA, 8 bits per channel, straight (not premultiplied) alpha.
pub type Rgba = [u8; 4];

pub fn premul(c: Rgba) -> [f32; 4] {
    let a = f32::from(c[3]) / 255.0;
    [
        f32::from(c[0]) / 255.0 * a,
        f32::from(c[1]) / 255.0 * a,
        f32::from(c[2]) / 255.0 * a,
        a,
    ]
}

pub fn over(top: [f32; 4], below: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - top[3];
    [
        top[0] + below[0] * k,
        top[1] + below[1] * k,
        top[2] + below[2] * k,
        top[3] + below[3] * k,
    ]
}

pub fn with_alpha(c: Rgba, k: f32) -> Rgba {
    [c[0], c[1], c[2], (f32::from(c[3]) * k).round() as u8]
}
