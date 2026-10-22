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
    levels: array<vec4<f32>, 3u * LEVELS>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma_u: texture_2d<f32>;
@group(0) @binding(3) var chroma_v: texture_2d<f32>;
@group(0) @binding(4) var bilinear: sampler;

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
    if p.flags.x != 0u {
        c = to_linear(c);
        c = vec3<f32>(dot(p.gamut0.xyz, c), dot(p.gamut1.xyz, c), dot(p.gamut2.xyz, c));
        let sig = max(max(max(c.r, c.g), c.b), 1e-6);
        let knee = p.tone.x;
        let mapped = select(p.tone.y * (sig + p.tone.z) / (sig + p.tone.w), sig, sig <= knee);
        c = c * (mapped / sig);
        c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4));
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn cut(a: vec4<f32>, b: vec4<f32>, s: vec4<f32>, px: vec2<f32>) -> vec3<f32> {
    let uv = vec2<f32>(dot(a.xy, px) + a.z, dot(b.xy, px) + b.z);
    let gu = length(a.xy);
    let gv = length(b.xy);
    let edges = vec4<f32>((uv.x - s.x) / gu, (s.z - uv.x) / gu, (uv.y - s.y) / gv, (s.w - uv.y) / gv);
    let e = clamp(edges + 0.5, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec3<f32>(uv, e.x * e.y * e.z * e.w);
}

fn placed(px: vec2<f32>) -> vec4<f32> {
    let n = u32(p.placed.x);
    var cover = 1.0;
    var uv = vec2<f32>(0.0);
    var across = vec2<f32>(0.0);
    var down = vec2<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        let a = p.levels[i * 3u];
        let b = p.levels[i * 3u + 1u];
        let c = cut(a, b, p.levels[i * 3u + 2u], px);
        uv = c.xy;
        cover *= c.z;
        across = vec2<f32>(a.x, b.x);
        down = vec2<f32>(a.y, b.y);
    }
    if cover <= 0.0 {
        discard;
    }
    let size = p.upright.xy;
    let spans = vec2<f32>(length(across * size), length(down * size));
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
    let a = cover * p.placed.y;
    return vec4<f32>(rgb(sum / f32(taps.x * taps.y)) * a, a);
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
    let taps = vec2<u32>(clamp(ceil(cover), vec2<f32>(1.0), vec2<f32>(8.0)));
    let step = vec2<f32>(1.0) / (p.place.zw * vec2<f32>(taps));
    let first = (px - vec2<f32>(0.5) - p.place.xy) / p.place.zw + step * 0.5;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < taps.y; j++) {
        for (var i = 0u; i < taps.x; i++) {
            sum += yuv(first + vec2<f32>(f32(i), f32(j)) * step);
        }
    }
    return vec4<f32>(rgb(sum / f32(taps.x * taps.y)), 1.0);
}

struct Lettering {
    across: vec4<f32>,
    down: vec4<f32>,
    levels: array<vec4<f32>, 3u * LEVELS>,
};

@group(0) @binding(5) var<uniform> lettering: Lettering;
@group(0) @binding(6) var letters: texture_2d<f32>;

@fragment
fn fs_title(in: Out) -> @location(0) vec4<f32> {
    let px = in.position.xy;
    let m = lettering.across;
    let k = lettering.down;
    let at = vec2<f32>(dot(m.xy, px) + m.z, dot(k.xy, px) + k.z);
    let across = vec2<f32>(m.x, k.x);
    let down = vec2<f32>(m.y, k.y);
    let size = vec2<f32>(textureDimensions(letters));
    let reach = (abs(across) + abs(down)) * 0.5 + vec2<f32>(0.5) / size;
    if any(at + reach <= vec2<f32>(0.0)) || any(at - reach >= vec2<f32>(1.0)) {
        discard;
    }
    var cover = 1.0;
    let n = u32(k.w);
    for (var i = 0u; i < n; i++) {
        let l = lettering.levels[i * 3u];
        cover *= cut(l, lettering.levels[i * 3u + 1u], lettering.levels[i * 3u + 2u], px).z;
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
    return sum / f32(taps.x * taps.y) * (cover * m.w);
}
