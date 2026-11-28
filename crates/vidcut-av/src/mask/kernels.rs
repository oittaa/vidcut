use crate::compute::Gpu;
use crate::compute::ops::{Act, Conv, HEADER, Ops, Res, SHARED, View, tensor};

const KEYS: u32 = 16;

fn attend_layout(d: u32, bq: u32) -> (u32, u32, u32) {
    let (ko, vo, g) = (bq * 9, bq * 5, d / 4);
    let values = (KEYS - 1) * g + 4 * g.div_ceil(4);
    (ko, vo, (ko + KEYS * 9).max(vo + values))
}

pub(super) fn attend_fits(d: u32) -> bool {
    let (_, _, size) = attend_layout(d, 64);
    d.is_multiple_of(4) && size * 16 + 64 * 4 * 4 <= SHARED
}

fn attend_source(d: u32, bq: u32) -> String {
    let g = d / 4;
    let parts = g.div_ceil(4);
    let threads = 4 * bq;
    let (ko, vo, mx) = attend_layout(d, bq);
    let mut decl = String::new();
    let mut rescale = String::new();
    let mut store = String::new();
    for j in 0..parts {
        decl.push_str(&format!("    var a{j} = vec4<f32>(0.0);\n"));
        rescale.push_str(&format!("        a{j} *= corr;\n"));
        store.push_str(&format!(
            r#"        if (part + {o}u < G) {{
            let o = a{j} * inv;
            let at = ob + (part + {o}u) * 4u;
            os[at] = o.x;
            os[at + 1u] = o.y;
            os[at + 2u] = o.z;
            os[at + 3u] = o.w;
        }}
"#,
            o = 4 * j
        ));
    }
    let mut scores = String::new();
    for i in 0..8 {
        scores.push_str(&format!("            let x{i} = sh[r * 9u + {i}u];\n"));
        for c in 0..4 {
            scores.push_str(&format!(
                "            s{c} += dot(x{i}, sh[KO + (part + {o}u) * 9u + {i}u]);\n",
                o = 4 * c
            ));
        }
    }
    let mut mix = String::new();
    for c in 0..KEYS / 4 {
        mix.push_str(&format!("        let w{c} = sh[r * 5u + {c}u];\n"));
    }
    for (e, comp) in (0..).zip(["x", "y", "z", "w"]) {
        for c in 0..KEYS / 4 {
            let kk = 4 * e + c;
            for j in 0..parts {
                mix.push_str(&format!(
                    "        a{j} = fma(vec4<f32>(w{c}.{comp}), sh[VO + {kk}u * G + part + {o}u], a{j});\n",
                    o = 4 * j
                ));
            }
        }
    }
    format!(
        r#"{HEADER}
const D: u32 = {d}u;
const G: u32 = {g}u;
const BQ: u32 = {bq}u;
const T: u32 = {threads}u;
const BK: u32 = {KEYS}u;
const KO: u32 = {ko}u;
const VO: u32 = {vo}u;
@group(0) @binding(1) var<storage, read> qs: array<f32>;
@group(0) @binding(2) var<storage, read> ks: array<f32>;
@group(0) @binding(3) var<storage, read> vs: array<f32>;
@group(0) @binding(4) var<storage, read_write> os: array<f32>;

var<workgroup> sh: array<vec4<f32>, {mx}>;
var<workgroup> red: array<f32, {threads}>;

@compute @workgroup_size({threads})
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let batches = u(0u); let lq = u(1u); let lk = u(2u); let heads = u(3u); let scale = f(4u); let tiles = u(5u);
    let g = gid(wg, nw);
    if (g >= batches * heads * tiles) {{
        return;
    }}
    let tile = g % tiles;
    let h = (g / tiles) % heads;
    let b = g / (tiles * heads);
    let r = li / 4u;
    let part = li % 4u;
    let q0 = tile * BQ;
    let qb = u(6u) + b * u(7u) + u(9u) + h * D;
    let kb = u(10u) + b * u(11u) + u(13u) + h * D;
    let vb = u(14u) + b * u(15u) + u(17u) + h * D;
    var m = -3.0e38;
    var l = 0.0;
{decl}    for (var k0 = 0u; k0 < lk; k0 += BK) {{
        var s0 = 0.0;
        var s1 = 0.0;
        var s2 = 0.0;
        var s3 = 0.0;
        for (var c = 0u; c < D; c += 32u) {{
            for (var e = li; e < BQ * 8u; e += T) {{
                let row = e / 8u;
                let at = c + (e % 8u) * 4u;
                var x = vec4<f32>(0.0);
                if (q0 + row < lq && at < D) {{
                    let i = qb + (q0 + row) * u(8u) + at;
                    x = vec4<f32>(qs[i], qs[i + 1u], qs[i + 2u], qs[i + 3u]) * scale;
                }}
                sh[row * 9u + e % 8u] = x;
            }}
            for (var e = li; e < BK * 8u; e += T) {{
                let row = e / 8u;
                let at = c + (e % 8u) * 4u;
                var x = vec4<f32>(0.0);
                if (k0 + row < lk && at < D) {{
                    let i = kb + (k0 + row) * u(12u) + at;
                    x = vec4<f32>(ks[i], ks[i + 1u], ks[i + 2u], ks[i + 3u]);
                }}
                sh[KO + row * 9u + e % 8u] = x;
            }}
            workgroupBarrier();
{scores}            workgroupBarrier();
        }}
        let v0 = k0 + part < lk;
        let v1 = k0 + part + 4u < lk;
        let v2 = k0 + part + 8u < lk;
        let v3 = k0 + part + 12u < lk;
        s0 = select(-3.0e38, s0, v0);
        s1 = select(-3.0e38, s1, v1);
        s2 = select(-3.0e38, s2, v2);
        s3 = select(-3.0e38, s3, v3);
        red[li] = max(max(s0, s1), max(s2, s3));
        workgroupBarrier();
        let mv = max(max(red[r * 4u], red[r * 4u + 1u]), max(red[r * 4u + 2u], red[r * 4u + 3u]));
        let mn = max(m, mv);
        let corr = exp(m - mn);
        let p0 = select(0.0, exp(s0 - mn), v0);
        let p1 = select(0.0, exp(s1 - mn), v1);
        let p2 = select(0.0, exp(s2 - mn), v2);
        let p3 = select(0.0, exp(s3 - mn), v3);
        l = l * corr + p0 + p1 + p2 + p3;
{rescale}        m = mn;
        sh[r * 5u + part] = vec4<f32>(p0, p1, p2, p3);
        for (var e = li; e < BK * G; e += T) {{
            let row = e / G;
            var x = vec4<f32>(0.0);
            if (k0 + row < lk) {{
                let i = vb + (k0 + row) * u(16u) + (e % G) * 4u;
                x = vec4<f32>(vs[i], vs[i + 1u], vs[i + 2u], vs[i + 3u]);
            }}
            sh[VO + e] = x;
        }}
        workgroupBarrier();
{mix}        workgroupBarrier();
    }}
    red[li] = l;
    workgroupBarrier();
    let inv = 1.0 / (red[r * 4u] + red[r * 4u + 1u] + red[r * 4u + 2u] + red[r * 4u + 3u]);
    let row = q0 + r;
    if (row < lq) {{
        let ob = u(18u) + b * u(19u) + row * u(20u) + u(21u) + h * D;
{store}    }}
}}
"#,
    )
}

