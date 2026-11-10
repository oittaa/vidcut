struct Params {
    place: vec4<f32>,
    upright: vec4<f32>,
    range: vec4<f32>,
    matrix: vec4<f32>,
    tone: vec4<f32>,
    gamut0: vec4<f32>,
    gamut1: vec4<f32>,
    gamut2: vec4<f32>,
    flags: vec4<u32>,
    per: vec4<u32>,
    placed: vec4<f32>,
    made: vec4<f32>,
    part: vec4<f32>,
    site: vec4<f32>,
    shrunk: vec4<f32>,
    levels: array<vec4<f32>, 5u>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma_u: texture_2d<f32>;
@group(0) @binding(3) var chroma_v: texture_2d<f32>;
@group(0) @binding(4) var bilinear: sampler;
@group(0) @binding(7) var alpha_plane: texture_2d<f32>;
@group(0) @binding(8) var halved: texture_2d<f32>;

struct Out {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    var out: Out;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn stored(u: vec2<f32>) -> vec2<f32> {
    switch p.flags.y {
        case 1u: { return vec2<f32>(u.y, 1.0 - u.x); }
        case 2u: { return vec2<f32>(1.0 - u.x, 1.0 - u.y); }
        case 3u: { return vec2<f32>(1.0 - u.y, u.x); }
        default: { return u; }
    }
}

fn stored_size() -> vec2<f32> {
    return select(p.upright.xy, p.upright.yx, (p.flags.y & 1u) == 1u);
}

fn pixel(u: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(halved, bilinear, stored(u), 0.0).rgb;
}

fn yuv_at(s: vec2<f32>) -> vec3<f32> {
    if p.flags.z == 3u {
        return textureSampleLevel(luma, bilinear, s, 0.0).rgb;
    }
    if p.flags.z == 2u {
        let ayuv = textureSampleLevel(luma, bilinear, s, 0.0);
        return ayuv.gba;
    }
    let y = textureSampleLevel(luma, bilinear, s, 0.0).r;
    let chroma = vec2<f32>(textureDimensions(chroma_u));
    let c = s * vec2<f32>(textureDimensions(luma)) / (vec2<f32>(p.per.xy) * chroma) + p.site.xy / chroma;
    if p.flags.z == 1u {
        let uv = textureSampleLevel(chroma_u, bilinear, c, 0.0).rg;
        return vec3<f32>(y, uv.x, uv.y);
    }
    let cb = textureSampleLevel(chroma_u, bilinear, c, 0.0).r;
    let cr = textureSampleLevel(chroma_v, bilinear, c, 0.0).r;
    return vec3<f32>(y, cb, cr);
}

fn read_as(a: f32) -> f32 {
    return select(a, 1.0 - a, p.site.z > 2.5);
}

fn opacity(u: vec2<f32>) -> f32 {
    if p.site.z > 0.5 && p.site.z < 1.5 {
        return 1.0;
    }
    return stored_opacity(stored(u) * p.upright.zw);
}

fn stored_opacity(s: vec2<f32>) -> f32 {
    if p.flags.z == 2u {
        return read_as(textureSampleLevel(luma, bilinear, s, 0.0).r);
    }
    if p.per.w == 1u {
        return read_as(textureSampleLevel(alpha_plane, bilinear, s, 0.0).r);
    }
    if p.per.w == 2u {
        return read_as(min(textureSampleLevel(alpha_plane, bilinear, s, 0.0).r * 65535.0 / 4095.0, 1.0));
    }
    return 1.0;
}

fn premultiplied() -> bool {
    return p.site.z > 1.5 && p.site.z < 2.5;
}

fn matte() -> bool {
    return p.site.w > 1.5 && p.site.w < 2.5;
}

var<private> placed_share: f32 = 1.0;
var<private> file_alpha: f32 = 1.0;

fn weight(x: f32, kind: u32) -> f32 {
    let a = abs(x);
    if kind == 1u {
        let k = -0.5;
        if a < 1.0 {
            return ((k + 2.0) * a - (k + 3.0)) * a * a + 1.0;
        }
        if a < 2.0 {
            return ((k * a - 5.0 * k) * a + 8.0 * k) * a - 4.0 * k;
        }
        return 0.0;
    }
    let lobes = select(4.0, 3.0, kind == 2u);
    if a < 1e-5 {
        return 1.0;
    }
    if a >= lobes {
        return 0.0;
    }
    let t = 3.14159265 * a;
    return lobes * sin(t) * sin(t / lobes) / (t * t);
}

fn tapered(d: f32, shrink: f32) -> f32 {
    if d >= 8.0 {
        return 0.0;
    }
    let t = 3.14159265 * d;
    let low = select(sin(shrink * t) / (shrink * t), 1.0, d < 1e-5);
    return shrink * low * (0.5 + 0.5 * cos(t / 8.0));
}

fn resize_weight(x: f32, shrink: f32) -> f32 {
    let a = abs(x) * 2.0;
    let k = floor(a);
    return mix(tapered(k * 0.5, shrink), tapered((k + 1.0) * 0.5, shrink), a - k);
}

fn resized(u: vec2<f32>, spans: vec2<f32>) -> vec4<f32> {
    let level = clamp(ceil(log2(max(spans.x, spans.y) / 8.0)), 0.0, p.shrunk.x);
    let scale = exp2(level);
    let size = p.upright.xy / scale;
    let shrink = vec2<f32>(1.0) / max(spans / scale, vec2<f32>(1.0));
    let t = u * size - 0.5;
    let base = floor(t);
    let f = t - base;
    let half = vec2<f32>(0.5) / size;
    var across: array<f32, 16>;
    var total_across = 0.0;
    for (var i = 0; i < 16; i++) {
        across[i] = resize_weight(f32(i - 7) - f.x, shrink.x);
        total_across += across[i];
    }
    var sum = vec4<f32>(0.0);
    var total_down = 0.0;
    for (var j = 0; j < 16; j++) {
        let wy = resize_weight(f32(j - 7) - f.y, shrink.y);
        total_down += wy;
        if abs(wy) < 1e-6 {
            continue;
        }
        for (var i = 0; i < 16; i++) {
            if abs(across[i]) < 1e-6 {
                continue;
            }
            let q = (base + vec2<f32>(f32(i - 7), f32(j - 7)) + 0.5) / size;
            sum += sample_at(clamp(q, half, vec2<f32>(1.0) - half), level) * across[i] * wy;
        }
    }
    return sum / (total_across * total_down);
}

fn box_taps(spans: vec2<f32>) -> vec2<u32> {
    return vec2<u32>(clamp(ceil(spans - vec2<f32>(1e-4)), vec2<f32>(1.0), vec2<f32>(8.0)));
}

fn resamples(u: vec2<f32>, spans: vec2<f32>) -> bool {
    let kind = p.flags.w & 7u;
    if kind == 0u {
        return false;
    }
    if (p.flags.w & 8u) != 0u {
        if any(abs(spans - vec2<f32>(1.0)) > vec2<f32>(1e-4)) {
            return true;
        }
        let t = u * p.upright.xy - 0.5;
        return kind == 4u && any(abs(t - round(t)) > vec2<f32>(1e-3));
    }
    return max(spans.x, spans.y) < 1.0;
}

fn sample_at(q: vec2<f32>, level: f32) -> vec4<f32> {
    let i = u32(level);
    let texels = vec2<f32>(textureDimensions(halved, i));
    let s = stored(q) * stored_size() / (exp2(level) * texels);
    return textureSampleLevel(halved, bilinear, s, level);
}

fn sharp(u: vec2<f32>, spans: vec2<f32>) -> vec3<f32> {
    let kind = p.flags.w & 7u;
    if kind == 4u {
        return resized(u, spans).rgb;
    }
    let level = clamp(ceil(log2(max(spans.x, spans.y) / 4.0)), 0.0, p.shrunk.x);
    let scale = exp2(level);
    let size = p.upright.xy / scale;
    let lobes = select(select(4.0, 3.0, kind == 2u), 2.0, kind == 1u);
    let wide = clamp(spans / scale, vec2<f32>(1.0), vec2<f32>(4.0));
    let reach = vec2<i32>(ceil(lobes * wide));
    let t = u * size - 0.5;
    let base = floor(t);
    let f = t - base;
    let half = vec2<f32>(0.5) / size;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var j = 1 - reach.y; j <= reach.y; j++) {
        let wy = weight((f32(j) - f.y) / wide.y, kind);
        for (var i = 1 - reach.x; i <= reach.x; i++) {
            let w = weight((f32(i) - f.x) / wide.x, kind) * wy;
            let q = (base + vec2<f32>(f32(i), f32(j)) + 0.5) / size;
            sum += sample_at(clamp(q, half, vec2<f32>(1.0) - half), level).rgb * w;
            total += w;
        }
    }
    return sum / total;
}

fn decode(c_in: vec3<f32>, transfer: u32, scale: f32) -> vec3<f32> {
    var c = max(c_in, vec3<f32>(0.0));
    if transfer == 1u {
        let e = pow(c, vec3<f32>(1.0 / 78.84375));
        let num = max(e - vec3<f32>(0.8359375), vec3<f32>(0.0));
        let den = max(vec3<f32>(18.8515625) - 18.6875 * e, vec3<f32>(1.17549435e-38));
        c = pow(num / den, vec3<f32>(1.0 / 0.1593017578125));
    } else {
        let high = (exp((c - vec3<f32>(0.55991073)) / 0.17883277) + vec3<f32>(0.28466892)) / 12.0;
        c = select(high, c * c / 3.0, c <= vec3<f32>(0.5));
        c = pow(c, vec3<f32>(1.2));
    }
    return c * scale;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return decode(c, p.flags.x, p.matrix.w);
}

fn encode(c_in: vec3<f32>, transfer: u32) -> vec3<f32> {
    let c = max(c_in, vec3<f32>(0.0));
    if transfer == 1u {
        let y = pow(c * (203.0 / 10000.0), vec3<f32>(0.1593017578125));
        return pow((vec3<f32>(0.8359375) + 18.8515625 * y) / (vec3<f32>(1.0) + 18.6875 * y), vec3<f32>(78.84375));
    }
    let l = pow(c * (203.0 / 1000.0), vec3<f32>(1.0 / 1.2));
    let high = 0.17883277 * log(max(12.0 * l - vec3<f32>(0.28466892), vec3<f32>(1e-30))) + vec3<f32>(0.55991073);
    return select(high, sqrt(3.0 * l), l <= vec3<f32>(1.0 / 12.0));
}

fn tone_mapped(c_in: vec3<f32>, tone: vec4<f32>) -> vec3<f32> {
    let sig = max(max(max(c_in.r, c_in.g), c_in.b), 1e-6);
    let mapped = select(tone.y * (sig + tone.z) / (sig + tone.w), sig, sig <= tone.x);
    let c = c_in * (mapped / sig);
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4));
}

