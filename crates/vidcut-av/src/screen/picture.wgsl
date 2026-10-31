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
    levels: array<vec4<f32>, 5u * LEVELS>,
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

fn opacity(u: vec2<f32>) -> f32 {
    let s = stored(u) * p.upright.zw;
    if p.flags.z == 2u {
        return textureSampleLevel(luma, bilinear, s, 0.0).r;
    }
    if p.per.w == 1u {
        return textureSampleLevel(alpha_plane, bilinear, s, 0.0).r;
    }
    return 1.0;
}

fn weight(x: f32, kind: u32) -> f32 {
    let a = abs(x);
    if kind == 1u || kind == 4u {
        let k = select(-0.5, -0.6, kind == 4u);
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

fn resamples(spans: vec2<f32>) -> bool {
    if (p.flags.w & 7u) == 0u {
        return false;
    }
    if (p.flags.w & 8u) != 0u {
        return any(abs(spans - vec2<f32>(1.0)) > vec2<f32>(1e-4));
    }
    return max(spans.x, spans.y) < 1.0;
}

fn sample_at(q: vec2<f32>, level: f32) -> vec3<f32> {
    let i = u32(level);
    let texels = vec2<f32>(textureDimensions(halved, i));
    let s = stored(q) * stored_size() / (exp2(level) * texels);
    return textureSampleLevel(halved, bilinear, s, level).rgb;
}

fn sharp(u: vec2<f32>, spans: vec2<f32>) -> vec3<f32> {
    let kind = p.flags.w & 7u;
    let level = clamp(ceil(log2(max(spans.x, spans.y) / 4.0)), 0.0, p.shrunk.x);
    let scale = exp2(level);
    let size = p.upright.xy / scale;
    let lobes = select(select(4.0, 3.0, kind == 2u), 2.0, kind == 1u || kind == 4u);
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
            sum += sample_at(clamp(q, half, vec2<f32>(1.0) - half), level) * w;
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
    if p.flags.z != 3u {
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
    if p.made.x > 4.5 {
        let a = textureLoad(halved, vec2<i32>(px), 0).a;
        if a <= 0.0 {
            discard;
        }
        return vec4<f32>(textureLoad(luma, vec2<i32>(px), 0).rgb, a * p.placed.y);
    }
    let n = u32(p.placed.x);
    var cover = 1.0;
    var uv = vec2<f32>(0.0);
    var across = vec2<f32>(0.0);
    var down = vec2<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        let a = p.levels[i * 5u];
        let b = p.levels[i * 5u + 1u];
        let c = cut(a, b, p.levels[i * 5u + 2u], px);
        uv = c.xy;
        cover *= c.z * faded(p.levels[i * 5u + 3u], p.levels[i * 5u + 4u], c.xy);
        let w = dot(vec3<f32>(px, 1.0), vec3<f32>(a.w, b.w, 1.0));
        if abs(w) >= 1e-6 {
            across = vec2<f32>(a.x - uv.x * a.w, b.x - uv.y * a.w) / w;
            down = vec2<f32>(a.y - uv.x * b.w, b.y - uv.y * b.w) / w;
        }
    }
    if cover <= 0.0 {
        discard;
    }
    if p.made.x > 3.5 {
        return vec4<f32>(1.0, 1.0, 1.0, cover * p.placed.y);
    }
    if p.made.x > 1.5 {
        let q = (px / p.made.yz - p.part.xy) / p.part.zw;
        return vec4<f32>(pixel(q), cover * p.placed.y);
    }
    let size = p.upright.xy;
    let spans = vec2<f32>(length(across * size), length(down * size));
    let a = cover * p.placed.y * opacity(uv);
    if resamples(spans) {
        return vec4<f32>(sharp(uv, spans), a);
    }
    let taps = vec2<u32>(clamp(ceil(spans), vec2<f32>(1.0), vec2<f32>(8.0)));
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
    let s = shown(px);
    return vec4<f32>(s.rgb * s.a, s.a);
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

fn dodge(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return select(min(b / (1.0 - a), vec3<f32>(1.0)), vec3<f32>(1.0), a >= vec3<f32>(1.0));
}

fn burn(b: vec3<f32>, a: vec3<f32>) -> vec3<f32> {
    return select(1.0 - min((1.0 - b) / a, vec3<f32>(1.0)), vec3<f32>(0.0), a <= vec3<f32>(0.0));
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

@fragment
fn fs_mode(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    let top = shown(px);
    let b = textureLoad(below, vec2<i32>(px), 0).rgb;
    let m = mixed(u32(p.placed.z + 0.5), top.rgb, b);
    if p.placed.w > 0.5 {
        return vec4<f32>(m * top.a, top.a);
    }
    return vec4<f32>(mix(b, m, top.a), 1.0);
}

@fragment
fn fs_paired(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let top = textureLoad(letters, px, 0);
    let b = textureLoad(below, px, 0).rgb;
    return vec4<f32>(top.rgb + b * (1.0 - top.a), 1.0);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    if p.placed.x > 0.5 {
        if (p.made.x > 0.5 && p.made.x < 1.5) || p.made.w > 0.5 {
            return vec4<f32>(shown(px + p.part.xy).rgb, 1.0);
        }
        return placed(px);
    }
    let rel = (px - p.place.xy) / p.place.zw;
    if any(rel < vec2<f32>(0.0)) || any(rel >= vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let cover = p.upright.xy / p.place.zw;
    let a = opacity(rel);
    if resamples(cover) {
        return vec4<f32>(sharp(rel, cover) * a, 1.0);
    }
    let taps = vec2<u32>(clamp(ceil(cover), vec2<f32>(1.0), vec2<f32>(8.0)));
    let step = vec2<f32>(1.0) / (p.place.zw * vec2<f32>(taps));
    let first = (px - vec2<f32>(0.5) - p.place.xy) / p.place.zw + step * 0.5;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            sum += pixel(first + vec2<f32>(f32(i), f32(j)) * step);
        }
    }
    return vec4<f32>(sum / f32(taps.x * taps.y) * a, 1.0);
}

struct Lettering {
    across: vec4<f32>,
    down: vec4<f32>,
    tilt: vec4<f32>,
    levels: array<vec4<f32>, 5u * LEVELS>,
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
    var cover = 1.0;
    let n = u32(k.w);
    for (var i = 0u; i < n; i++) {
        let c = cut(lettering.levels[i * 5u], lettering.levels[i * 5u + 1u], lettering.levels[i * 5u + 2u], px);
        cover *= c.z * faded(lettering.levels[i * 5u + 3u], lettering.levels[i * 5u + 4u], c.xy);
    }
    if cover <= 0.0 {
        discard;
    }
    let spans = vec2<f32>(length(across * size), length(down * size));
    let taps = vec2<u32>(clamp(ceil(spans), vec2<f32>(1.0), vec2<f32>(8.0)));
    var sum = vec4<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / vec2<f32>(taps) - 0.5;
            let q = at + across * o.x + down * o.y;
            let t = q * size;
            let e = clamp(t + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * clamp(size + 0.5 - t, vec2<f32>(0.0), vec2<f32>(1.0));
            sum += textureSampleLevel(letters, bilinear, q, 0.0) * (e.x * e.y);
        }
    }
    var shown = sum / f32(taps.x * taps.y);
    let transfer = u32(lettering.tilt.z + 0.5);
    if transfer != 0u && shown.a > 0.0 {
        shown = vec4<f32>(widened(shown.rgb / shown.a, transfer) * shown.a, shown.a);
    }
    return shown * (cover * m.w);
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
    gaps: vec4<u32>,
    mode: vec4<u32>,
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

fn covers(q: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(outgoing));
    return select(1.0, 0.0, any(q < vec2<f32>(0.0)) || any(q >= size));
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
    var upper_gap: bool;
    if upper_in {
        upper = incoming_at(u);
        lower = outgoing_at(l);
        upper_gap = motion.gaps.y == 1u;
    } else {
        upper = outgoing_at(u);
        lower = incoming_at(l);
        upper_gap = motion.gaps.x == 1u;
    }
    var block = upper.a;
    if upper_gap {
        block = covers(u);
    }
    let c = q + vec2<f32>(0.5);
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

@fragment
fn fs_motion_mode(in: Out) -> @location(0) vec4<f32> {
    let top = moved(floor(in.position.xy));
    let b = textureLoad(below, vec2<i32>(in.position.xy), 0).rgb;
    if top.a <= 0.0 {
        return vec4<f32>(b, 1.0);
    }
    let m = mixed(motion.mode.x, top.rgb / top.a, b);
    return vec4<f32>(mix(b, m, top.a), 1.0);
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
};

@group(0) @binding(10) var<uniform> encoding: Encoding;

fn encoded_rgb(code: vec3<f32>) -> vec3<f32> {
    let coded = encoding.how.x;
    let wanted = encoding.how.y;
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

fn pixel_at(x: i32, y: i32) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(timeline));
    let at = clamp(vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
    return encoded_rgb(textureLoad(timeline, at, 0).rgb);
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
        out.x = ((1u << encoding.how.z) - 1u) << encoding.how.w;
    } else {
        let top = (1u << encoding.how.z) - 1u;
        out = vec4<u32>(vec3<u32>(round(pixel_at(px.x, px.y) * f32(top))), top);
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

struct Blur {
    spread: vec4<f32>,
    way: vec4<i32>,
    blend: vec4<f32>,
};

@group(0) @binding(0) var<uniform> blurring: Blur;
@group(0) @binding(1) var blur_source: texture_2d<f32>;
@group(0) @binding(2) var blur_start: texture_2d<f32>;

fn blur_folded(i: i32, n: i32) -> i32 {
    switch blurring.way.w {
        case 1: {
            return clamp(i, 0, n - 1);
        }
        case 2: {
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
        case 3: {
            var m = i % n;
            if m < 0 {
                m += n;
            }
            return m;
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
    if blurring.blend.w > 0.5 && blurring.blend.y < 0.5 && c.a > 0.0 {
        return vec4<f32>(c.rgb / c.a, c.a);
    }
    return c;
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
        let w = select(vec4<f32>(0.0), exp(-d * d * k), wide);
        sum += (blur_tap(px, j * step, size) + blur_tap(px, -j * step, size)) * w;
        total += 2.0 * w;
    }
    var out = sum / total;
    if blurring.blend.z > 0.5 {
        let was = textureLoad(blur_start, px, 0);
        if blurring.blend.y < 0.5 {
            out = vec4<f32>(out.rgb * was.a, was.a);
        }
        out = mix(was, out, blurring.blend.x);
    }
    return out;
}