const WINDOWS: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let back = u(0u); let h = u(1u); let w = u(2u); let ws = u(3u); let across = u(4u); let down = u(5u); let c = u(6u);
    let mb = u(7u); let mstr = u(8u); let mat = u(9u);
    let wb = u(10u); let wstr = u(11u); let wat = u(12u);
    let i = gid(wg, nw) * 256u + li;
    let ch = i % c;
    let t = i / c;
    if (back == 0u) {
        if (t >= across * down * ws * ws) {
            return;
        }
        let ix = t % ws;
        let iy = (t / ws) % ws;
        let win = t / (ws * ws);
        let y = (win / across) * ws + iy;
        let x = (win % across) * ws + ix;
        var v = 0.0;
        if (y < h && x < w) {
            v = src[mb + (y * w + x) * mstr + mat + ch];
        }
        dst[wb + t * wstr + wat + ch] = v;
        return;
    }
    if (t >= h * w) {
        return;
    }
    let y = t / w;
    let x = t % w;
    let win = (y / ws) * across + x / ws;
    let at = (win * ws + y % ws) * ws + x % ws;
    dst[mb + t * mstr + mat + ch] += src[wb + at * wstr + wat + ch];
}
"#;

const SCALE: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read> bounds: array<i32>;
@group(0) @binding(3) var<storage, read> taps: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let w_in = u(0u); let w_out = u(1u); let rows = u(2u); let size = u(3u); let down = u(4u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= rows * w_out * 3u) {
        return;
    }
    let ch = i % 3u;
    let px = i / 3u;
    let x = px % w_out;
    let y = px / w_out;
    var acc = 0.0;
    if (down == 0u) {
        let first = bounds[x * 2u];
        let count = bounds[x * 2u + 1u];
        for (var k = 0i; k < count; k++) {
            let v = clamp(src[(y * w_in + u32(first + k)) * 3u + ch], 0.0, 1.0);
            acc += v * taps[x * size + u32(k)];
        }
        dst[i] = acc;
        return;
    }
    let first = bounds[y * 2u];
    let count = bounds[y * 2u + 1u];
    for (var k = 0i; k < count; k++) {
        acc += src[(u32(first + k) * w_out + x) * 3u + ch] * taps[y * size + u32(k)];
    }
    dst[i] = (clamp(acc, 0.0, 1.0) - f(5u + ch)) / f(8u + ch);
}
"#;