fn rgb(s: vec3<f32>) -> vec3<f32> {
    var c = s;
    if p.flags.z == 4u {
        c = s.zxy;
    } else if p.flags.z != 3u {
        let y = (s.x - p.range.x) * p.range.y;
        let cb = (s.y - p.range.z) * p.range.w;
        let cr = (s.z - p.range.z) * p.range.w;
        let kr = p.matrix.x;
        let kb = p.matrix.y;
        let kg = p.matrix.z;
        let r = y + (2.0 - 2.0 * kr) * cr;
        let b = y + (2.0 - 2.0 * kb) * cb;
        let g = (y - kr * r - kb * b) / kg;
        c = vec3<f32>(r, g, b);
    }
    if p.per.z != 0u {
        if p.flags.x != 0u {
            c = to_linear(c);
        } else {
            c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.4));
        }
        c = vec3<f32>(dot(p.gamut0.xyz, c), dot(p.gamut1.xyz, c), dot(p.gamut2.xyz, c));
        return clamp(encode(c, p.per.z), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if p.flags.x != 0u {
        c = to_linear(c);
        c = vec3<f32>(dot(p.gamut0.xyz, c), dot(p.gamut1.xyz, c), dot(p.gamut2.xyz, c));
        c = tone_mapped(c, p.tone);
    }
    return c;
}

fn cut(a: vec4<f32>, b: vec4<f32>, s: vec4<f32>, px: vec2<f32>) -> vec3<f32> {
    let w = dot(vec3<f32>(px, 1.0), vec3<f32>(a.w, b.w, 1.0));
    if abs(w) < 1e-6 {
        return vec3<f32>(0.0);
    }
    let uv = vec2<f32>(dot(a.xy, px) + a.z, dot(b.xy, px) + b.z) / w;
    let du = vec2<f32>(a.x - uv.x * a.w, a.y - uv.x * b.w) / w;
    let dv = vec2<f32>(b.x - uv.y * a.w, b.y - uv.y * b.w) / w;
    let gu = length(du);
    let gv = length(dv);
    let edges = vec4<f32>((uv.x - s.x) / gu, (s.z - uv.x) / gu, (uv.y - s.y) / gv, (s.w - uv.y) / gv);
    let e = clamp(edges + 0.5, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec3<f32>(uv, e.x * e.y * e.z * e.w);
}

fn faded(inner: vec4<f32>, reach: vec4<f32>, uv: vec2<f32>) -> f32 {
    if reach.x <= 0.0 || reach.y <= 0.0 {
        return reach.z;
    }
    let a = max(max(inner.x - uv.x, uv.x - inner.z), 0.0) / reach.x;
    let b = max(max(inner.y - uv.y, uv.y - inner.w), 0.0) / reach.y;
    let h = max(0.5 - abs(a - b), 0.0) / 0.5;
    let m = max(a, b) + h * h * 0.125;
    let t = clamp((1.0 - m) * (8.0 / 7.0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t) * reach.z;
}

fn shown(px: vec2<f32>) -> vec4<f32> {
    if p.made.x > 2.5 && p.made.x < 3.5 {
        let c = textureLoad(halved, vec2<i32>(px), 0);
        if c.a <= 0.0 {
            discard;
        }
        return vec4<f32>(c.rgb / c.a, c.a * p.placed.y);
    }
    if p.made.x > 4.5 && p.made.x < 5.5 {
        let a = textureLoad(halved, vec2<i32>(px), 0).a;
        if a <= 0.0 {
            discard;
        }
        return vec4<f32>(textureLoad(luma, vec2<i32>(px), 0).rgb, a * p.placed.y);
    }
    let row0 = p.levels[0];
    let row1 = p.levels[1];
    let edge = cut(row0, row1, p.levels[2], px);
    let uv = edge.xy;
    let cover = edge.z * faded(p.levels[3], p.levels[4], edge.xy);
    let tilt = dot(vec3<f32>(px, 1.0), vec3<f32>(row0.w, row1.w, 1.0));
    var across = vec2<f32>(0.0);
    var down = vec2<f32>(0.0);
    if abs(tilt) >= 1e-6 {
        across = vec2<f32>(row0.x - uv.x * row0.w, row1.x - uv.y * row0.w) / tilt;
        down = vec2<f32>(row0.y - uv.x * row1.w, row1.y - uv.y * row1.w) / tilt;
    }
    if cover <= 0.0 {
        discard;
    }
    if p.made.x > 5.5 {
        let spans = vec2<f32>(length(across * p.upright.xy), length(down * p.upright.xy));
        var c = textureSampleLevel(halved, bilinear, uv, 0.0);
        if resamples(uv, spans) {
            c = resized(uv, spans);
        }
        if c.a <= 0.0 {
            discard;
        }
        return vec4<f32>(c.rgb / c.a, c.a * cover * p.placed.y);
    }
    if p.made.x > 3.5 {
        return vec4<f32>(1.0, 1.0, 1.0, cover * p.placed.y);
    }
    if p.made.x > 1.5 {
        let q = (px / p.made.yz - p.part.xy) / p.part.zw;
        file_alpha = opacity(q);
        placed_share = cover * p.placed.y;
        return vec4<f32>(pixel(q), placed_share * file_alpha);
    }
    let size = p.upright.xy;
    let spans = vec2<f32>(length(across * size), length(down * size));
    file_alpha = opacity(uv);
    placed_share = cover * p.placed.y;
    let a = placed_share * file_alpha;
    if resamples(uv, spans) {
        return vec4<f32>(sharp(uv, spans), a);
    }
    let taps = box_taps(spans);
    let half = vec2<f32>(0.5) / size;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / vec2<f32>(taps) - 0.5;
            let q = uv + across * o.x + down * o.y;
            sum += pixel(clamp(q, half, vec2<f32>(1.0) - half));
        }
    }
    return vec4<f32>(sum / f32(taps.x * taps.y), a);
}

fn yuv_woven(px: vec2<f32>) -> vec3<f32> {
    let size = stored_size();
    let s = (px + 0.5) / size * p.upright.zw;
    if p.shrunk.y < 0.5 || p.per.y < 2u || p.flags.z >= 2u {
        return yuv_at(s);
    }
    let y = textureSampleLevel(luma, bilinear, s, 0.0).r;
    let chroma = vec2<f32>(textureDimensions(chroma_u));
    let line = u32(px.y);
    let field = f32(line & 1u);
    let rows = floor((ceil(size.y / f32(p.per.y)) - field + 1.0) * 0.5);
    let at = (f32(line >> 1u) + 0.5) / f32(p.per.y) + p.site.y - 0.5;
    let k = floor(at);
    let v = (2.0 * clamp(vec2<f32>(k, k + 1.0), vec2<f32>(0.0), vec2<f32>(rows - 1.0)) + field + 0.5) / chroma.y;
    let u = (s.x * f32(textureDimensions(luma).x) / f32(p.per.x) + p.site.x) / chroma.x;
    var c0: vec2<f32>;
    var c1: vec2<f32>;
    if p.flags.z == 1u {
        c0 = textureSampleLevel(chroma_u, bilinear, vec2<f32>(u, v.x), 0.0).rg;
        c1 = textureSampleLevel(chroma_u, bilinear, vec2<f32>(u, v.y), 0.0).rg;
    } else {
        c0 = vec2<f32>(
            textureSampleLevel(chroma_u, bilinear, vec2<f32>(u, v.x), 0.0).r,
            textureSampleLevel(chroma_v, bilinear, vec2<f32>(u, v.x), 0.0).r,
        );
        c1 = vec2<f32>(
            textureSampleLevel(chroma_u, bilinear, vec2<f32>(u, v.y), 0.0).r,
            textureSampleLevel(chroma_v, bilinear, vec2<f32>(u, v.y), 0.0).r,
        );
    }
    let c = mix(c0, c1, at - k);
    return vec3<f32>(y, c.x, c.y);
}

@fragment
fn fs_convert(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(rgb(yuv_woven(floor(in.position.xy))), 1.0);
}

struct Table {
    ranges: vec4<f32>,
    sizes: vec4<u32>,
};

@group(1) @binding(1) var<uniform> table: Table;
@group(1) @binding(2) var table_rows: texture_2d<f32>;
@group(1) @binding(3) var table_cube: texture_3d<f32>;

fn table_place(c: vec3<f32>, low: f32, high: f32, size: u32) -> vec3<f32> {
    if low == high {
        return vec3<f32>(0.0);
    }
    return clamp((c - vec3<f32>(low)) / (high - low), vec3<f32>(0.0), vec3<f32>(1.0)) * f32(size - 1u);
}

fn table_row(i: u32) -> vec3<f32> {
    let width = table.sizes.z;
    return textureLoad(table_rows, vec2<u32>(i % width, i / width), 0).rgb;
}

fn table_cell(i: vec3<u32>) -> vec3<f32> {
    return textureLoad(table_cube, i, 0).rgb;
}

