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
    levels: array<vec4<f32>, 5u * LEVELS>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma_u: texture_2d<f32>;
@group(0) @binding(3) var chroma_v: texture_2d<f32>;
@group(0) @binding(4) var bilinear: sampler;
@group(0) @binding(7) var alpha_plane: texture_2d<f32>;

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

fn yuv(u: vec2<f32>) -> vec3<f32> {
    let s = stored(u) * p.upright.zw;
    if p.flags.z == 2u {
        let ayuv = textureSampleLevel(luma, bilinear, s, 0.0);
        return ayuv.gba;
    }
    let y = textureSampleLevel(luma, bilinear, s, 0.0).r;
    let c = s * vec2<f32>(textureDimensions(luma)) / vec2<f32>(p.per.xy * textureDimensions(chroma_u));
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
    if kind == 1u {
        if a < 1.0 {
            return (1.5 * a - 2.5) * a * a + 1.0;
        }
        if a < 2.0 {
            return ((-0.5 * a + 2.5) * a - 4.0) * a + 2.0;
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

fn sharp(u: vec2<f32>) -> vec3<f32> {
    let kind = p.flags.w;
    let size = p.upright.xy;
    let reach = select(select(4, 3, kind == 2u), 2, kind == 1u);
    let t = u * size - 0.5;
    let base = floor(t);
    let f = t - base;
    let half = vec2<f32>(0.5) / size;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var j = 1 - reach; j <= reach; j++) {
        let wy = weight(f32(j) - f.y, kind);
        for (var i = 1 - reach; i <= reach; i++) {
            let w = weight(f32(i) - f.x, kind) * wy;
            let q = (base + vec2<f32>(f32(i), f32(j)) + 0.5) / size;
            sum += yuv(clamp(q, half, vec2<f32>(1.0) - half)) * w;
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
    let size = p.upright.xy;
    let spans = vec2<f32>(length(across * size), length(down * size));
    let a = cover * p.placed.y * opacity(uv);
    if p.flags.w > 0u && max(spans.x, spans.y) < 1.0 {
        return vec4<f32>(rgb(sharp(uv)), a);
    }
    let taps = vec2<u32>(clamp(ceil(spans), vec2<f32>(1.0), vec2<f32>(8.0)));
    let half = vec2<f32>(0.5) / size;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / vec2<f32>(taps) - 0.5;
            let q = uv + across * o.x + down * o.y;
            sum += yuv(clamp(q, half, vec2<f32>(1.0) - half));
        }
    }
    return vec4<f32>(rgb(sum / f32(taps.x * taps.y)), a);
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
        return placed(px);
    }
    let rel = (px - p.place.xy) / p.place.zw;
    if any(rel < vec2<f32>(0.0)) || any(rel >= vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let cover = p.upright.xy / p.place.zw;
    let a = opacity(rel);
    if p.flags.w > 0u && max(cover.x, cover.y) < 1.0 {
        return vec4<f32>(rgb(sharp(rel)) * a, 1.0);
    }
    let taps = vec2<u32>(clamp(ceil(cover), vec2<f32>(1.0), vec2<f32>(8.0)));
    let step = vec2<f32>(1.0) / (p.place.zw * vec2<f32>(taps));
    let first = (px - vec2<f32>(0.5) - p.place.xy) / p.place.zw + step * 0.5;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            sum += yuv(first + vec2<f32>(f32(i), f32(j)) * step);
        }
    }
    return vec4<f32>(rgb(sum / f32(taps.x * taps.y)) * a, 1.0);
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
        let s = clamp(shown.rgb / shown.a, vec3<f32>(0.0), vec3<f32>(1.0));
        let lin = select(pow((s + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), s / 12.92, s <= vec3<f32>(0.04045));
        let wide = vec3<f32>(
            dot(vec3<f32>(0.6274, 0.3293, 0.0433), lin),
            dot(vec3<f32>(0.0691, 0.9195, 0.0114), lin),
            dot(vec3<f32>(0.0164, 0.0880, 0.8956), lin),
        );
        shown = vec4<f32>(clamp(encode(wide, transfer), vec3<f32>(0.0), vec3<f32>(1.0)) * shown.a, shown.a);
    }
    return shown * (cover * m.w);
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