const ROPE: &str = r#"
@group(0) @binding(1) var<storage, read> turns: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> x: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let rows = u(0u); let base = u(1u); let stride = u(2u); let period = u(3u); let pairs = u(4u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= rows * pairs) {
        return;
    }
    let j = i % pairs;
    let r = i / pairs;
    let cs = turns[(r % period) * pairs + j];
    let at = base + r * stride + 2u * j;
    let a = x[at];
    let b = x[at + 1u];
    x[at] = a * cs.x - b * cs.y;
    x[at + 1u] = a * cs.y + b * cs.x;
}
"#;

const MEMORY_MASK: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u); let way = u(1u); let scale = f(2u); let bias = f(3u); let sb = u(4u); let db = u(5u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n) {
        return;
    }
    let v = src[sb + i];
    var m = 0.0;
    if (way == 0u) {
        m = select(0.0, 1.0, v > 0.0);
    } else if (way == 1u) {
        m = 1.0 / (1.0 + exp(-v));
    }
    dst[db + i] = m * scale + bias;
}
"#;

const SOFTMAX: &str = r#"
@group(0) @binding(1) var<storage, read_write> x: array<f32>;

var<workgroup> red: array<f32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let rows = u(0u); let base = u(1u); let len = u(2u); let valid = u(3u); let scale = f(4u);
    let r = gid(wg, nw);
    if (r >= rows) {
        return;
    }
    let at = base + r * len;
    var m = -3.0e38;
    for (var j = li; j < valid; j += 256u) {
        m = max(m, x[at + j]);
    }
    red[li] = m;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if (li < s) {
            red[li] = max(red[li], red[li + s]);
        }
        workgroupBarrier();
    }
    m = red[0];
    workgroupBarrier();
    var sum = 0.0;
    for (var j = li; j < valid; j += 256u) {
        let e = exp((x[at + j] - m) * scale);
        x[at + j] = e;
        sum += e;
    }
    red[li] = sum;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if (li < s) {
            red[li] += red[li + s];
        }
        workgroupBarrier();
    }
    let inv = 1.0 / red[0];
    for (var j = li; j < len; j += 256u) {
        x[at + j] = select(0.0, x[at + j] * inv, j < valid);
    }
}
"#;

const TRANSPOSE: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u); let c = u(1u); let sb = u(2u); let sstr = u(3u); let len = u(4u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * c) {
        return;
    }
    let row = i / c;
    let d = i % c;
    dst[d * len + row] = src[sb + row * sstr + d];
}
"#;

pub(super) struct Kernels {
    windows: wgpu::ComputePipeline,
    scale: wgpu::ComputePipeline,
    rope: wgpu::ComputePipeline,
    memory_mask: wgpu::ComputePipeline,
    softmax: wgpu::ComputePipeline,
    transpose: wgpu::ComputePipeline,
}

impl Kernels {
    pub(super) fn new(gpu: &Gpu) -> Self {
        Self {
            windows: gpu.pipe("mask-windows", || format!("{HEADER}{WINDOWS}")),
            scale: gpu.pipe("mask-scale", || format!("{HEADER}{SCALE}")),
            rope: gpu.pipe("mask-rope", || format!("{HEADER}{ROPE}")),
            memory_mask: gpu.pipe("mask-memory-mask", || format!("{HEADER}{MEMORY_MASK}")),
            softmax: gpu.pipe("mask-softmax", || format!("{HEADER}{SOFTMAX}")),
            transpose: gpu.pipe("mask-transpose", || format!("{HEADER}{TRANSPOSE}")),
        }
    }
}

