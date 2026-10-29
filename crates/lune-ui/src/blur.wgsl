// Backdrop blur for glass menus (Luneium's context-menu glass): downsample the scene, separable
// Gaussian, then draw it inside the menu's rounded rect with extra saturation. Premultiplied.

struct Params {
    src_texel: vec2<f32>,
    dir: vec2<f32>,
    sigma: f32,
    radius: f32,
    _pad: vec2<f32>,
};

struct Backdrop {
    rect: vec4<f32>,         // physical px, already animated
    target_size: vec2<f32>,  // surface size
    radius: f32,
    alpha: f32,
    saturation: f32,
    // Three scalars, not a vec3: a vec3 aligns to 16 and would grow the struct to 64 bytes.
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> p: Params;
@group(0) @binding(3) var<uniform> b: Backdrop;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: FullOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

// Copy / 2× downsample (bilinear averages 2×2 texels at the output texel centre).
@fragment
fn fs_copy(i: FullOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, samp, i.uv, 0.0);
}

@fragment
fn fs_blur(i: FullOut) -> @location(0) vec4<f32> {
    let r = i32(p.radius);
    var acc = vec4<f32>(0.0);
    var total = 0.0;
    for (var k = -r; k <= r; k++) {
        let x = f32(k);
        let w = exp(-(x * x) / (2.0 * p.sigma * p.sigma));
        acc += w * textureSampleLevel(src, samp, i.uv + p.dir * p.src_texel * x, 0.0);
        total += w;
    }
    return acc / total;
}

struct BackdropOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
};

@vertex
fn vs_backdrop(@builtin(vertex_index) vi: u32) -> BackdropOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let px = b.rect.xy - vec2<f32>(1.0) + corner * (b.rect.zw + vec2<f32>(2.0));
    var o: BackdropOut;
    o.pos = vec4<f32>(px.x / b.target_size.x * 2.0 - 1.0, 1.0 - px.y / b.target_size.y * 2.0, 0.0, 1.0);
    o.px = px;
    return o;
}

fn sd_round_rect(q0: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(q0) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_backdrop(i: BackdropOut) -> @location(0) vec4<f32> {
    let center = b.rect.xy + b.rect.zw * 0.5;
    let cov = clamp(0.5 - sd_round_rect(i.px - center, b.rect.zw * 0.5, b.radius), 0.0, 1.0);
    var c = textureSampleLevel(src, samp, i.px / b.target_size, 0.0);
    if c.a > 0.0001 {
        let straight = c.rgb / c.a;
        let luma = dot(straight, vec3<f32>(0.2126, 0.7152, 0.0722));
        let sat = clamp(mix(vec3<f32>(luma), straight, b.saturation), vec3<f32>(0.0), vec3<f32>(1.0));
        c = vec4<f32>(sat * c.a, c.a);
    }
    return c * cov * b.alpha;
}
