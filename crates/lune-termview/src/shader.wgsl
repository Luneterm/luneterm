// Terminal grid: rects (cell backgrounds, cursor, selection, link hover), glyphs from the two
// atlas pages, and decorations. Positions are physical pixels; colours are premultiplied.
// One unit quad per instance (triangle strip, 4 vertices).

struct Globals {
    size: vec2<f32>,
    // The whole grid's opacity (1 draws it as it is): every fragment is premultiplied, so scaling
    // the output scales colour and alpha together. A tab-switch crossfade draws two grids.
    opacity: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var mask_page: texture_2d<f32>;
@group(1) @binding(1) var color_page: texture_2d<f32>;

const TAU: f32 = 6.2831853;

fn to_ndc(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / g.size.x * 2.0 - 1.0, 1.0 - p.y / g.size.y * 2.0, 0.0, 1.0);
}

fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
}

// -- Rects ---------------------------------------------------------------------------------

struct RectIn {
    @location(0) rect: vec4<f32>, // x, y, w, h
    @location(1) color: vec4<f32>,
};

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_rect(@builtin(vertex_index) vi: u32, r: RectIn) -> RectOut {
    var o: RectOut;
    o.pos = to_ndc(r.rect.xy + corner(vi) * r.rect.zw);
    o.color = r.color;
    return o;
}

@fragment
fn fs_rect(in: RectOut) -> @location(0) vec4<f32> {
    return in.color * g.opacity;
}

// -- Glyphs --------------------------------------------------------------------------------

struct GlyphIn {
    @location(0) rect: vec4<f32>,  // x, y, w, h of the bitmap (1 texel = 1 pixel)
    @location(1) uv: vec4<f32>,    // x, y, w, h in the atlas page, texels
    @location(2) color: vec4<f32>, // premultiplied
    @location(3) clip: vec4<f32>,  // x0, y0, x1, y1
};

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) uv: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn vs_glyph(@builtin(vertex_index) vi: u32, s: GlyphIn) -> GlyphOut {
    var o: GlyphOut;
    o.pos = to_ndc(s.rect.xy + corner(vi) * s.rect.zw);
    o.rect = s.rect;
    o.uv = s.uv;
    o.color = s.color;
    o.clip = s.clip;
    return o;
}

fn outside(p: vec2<f32>, c: vec4<f32>) -> bool {
    return p.x < c.x || p.y < c.y || p.x >= c.z || p.y >= c.w;
}

fn texel(in: GlyphOut) -> vec2<i32> {
    return vec2<i32>(in.uv.xy) + vec2<i32>(floor(in.pos.xy - in.rect.xy));
}

@fragment
fn fs_glyph_mask(in: GlyphOut) -> @location(0) vec4<f32> {
    if outside(in.pos.xy, in.clip) {
        discard;
    }
    let t = textureLoad(mask_page, texel(in), 0);
    return in.color * t.r * g.opacity;
}

@fragment
fn fs_glyph_color(in: GlyphOut) -> @location(0) vec4<f32> {
    if outside(in.pos.xy, in.clip) {
        discard;
    }
    // the page holds premultiplied colour; the instance alpha carries DIM
    let t = textureLoad(color_page, texel(in), 0);
    return t * in.color.a * g.opacity;
}

// -- Decorations -----------------------------------------------------------------------------

struct DecoIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    // style (0 solid, 1 dotted, 2 dashed, 3 curly), line width, grid origin x, cell width
    @location(2) params: vec4<f32>,
};

struct DecoOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_deco(@builtin(vertex_index) vi: u32, d: DecoIn) -> DecoOut {
    var o: DecoOut;
    o.pos = to_ndc(d.rect.xy + corner(vi) * d.rect.zw);
    o.rect = d.rect;
    o.color = d.color;
    o.params = d.params;
    return o;
}

fn fmod(a: f32, b: f32) -> f32 {
    return a - floor(a / b) * b;
}

@fragment
fn fs_deco(in: DecoOut) -> @location(0) vec4<f32> {
    let style = u32(in.params.x + 0.5);
    let lw = in.params.y;
    let cw = max(in.params.w, 1.0);
    let u = in.pos.x - in.params.z;
    var cov = 1.0;
    if style == 1u {
        // dotted: lw on, lw off, continuous across cells
        cov = select(0.0, 1.0, fmod(u, 2.0 * lw) < lw);
    } else if style == 2u {
        // dashed per cell: line 0.6, gap 0.3, end 0.1 of the cell width (xterm.js)
        let line = floor(0.6 * cw);
        let gap = floor(0.3 * cw);
        let m = fmod(u, cw);
        cov = select(1.0, 0.0, m >= line && m < line + gap);
    } else if style == 3u {
        // curly: one sine period per cell, down first, so neighbours join
        let amp = (in.rect.w - lw) * 0.5;
        let phase = TAU * u / cw;
        let yc = in.rect.y + in.rect.w * 0.5 + amp * sin(phase);
        let slope = amp * TAU / cw * cos(phase);
        let d = abs(in.pos.y - yc) / sqrt(1.0 + slope * slope);
        cov = clamp(lw * 0.5 + 0.5 - d, 0.0, 1.0);
    }
    return in.color * cov * g.opacity;
}