fn through_rows(c: vec3<f32>) -> vec3<f32> {
    let n = table.sizes.x;
    let t = table_place(c, table.ranges.x, table.ranges.y, n);
    let i = min(vec3<u32>(t), vec3<u32>(n - 2u));
    let f = t - vec3<f32>(i);
    return vec3<f32>(
        mix(table_row(i.x).x, table_row(i.x + 1u).x, f.x),
        mix(table_row(i.y).y, table_row(i.y + 1u).y, f.y),
        mix(table_row(i.z).z, table_row(i.z + 1u).z, f.z),
    );
}

fn through_cube(c: vec3<f32>) -> vec3<f32> {
    let n = table.sizes.y;
    let t = table_place(c, table.ranges.z, table.ranges.w, n);
    let i = min(vec3<u32>(t), vec3<u32>(n - 2u));
    let f = t - vec3<f32>(i);
    let near = mix(
        mix(table_cell(i), table_cell(i + vec3<u32>(1u, 0u, 0u)), f.x),
        mix(table_cell(i + vec3<u32>(0u, 1u, 0u)), table_cell(i + vec3<u32>(1u, 1u, 0u)), f.x),
        f.y,
    );
    let far = mix(
        mix(table_cell(i + vec3<u32>(0u, 0u, 1u)), table_cell(i + vec3<u32>(1u, 0u, 1u)), f.x),
        mix(table_cell(i + vec3<u32>(0u, 1u, 1u)), table_cell(i + vec3<u32>(1u, 1u, 1u)), f.x),
        f.y,
    );
    return mix(near, far, f.z);
}

fn looked_up(c: vec3<f32>) -> vec3<f32> {
    var out = c;
    if table.sizes.x > 0u {
        out = through_rows(out);
    }
    if table.sizes.y > 0u {
        out = through_cube(out);
    }
    return out;
}

@fragment
fn fs_convert_lut(in: Out) -> @location(0) vec4<f32> {
    let px = floor(in.position.xy);
    let c = rgb(yuv_woven(px));
    if !premultiplied() {
        return vec4<f32>(looked_up(c), 1.0);
    }
    let a = stored_opacity((px + 0.5) / stored_size() * p.upright.zw);
    if a <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return vec4<f32>(looked_up(c / a) * a, 1.0);
}

@fragment
fn fs_lut(in: Out) -> @location(0) vec4<f32> {
    let c = textureLoad(timeline, vec2<i32>(in.position.xy), 0);
    if c.a <= 0.0 {
        return c;
    }
    return vec4<f32>(looked_up(c.rgb / c.a) * c.a, c.a);
}

@group(0) @binding(1) var before: texture_2d<f32>;
@group(0) @binding(2) var middle: texture_2d<f32>;
@group(0) @binding(3) var after: texture_2d<f32>;

const LOW = vec2<f32>(4309.0, 213.0) / 8192.0;
const HIGH = vec3<f32>(5570.0, 3801.0, 1016.0) / 8192.0;
const ALONE = vec2<f32>(5077.0, 981.0) / 8192.0;

fn line_of(t: texture_2d<f32>, x: i32, y: i32) -> vec3<f32> {
    let h = i32(textureDimensions(t).y);
    return textureLoad(t, vec2<i32>(x, clamp(y, 0, h - 1)), 0).rgb;
}

fn both(x: i32, y: i32) -> vec3<f32> {
    return line_of(before, x, y) + line_of(middle, x, y);
}

@fragment
fn fs_bwdif(in: Out) -> @location(0) vec4<f32> {
    let x = i32(in.position.x);
    let y = i32(in.position.y);
    let h = i32(textureDimensions(middle).y);
    let kept = select(1, 0, p.shrunk.y < 1.5);
    if (y & 1) == kept {
        return vec4<f32>(line_of(middle, x, y), 1.0);
    }
    let up = select(1, -1, y > 0);
    let down = select(-1, 1, y + 1 < h);
    let c = line_of(middle, x, y + up);
    let e = line_of(middle, x, y + down);
    if p.shrunk.z < 0.5 {
        let up3 = select(1, -3, y > 2);
        let down3 = select(-1, 3, y + 3 < h);
        let far = line_of(middle, x, y + up3) + line_of(middle, x, y + down3);
        return vec4<f32>(ALONE.x * (c + e) - ALONE.y * far, 1.0);
    }
    let p0 = line_of(before, x, y);
    let n0 = line_of(middle, x, y);
    let d = (p0 + n0) * 0.5;
    let t0 = abs(p0 - n0);
    let t1 = (abs(line_of(before, x, y + up) - c) + abs(line_of(before, x, y + down) - e)) * 0.5;
    let t2 = (abs(line_of(after, x, y + up) - c) + abs(line_of(after, x, y + down) - e)) * 0.5;
    var diff = max(max(t0 * 0.5, t1), t2);
    let edge = y < 4 || y + 5 > h;
    if !edge || !(y < 2 || y + 3 > h) {
        let b = both(x, y - 2) * 0.5 - c;
        let f = both(x, y + 2) * 0.5 - e;
        let hi = max(max(d - e, d - c), min(b, f));
        let lo = min(min(d - e, d - c), max(b, f));
        diff = max(max(diff, lo), -hi);
    }
    var made = (c + e) * 0.5;
    if !edge {
        let far = line_of(middle, x, y - 3) + line_of(middle, x, y + 3);
        let high = HIGH.x * (p0 + n0) - HIGH.y * (both(x, y - 2) + both(x, y + 2))
            + HIGH.z * (both(x, y - 4) + both(x, y + 4));
        let moving = high * 0.25 + LOW.x * (c + e) - LOW.y * far;
        let still = ALONE.x * (c + e) - ALONE.y * far;
        made = select(still, moving, abs(c - e) > t0);
    }
    let held = clamp(made, d - diff, d + diff);
    return vec4<f32>(select(held, d, diff == vec3<f32>(0.0)), 1.0);
}

@fragment
fn fs_halve(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(halved));
    let at = min(floor(in.position.xy) * 2.0 + 1.0, size - 0.5);
    return textureSampleLevel(halved, bilinear, at / size, 0.0);
}

fn placed(px: vec2<f32>) -> vec4<f32> {
    if p.made.x > 2.5 && p.made.x < 3.5 {
        return textureLoad(halved, vec2<i32>(px), 0) * p.placed.y;
    }
    let s = shown(px);
    return vec4<f32>(s.rgb * select(s.a, placed_share, premultiplied()), s.a);
}

const BT709 = vec3<f32>(0.2126, 0.7152, 0.0722);
const BT601 = vec3<f32>(0.299, 0.587, 0.114);

fn spread(c: vec3<f32>) -> f32 {
    let d = c - c.gbr;
    return sqrt(dot(d, d));
}

fn ycc(c: vec3<f32>) -> vec3<f32> {
    let y = dot(c, BT601);
    return vec3<f32>(y, (c.b - y) / (2.0 * (1.0 - BT601.b)), (c.r - y) / (2.0 * (1.0 - BT601.r)));
}

fn from_ycc(c: vec3<f32>) -> vec3<f32> {
    return c.x + c.y * vec3<f32>(-0.001336, -0.343456, 1.770592) + c.z * vec3<f32>(1.402406, -0.714457, 0.002013);
}

fn dodged(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return select(b / (1.0 - a), vec3<f32>(1.0), a >= vec3<f32>(1.0));
}

fn dodge(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return min(dodged(b, a), vec3<f32>(1.0));
}

fn burned(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return select(1.0 - (1.0 - b) / a, vec3<f32>(0.0), a <= vec3<f32>(0.0));
}

fn burn(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return max(burned(b, a), vec3<f32>(0.0));
}

fn hard(by: vec3<f32>, other: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - by) * (1.0 - other), 2.0 * by * other, by <= vec3<f32>(0.5));
}

fn soft(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let d = select(sqrt(max(b, vec3<f32>(0.0))), ((16.0 * b - 12.0) * b + 4.0) * b, b <= vec3<f32>(0.25));
    return select(b + (2.0 * a - 1.0) * (d - b), b - (1.0 - 2.0 * a) * b * (1.0 - b), a <= vec3<f32>(0.5));
}

fn divide(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    let none = select(vec3<f32>(0.0), vec3<f32>(1.0), b > vec3<f32>(0.0));
    return select(min(b / a, vec3<f32>(1.0)), none, a <= vec3<f32>(0.0));
}

fn mixed(mode: u32, a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u, 19u: { return a + b; }
        case 2u: { return b - a; }
        case 3u: { return abs(a - b); }
        case 4u: { return a * b; }
        case 5u: { return 1.0 - (1.0 - a) * (1.0 - b); }
        case 6u: { return hard(b, a); }
        case 7u: { return hard(a, b); }
        case 8u: { return soft(a, b); }
        case 9u: { return min(a, b); }
        case 10u: { return max(a, b); }
        case 11u: { return dodge(b, a); }
        case 12u: { return burn(b, a); }
        case 13u: { return a + b - 2.0 * a * b; }
        case 14u: {
            let wide = spread(a);
            let k = select(0.0, spread(b) / wide, wide > 0.0);
            return dot(b, BT709) + k * (a - dot(a, BT709));
        }
        case 15u: {
            let wide = spread(b);
            let k = select(0.0, spread(a) / wide, wide > 0.0);
            let lb = dot(b, BT709);
            return lb + k * (b - lb);
        }
        case 16u: { return from_ycc(vec3<f32>(ycc(b).x, ycc(a).yz)); }
        case 17u: { return from_ycc(vec3<f32>(ycc(a).x, ycc(b).yz)); }
        case 18u: { return divide(b, a); }
        case 20u: { return a + b - 1.0; }
        case 21u: { return b + 2.0 * a - 1.0; }
        case 22u: { return select(dodge(b, 2.0 * a - 1.0), burn(b, 2.0 * a), a <= vec3<f32>(0.5)); }
        case 23u: { return select(max(b, 2.0 * a - 1.0), min(b, 2.0 * a), a <= vec3<f32>(0.5)); }
        case 24u: { return select(vec3<f32>(0.0), vec3<f32>(1.0), a + b >= vec3<f32>(1.0)); }
        case 25u: { return select(b, a, dot(a, vec3<f32>(1.0)) > dot(b, vec3<f32>(1.0))); }
        case 26u: { return select(b, a, dot(a, vec3<f32>(1.0)) < dot(b, vec3<f32>(1.0))); }
        default: { return a; }
    }
}

