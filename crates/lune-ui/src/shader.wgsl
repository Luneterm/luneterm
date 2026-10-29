// Shapes (rounded rects, acrylic layers) and strokes (outline loop, icons).
// All positions are physical pixels; colours are premultiplied.

struct Globals {
    size: vec2<f32>,
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var noise_tex: texture_2d<f32>;
@group(0) @binding(2) var noise_samp: sampler;

const NOISE_OPACITY: f32 = 0.0225;

fn to_ndc(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / g.size.x * 2.0 - 1.0, 1.0 - p.y / g.size.y * 2.0, 0.0, 1.0);
}

fn clipped(p: vec2<f32>, c: vec4<f32>) -> bool {
    return c.z > 0.0 && (p.x < c.x || p.y < c.y || p.x >= c.x + c.z || p.y >= c.y + c.w);
}

// ── Shapes ──────────────────────────────────────────────────────────────────

struct ShapeIn {
    @location(0) rect: vec4<f32>,   // x, y, w, h
    @location(1) radii: vec4<f32>,  // tl, tr, br, bl
    @location(2) color: vec4<f32>,
    @location(3) under: vec4<f32>,  // acrylic: what this layer replaces (for AA edges)
    @location(4) params: vec4<f32>, // kind (0 fill, 1 ring), ring width, acrylic flag, noise flag
    @location(5) clip: vec4<f32>,
};

struct ShapeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) under: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn vs_shape(@builtin(vertex_index) vi: u32, s: ShapeIn) -> ShapeOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = s.rect.xy - vec2<f32>(1.0) + corner * (s.rect.zw + vec2<f32>(2.0));
    var o: ShapeOut;
    o.pos = to_ndc(p);
    o.p = p;
    o.rect = s.rect;
    o.radii = s.radii;
    o.color = s.color;
    o.under = s.under;
    o.params = s.params;
    o.clip = s.clip;
    return o;
}

fn sd_round_rect(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    var rad = r.x;
    if p.x >= 0.0 && p.y < 0.0 { rad = r.y; }
    if p.x >= 0.0 && p.y >= 0.0 { rad = r.z; }
    if p.x < 0.0 && p.y >= 0.0 { rad = r.w; }
    let q = abs(p) - half + vec2<f32>(rad);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rad;
}

// Straight-alpha noise OVER a premultiplied colour (Skia's compose with SrcOver).
fn add_noise(c: vec4<f32>, n: vec4<f32>) -> vec4<f32> {
    let na = n.a * NOISE_OPACITY;
    return vec4<f32>(n.rgb * na + c.rgb * (1.0 - na), na + c.a * (1.0 - na));
}

@fragment
fn fs_shape(i: ShapeOut) -> @location(0) vec4<f32> {
    if clipped(i.p, i.clip) { discard; }
    let center = i.rect.xy + i.rect.zw * 0.5;
    let d = sd_round_rect(i.p - center, i.rect.zw * 0.5, i.radii);
    var cov: f32;
    if i.params.x > 0.5 {
        let w = i.params.y;
        cov = clamp(w * 0.5 + 0.5 - abs(d + w * 0.5), 0.0, 1.0);
    } else {
        cov = clamp(0.5 - d, 0.0, 1.0);
    }
    if i.params.z > 0.5 {
        // Acrylic layer, drawn with REPLACE blending: Skia's `Src` + antialiasing is
        // lerp(dst, src, coverage), and dst here is the known layer underneath.
        let n = textureSampleLevel(noise_tex, noise_samp, i.p / g.scale / 256.0, 0.0) * i.params.w;
        let top = add_noise(i.color, n);
        let below = select(add_noise(i.under, n), vec4<f32>(0.0), i.under.a <= 0.0);
        return mix(below, top, cov);
    }
    return i.color * cov;
}

// ── Strokes ─────────────────────────────────────────────────────────────────

struct StrokeIn {
    @location(0) pos: vec2<f32>,
    @location(1) aa: vec2<f32>,     // signed distance across the stroke (px), distance along (logical px)
    @location(2) color: vec4<f32>,
    @location(3) style: vec4<f32>,  // mode (0 solid, 1 conic sweep, 2 comet), half width (px), a, b
    @location(4) extra: vec4<f32>,  // sweep: centre x, y (px); comet: -, -, contour length
    @location(5) clip: vec4<f32>,
};

struct StrokeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) aa: vec2<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) style: vec4<f32>,
    @location(4) @interpolate(flat) extra: vec4<f32>,
    @location(5) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn vs_stroke(s: StrokeIn) -> StrokeOut {
    var o: StrokeOut;
    o.pos = to_ndc(s.pos);
    o.p = s.pos;
    o.aa = s.aa;
    o.color = s.color;
    o.style = s.style;
    o.extra = s.extra;
    o.clip = s.clip;
    return o;
}

// Lunegit's conic stop table: alpha 1 → 64/255 → 0 … 0 → 64/255 → 1.
fn sweep_alpha(t: f32) -> f32 {
    let faint = 64.0 / 255.0;
    if t < 0.1528 { return mix(1.0, faint, t / 0.1528); }
    if t < 0.3056 { return mix(faint, 0.0, (t - 0.1528) / 0.1528); }
    if t < 0.6944 { return 0.0; }
    if t < 0.8472 { return mix(0.0, faint, (t - 0.6944) / 0.1528); }
    return mix(faint, 1.0, (t - 0.8472) / 0.1528);
}

@fragment
fn fs_stroke(i: StrokeOut) -> @location(0) vec4<f32> {
    if clipped(i.p, i.clip) { discard; }
    let half = i.style.y;
    let cov = clamp(half + 0.5 - abs(i.aa.x), 0.0, 1.0);
    var k = 1.0;
    let mode = i.style.x;
    if mode > 0.5 && mode < 1.5 {
        let v = i.p - i.extra.xy;
        // Angle measured clockwise from 12 o'clock, rotated by the animation phase (turns).
        let ang = atan2(v.x, -v.y) / 6.2831853;
        k = sweep_alpha(fract(ang - i.style.z));
    } else if mode > 1.5 {
        let s = (i.aa.y + i.style.w) % i.extra.z;
        k = select(0.0, 1.0, s < i.style.z);
    }
    return i.color * cov * k;
}
