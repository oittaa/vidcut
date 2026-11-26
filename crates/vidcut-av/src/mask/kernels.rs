use crate::compute::Gpu;
use crate::compute::ops::{HEADER, Ops, SHARED, View};

const LANES: u32 = 64;

fn attend_source(d: u32, kt: u32) -> String {
    format!(
        r#"{HEADER}
const D: u32 = {d}u;
const KT: u32 = {kt}u;
@group(0) @binding(1) var<storage, read> qs: array<f32>;
@group(0) @binding(2) var<storage, read> ks: array<f32>;
@group(0) @binding(3) var<storage, read> vs: array<f32>;
@group(0) @binding(4) var<storage, read_write> os: array<f32>;

var<workgroup> kt: array<f32, {tile}>;
var<workgroup> vt: array<f32, {tile}>;

@compute @workgroup_size({LANES})
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let batches = u(0u); let lq = u(1u); let lk = u(2u); let heads = u(3u); let scale = f(4u); let tiles = u(5u);
    let g = gid(wg, nw);
    if (g >= batches * heads * tiles) {{
        return;
    }}
    let tile = g % tiles;
    let h = (g / tiles) % heads;
    let b = g / (tiles * heads);
    let qi = tile * {LANES}u + li;
    let live = qi < lq;
    var q: array<f32, {d}>;
    var acc: array<f32, {d}>;
    var s: array<f32, {kt}>;
    if (live) {{
        let at = u(6u) + b * u(7u) + qi * u(8u) + u(9u) + h * D;
        for (var d = 0u; d < D; d++) {{
            q[d] = qs[at + d] * scale;
            acc[d] = 0.0;
        }}
    }}
    var m = -3.0e38;
    var l = 0.0;
    let kb = u(10u) + b * u(11u) + u(13u) + h * D;
    let vb = u(14u) + b * u(15u) + u(17u) + h * D;
    for (var k0 = 0u; k0 < lk; k0 += KT) {{
        for (var i = li; i < KT * D; i += {LANES}u) {{
            let j = k0 + i / D;
            let d = i % D;
            var kv = 0.0;
            var vv = 0.0;
            if (j < lk) {{
                kv = ks[kb + j * u(12u) + d];
                vv = vs[vb + j * u(16u) + d];
            }}
            kt[i] = kv;
            vt[i] = vv;
        }}
        workgroupBarrier();
        if (live) {{
            let n = min(KT, lk - k0);
            var mx = m;
            for (var j = 0u; j < n; j++) {{
                var dot = 0.0;
                for (var d = 0u; d < D; d++) {{
                    dot = fma(q[d], kt[j * D + d], dot);
                }}
                s[j] = dot;
                mx = max(mx, dot);
            }}
            let corr = exp(m - mx);
            l *= corr;
            for (var d = 0u; d < D; d++) {{
                acc[d] *= corr;
            }}
            for (var j = 0u; j < n; j++) {{
                let p = exp(s[j] - mx);
                l += p;
                for (var d = 0u; d < D; d++) {{
                    acc[d] = fma(p, vt[j * D + d], acc[d]);
                }}
            }}
            m = mx;
        }}
        workgroupBarrier();
    }}
    if (live) {{
        let at = u(18u) + b * u(19u) + qi * u(20u) + u(21u) + h * D;
        let inv = 1.0 / l;
        for (var d = 0u; d < D; d++) {{
            os[at + d] = acc[d] * inv;
        }}
    }}
}}
"#,
        tile = kt * d,
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

pub(super) struct Kernels {
    windows: wgpu::ComputePipeline,
    scale: wgpu::ComputePipeline,
}

impl Kernels {
    pub(super) fn new(gpu: &Gpu) -> Self {
        Self {
            windows: gpu.pipe("mask-windows", || format!("{HEADER}{WINDOWS}")),
            scale: gpu.pipe("mask-scale", || format!("{HEADER}{SCALE}")),
        }
    }
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
    let kt = (SHARED / 8 / d).clamp(1, 32);
    let pipe = ops
        .gpu
        .pipe(&format!("mask-attend-{d}-{kt}"), || attend_source(d, kt));
    let tiles = h.lq.div_ceil(LANES);
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