@group(1) @binding(0) var below: texture_2d<f32>;

fn own_color(c: vec3<f32>) -> vec3<f32> {
    return select(c, c / file_alpha, p.site.w > 2.5 && file_alpha > 0.0);
}

fn mode_mixed(mode: u32, c: vec3<f32>, under: vec4<f32>) -> vec3<f32> {
    let b = select(under.rgb, under.rgb / under.a, under.a > 0.0);
    return mix(c, mixed(mode, c, b), under.a);
}

@fragment
fn fs_mode(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    let top = shown(px);
    let under = textureLoad(below, vec2<i32>(px), 0);
    let c = own_color(top.rgb);
    if p.placed.w > 1.5 {
        var alone = c;
        if under.a > 0.0 {
            alone = mix(c, mixed(u32(p.placed.z + 0.5), c, under.rgb / under.a), under.a);
        }
        return vec4<f32>(alone * top.a, top.a);
    }
    let m = mode_mixed(u32(p.placed.z + 0.5), c, under);
    if p.placed.w > 0.5 {
        return vec4<f32>(m * top.a, top.a);
    }
    return vec4<f32>(m * top.a + under.rgb * (1.0 - top.a), top.a + under.a * (1.0 - top.a));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    if p.placed.x > 0.5 {
        if (p.made.x > 0.5 && p.made.x < 1.5) || p.made.w > 0.5 {
            let c = shown(px + p.part.xy).rgb;
            if matte() {
                return vec4<f32>(vec3<f32>(file_alpha), 1.0);
            }
            return vec4<f32>(c * select(1.0, file_alpha, p.site.w > 0.5 && !premultiplied()), 1.0);
        }
        return placed(px);
    }
    let keeps = (p.flags.w & 16u) != 0u;
    let sides = vec4<f32>(px - p.place.xy, p.place.xy + p.place.zw - px);
    let e = clamp(sides + 0.5, vec4<f32>(0.0), vec4<f32>(1.0));
    let edge = select(e.x * e.y * e.z * e.w, 1.0, p.shrunk.w > 0.5);
    if edge <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, select(1.0, 0.0, keeps));
    }
    let rel = (px - p.place.xy) / p.place.zw;
    let cover = p.upright.xy / p.place.zw;
    let a = opacity(clamp(rel, vec2<f32>(0.0), vec2<f32>(1.0))) * edge;
    if matte() {
        return vec4<f32>(vec3<f32>(a), 1.0);
    }
    let solid = select(1.0, a, keeps);
    let share = select(a, edge, premultiplied());
    if resamples(rel, cover) {
        return vec4<f32>(sharp(rel, cover) * share, solid);
    }
    let taps = box_taps(cover);
    let step = vec2<f32>(1.0) / (p.place.zw * vec2<f32>(taps));
    let first = (px - vec2<f32>(0.5) - p.place.xy) / p.place.zw + step * 0.5;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            sum += pixel(first + vec2<f32>(f32(i), f32(j)) * step);
        }
    }
    return vec4<f32>(sum / f32(taps.x * taps.y) * share, solid);
}

struct Lettering {
    across: vec4<f32>,
    down: vec4<f32>,
    tilt: vec4<f32>,
    keep: vec4<f32>,
};

@group(0) @binding(5) var<uniform> lettering: Lettering;
@group(0) @binding(6) var letters: texture_2d<f32>;

@fragment
fn fs_title(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    let m = lettering.across;
    let k = lettering.down;
    let w = dot(vec3<f32>(px, 1.0), vec3<f32>(lettering.tilt.xy, 1.0));
    if abs(w) < 1e-6 {
        discard;
    }
    let at = vec2<f32>(dot(m.xy, px) + m.z, dot(k.xy, px) + k.z) / w;
    let across = vec2<f32>(m.x - at.x * lettering.tilt.x, k.x - at.y * lettering.tilt.x) / w;
    let down = vec2<f32>(m.y - at.x * lettering.tilt.y, k.y - at.y * lettering.tilt.y) / w;
    let size = vec2<f32>(textureDimensions(letters));
    let reach = (abs(across) + abs(down)) * 0.5 + vec2<f32>(0.5) / size;
    if any(at + reach <= vec2<f32>(0.0)) || any(at - reach >= vec2<f32>(1.0)) {
        discard;
    }
    let spans = vec2<f32>(length(across * size), length(down * size));
    let taps = box_taps(spans);
    var sum = vec4<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / vec2<f32>(taps) - 0.5;
            let q = at + across * o.x + down * o.y;
            let t = q * size;
            let e = clamp(t + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * clamp(size + 0.5 - t, vec2<f32>(0.0), vec2<f32>(1.0));
            let kept = clamp((q - lettering.keep.xy) * size + 0.5, vec2<f32>(0.0), vec2<f32>(1.0))
                * clamp((lettering.keep.zw - q) * size + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
            sum += textureSampleLevel(letters, bilinear, q, 0.0) * (e.x * e.y * kept.x * kept.y);
        }
    }
    var shown = sum / f32(taps.x * taps.y);
    let transfer = u32(lettering.tilt.z + 0.5);
    if transfer != 0u && shown.a > 0.0 {
        shown = vec4<f32>(widened(shown.rgb / shown.a, transfer) * shown.a, shown.a);
    }
    return shown * m.w;
}

fn widened(rgb: vec3<f32>, transfer: u32) -> vec3<f32> {
    let s = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let lin = select(pow((s + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), s / 12.92, s <= vec3<f32>(0.04045));
    let wide = vec3<f32>(
        dot(vec3<f32>(0.6274, 0.3293, 0.0433), lin),
        dot(vec3<f32>(0.0691, 0.9195, 0.0114), lin),
        dot(vec3<f32>(0.0164, 0.0880, 0.8956), lin),
    );
    return clamp(encode(wide, transfer), vec3<f32>(0.0), vec3<f32>(1.0));
}

struct Motion {
    offsets: vec4<f32>,
    picture: vec4<f32>,
    band: vec4<f32>,
    edge: vec4<f32>,
    blur: vec4<f32>,
    color: vec4<f32>,
    flags: vec4<u32>,
};

@group(0) @binding(11) var<uniform> motion: Motion;
@group(0) @binding(12) var outgoing: texture_2d<f32>;
@group(0) @binding(13) var incoming: texture_2d<f32>;

fn outgoing_at(q: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(outgoing));
    if any(q < vec2<f32>(0.0)) || any(q >= size) {
        return vec4<f32>(0.0);
    }
    return textureLoad(outgoing, vec2<i32>(q), 0);
}

fn incoming_at(q: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(incoming));
    if any(q < vec2<f32>(0.0)) || any(q >= size) {
        return vec4<f32>(0.0);
    }
    return textureLoad(incoming, vec2<i32>(q), 0);
}

fn inside(c: vec2<f32>, r: vec4<f32>) -> bool {
    return all(c >= r.xy) && all(c < r.zw);
}

fn edge_ramp(q: f32, edge: f32, facing: f32, b: f32) -> f32 {
    if facing > 0.0 {
        return clamp((edge - q - 1.0) / b, 0.0, 1.0);
    }
    if facing < 0.0 {
        return clamp((q - edge) / b, 0.0, 1.0);
    }
    return 1.0;
}

fn feathered(q: vec2<f32>) -> f32 {
    let b = motion.blur.w;
    if b <= 0.0 {
        return 1.0;
    }
    let x = edge_ramp(q.x, motion.edge.x, motion.edge.z, b);
    let y = edge_ramp(q.y, motion.edge.y, motion.edge.w, b);
    return min(x, y);
}

struct Tap {
    color: vec4<f32>,
    cover: f32,
};

fn tap(q: vec2<f32>) -> Tap {
    var t: Tap;
    if motion.flags.w == 1u {
        t.color = outgoing_at(q);
        t.cover = 0.0;
        return t;
    }
    let upper_in = motion.flags.x == 1u;
    let u = q - motion.offsets.xy;
    let l = q - motion.offsets.zw;
    var upper: vec4<f32>;
    var lower: vec4<f32>;
    if upper_in {
        upper = incoming_at(u);
        lower = outgoing_at(l);
    } else {
        upper = outgoing_at(u);
        lower = incoming_at(l);
    }
    let c = q + vec2<f32>(0.5);
    let whole = vec4<f32>(ceil(motion.picture.xy), floor(motion.picture.zw));
    var block = max(upper.a, select(0.0, 1.0, inside(c, whole)));
    if motion.flags.y == 1u {
        let f = feathered(q);
        upper *= f;
        block *= f;
    } else if inside(c, motion.band) && !inside(c, motion.picture) {
        let transfer = u32(motion.color.w + 0.5);
        var band = motion.color.rgb;
        if transfer != 0u {
            band = widened(band, transfer);
        }
        t.color = vec4<f32>(band, 1.0);
        t.cover = 1.0;
        return t;
    }
    t.color = upper + lower * (1.0 - block);
    t.cover = block;
    return t;
}

@fragment
fn fs_motion(in: Out) -> @location(0) vec4<f32> {
    return moved(floor(in.position.xy));
}

