// The viewer's picture: a player's decoded YUV picture fitted into the
// viewer as export fits it, turned upright, scaled in YUV and converted to
// BT.709 RGB with the export's math (vidcut_av::color::shader_colors).
// See picture.rs.

struct Params {
    // Where the upright picture goes in the target: x, y, width, height in
    // pixels; outside it, black.
    place: vec4<f32>,
    // The upright picture's size in luma samples (width, height), and how
    // much of its textures' width and height the stored picture covers (zw:
    // 1, but less where a texture is rounded up to even sides).
    upright: vec4<f32>,
    // y' = (sample - range.x) * range.y; chroma = (sample - range.z) * range.w.
    range: vec4<f32>,
    // Kr, Kb, Kg of the YUV matrix, and what linear light is multiplied by.
    matrix: vec4<f32>,
    // The tone curve: knee, scale, a, b.
    tone: vec4<f32>,
    // Linear BT.2020 to BT.709, by rows (w unused).
    gamut0: vec4<f32>,
    gamut1: vec4<f32>,
    gamut2: vec4<f32>,
    // x: 0 SDR, 1 PQ, 2 HLG; y: quarter turns clockwise that show it
    // upright; z: the layout, 0 planar (Y, U, V), 1 interleaved chroma
    // (Y, UV); w unused.
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma_u: texture_2d<f32>;
@group(0) @binding(3) var chroma_v: texture_2d<f32>;
@group(0) @binding(4) var bilinear: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
};

// One triangle covering the target.
@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    var out: Out;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// Where upright position `u` (0..1 each way) is in the stored picture.
fn stored(u: vec2<f32>) -> vec2<f32> {
    switch p.flags.y {
        case 1u: { return vec2<f32>(u.y, 1.0 - u.x); }
        case 2u: { return vec2<f32>(1.0 - u.x, 1.0 - u.y); }
        case 3u: { return vec2<f32>(1.0 - u.y, u.x); }
        default: { return u; }
    }
}

// The YUV samples at upright position `u`.
fn yuv(u: vec2<f32>) -> vec3<f32> {
    let s = stored(u) * p.upright.zw;
    let y = textureSampleLevel(luma, bilinear, s, 0.0).r;
    if p.flags.z == 1u {
        let uv = textureSampleLevel(chroma_u, bilinear, s, 0.0).rg;
        return vec3<f32>(y, uv.x, uv.y);
    }
    let cb = textureSampleLevel(chroma_u, bilinear, s, 0.0).r;
    let cr = textureSampleLevel(chroma_v, bilinear, s, 0.0).r;
    return vec3<f32>(y, cb, cr);
}

// PQ's EOTF or HLG's inverse OETF and 1.2 system gamma, channel by channel,
// the reference white at 1.0 (zimg's approximate path, as the export).
fn to_linear(c_in: vec3<f32>) -> vec3<f32> {
    var c = max(c_in, vec3<f32>(0.0));
    if p.flags.x == 1u {
        let e = pow(c, vec3<f32>(1.0 / 78.84375));
        let num = max(e - vec3<f32>(0.8359375), vec3<f32>(0.0));
        let den = max(vec3<f32>(18.8515625) - 18.6875 * e, vec3<f32>(1.17549435e-38));
        c = pow(num / den, vec3<f32>(1.0 / 0.1593017578125));
    } else {
        let high = (exp((c - vec3<f32>(0.55991073)) / 0.17883277) + vec3<f32>(0.28466892)) / 12.0;
        c = select(high, c * c / 3.0, c <= vec3<f32>(0.5));
        c = pow(c, vec3<f32>(1.2));
    }
    return c * p.matrix.w;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    let rel = (px - p.place.xy) / p.place.zw;
    if any(rel < vec2<f32>(0.0)) || any(rel >= vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // Scaled in YUV, as export scales before converting: down, the average
    // of taps spread over the stored samples the pixel covers (a box), up,
    // bilinear.
    let cover = p.upright.xy / p.place.zw;
    let taps = vec2<u32>(clamp(ceil(cover), vec2<f32>(1.0), vec2<f32>(8.0)));
    let step = vec2<f32>(1.0) / (p.place.zw * vec2<f32>(taps));
    let first = (px - vec2<f32>(0.5) - p.place.xy) / p.place.zw + step * 0.5;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            sum += yuv(first + vec2<f32>(f32(i), f32(j)) * step);
        }
    }
    let s = sum / f32(taps.x * taps.y);
    // The matrix and range.
    let y = (s.x - p.range.x) * p.range.y;
    let cb = (s.y - p.range.z) * p.range.w;
    let cr = (s.z - p.range.z) * p.range.w;
    let kr = p.matrix.x;
    let kb = p.matrix.y;
    let kg = p.matrix.z;
    let r = y + (2.0 - 2.0 * kr) * cr;
    let b = y + (2.0 - 2.0 * kb) * cb;
    let g = (y - kr * r - kb * b) / kg;
    var c = vec3<f32>(r, g, b);
    if p.flags.x != 0u {
        // HDR into SDR: linear light, BT.709's primaries, the tone curve on
        // the brightest channel, BT.709's transfer as a 2.4 power.
        c = to_linear(c);
        c = vec3<f32>(dot(p.gamut0.xyz, c), dot(p.gamut1.xyz, c), dot(p.gamut2.xyz, c));
        let sig = max(max(max(c.r, c.g), c.b), 1e-6);
        let knee = p.tone.x;
        let mapped = select(p.tone.y * (sig + p.tone.z) / (sig + p.tone.w), sig, sig <= knee);
        c = c * (mapped / sig);
        c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4));
    }
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