fn softmax(ops: &mut Ops, k: &Kernels, x: &View, (valid, scale): (u32, f32)) {
    let p = [x.w, x.base, x.c, valid, scale.to_bits()];
    let param = ops.gpu.uniform(&p);
    ops.gpu
        .dispatch(ops.pass, &k.softmax, param, &[&x.buf], x.w);
}

fn transpose(ops: &mut Ops, k: &Kernels, src: &View, dst: &wgpu::Buffer, len: u32) {
    let p = [src.w, src.c, src.base + src.at, src.stride, len];
    let param = ops.gpu.uniform(&p);
    let items = src.w * src.c;
    ops.gpu.dispatch(
        ops.pass,
        &k.transpose,
        param,
        &[&src.buf, dst],
        items.div_ceil(256),
    );
}

pub(super) fn rope(ops: &mut Ops, k: &Kernels, turns: &wgpu::Buffer, x: &View, rows: u32) {
    let pairs = x.c / 2;
    let p = [rows, x.base + x.at, x.stride, ROPE_PERIOD, pairs];
    let param = ops.gpu.uniform(&p);
    ops.gpu.dispatch(
        ops.pass,
        &k.rope,
        param,
        &[turns, &x.buf],
        (rows * pairs).div_ceil(256),
    );
}

const ROPE_PERIOD: u32 = 64 * 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Remembered {
    Cut,
    Soft,
    Empty,
}

pub(super) fn memory_mask(
    ops: &mut Ops,
    k: &Kernels,
    (scale, bias): (f32, f32),
    way: Remembered,
    (src, dst): (&View, &View),
) {
    let n = src.n * src.h * src.w * src.c;
    let way = match way {
        Remembered::Cut => 0,
        Remembered::Soft => 1,
        Remembered::Empty => 2,
    };
    let p = [n, way, scale.to_bits(), bias.to_bits(), src.base, dst.base];
    let param = ops.gpu.uniform(&p);
    ops.gpu.dispatch(
        ops.pass,
        &k.memory_mask,
        param,
        &[&src.buf, &dst.buf],
        n.div_ceil(256),
    );
}

fn place(v: &View) -> [u32; 3] {
    [v.base, v.stride, v.at]
}

pub(super) struct Rows<'a> {
    pub(super) view: &'a View,
    pub(super) each: u32,
}

pub(super) struct Heads {
    pub(super) batches: u32,
    pub(super) lq: u32,
    pub(super) lk: u32,
    pub(super) heads: u32,
    pub(super) dim: u32,
}

pub(super) fn attend(ops: &mut Ops, h: &Heads, q: Rows, k: Rows, v: Rows, out: Rows) {
    let d = h.dim;
    let bq = match h.lq >= 64 {
        true => 64,
        false => 16,
    };
    let pipe = ops
        .gpu
        .pipe(&format!("mask-attend-{d}-{bq}"), || attend_source(d, bq));
    let tiles = h.lq.div_ceil(bq);
    let scale = 1.0 / (d as f32).sqrt();
    let rows = |r: &Rows| [r.view.base, r.each, r.view.stride, r.view.at];
    let mut p = vec![h.batches, h.lq, h.lk, h.heads, scale.to_bits(), tiles];
    for r in [&q, &k, &v, &out] {
        p.extend(rows(r));
    }
    let param = ops.gpu.uniform(&p);
    let bufs = [&q.view.buf, &k.view.buf, &v.view.buf, &out.view.buf];
    ops.gpu
        .dispatch(ops.pass, &pipe, param, &bufs, h.batches * h.heads * tiles);
}

pub(super) struct Grid {
    pub(super) h: u32,
    pub(super) w: u32,
    pub(super) ws: u32,
}

impl Grid {
    pub(super) fn across(&self) -> u32 {
        self.w.div_ceil(self.ws)
    }

    pub(super) fn down(&self) -> u32 {
        self.h.div_ceil(self.ws)
    }

    pub(super) fn count(&self) -> u32 {
        self.across() * self.down()
    }
}

fn windows_call(ops: &mut Ops, k: &Kernels, g: &Grid, back: bool, (map, win): (&View, &View)) {
    let c = map.c;
    let mut p = vec![u32::from(back), g.h, g.w, g.ws, g.across(), g.down(), c];
    p.extend(place(map));
    p.extend(place(win));
    let param = ops.gpu.uniform(&p);
    let (items, bufs) = match back {
        false => (g.count() * g.ws * g.ws * c, [&map.buf, &win.buf]),
        true => (g.h * g.w * c, [&win.buf, &map.buf]),
    };
    ops.gpu
        .dispatch(ops.pass, &k.windows, param, &bufs, items.div_ceil(256));
}