fn moved(q: vec2<f32>) -> vec4<f32> {
    let here = tap(q);
    let len = motion.blur.z;
    if len < 0.5 {
        return here.color;
    }
    let size = vec2<f32>(textureDimensions(outgoing));
    let n = u32(clamp(ceil(len) + 1.0, 2.0, 512.0));
    var sum = vec4<f32>(0.0);
    var cover = 0.0;
    for (var i = 0u; i < n; i++) {
        let s = len * f32(i) / f32(n - 1u);
        let at = clamp(floor(q + vec2<f32>(0.5) - motion.blur.xy * s), vec2<f32>(0.0), size - vec2<f32>(1.0));
        let t = tap(at);
        sum += t.color;
        cover += t.cover;
    }
    let boxed = sum / f32(n);
    if motion.flags.z == 0u {
        return boxed;
    }
    return mix(here.color, boxed, cover / f32(n));
}

struct Screen {
    tone: vec4<f32>,
    gamut0: vec4<f32>,
    gamut1: vec4<f32>,
    gamut2: vec4<f32>,
    mode: vec4<f32>,
};

@group(0) @binding(8) var<uniform> screen: Screen;
@group(0) @binding(9) var timeline: texture_2d<f32>;

@fragment
fn fs_screen(in: Out) -> @location(0) vec4<f32> {
    let code = textureLoad(timeline, vec2<i32>(in.position.xy), 0).rgb;
    let transfer = u32(screen.mode.x + 0.5);
    if transfer == 0u {
        return vec4<f32>(clamp(code, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    var c = decode(code, transfer, screen.mode.y);
    c = vec3<f32>(dot(screen.gamut0.xyz, c), dot(screen.gamut1.xyz, c), dot(screen.gamut2.xyz, c));
    return vec4<f32>(clamp(tone_mapped(c, screen.tone), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

struct Encoding {
    tone: vec4<f32>,
    gamut0: vec4<f32>,
    gamut1: vec4<f32>,
    gamut2: vec4<f32>,
    matrix: vec4<f32>,
    how: vec4<u32>,
    plane: vec4<u32>,
    site: vec4<f32>,
    alpha: vec4<u32>,
};

@group(0) @binding(10) var<uniform> encoding: Encoding;

fn encoded_rgb(code: vec3<f32>) -> vec3<f32> {
    let coded = encoding.how.x;
    let wanted = encoding.how.y;
    if coded == wanted && encoding.how.z == 32u {
        return code;
    }
    if coded == wanted {
        return clamp(code, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    var c: vec3<f32>;
    if coded == 0u {
        c = pow(max(code, vec3<f32>(0.0)), vec3<f32>(2.4));
    } else {
        c = decode(code, coded, encoding.matrix.z);
    }
    c = vec3<f32>(dot(encoding.gamut0.xyz, c), dot(encoding.gamut1.xyz, c), dot(encoding.gamut2.xyz, c));
    if wanted == 0u {
        return clamp(tone_mapped(c, encoding.tone), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return clamp(encode(c, wanted), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn drawn_at(x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(timeline));
    let at = clamp(vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
    return textureLoad(timeline, at, 0);
}

fn pixel_at(x: i32, y: i32) -> vec3<f32> {
    let c = drawn_at(x, y);
    if encoding.alpha.x == 0u || c.a <= 0.0 {
        return encoded_rgb(c.rgb);
    }
    let color = encoded_rgb(c.rgb / c.a);
    return select(color, color * min(c.a, 1.0), encoding.alpha.x == 2u);
}

fn output_ycc(c: vec3<f32>) -> vec3<f32> {
    let kr = encoding.matrix.x;
    let kb = encoding.matrix.y;
    let y = kr * c.r + (1.0 - kr - kb) * c.g + kb * c.b;
    return vec3<f32>(y, (c.b - y) / (2.0 - 2.0 * kb), (c.r - y) / (2.0 - 2.0 * kr));
}

fn code_of(v: f32, base: f32, span: f32) -> u32 {
    let depth = encoding.how.z;
    let scale = f32(1u << (depth - 8u));
    let top = f32((1u << depth) - 1u);
    return u32(clamp(round((base + span * v) * scale), 0.0, top)) << encoding.how.w;
}

fn covered(k: i32, at: f32, block: f32) -> f32 {
    let x = f32(k);
    return clamp(min(x + 0.5, at + block * 0.5) - max(x - 0.5, at - block * 0.5), 0.0, 1.0);
}

fn chroma_at(px: vec2<i32>) -> vec2<f32> {
    let block = vec2<f32>(f32(1u << encoding.plane.y), f32(1u << encoding.plane.z));
    let at = vec2<f32>(px) * block + encoding.site.xy;
    let low = vec2<i32>(floor(at - block * 0.5 + 0.5));
    let high = vec2<i32>(ceil(at + block * 0.5 - 0.5));
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var j = low.y; j <= high.y; j++) {
        let wy = covered(j, at.y, block.y);
        for (var i = low.x; i <= high.x; i++) {
            let w = covered(i, at.x, block.x) * wy;
            sum += output_ycc(pixel_at(i, j)).yz * w;
            total += w;
        }
    }
    return sum / total;
}

fn kept_alpha(px: vec2<i32>) -> f32 {
    return select(1.0, clamp(drawn_at(px.x, px.y).a, 0.0, 1.0), encoding.alpha.x != 0u);
}

fn alpha_code(px: vec2<i32>) -> u32 {
    let top = (1u << encoding.how.z) - 1u;
    return u32(round(kept_alpha(px) * f32(top))) << encoding.how.w;
}

fn encoded_at(px: vec2<i32>) -> vec4<u32> {
    let kind = encoding.plane.x;
    var out = vec4<u32>(0u);
    if kind == 0u {
        out.x = code_of(output_ycc(pixel_at(px.x, px.y)).x, 16.0, 219.0);
    } else if kind <= 3u {
        let c = chroma_at(px);
        let cb = code_of(c.x, 128.0, 224.0);
        let cr = code_of(c.y, 128.0, 224.0);
        out = select(vec4<u32>(cb, cr, 0u, 0u), vec4<u32>(cr, 0u, 0u, 0u), kind == 3u);
    } else if kind == 4u {
        out.x = alpha_code(px);
    } else if kind == 6u {
        let c = output_ycc(pixel_at(px.x, px.y));
        out = vec4<u32>(alpha_code(px), code_of(c.x, 16.0, 219.0), code_of(c.y, 128.0, 224.0), code_of(c.z, 128.0, 224.0));
    } else {
        let top = (1u << encoding.how.z) - 1u;
        out = vec4<u32>(vec3<u32>(round(pixel_at(px.x, px.y) * f32(top))), alpha_code(px));
    }
    return out;
}

@fragment
fn fs_encode(in: Out) -> @location(0) vec4<u32> {
    return encoded_at(vec2<i32>(in.position.xy));
}

@fragment
fn fs_encode_unorm(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(encoded_at(vec2<i32>(in.position.xy))) / f32(encoding.plane.w);
}

@fragment
fn fs_encode_float(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let kind = encoding.plane.x;
    if kind == 10u {
        return vec4<f32>(kept_alpha(px), 0.0, 0.0, 1.0);
    }
    let c = pixel_at(px.x, px.y);
    return vec4<f32>(c[kind - 7u], 0.0, 0.0, 1.0);
}

struct Blur {
    spread: vec4<f32>,
    way: vec4<i32>,
    blend: vec4<f32>,
    window: vec4<f32>,
};

@group(0) @binding(0) var<uniform> blurring: Blur;
@group(0) @binding(1) var blur_source: texture_2d<f32>;
@group(0) @binding(2) var blur_start: texture_2d<f32>;

fn mirrored(i: i32, n: i32) -> i32 {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    var m = i % period;
    if m < 0 {
        m += period;
    }
    return select(m, period - m, m >= n);
}

fn blur_folded(i: i32, n: i32) -> i32 {
    switch blurring.way.w {
        case 1: {
            return clamp(i, 0, n - 1);
        }
        case 2: {
            return mirrored(i, n);
        }
        case 3: {
            var m = i % n;
            if m < 0 {
                m += n;
            }
            return m;
        }
        case 4: {
            var m = i % (2 * n);
            if m < 0 {
                m += 2 * n;
            }
            return select(m, 2 * n - 1 - m, m >= n);
        }
        default: {
            return i;
        }
    }
}

fn blur_tap(px: vec2<i32>, k: i32, size: vec2<i32>) -> vec4<f32> {
    let down = blurring.way.x == 1;
    let n = select(size.x, size.y, down);
    let i = select(px.x, px.y, down) + k;
    if blurring.way.w == 0 && (i < 0 || i >= n) {
        return vec4<f32>(0.0);
    }
    let at = blur_folded(i, n);
    let c = textureLoad(blur_source, select(vec2<i32>(at, px.y), vec2<i32>(px.x, at), down), 0);
    if blurring.blend.w > 0.5 {
        return effect_taken(c, blurring.blend.y);
    }
    return c;
}

fn effect_taken(c: vec4<f32>, use_alpha: f32) -> vec4<f32> {
    if use_alpha < 0.5 && c.a > 0.0 {
        return vec4<f32>(c.rgb / c.a, c.a);
    }
    return c;
}

fn effect_mixed(px: vec2<i32>, made: vec4<f32>, blend: f32, use_alpha: f32) -> vec4<f32> {
    let was = textureLoad(blur_start, px, 0);
    let out = select(vec4<f32>(made.rgb * was.a, was.a), made, use_alpha > 0.5);
    return mix(was, out, blend);
}

struct Masking {
    how: vec4<f32>,
    more: vec4<f32>,
};

@group(0) @binding(0) var<uniform> masking: Masking;
@group(0) @binding(3) var<storage, read> area_maps: array<f32>;

fn area_point(i: i32) -> vec2<f32> {
    let o = i32(masking.how.w) * 9 + i * 2;
    return vec2<f32>(area_maps[o], area_maps[o + 1]);
}

fn area_sides(p: vec2<f32>) -> f32 {
    let n = i32(masking.more.x);
    var near = 1e18;
    var sign = 1.0;
    var b = area_point(n - 1);
    for (var i = 0; i < n; i++) {
        let a = area_point(i);
        let e = b - a;
        let w = p - a;
        let along = clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
        let off = w - e * along;
        near = min(near, dot(off, off));
        let up = p.y >= a.y;
        let under = p.y < b.y;
        let left = e.x * w.y > e.y * w.x;
        if (up && under && left) || (!up && !under && !left) {
            sign = -sign;
        }
        b = a;
    }
    return sign * sqrt(near);
}

fn area_reach(i: i32, at: vec3<f32>) -> f32 {
    let o = i * 9;
    let across = vec3<f32>(area_maps[o], area_maps[o + 1], area_maps[o + 2]);
    let down = vec3<f32>(area_maps[o + 3], area_maps[o + 4], area_maps[o + 5]);
    let far = vec3<f32>(area_maps[o + 6], area_maps[o + 7], area_maps[o + 8]);
    let depth = dot(far, at);
    if depth <= 1e-6 {
        return 1e9;
    }
    let q = vec2<f32>(dot(across, at), dot(down, at)) / depth;
    if masking.how.x > 1.5 {
        return 1.0 + 2.0 * area_sides(q * 0.5 + 0.5);
    }
    return select(length(q), max(abs(q.x), abs(q.y)), masking.how.x > 0.5);
}

fn area_inside(i: i32, xy: vec2<f32>) -> f32 {
    let r = area_reach(i, vec3<f32>(xy, 1.0));
    let across = area_reach(i, vec3<f32>(xy + vec2<f32>(1.0, 0.0), 1.0));
    let down = area_reach(i, vec3<f32>(xy + vec2<f32>(0.0, 1.0), 1.0));
    let band = max(max(masking.how.y, abs(across - r) + abs(down - r)), 1e-6);
    let t = clamp((1.0 - r) / band, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

@fragment
fn fs_area(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    var outside = 1.0;
    for (var i = 0; i < i32(masking.how.w); i++) {
        outside *= 1.0 - area_inside(i, in.position.xy);
    }
    let share = select(1.0 - outside, outside, masking.how.z > 0.5);
    return mix(textureLoad(blur_start, px, 0), textureLoad(blur_source, px, 0), share);
}

fn blur_end(d: f32) -> vec4<f32> {
    if blurring.window.x < 0.5 {
        return vec4<f32>(1.0);
    }
    let reach = max(blurring.spread, vec4<f32>(1e-6));
    return 1.0 - smoothstep(blurring.window.yyyy, blurring.window.zzzz, d / reach);
}

@fragment
fn fs_blur(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let size = vec2<i32>(textureDimensions(blur_source));
    let wide = blurring.spread > vec4<f32>(0.0);
    let k = select(vec4<f32>(0.0), 0.5 / (blurring.spread * blurring.spread), wide);
    let step = blurring.way.y;
    var sum = blur_tap(px, 0, size);
    var total = vec4<f32>(1.0);
    for (var j = 1; j <= blurring.way.z; j++) {
        let d = f32(j * step);
        let w = select(vec4<f32>(0.0), exp(-d * d * k) * blur_end(d), wide);
        sum += (blur_tap(px, j * step, size) + blur_tap(px, -j * step, size)) * w;
        total += 2.0 * w;
    }
    let out = sum / total;
    if blurring.blend.z > 0.5 {
        return effect_mixed(px, out, blurring.blend.x, blurring.blend.y);
    }
    return out;
}

struct Vignetting {
    first: vec4<f32>,
    color: vec4<f32>,
    way: vec4<i32>,
};

@group(0) @binding(0) var<uniform> vignetting: Vignetting;
@group(0) @binding(3) var<storage, read> vignette_cells: array<f32>;

const GRAY_HUE = vec3<f32>(-0.2625036, 0.14574469, -0.67075189);

fn vignette_mixed(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let mode = u32(vignetting.way.z);
    switch mode {
        case 11u: { return dodged(b, a); }
        case 12u: { return burned(b, a); }
        case 14u: {
            if spread(a) > 0.0 {
                return mixed(mode, a, b);
            }
            if a.r <= 0.0 || a.r >= 1.0 {
                return mixed(mode, vec3<f32>(1.0, 0.0, 0.0), b);
            }
            return dot(b, BT709) + spread(b) * GRAY_HUE;
        }
        case 18u: {
            return b / (max(a, vec3<f32>(0.0)) + 1e-6);
        }
        case 20u: { return max(a + b - 1.0, vec3<f32>(0.0)); }
        case 22u: { return select(dodged(b, 2.0 * a - 1.0), burned(b, 2.0 * a), a <= vec3<f32>(0.5)); }
        case 25u: { return select(b, a, dot(a, vec3<f32>(1.0)) >= dot(b, vec3<f32>(1.0))); }
        case 26u: { return select(b, a, dot(a, vec3<f32>(1.0)) <= dot(b, vec3<f32>(1.0))); }
        default: { return mixed(mode, a, b); }
    }
}

fn vignette_cell(x: i32, y: i32) -> f32 {
    let n = vignetting.way.xy;
    return vignette_cells[clamp(y, 0, n.y - 1) * n.x + clamp(x, 0, n.x - 1)];
}

fn vignette_kept(x: i32, y: i32, wide: f32, least: f32) -> f32 {
    return min(vignette_cell(x, y) / wide - least, 1.0);
}

fn vignette_shape(px: vec2<i32>) -> f32 {
    let n = vignetting.way.xy;
    let top = vignette_cell(n.x / 2, n.y / 2);
    let least = vignette_cell(0, 0);
    let wide = top - least;
    if wide <= 0.0 {
        return 0.0;
    }
    let at = (vec2<f32>(px) - vignetting.first.xy) * 0.5;
    let whole = floor(at);
    let part = at - whole;
    let i = vec2<i32>(whole);
    let upper = mix(vignette_kept(i.x, i.y, wide, least), vignette_kept(i.x + 1, i.y, wide, least), part.x);
    let lower = mix(vignette_kept(i.x, i.y + 1, wide, least), vignette_kept(i.x + 1, i.y + 1, wide, least), part.x);
    return mix(upper, lower, part.y);
}

@fragment
fn fs_vignette(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let use_alpha = vignetting.color.w;
    let was = effect_taken(textureLoad(blur_source, px, 0), use_alpha);
    let share = (1.0 - vignette_shape(px)) * vignetting.first.z;
    let m = vignette_mixed(vignetting.color.rgb, was.rgb);
    let made = clamp(was.rgb + (m - was.rgb) * share, vec3<f32>(0.0), vec3<f32>(1.0));
    return effect_mixed(px, vec4<f32>(made, was.a), vignetting.first.w, use_alpha);
}

struct Sharpening {
    more: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sharpening: Sharpening;
@group(0) @binding(3) var<storage, read> sharpen_planes: array<f32>;

@fragment
fn fs_sharpen_taken(in: Out) -> @location(0) vec4<f32> {
    return effect_taken(textureLoad(blur_source, vec2<i32>(in.position.xy), 0), sharpening.more.y);
}

@fragment
fn fs_sharpen(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let was = textureLoad(blur_source, px, 0);
    let size = vec2<i32>(textureDimensions(blur_source));
    let at = px.y * size.x + px.x;
    var red = 0.0;
    var blue = 0.0;
    if sharpening.more.z > 0.5 {
        let plane = size.x * size.y;
        red = sharpen_planes[plane + at];
        blue = sharpen_planes[2 * plane + at];
    }
    let green = -(BT709.r * red + BT709.b * blue) / BT709.g;
    let made = was.rgb + sharpen_planes[at] + vec3<f32>(red, green, blue);
    return effect_mixed(px, vec4<f32>(made, was.a), sharpening.more.x, sharpening.more.y);
}

struct Mosaic {
    grid: vec4<f32>,
    way: vec4<i32>,
    more: vec4<f32>,
    alpha: vec4<f32>,
};

@group(0) @binding(0) var<uniform> mosaicking: Mosaic;

const MOSAIC_NEAR: f32 = 0.00390625;

const MOSAIC_WHOLE: f32 = 0.999;

fn mosaic_odd(v: f32) -> bool {
    return v - 2.0 * floor(v / 2.0) > 0.5;
}

fn mosaic_x(c: f32) -> f32 {
    let half = select(0.0, 0.5, mosaicking.way.z == 0);
    return mosaicking.grid.z + (c + half) * mosaicking.more.x;
}

fn mosaic_y(c: f32, r: f32) -> f32 {
    var part = 0.5;
    if mosaicking.way.z == 1 {
        part = select(0.0, 0.5, mosaic_odd(c));
    } else if mosaicking.way.z == 2 {
        part = select(1.0 / 3.0, 2.0 / 3.0, mosaic_odd(c + r));
    }
    return mosaicking.grid.w + (r + part) * mosaicking.grid.y;
}

fn mosaic_nearest(p: vec2<f32>) -> vec2<f32> {
    let exact = p - mosaicking.grid.zw;
    let at = exact + vec2<f32>(MOSAIC_NEAR);
    let step = mosaicking.more.x;
    let high = mosaicking.grid.y;
    let first = floor(at.x / step);
    if mosaicking.way.z == 1 {
        let wide = high / mosaicking.grid.x;
        var best = vec2<f32>(0.0);
        var least = -1.0;
        for (var d = 0.0; d <= 1.0; d += 1.0) {
            let c = first + d;
            let shift = select(0.0, 0.5, mosaic_odd(c));
            let r = floor(at.y / high - shift + 0.5);
            let off = vec2<f32>((exact.x - c * step) * wide, exact.y - (r + shift) * high);
            let far = dot(off, off);
            if least < 0.0 || far < least {
                least = far;
                best = vec2<f32>(c, r);
            }
        }
        return best;
    }
    let r = floor(at.y / high);
    if mosaicking.way.z == 2 {
        let across = exact.x / step - first;
        let down = exact.y / high - r;
        let inside = select(across < down, across < 1.0 - down, !mosaic_odd(first + r));
        return vec2<f32>(select(first + 1.0, first, inside), r);
    }
    return vec2<f32>(first, r);
}

fn mosaic_column(slot: i32, n: i32) -> vec2<f32> {
    let samples = mosaicking.way.w;
    let low = slot * samples;
    let step = mosaicking.more.x;
    let gap = f32(low) - mosaicking.grid.z;
    if step < f32(samples) {
        return vec2<f32>(f32(low), floor(gap / step));
    }
    let c = floor(gap / step);
    for (var d = -1.0; d <= 1.0; d += 1.0) {
        let x = i32(floor(mosaic_x(c + d) + MOSAIC_NEAR));
        if x >= low && x < low + samples {
            return vec2<f32>(f32(x), c + d);
        }
    }
    if low == 0 {
        return vec2<f32>(0.0, c);
    }
    if low + samples >= n {
        return vec2<f32>(f32(n - 1), c + 1.0);
    }
    return vec2<f32>(-1.0, 0.0);
}

fn mosaic_row(c: f32, slot: i32, n: i32) -> i32 {
    let samples = mosaicking.way.w;
    if samples == 1 {
        return slot;
    }
    let low = slot * samples;
    let r = floor((f32(low) - mosaicking.grid.w) / mosaicking.grid.y);
    for (var d = -1.0; d <= 1.0; d += 1.0) {
        let y = i32(floor(mosaic_y(c, r + d) + MOSAIC_NEAR));
        if y >= low && y < low + samples {
            return y;
        }
    }
    return select(low, n - 1, low + samples >= n);
}

fn mosaic_load(i: i32, at: i32, down: bool) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(blur_source));
    if down {
        return textureLoad(blur_source, vec2<i32>(at, clamp(i, 0, size.y - 1)), 0);
    }
    let c = textureLoad(blur_source, vec2<i32>(clamp(i, 0, size.x - 1), at), 0);
    return effect_taken(c, mosaicking.more.z);
}

fn mosaic_tap(s: i32, at: i32, down: bool) -> vec4<f32> {
    if mosaicking.way.w == 1 {
        return mosaic_load(s, at, down);
    }
    if s % 2 == 1 {
        let m = (s - 1) / 2;
        return mix(mosaic_load(m, at, down), mosaic_load(m + 1, at, down), 0.25);
    }
    let m = s / 2 - 1;
    return mix(mosaic_load(m, at, down), mosaic_load(m + 1, at, down), 0.75);
}

fn mosaic_box(middle: i32, reach: i32, n: i32, at: i32, down: bool) -> vec4<f32> {
    let first = max(middle - reach, 0);
    let last = min(middle + reach, n - 1);
    var sum = mosaic_tap(0, at, down) * f32(first - (middle - reach));
    sum += mosaic_tap(n - 1, at, down) * f32(middle + reach - last);
    for (var s = first; s <= last; s++) {
        sum += mosaic_tap(s, at, down);
    }
    return sum / f32(2 * reach + 1);
}

fn mosaic_cell(p: vec2<f32>, n: vec2<i32>) -> vec4<f32> {
    let cell = mosaic_nearest(p);
    let x = clamp(i32(floor(mosaic_x(cell.x) + MOSAIC_NEAR)), 0, n.x - 1);
    let y = clamp(i32(floor(mosaic_y(cell.x, cell.y) + MOSAIC_NEAR)), 0, n.y - 1);
    return textureLoad(blur_source, vec2<i32>(x, y) / mosaicking.way.w, 0);
}

fn mosaic_alpha(cell: vec4<f32>, was: vec4<f32>) -> vec4<f32> {
    if mosaicking.alpha.x > 0.5 {
        return vec4<f32>(cell.rgb, was.a);
    }
    if mosaicking.alpha.y < 0.5 {
        return cell;
    }
    if cell.a >= MOSAIC_WHOLE {
        return vec4<f32>(cell.rgb, 1.0);
    }
    return vec4<f32>(was.rgb, 0.0);
}

@fragment
fn fs_mosaic(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let samples = mosaicking.way.w;
    let n = vec2<i32>(textureDimensions(blur_source)) * samples;
    if mosaicking.more.w < 1.5 {
        let column = mosaic_column(px.x, n.x);
        if column.x < 0.0 {
            return vec4<f32>(0.0);
        }
        if mosaicking.more.w < 0.5 {
            return mosaic_box(i32(column.x), mosaicking.way.x, n.x, px.y, false);
        }
        let row = mosaic_row(column.y, px.y, n.y);
        return mosaic_box(row, mosaicking.way.y, n.y, px.x, true);
    }
    let was = textureLoad(blur_start, px, 0);
    var sum = vec4<f32>(0.0);
    for (var b = 0; b < samples; b++) {
        for (var a = 0; a < samples; a++) {
            sum += mosaic_alpha(mosaic_cell(vec2<f32>(px * samples + vec2<i32>(a, b)), n), was);
        }
    }
    let out = sum / f32(samples * samples);
    return effect_mixed(px, out, mosaicking.more.y, mosaicking.more.z);
}

struct Glow {
    shine: vec4<f32>,
    tint: vec4<f32>,
    second: vec4<f32>,
    mixing: vec4<f32>,
    modes: vec4<i32>,
    grid_one: vec4<f32>,
    grid_two: vec4<f32>,
    sharp_one: vec4<f32>,
    sharp_two: vec4<f32>,
    noise: vec4<f32>,
    depth: vec4<f32>,
};

@group(0) @binding(0) var<uniform> glowing: Glow;
@group(0) @binding(14) var glow_second: texture_2d<f32>;
@group(0) @binding(15) var glow_mask: texture_2d<f32>;

const GLOW_INVERT: i32 = 1;
const GLOW_SEEN: i32 = 2;
const GLOW_TWO: i32 = 4;
const GLOW_NOISE: i32 = 8;
const GLOW_ALPHA_SOURCE: i32 = 2;
const GLOW_ROOM: f32 = 0.001;
const GLOW_RECOVERY: f32 = 12.0 / 7.0;
const GLOW_EDGE: f32 = 4.0;
const GLOW_EDGE_WIDTH: f32 = 3.5;
const GLOW_OCTAVES: i32 = 7;
const GLOW_NOISE_GAIN: f32 = 1.9;
const GLOW_LAYERS: f32 = 17.0;

fn glow_has(flag: i32) -> bool {
    return (glowing.modes.w & flag) != 0;
}

fn glow_taken(px: vec2<i32>, picture: texture_2d<f32>) -> vec4<f32> {
    let use_alpha = glowing.mixing.w;
    let c = effect_taken(textureLoad(picture, px, 0), use_alpha);
    var rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if glow_has(GLOW_INVERT) {
        rgb = 1.0 - rgb;
    }
    return vec4<f32>(rgb, c.a);
}

fn glow_shining(c: vec3<f32>) -> vec3<f32> {
    let t = glowing.shine.x;
    let m = max(c - t, vec3<f32>(0.0)) / (1.0 - t + GLOW_ROOM);
    return (1.0 - t) * m + t * (m.r + m.g + m.b) / 3.0;
}

@fragment
fn fs_glow_mask(in: Out) -> @location(0) vec4<f32> {
    let c = glow_taken(vec2<i32>(in.position.xy), blur_source);
    if glowing.modes.y == GLOW_ALPHA_SOURCE {
        let th = glowing.shine.y;
        return vec4<f32>(vec3<f32>(max(c.a - th + GLOW_ROOM, 0.0) / (1.0 - th + GLOW_ROOM)), 1.0);
    }
    return vec4<f32>(c.a * glow_shining(c.rgb), 1.0);
}

struct GlowCells {
    step: vec4<i32>,
    ends: vec4<i32>,
};

@group(0) @binding(0) var<uniform> glow_cells: GlowCells;

fn glow_cell_at(x: i32, low: i32, last: i32, n: i32) -> i32 {
    if glow_cells.step.z == 1 {
        return select(x, -1, x < 0 || x >= n);
    }
    return clamp(mirrored(x - low, last - low + 1) + low, 0, n - 1);
}

@fragment
fn fs_glow_cells(in: Out) -> @location(0) vec4<f32> {
    let node = vec2<i32>(in.position.xy);
    let n = vec2<i32>(textureDimensions(blur_source));
    let d = glow_cells.step.xy;
    let low = glow_cells.ends.xy;
    let at = low + node * d;
    var sum = vec4<f32>(0.0);
    for (var b = 1 - d.y; b < d.y; b++) {
        let y = glow_cell_at(at.y + b, low.y, glow_cells.ends.w, n.y);
        if y < 0 {
            continue;
        }
        let wy = f32(d.y - abs(b));
        for (var a = 1 - d.x; a < d.x; a++) {
            let x = glow_cell_at(at.x + a, low.x, glow_cells.ends.z, n.x);
            if x < 0 {
                continue;
            }
            sum += wy * f32(d.x - abs(a)) * textureLoad(blur_source, vec2<i32>(x, y), 0);
        }
    }
    return sum / f32(d.x * d.x * d.y * d.y);
}

fn glow_weights(f: f32) -> vec4<f32> {
    let f2 = f * f;
    let f3 = f2 * f;
    return vec4<f32>(
        0.5 * (2.0 * f2 - f3 - f),
        0.5 * (3.0 * f3 - 5.0 * f2 + 2.0),
        0.5 * (4.0 * f2 - 3.0 * f3 + f),
        0.5 * (f3 - f2),
    );
}

fn glow_blurred(t: texture_2d<f32>, px: vec2<f32>, grid: vec4<f32>) -> vec3<f32> {
    let n = vec2<i32>(textureDimensions(t));
    let u = (px - grid.zw) / grid.xy;
    let base = floor(u);
    let wx = glow_weights(u.x - base.x);
    let wy = glow_weights(u.y - base.y);
    let at = vec2<i32>(base);
    var sum = vec3<f32>(0.0);
    for (var b = 0; b < 4; b++) {
        if wy[b] == 0.0 {
            continue;
        }
        let y = mirrored(at.y + b - 1, n.y);
        for (var a = 0; a < 4; a++) {
            if wx[a] == 0.0 {
                continue;
            }
            let x = mirrored(at.x + a - 1, n.x);
            sum += wy[b] * wx[a] * textureLoad(t, vec2<i32>(x, y), 0).rgb;
        }
    }
    return sum;
}

fn glow_normal(z: f32) -> f32 {
    let x = abs(z) / sqrt(2.0);
    let p = 1.0 + x * (0.278393 + x * (0.230389 + x * (0.000972 + x * 0.078108)));
    let q = p * p;
    let e = 1.0 - 1.0 / (q * q);
    return 0.5 * (1.0 + select(-e, e, z >= 0.0));
}

fn glow_framed(at: f32, n: f32) -> f32 {
    return glow_normal((at + GLOW_EDGE) / GLOW_EDGE_WIDTH) * glow_normal((n - GLOW_EDGE - at) / GLOW_EDGE_WIDTH);
}

fn glow_hash(p: vec3<i32>) -> u32 {
    var h = (u32(p.x) * 73856093u) ^ (u32(p.y) * 19349663u) ^ (u32(p.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return h ^ (h >> 16u);
}

fn glow_slope(p: vec3<i32>, f: vec3<f32>) -> f32 {
    let k = glow_hash(p) & 15u;
    let u = select(f.y, f.x, k < 8u);
    let v = select(select(f.z, f.x, k == 12u || k == 14u), f.y, k < 4u);
    return select(-u, u, (k & 1u) == 0u) + select(-v, v, (k & 2u) == 0u);
}

fn glow_cloud(p: vec3<f32>) -> f32 {
    let base = floor(p);
    let i = vec3<i32>(base);
    let f = p - base;
    let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let near = mix(
        mix(glow_slope(i, f), glow_slope(i + vec3<i32>(1, 0, 0), f - vec3<f32>(1.0, 0.0, 0.0)), w.x),
        mix(glow_slope(i + vec3<i32>(0, 1, 0), f - vec3<f32>(0.0, 1.0, 0.0)), glow_slope(i + vec3<i32>(1, 1, 0), f - vec3<f32>(1.0, 1.0, 0.0)), w.x),
        w.y,
    );
    let far = mix(
        mix(glow_slope(i + vec3<i32>(0, 0, 1), f - vec3<f32>(0.0, 0.0, 1.0)), glow_slope(i + vec3<i32>(1, 0, 1), f - vec3<f32>(1.0, 0.0, 1.0)), w.x),
        mix(glow_slope(i + vec3<i32>(0, 1, 1), f - vec3<f32>(0.0, 1.0, 1.0)), glow_slope(i + vec3<i32>(1, 1, 1), f - vec3<f32>(1.0, 1.0, 1.0)), w.x),
        w.y,
    );
    return mix(near, far, w.z);
}

fn glow_atmosphere(px: vec2<f32>) -> f32 {
    var sum = 0.0;
    var total = 0.0;
    var amount = 1.0;
    var size = 1.0;
    for (var i = 0; i < GLOW_OCTAVES; i++) {
        let layer = GLOW_LAYERS * f32(i);
        let at = vec3<f32>(px * glowing.noise.zw * size + layer, glowing.depth.x * size + layer);
        sum += amount * glow_cloud(at);
        total += amount;
        amount *= glowing.noise.y;
        size *= 2.0;
    }
    return GLOW_NOISE_GAIN * sum / total;
}

fn glow_light(c: vec3<f32>) -> f32 {
    return sqrt(max(dot(c * c, BT601), 0.0));
}

fn glow_saturated(c: vec3<f32>, k: f32) -> vec3<f32> {
    let high = max(c.r, max(c.g, c.b));
    let low = min(c.r, min(c.g, c.b));
    let wide = high - low;
    let s = select(0.0, wide / max(high, 1e-12), high > 0.0);
    let s2 = clamp(s * k, 0.0, 1.0);
    let t = select(vec3<f32>(1.0), (c - low) / max(wide, 1e-12), wide > 0.0);
    let d = (1.0 - s2) + t * s2;
    return d * glow_light(c) / max(glow_light(d), 1e-12);
}

fn glow_mixed(mode: u32, g: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let m = mixed(mode, g, c);
    let ends = vec3<f32>(1.0);
    switch mode {
        case 11u: { return select(m, select(vec3<f32>(0.0), ends, c > vec3<f32>(0.0)), g >= ends); }
        case 12u: { return select(m, select(vec3<f32>(0.0), ends, c >= ends), g <= vec3<f32>(0.0)); }
        default: { return m; }
    }
}

@fragment
fn fs_glow(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let at = vec2<f32>(px);
    let was = glow_taken(px, blur_start);
    let mask = textureLoad(glow_mask, px, 0).rgb;
    var made = mask;
    var g = vec3<f32>(0.0);
    if glowing.modes.x != 0 {
        let one = mix(glow_blurred(blur_source, at, glowing.grid_one), mask, glowing.sharp_one.rgb);
        g = glowing.shine.z * one;
        if glow_has(GLOW_TWO) {
            let two = mix(glow_blurred(glow_second, at, glowing.grid_two), mask, glowing.sharp_two.rgb);
            g += glowing.shine.w * two * glowing.second.rgb;
        }
        if glow_has(GLOW_SEEN) {
            let n = vec2<f32>(textureDimensions(blur_start));
            g *= glow_framed(at.x, n.x) * glow_framed(at.y, n.y);
        }
        g *= glowing.tint.rgb;
        g *= max(1.0 - GLOW_RECOVERY * glowing.mixing.x * max(mask.r, max(mask.g, mask.b)), 0.0);
        if glow_has(GLOW_NOISE) {
            g *= max(1.0 + glowing.noise.x * glow_atmosphere(at), 0.0);
        }
        if glowing.second.w != 1.0 {
            g = glow_saturated(g, glowing.second.w);
        }
        if glowing.tint.w != 1.0 {
            let light = glow_light(g);
            g *= select(0.0, pow(light, 1.0 / glowing.tint.w) / light, light > 0.0);
        }
        made = g;
        if glowing.modes.x == 2 {
            made = glow_mixed(u32(glowing.modes.z), g, was.rgb);
        }
    }
    var out = mix(was, vec4<f32>(made, 1.0), glowing.mixing.y);
    if glow_has(GLOW_INVERT) {
        out = vec4<f32>(1.0 - out.rgb, out.a);
    }
    out.a = max(was.a, max(out.r, max(out.g, out.b)));
    let last = effect_mixed(px, out, glowing.mixing.z, glowing.mixing.w);
    return clamp(last, vec4<f32>(0.0), vec4<f32>(1.0));
}

struct Grain {
    noise: vec4<f32>,
    curve: vec4<f32>,
    tails: vec4<f32>,
    gains: vec4<f32>,
    tones: vec4<f32>,
    shift: vec4<i32>,
};

@group(0) @binding(0) var<uniform> graining: Grain;
@group(0) @binding(15) var grain_input: texture_2d<f32>;

const GRAIN_LIFT: f32 = 256.0 / 255.0;
const GRAIN_ONLY: i32 = 1;
const GRAIN_DETAIL: i32 = 2;
const GRAIN_LOW_END: f32 = 0.4005;
const GRAIN_HIGH_START: f32 = 0.5995;
const GRAIN_TONE_WIDTH: f32 = 0.299;

@fragment
fn fs_grain_noise(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(blur_source));
    let at = (vec2<i32>(in.position.xy) + graining.shift.xy) % size;
    let u = textureLoad(blur_source, at, 0).rgb;
    let x = graining.noise.x * mix(vec3<f32>(dot(u, BT709)), u, graining.noise.y);
    let near = clamp(x, vec3<f32>(-graining.tails.z), vec3<f32>(graining.tails.z));
    let c = graining.curve;
    let y = c.x + near * (c.y + near * (c.z + near * c.w));
    let tail = (x - near) * select(graining.tails.yyy, graining.tails.xxx, x < vec3<f32>(0.0));
    return vec4<f32>((y + tail) * graining.gains.rgb, 1.0);
}

fn grain_tones(c: vec3<f32>) -> vec3<f32> {
    let low = clamp((GRAIN_LOW_END - c) / GRAIN_TONE_WIDTH, vec3<f32>(0.0), vec3<f32>(1.0));
    let high = clamp((c - GRAIN_HIGH_START) / GRAIN_TONE_WIDTH, vec3<f32>(0.0), vec3<f32>(1.0));
    let t = graining.tones;
    return 2.0 * (t.x * low + t.y * (1.0 - low - high) + t.z * high);
}

@fragment
fn fs_grain(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let offset = graining.noise.z;
    let o = graining.noise.w;
    let flags = graining.shift.w;
    let n = textureLoad(blur_source, px, 0).rgb;
    let only = clamp(offset + n, vec3<f32>(0.0), vec3<f32>(1.0));
    var made = only;
    if (flags & GRAIN_ONLY) == 0 {
        let taken = textureLoad(grain_input, px, 0);
        let c = select(effect_taken(taken, 0.0).rgb, taken.rgb, (flags & GRAIN_DETAIL) != 0);
        let l = 0.5 + o * GRAIN_LIFT * grain_tones(c) * (only - offset);
        made = mix(c, mixed(u32(graining.shift.z), l, c), o);
    }
    let shown = vec4<f32>(clamp(made, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    return clamp(effect_mixed(px, shown, graining.gains.w, 0.0), vec4<f32>(0.0), vec4<f32>(1.0));
}