pub(super) fn partition(ops: &mut Ops, k: &Kernels, g: &Grid, map: &View, win: &View) {
    windows_call(ops, k, g, false, (map, win));
}

pub(super) fn unpartition_add(ops: &mut Ops, k: &Kernels, g: &Grid, win: &View, map: &View) {
    windows_call(ops, k, g, true, (map, win));
}

pub(super) struct Axis {
    pub(super) bounds: wgpu::Buffer,
    pub(super) taps: wgpu::Buffer,
    pub(super) size: u32,
}

pub(super) fn scale_across(
    ops: &mut Ops,
    k: &Kernels,
    a: &Axis,
    (w_in, rows, w_out): (u32, u32, u32),
    src: &wgpu::Buffer,
    dst: &wgpu::Buffer,
) {
    let p = [w_in, w_out, rows, a.size, 0, 0, 0, 0, 0, 0, 0];
    let param = ops.gpu.uniform(&p);
    let items = rows * w_out * 3;
    ops.gpu.dispatch(
        ops.pass,
        &k.scale,
        param,
        &[src, &a.bounds, &a.taps, dst],
        items.div_ceil(256),
    );
}

pub(super) fn scale_down(
    ops: &mut Ops,
    k: &Kernels,
    a: &Axis,
    (w, h): (u32, u32),
    (mean, std): ([f32; 3], [f32; 3]),
    src: &wgpu::Buffer,
    dst: &wgpu::Buffer,
) {
    let mut p = vec![w, w, h, a.size, 1];
    p.extend(mean.map(f32::to_bits));
    p.extend(std.map(f32::to_bits));
    let param = ops.gpu.uniform(&p);
    let items = h * w * 3;
    ops.gpu.dispatch(
        ops.pass,
        &k.scale,
        param,
        &[src, &a.bounds, &a.taps, dst],
        items.div_ceil(256),
    );
}

pub(super) struct Gemm {
    kt: wgpu::Buffer,
    vt: View,
    scores: View,
    zeros: wgpu::Buffer,
    queries: u32,
}

impl Gemm {
    pub(super) fn new(gpu: &Gpu, dim: u32, keys: u32, queries: u32) -> Self {
        let (len, cols) = (keys.next_multiple_of(64), dim.next_multiple_of(64));
        Self {
            kt: gpu.buffer((dim * len * 4) as usize),
            vt: tensor(gpu, 1, 1, len, cols),
            scores: tensor(gpu, 1, 1, queries, len),
            zeros: gpu.buffer((len.max(cols) * 4) as usize),
            queries,
        }
    }
}

fn rows_of(v: &View, from: u32, count: u32) -> View {
    View {
        base: v.base + from * v.stride,
        w: count,
        ..v.clone()
    }
}

pub(super) fn attend_gemm(
    ops: &mut Ops,
    k: &Kernels,
    g: &Gemm,
    (q, keys, values, out): (&View, &View, &View, &View),
) {
    let (d, lq, lk) = (q.c, q.w, keys.w);
    let (len, cols) = (lk.next_multiple_of(64), d.next_multiple_of(64));
    transpose(ops, k, keys, &g.kt, len);
    let packed = values.base == 0 && values.at == 0 && values.stride == cols;
    let mixed = match packed {
        true => values.buf.clone(),
        false => {
            let to = View {
                c: d,
                ..g.vt.shaped(1, 1, lk, cols)
            };
            ops.copy(values, &to, 1.0, 0.0, Act::None);
            g.vt.buf.clone()
        }
    };
    let gemm = |cin: u32, cout: u32, weights: wgpu::Buffer| Conv {
        cin,
        cout,
        kh: 1,
        kw: 1,
        stride: (1, 1),
        pad: (0, 0),
        dil: (1, 1),
        cols: cout.next_multiple_of(64),
        weights,
        bias: g.zeros.clone(),
        alpha: g.zeros.clone(),
        act: Act::None,
    };
    let score = gemm(d, len, g.kt.clone());
    let mix = gemm(len, d, mixed);
    let scale = 1.0 / (d as f32).sqrt();
    let step = g.queries.min(lq);
    for at in (0..lq).step_by(step as usize) {
        let n = step.min(lq - at);
        let s = g.scores.shaped(1, 1, n, len);
        ops.conv(&score, &rows_of(q, at, n), &s, Res::None);
        softmax(ops, k, &s, (lk, scale));
        ops.conv(&mix, &s, &rows_of(out, at, n), Res::None);
    }
}
