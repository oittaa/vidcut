use crate::compute::{Gpu, bytes};
use crate::error::{Error, Result};

#[derive(Clone)]
pub(super) struct View {
    pub(super) buf: wgpu::Buffer,
    pub(super) base: u32,
    pub(super) n: u32,
    pub(super) h: u32,
    pub(super) w: u32,
    pub(super) c: u32,
    pub(super) stride: u32,
    pub(super) at: u32,
}

impl View {
    pub(super) fn part(&self, at: u32, c: u32) -> View {
        View {
            at: self.at + at,
            c,
            ..self.clone()
        }
    }

    pub(super) fn frame(&self, b: u32) -> View {
        View {
            base: self.base + b * self.h * self.w * self.stride,
            n: 1,
            ..self.clone()
        }
    }

    pub(super) fn rows(&self, y: u32, h: u32) -> View {
        debug_assert_eq!(self.n, 1);
        View {
            base: self.base + y * self.w * self.stride,
            h,
            ..self.clone()
        }
    }

    pub(super) fn shaped(&self, n: u32, h: u32, w: u32, c: u32) -> View {
        View {
            n,
            h,
            w,
            c,
            stride: c,
            at: 0,
            ..self.clone()
        }
    }

    fn words(&self) -> [u32; 6] {
        [self.base, self.h, self.w, self.stride, self.at, self.c]
    }
}

pub(super) struct Carve<'a> {
    bufs: &'a [wgpu::Buffer],
    dummy: &'a wgpu::Buffer,
    plan: &'a [usize],
    cap: u64,
    asked: Vec<(u64, u64, u64)>,
    live: Vec<usize>,
    time: u64,
    over: bool,
}

impl Carve<'_> {
    pub(super) fn view(&mut self, n: u32, h: u32, w: u32, c: u32) -> View {
        let size = u64::from(n) * u64::from(h) * u64::from(w) * u64::from(c);
        self.over |= size > self.cap;
        let i = self.asked.len();
        self.asked.push((size, self.time, u64::MAX));
        self.live.push(i);
        self.time += 1;
        View {
            buf: self
                .plan
                .get(i)
                .and_then(|&slot| self.bufs.get(slot))
                .unwrap_or(self.dummy)
                .clone(),
            base: 0,
            n,
            h,
            w,
            c,
            stride: c,
            at: 0,
        }
    }

    pub(super) fn mark(&self) -> usize {
        self.live.len()
    }

    pub(super) fn reset(&mut self, mark: usize) {
        for i in self.live.drain(mark..) {
            self.asked[i].2 = self.time;
        }
        self.time += 1;
    }
}

fn slots(asked: &[(u64, u64, u64)]) -> (Vec<u64>, Vec<usize>) {
    let mut order: Vec<usize> = (0..asked.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(asked[i].0));
    let mut sizes: Vec<u64> = Vec::new();
    let mut users: Vec<Vec<usize>> = Vec::new();
    let mut plan = vec![0; asked.len()];
    for i in order {
        let (size, from, to) = asked[i];
        let free = |slot: &Vec<usize>| slot.iter().all(|&j| asked[j].2 <= from || to <= asked[j].1);
        let open: Vec<usize> = (0..sizes.len()).filter(|&k| free(&users[k])).collect();
        let fit = open
            .iter()
            .copied()
            .filter(|&k| sizes[k] >= size)
            .min_by_key(|&k| sizes[k])
            .or_else(|| open.iter().copied().max_by_key(|&k| sizes[k]));
        let slot = fit.unwrap_or_else(|| {
            sizes.push(0);
            users.push(Vec::new());
            sizes.len() - 1
        });
        sizes[slot] = sizes[slot].max(size);
        users[slot].push(i);
        plan[i] = slot;
    }
    (sizes, plan)
}

pub(super) fn arena<T>(
    gpu: &Gpu,
    plan: impl Fn(&mut Carve) -> T,
) -> Result<(Vec<wgpu::Buffer>, T)> {
    let limits = gpu.device.limits();
    let cap = (limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size)
        / 4)
    .min(u64::from(u32::MAX));
    let dummy = gpu.buffer(16);
    let carve = |bufs, plan| Carve {
        bufs,
        dummy: &dummy,
        plan,
        cap,
        asked: Vec::new(),
        live: Vec::new(),
        time: 0,
        over: false,
    };
    let mut c = carve(&[], &[]);
    plan(&mut c);
    if c.over {
        return Err(Error::Invalid(
            "frame interpolation on the GPU: the frame is larger than the GPU's buffers".into(),
        ));
    }
    let (sizes, slots) = slots(&c.asked);
    let bufs: Vec<wgpu::Buffer> = sizes.iter().map(|&f| gpu.buffer(f as usize * 4)).collect();
    let mut c = carve(&bufs, &slots);
    let t = plan(&mut c);
    Ok((bufs, t))
}

pub(super) fn tensor(gpu: &Gpu, n: u32, h: u32, w: u32, c: u32) -> View {
    let size = n as usize * h as usize * w as usize * c as usize * 4;
    View {
        buf: gpu.buffer(size),
        base: 0,
        n,
        h,
        w,
        c,
        stride: c,
        at: 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Act {
    None,
    Leaky(f32),
    Prelu,
    Gelu,
    Sigmoid,
}

impl Act {
    fn code(self) -> (u32, f32) {
        match self {
            Act::None => (0, 0.0),
            Act::Leaky(a) => (1, a),
            Act::Prelu => (2, 0.0),
            Act::Gelu => (4, 0.0),
            Act::Sigmoid => (5, 0.0),
        }
    }
}

pub(super) struct Conv {
    pub(super) cin: u32,
    pub(super) cout: u32,
    pub(super) kh: u32,
    pub(super) kw: u32,
    pub(super) stride: (u32, u32),
    pub(super) pad: (i32, i32),
    pub(super) dil: (u32, u32),
    pub(super) cols: u32,
    pub(super) weights: wgpu::Buffer,
    pub(super) bias: wgpu::Buffer,
    pub(super) alpha: wgpu::Buffer,
    pub(super) act: Act,
}

pub(super) struct Shape {
    pub(super) cin: u32,
    pub(super) cout: u32,
    pub(super) k: (u32, u32),
    pub(super) stride: u32,
    pub(super) pad: (i32, i32),
    pub(super) dil: u32,
}

impl Shape {
    pub(super) fn square(cin: u32, cout: u32, k: u32, stride: u32, pad: i32) -> Self {
        Self {
            cin,
            cout,
            k: (k, k),
            stride,
            pad: (pad, pad),
            dil: 1,
        }
    }
}

pub(super) fn conv(
    gpu: &Gpu,
    s: Shape,
    weights: &[f32],
    bias: Option<&[f32]>,
    act: Act,
    alpha: Option<&[f32]>,
) -> Conv {
    let (kh, kw) = s.k;
    let k = (kh * kw * s.cin) as usize;
    let cols = s.cout.next_multiple_of(64);
    let mut laid = vec![0f32; k * cols as usize];
    for co in 0..s.cout as usize {
        for ci in 0..s.cin as usize {
            for ky in 0..kh as usize {
                for kx in 0..kw as usize {
                    let row = (ky * kw as usize + kx) * s.cin as usize + ci;
                    let from = ((co * s.cin as usize + ci) * kh as usize + ky) * kw as usize + kx;
                    laid[row * cols as usize + co] = weights[from];
                }
            }
        }
    }
    let mut b = vec![0f32; cols as usize];
    if let Some(bias) = bias {
        b[..s.cout as usize].copy_from_slice(bias);
    }
    let mut a = vec![0f32; cols as usize];
    if let Some(alpha) = alpha {
        a[..alpha.len()].copy_from_slice(alpha);
    }
    Conv {
        cin: s.cin,
        cout: s.cout,
        kh,
        kw,
        stride: (s.stride, s.stride),
        pad: s.pad,
        dil: (s.dil, s.dil),
        cols,
        weights: gpu.upload(&bytes(&laid)),
        bias: gpu.upload(&bytes(&b)),
        alpha: gpu.upload(&bytes(&a)),
        act,
    }
}

pub(super) struct Deconv {
    pub(super) parts: Vec<Conv>,
}

pub(super) fn deconv(
    gpu: &Gpu,
    cin: u32,
    cout: u32,
    weights: &[f32],
    bias: Option<&[f32]>,
    act: Act,
    alpha: Option<&[f32]>,
) -> Deconv {
    let mut parts = Vec::new();
    for py in 0..2usize {
        for px in 0..2usize {
            let taps = |p: usize| if p == 0 { [3usize, 1] } else { [2usize, 0] };
            let (ty, tx) = (taps(py), taps(px));
            let mut w = vec![0f32; (cout * cin * 4) as usize];
            for co in 0..cout as usize {
                for ci in 0..cin as usize {
                    for (jy, ky) in ty.iter().enumerate() {
                        for (jx, kx) in tx.iter().enumerate() {
                            let from = ((ci * cout as usize + co) * 4 + ky) * 4 + kx;
                            w[((co * cin as usize + ci) * 2 + jy) * 2 + jx] = weights[from];
                        }
                    }
                }
            }
            let pad = |p: usize| if p == 0 { 1 } else { 0 };
            let shape = Shape {
                cin,
                cout,
                k: (2, 2),
                stride: 1,
                pad: (pad(py), pad(px)),
                dil: 1,
            };
            parts.push(conv(gpu, shape, &w, bias, act, alpha));
        }
    }
    Deconv { parts }
}

pub(super) struct Kernels {
    conv: wgpu::ComputePipeline,
    dwconv: wgpu::ComputePipeline,
    resize: wgpu::ComputePipeline,
    warp: wgpu::ComputePipeline,
    shuffle: wgpu::ComputePipeline,
    copy: wgpu::ComputePipeline,
    add: wgpu::ComputePipeline,
    norm: wgpu::ComputePipeline,
    fill: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    load: wgpu::ComputePipeline,
    store: wgpu::ComputePipeline,
    predict: wgpu::ComputePipeline,
    combine: wgpu::ComputePipeline,
    window: wgpu::ComputePipeline,
    cor: wgpu::ComputePipeline,
    attend: wgpu::ComputePipeline,
    pub(super) dummy: wgpu::Buffer,
}

const HEADER: &str = r#"
struct P { v: array<vec4<u32>, 16> }
@group(0) @binding(0) var<uniform> p: P;

fn u(i: u32) -> u32 {
    return p.v[i / 4u][i % 4u];
}

fn f(i: u32) -> f32 {
    return bitcast<f32>(u(i));
}

fn s(i: u32) -> i32 {
    return bitcast<i32>(u(i));
}

fn gid(wg: vec3<u32>, nw: vec3<u32>) -> u32 {
    return wg.x + wg.y * nw.x;
}

fn erf(x: f32) -> f32 {
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return select(-y, y, x >= 0.0);
}

fn act(v: f32, code: u32, a: f32, pa: f32) -> f32 {
    switch code {
        case 1u: { return select(v * a, v, v >= 0.0); }
        case 2u: { return select(v * pa, v, v >= 0.0); }
        case 4u: { return 0.5 * v * (1.0 + erf(v * 0.7071067811865476)); }
        case 5u: { return 1.0 / (1.0 + exp(-v)); }
        default: { return v; }
    }
}
"#;

const CONV: &str = r#"
@group(0) @binding(1) var<storage, read> wts: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;
@group(0) @binding(3) var<storage, read> alpha: array<f32>;
@group(0) @binding(4) var<storage, read> src: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<f32>;
@group(0) @binding(6) var<storage, read> res: array<f32>;

var<workgroup> sa: array<f32, 1024>;
var<workgroup> sb: array<vec4<f32>, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let ih = u(2u); let iw = u(3u); let istr = u(4u); let iat = u(5u); let cin = u(6u);
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u); let cout = u(12u);
    let qh = u(13u); let qw = u(14u);
    let oym = u(15u); let oya = u(16u); let oxm = u(17u); let oxa = u(18u);
    let kw = u(20u); let sy = u(21u); let sx = u(22u);
    let pady = s(23u); let padx = s(24u); let dy = u(25u); let dx = u(26u);
    let code = u(27u); let la = f(28u); let rmode = u(29u);
    let rb = u(30u); let rstr = u(33u); let rat = u(34u);
    let kk = u(36u); let cols = u(37u); let m_all = u(38u); let shuffle = u(39u);
    let g = gid(wg, nw);
    let tiles_m = (m_all + 63u) / 64u;
    let tiles_n = (cout + 63u) / 64u;
    if (g >= tiles_m * tiles_n) {
        return;
    }
    let tm = g % tiles_m;
    let tn = g / tiles_m;
    let tx = li % 16u;
    let ty = li / 16u;
    let per = qh * qw;
    var py: array<i32, 4>;
    var px: array<i32, 4>;
    var pb: array<u32, 4>;
    var pv: array<bool, 4>;
    for (var j = 0u; j < 4u; j++) {
        let m = tm * 64u + ty + 16u * j;
        pv[j] = m < m_all;
        let b = m / per;
        let r = m % per;
        pb[j] = b;
        py[j] = i32((r / qw) * sy) - pady;
        px[j] = i32((r % qw) * sx) - padx;
    }
    var acc: array<vec4<f32>, 4>;
    for (var k0 = 0u; k0 < kk; k0 += 16u) {
        let k = k0 + tx;
        var oy = 0;
        var ox = 0;
        var ci = 0u;
        let kin = k < kk;
        if (kin) {
            ci = k % cin;
            let tap = k / cin;
            oy = i32((tap / kw) * dy);
            ox = i32((tap % kw) * dx);
        }
        for (var j = 0u; j < 4u; j++) {
            var v = 0.0;
            let y = py[j] + oy;
            let x = px[j] + ox;
            if (kin && pv[j] && y >= 0 && x >= 0 && y < i32(ih) && x < i32(iw)) {
                v = src[ib + ((pb[j] * ih + u32(y)) * iw + u32(x)) * istr + iat + ci];
            }
            sa[tx * 64u + ty + 16u * j] = v;
        }
        let kb = k0 + ty;
        var w4 = vec4<f32>(0.0);
        if (kb < kk) {
            w4 = wts[(kb * cols + tn * 64u + tx * 4u) / 4u];
        }
        sb[ty * 16u + tx] = w4;
        workgroupBarrier();
        for (var q = 0u; q < 16u; q++) {
            let b4 = sb[q * 16u + tx];
            for (var j = 0u; j < 4u; j++) {
                acc[j] = fma(vec4<f32>(sa[q * 64u + ty + 16u * j]), b4, acc[j]);
            }
        }
        workgroupBarrier();
    }
    for (var j = 0u; j < 4u; j++) {
        if (!pv[j]) {
            continue;
        }
        let m = tm * 64u + ty + 16u * j;
        let b = m / per;
        let r = m % per;
        let qy = r / qw;
        let qx = r % qw;
        for (var q = 0u; q < 4u; q++) {
            let co = tn * 64u + tx * 4u + q;
            if (co >= cout) {
                continue;
            }
            var y = qy * oym + oya;
            var x = qx * oxm + oxa;
            var c = co;
            if (shuffle > 1u) {
                let sq = shuffle * shuffle;
                y = y * shuffle + (co % sq) / shuffle;
                x = x * shuffle + co % shuffle;
                c = co / sq;
            }
            var v = acc[j][q] + bias[co];
            if (rmode == 1u) {
                v += res[rb + ((b * oh + y) * ow + x) * rstr + rat + c];
            }
            v = act(v, code, la, alpha[co]);
            dst[ob + ((b * oh + y) * ow + x) * ostr + oat + c] = v;
        }
    }
}
"#;

const DWCONV: &str = r#"
@group(0) @binding(1) var<storage, read> wts: array<f32>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;
@group(0) @binding(3) var<storage, read> src: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let h = u(2u); let w = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let ostr = u(10u); let oat = u(11u);
    let k = u(13u); let pad = s(14u); let code = u(15u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    let x = px % w;
    let y = (px / w) % h;
    let b = px / (w * h);
    var v = bias[ch];
    for (var ky = 0u; ky < k; ky++) {
        let yy = i32(y + ky) - pad;
        if (yy < 0 || yy >= i32(h)) {
            continue;
        }
        for (var kx = 0u; kx < k; kx++) {
            let xx = i32(x + kx) - pad;
            if (xx < 0 || xx >= i32(w)) {
                continue;
            }
            v = fma(src[ib + ((b * h + u32(yy)) * w + u32(xx)) * istr + iat + ch], wts[(ch * k + ky) * k + kx], v);
        }
    }
    dst[ob + ((b * h + y) * w + x) * ostr + oat + ch] = act(v, code, 0.0, 0.0);
}
"#;

const RESIZE: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let ih = u(2u); let iw = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u);
    let scy = f(13u); let scx = f(14u); let mul = f(15u); let mulc = u(16u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * oh * ow * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    let x = px % ow;
    let y = (px / ow) % oh;
    let b = px / (ow * oh);
    let fy = max((f32(y) + 0.5) * scy - 0.5, 0.0);
    let fx = max((f32(x) + 0.5) * scx - 0.5, 0.0);
    let y0 = min(u32(floor(fy)), ih - 1u);
    let x0 = min(u32(floor(fx)), iw - 1u);
    let y1 = min(y0 + 1u, ih - 1u);
    let x1 = min(x0 + 1u, iw - 1u);
    let ly = fy - f32(y0);
    let lx = fx - f32(x0);
    let row0 = (b * ih + y0) * iw;
    let row1 = (b * ih + y1) * iw;
    let a = src[ib + (row0 + x0) * istr + iat + ch];
    let bb = src[ib + (row0 + x1) * istr + iat + ch];
    let cc = src[ib + (row1 + x0) * istr + iat + ch];
    let d = src[ib + (row1 + x1) * istr + iat + ch];
    var v = (1.0 - ly) * ((1.0 - lx) * a + lx * bb) + ly * ((1.0 - lx) * cc + lx * d);
    if (ch < mulc) {
        v *= mul;
    }
    dst[ob + ((b * oh + y) * ow + x) * ostr + oat + ch] = v;
}
"#;

const WARP: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read> flow: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let ih = u(2u); let w = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let h = u(8u); let ostr = u(10u); let oat = u(11u);
    let fb = u(13u); let fstr = u(16u); let fat = u(17u);
    let oy = u(19u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    let x = px % w;
    let y = (px / w) % h;
    let b = px / (w * h);
    let fl = fb + ((b * h + y) * w + x) * fstr + fat;
    let gx = -1.0 + 2.0 * f32(x) / f32(w - 1u) + flow[fl] / ((f32(w) - 1.0) / 2.0);
    let gy = -1.0 + 2.0 * f32(y + oy) / f32(ih - 1u) + flow[fl + 1u] / ((f32(ih) - 1.0) / 2.0);
    let sx = clamp((gx + 1.0) / 2.0 * (f32(w) - 1.0), 0.0, f32(w) - 1.0);
    let sy = clamp((gy + 1.0) / 2.0 * (f32(ih) - 1.0), 0.0, f32(ih) - 1.0);
    let x0 = floor(sx);
    let y0 = floor(sy);
    let lx = sx - x0;
    let ly = sy - y0;
    var v = 0.0;
    for (var j = 0u; j < 4u; j++) {
        let xx = i32(x0) + i32(j % 2u);
        let yy = i32(y0) + i32(j / 2u);
        let wt = select(1.0 - lx, lx, j % 2u == 1u) * select(1.0 - ly, ly, j / 2u == 1u);
        if (xx >= 0 && yy >= 0 && xx < i32(w) && yy < i32(ih)) {
            v += wt * src[ib + ((b * ih + u32(yy)) * w + u32(xx)) * istr + iat + ch];
        }
    }
    dst[ob + ((b * h + y) * w + x) * ostr + oat + ch] = v;
}
"#;

const SHUFFLE: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let ih = u(2u); let iw = u(3u); let istr = u(4u); let iat = u(5u);
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u); let c = u(12u);
    let r = u(13u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * oh * ow * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    let x = px % ow;
    let y = (px / ow) % oh;
    let b = px / (ow * oh);
    let sc = ch * r * r + (y % r) * r + x % r;
    dst[ob + ((b * oh + y) * ow + x) * ostr + oat + ch] = src[ib + ((b * ih + y / r) * iw + x / r) * istr + iat + sc];
}
"#;

const COPY: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let h = u(2u); let w = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let ostr = u(10u); let oat = u(11u);
    let mul = f(13u); let add = f(14u); let code = u(15u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    dst[ob + px * ostr + oat + ch] = act(src[ib + px * istr + iat + ch] * mul + add, code, 0.0, 0.0);
}
"#;

const ADD: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let h = u(2u); let w = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let ostr = u(10u); let oat = u(11u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    dst[ob + px * ostr + oat + ch] += src[ib + px * istr + iat + ch];
}
"#;

const NORM: &str = r#"
@group(0) @binding(1) var<storage, read> gamma: array<f32>;
@group(0) @binding(2) var<storage, read> beta: array<f32>;
@group(0) @binding(3) var<storage, read> src: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<f32>;

var<workgroup> part: array<f32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let h = u(2u); let w = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let ostr = u(10u); let oat = u(11u);
    let eps = f(13u);
    let lane = li % 64u;
    let row = li / 64u;
    let px = gid(wg, nw) * 4u + row;
    let live = px < n * h * w;
    var sum = 0.0;
    if (live) {
        for (var ch = lane; ch < c; ch += 64u) {
            sum += src[ib + px * istr + iat + ch];
        }
    }
    part[li] = sum;
    workgroupBarrier();
    for (var step = 32u; step > 0u; step /= 2u) {
        if (lane < step) {
            part[li] += part[li + step];
        }
        workgroupBarrier();
    }
    let mean = part[row * 64u] / f32(c);
    workgroupBarrier();
    var sq = 0.0;
    if (live) {
        for (var ch = lane; ch < c; ch += 64u) {
            let d = src[ib + px * istr + iat + ch] - mean;
            sq += d * d;
        }
    }
    part[li] = sq;
    workgroupBarrier();
    for (var step = 32u; step > 0u; step /= 2u) {
        if (lane < step) {
            part[li] += part[li + step];
        }
        workgroupBarrier();
    }
    let inv = inverseSqrt(part[row * 64u] / f32(c) + eps);
    if (live) {
        for (var ch = lane; ch < c; ch += 64u) {
            let d = src[ib + px * istr + iat + ch] - mean;
            dst[ob + px * ostr + oat + ch] = d * inv * gamma[ch] + beta[ch];
        }
    }
}
"#;

const FILL: &str = r#"
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ob = u(1u); let h = u(2u); let w = u(3u); let ostr = u(4u); let oat = u(5u); let c = u(6u);
    let v = f(7u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    dst[ob + (i / c) * ostr + oat + i % c] = v;
}
"#;

const FINISH: &str = r#"
@group(0) @binding(1) var<storage, read> w0: array<f32>;
@group(0) @binding(2) var<storage, read> w1: array<f32>;
@group(0) @binding(3) var<storage, read> mask: array<f32>;
@group(0) @binding(4) var<storage, read> res: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<u32>;

fn value(p: u32, ch: u32) -> f32 {
    let h = u(2u); let w = u(3u);
    let b0 = u(1u); let s0 = u(4u); let a0 = u(5u);
    let b1 = u(7u); let s1 = u(10u); let a1 = u(11u);
    let bm = u(13u); let sm = u(16u); let am = u(17u);
    let br = u(19u); let sr = u(22u); let ar = u(23u);
    let top = u(25u); let left = u(26u); let ow = u(27u); let refine = u(29u);
    let y = p / ow + top;
    let x = p % ow + left;
    let q = y * w + x;
    let m = 1.0 / (1.0 + exp(-mask[bm + q * sm + am]));
    var v = w0[b0 + q * s0 + a0 + ch] * m + w1[b1 + q * s1 + a1 + ch] * (1.0 - m);
    if (refine == 1u) {
        v += res[br + q * sr + ar + ch] * 2.0 - 1.0;
    }
    return clamp(v, 0.0, 1.0) * 255.0;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let words = u(28u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= words) {
        return;
    }
    let total = u(27u) * u(30u) * 3u;
    var out = 0u;
    for (var j = 0u; j < 4u; j++) {
        let byte = i * 4u + j;
        if (byte < total) {
            let v = u32(round(value(byte / 3u, byte % 3u)));
            out |= v << (8u * j);
        }
    }
    dst[i] = out;
}
"#;

const LOAD: &str = r#"
@group(0) @binding(1) var<storage, read> rgb: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

fn byte(i: u32) -> f32 {
    return f32((rgb[i / 4u] >> (8u * (i % 4u))) & 255u) / 255.0;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ob = u(1u); let h = u(2u); let w = u(3u); let ostr = u(4u); let oat = u(5u);
    let sw = u(7u); let sh = u(8u); let top = u(9u); let left = u(10u); let edge = u(11u);
    let flip = u(12u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= h * w * 3u) {
        return;
    }
    let ch = i % 3u;
    let px = i / 3u;
    var x = px % w;
    var y = px / w;
    if (flip == 1u) {
        x = w - 1u - x;
        y = h - 1u - y;
    }
    let yy = i32(y) - i32(top);
    let xx = i32(x) - i32(left);
    let at = px;
    var v = 0.0;
    if (edge == 1u) {
        let cy = u32(clamp(yy, 0, i32(sh) - 1));
        let cx = u32(clamp(xx, 0, i32(sw) - 1));
        v = byte((cy * sw + cx) * 3u + ch);
    } else if (yy >= 0 && xx >= 0 && yy < i32(sh) && xx < i32(sw)) {
        v = byte((u32(yy) * sw + u32(xx)) * 3u + ch);
    }
    dst[ob + at * ostr + oat + ch] = v;
}
"#;

const PREDICT: &str = r#"
@group(0) @binding(1) var<storage, read> w0: array<f32>;
@group(0) @binding(2) var<storage, read> w1: array<f32>;
@group(0) @binding(3) var<storage, read> mask: array<f32>;
@group(0) @binding(4) var<storage, read> res: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b0 = u(1u); let h = u(2u); let w = u(3u); let s0 = u(4u); let a0 = u(5u);
    let b1 = u(7u); let s1 = u(10u); let a1 = u(11u);
    let bm = u(13u); let sm = u(16u); let am = u(17u);
    let br = u(19u); let sr = u(22u); let ar = u(23u);
    let bd = u(25u); let sd = u(28u); let ad = u(29u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= h * w * 3u) {
        return;
    }
    let ch = i % 3u;
    let q = i / 3u;
    let m = 1.0 / (1.0 + exp(-mask[bm + q * sm + am]));
    let v = w0[b0 + q * s0 + a0 + ch] * m + w1[b1 + q * s1 + a1 + ch] * (1.0 - m) + res[br + q * sr + ar + ch] * 2.0 - 1.0;
    dst[bd + q * sd + ad + ch] = clamp(v, 0.0, 1.0);
}
"#;

const STORE: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
@group(0) @binding(3) var<storage, read> other: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ib = u(1u); let w = u(3u); let istr = u(4u); let iat = u(5u);
    let top = u(7u); let left = u(8u); let ow = u(9u); let oh = u(10u); let words = u(11u);
    let two = u(12u); let h = u(2u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= words) {
        return;
    }
    let total = ow * oh * 3u;
    var out = 0u;
    for (var j = 0u; j < 4u; j++) {
        let byte = i * 4u + j;
        if (byte < total) {
            let p = byte / 3u;
            let y = p / ow + top;
            let x = p % ow + left;
            var v = clamp(src[ib + (y * w + x) * istr + iat + byte % 3u], 0.0, 1.0);
            if (two == 1u) {
                let f = clamp(other[(((h - 1u - y) * w) + (w - 1u - x)) * istr + iat + byte % 3u], 0.0, 1.0);
                v = (v + f) / 2.0;
            }
            out |= u32(round(v * 255.0)) << (8u * j);
        }
    }
    dst[i] = out;
}
"#;

const COMBINE: &str = r#"
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ab = u(1u); let h = u(2u); let w = u(3u); let astr = u(4u); let aat = u(5u); let c = u(6u);
    let bb = u(7u); let bstr = u(10u); let bat = u(11u);
    let ob = u(13u); let ostr = u(16u); let oat = u(17u);
    let wa = f(19u); let wb = f(20u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    dst[ob + px * ostr + oat + ch] = a[ab + px * astr + aat + ch] * wa + b[bb + px * bstr + bat + ch] * wb;
}
"#;

const WINDOW: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let sb = u(1u); let h = u(2u); let w = u(3u); let sstr = u(4u); let sat = u(5u); let c = u(6u);
    let db = u(7u); let dstr = u(10u); let dat = u(11u);
    let hp = u(13u); let wp = u(14u); let pt = u(15u); let pl = u(16u); let shift = u(17u); let back = u(18u);
    let i = gid(wg, nw) * 256u + li;
    let nwx = wp / 7u;
    let per = hp * wp;
    if (back == 0u) {
        if (i >= n * per * c) {
            return;
        }
        let ch = i % c;
        let g = i / c;
        let b = g / per;
        let r = g % per;
        let win = r / 49u;
        let t = r % 49u;
        let yy = (win / nwx) * 7u + t / 7u;
        let xx = (win % nwx) * 7u + t % 7u;
        let y = i32((yy + shift) % hp) - i32(pt);
        let x = i32((xx + shift) % wp) - i32(pl);
        var v = 0.0;
        if (y >= 0 && x >= 0 && y < i32(h) && x < i32(w)) {
            v = src[sb + ((b * h + u32(y)) * w + u32(x)) * sstr + sat + ch];
        }
        dst[db + g * dstr + dat + ch] = v;
    } else {
        if (i >= n * h * w * c) {
            return;
        }
        let ch = i % c;
        let px = i / c;
        let x = px % w;
        let y = (px / w) % h;
        let b = px / (w * h);
        let yy = (y + pt + hp - shift) % hp;
        let xx = (x + pl + wp - shift) % wp;
        let g = b * per + ((yy / 7u) * nwx + xx / 7u) * 49u + (yy % 7u) * 7u + xx % 7u;
        dst[db + px * dstr + dat + ch] = src[sb + g * sstr + sat + ch];
    }
}
"#;

const COR: &str = r#"
@group(0) @binding(1) var<storage, read> wts: array<f32>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let db = u(1u); let dstr = u(4u); let dat = u(5u); let cm = u(6u);
    let h = u(7u); let w = u(8u); let hp = u(9u); let wp = u(10u); let pt = u(11u); let pl = u(12u); let shift = u(13u);
    let i = gid(wg, nw) * 256u + li;
    let per = hp * wp;
    if (i >= n * per * cm) {
        return;
    }
    let j = i % cm;
    let g = i / cm;
    let r = g % per;
    let nwx = wp / 7u;
    let win = r / 49u;
    let t = r % 49u;
    let yy = (win / nwx) * 7u + t / 7u;
    let xx = (win % nwx) * 7u + t % 7u;
    let y = i32((yy + shift) % hp) - i32(pt);
    let x = i32((xx + shift) % wp) - i32(pl);
    var cx = 0.0;
    var cy = 0.0;
    if (y >= 0 && x >= 0 && y < i32(h) && x < i32(w)) {
        cx = -1.0 + f32(x) * (2.0 / f32(w - 1u));
        cy = -1.0 + f32(y) * (2.0 / f32(h - 1u));
    }
    dst[db + g * dstr + dat + j] = wts[j * 2u] * cx + wts[j * 2u + 1u] * cy + bias[j];
}
"#;

const ATTEND: &str = r#"
@group(0) @binding(1) var<storage, read> q: array<f32>;
@group(0) @binding(2) var<storage, read> kv: array<f32>;
@group(0) @binding(3) var<storage, read> ce: array<f32>;
@group(0) @binding(4) var<storage, read> ids: array<u32>;
@group(0) @binding(5) var<storage, read_write> xo: array<f32>;
@group(0) @binding(6) var<storage, read_write> cr: array<f32>;

const HD: u32 = 32u;
const CMH: u32 = 8u;
const N: u32 = 49u;

var<workgroup> sk: array<f32, 1568>;
var<workgroup> sv: array<f32, 1568>;
var<workgroup> sc: array<f32, 392>;

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let c = u(0u); let cm = u(1u); let heads = u(2u); let nwin = u(3u);
    let wp = u(4u); let use_pad = u(5u); let use_shift = u(6u); let scale = f(7u);
    let g = gid(wg, nw);
    if (g >= 2u * nwin * heads) {
        return;
    }
    let win = g / heads;
    let h = g % heads;
    let partner = (win + nwin) % (2u * nwin);
    if (t < N) {
        let kr = (partner * N + t) * 2u * c + h * HD;
        for (var d = 0u; d < HD; d++) {
            sk[t * HD + d] = kv[kr + d];
            sv[t * HD + d] = kv[kr + c + d];
        }
        let cr0 = (win * N + t) * cm + h * CMH;
        for (var j = 0u; j < CMH; j++) {
            sc[t * CMH + j] = ce[cr0 + j];
        }
    }
    workgroupBarrier();
    if (t >= N) {
        return;
    }
    let nwx = wp / 7u;
    let wl = win % nwin;
    let base_y = (wl / nwx) * 7u;
    let base_x = (wl % nwx) * 7u;
    let mine = ids[(base_y + t / 7u) * wp + base_x + t % 7u];
    var qv: array<f32, 32>;
    let qr = (win * N + t) * c + h * HD;
    for (var d = 0u; d < HD; d++) {
        qv[d] = q[qr + d];
    }
    var sv_: array<f32, 49>;
    var top = -3.4e38;
    for (var j = 0u; j < N; j++) {
        var acc = 0.0;
        for (var d = 0u; d < HD; d++) {
            acc = fma(qv[d], sk[j * HD + d], acc);
        }
        acc *= scale;
        let other = ids[(base_y + j / 7u) * wp + base_x + j % 7u];
        let pad_off = use_pad == 1u && (mine & 255u) != (other & 255u);
        let shift_off = use_shift == 1u && (mine >> 8u) != (other >> 8u);
        if (pad_off || shift_off) {
            acc += -100.0;
        }
        sv_[j] = acc;
        top = max(top, acc);
    }
    var sum = 0.0;
    for (var j = 0u; j < N; j++) {
        let e = exp(sv_[j] - top);
        sv_[j] = e;
        sum += e;
    }
    var out: array<f32, 32>;
    var cout: array<f32, 8>;
    for (var j = 0u; j < N; j++) {
        let p = sv_[j] / sum;
        for (var d = 0u; d < HD; d++) {
            out[d] = fma(p, sv[j * HD + d], out[d]);
        }
        for (var k = 0u; k < CMH; k++) {
            cout[k] = fma(p, sc[j * CMH + k], cout[k]);
        }
    }
    for (var d = 0u; d < HD; d++) {
        xo[qr + d] = out[d];
    }
    let co = (win * N + t) * cm + h * CMH;
    for (var k = 0u; k < CMH; k++) {
        cr[co + k] = cout[k];
    }
}
"#;

fn groups(items: u64) -> u32 {
    items.div_ceil(256) as u32
}

impl Kernels {
    pub(super) fn new(gpu: &Gpu) -> Self {
        let make = |key: &str, body: &str| gpu.pipe(key, || format!("{HEADER}{body}"));
        Self {
            conv: make("interp-conv", CONV),
            dwconv: make("interp-dwconv", DWCONV),
            resize: make("interp-resize", RESIZE),
            warp: make("interp-warp", WARP),
            shuffle: make("interp-shuffle", SHUFFLE),
            copy: make("interp-copy", COPY),
            add: make("interp-add", ADD),
            norm: make("interp-norm", NORM),
            fill: make("interp-fill", FILL),
            finish: make("interp-finish", FINISH),
            load: make("interp-load", LOAD),
            store: make("interp-store", STORE),
            predict: make("interp-predict", PREDICT),
            combine: make("interp-combine", COMBINE),
            window: make("interp-window", WINDOW),
            cor: make("interp-cor", COR),
            attend: make("interp-attend", ATTEND),
            dummy: gpu.upload(&[0u8; 16]),
        }
    }
}

pub(super) struct Ops<'a, 'p> {
    pub(super) gpu: &'a Gpu,
    pub(super) k: &'a Kernels,
    pub(super) pass: &'a mut wgpu::ComputePass<'p>,
}

pub(super) enum Res<'v> {
    None,
    Before(&'v View),
}

fn words(parts: &[&[u32]]) -> Vec<u32> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

impl Ops<'_, '_> {
    pub(super) fn conv(&mut self, c: &Conv, src: &View, dst: &View, res: Res) {
        let qh = (src.h as i32 + 2 * c.pad.0 - (c.dil.0 * (c.kh - 1)) as i32 - 1)
            / c.stride.0 as i32
            + 1;
        let qw = (src.w as i32 + 2 * c.pad.1 - (c.dil.1 * (c.kw - 1)) as i32 - 1)
            / c.stride.1 as i32
            + 1;
        self.conv_mapped(c, src, dst, res, (qh as u32, qw as u32), (1, 0, 1, 0), 1);
    }

    pub(super) fn deconv(&mut self, d: &Deconv, src: &View, dst: &View, shuffle: u32) {
        for (i, c) in d.parts.iter().enumerate() {
            let (py, px) = ((i / 2) as u32, (i % 2) as u32);
            self.conv_mapped(
                c,
                src,
                dst,
                Res::None,
                (src.h, src.w),
                (2, py, 2, px),
                shuffle,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn conv_mapped(
        &mut self,
        c: &Conv,
        src: &View,
        dst: &View,
        res: Res,
        (qh, qw): (u32, u32),
        (oym, oya, oxm, oxa): (u32, u32, u32, u32),
        shuffle: u32,
    ) {
        debug_assert_eq!(src.c, c.cin);
        let (code, leak) = c.act.code();
        let (mode, r) = match res {
            Res::None => (0, self.k.dummy_view()),
            Res::Before(v) => (1, v.clone()),
        };
        let m = src.n * qh * qw;
        let mut p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[
                qh, qw, oym, oya, oxm, oxa, c.kh, c.kw, c.stride.0, c.stride.1,
            ],
            &[
                c.pad.0 as u32,
                c.pad.1 as u32,
                c.dil.0,
                c.dil.1,
                code,
                leak.to_bits(),
                mode,
            ],
            &r.words(),
            &[c.kh * c.kw * c.cin, c.cols, m, shuffle],
        ]);
        p[5 + 1] = c.cin;
        p[12] = c.cout;
        let param = self.gpu.uniform(&p);
        let tiles = m.div_ceil(64) * c.cout.div_ceil(64);
        self.gpu.dispatch(
            self.pass,
            &self.k.conv,
            param,
            &[&c.weights, &c.bias, &c.alpha, &src.buf, &dst.buf, &r.buf],
            tiles,
        );
    }

    pub(super) fn dwconv(
        &mut self,
        w: &wgpu::Buffer,
        b: &wgpu::Buffer,
        k: u32,
        act: Act,
        src: &View,
        dst: &View,
    ) {
        let (code, _) = act.code();
        let p = words(&[&[src.n], &src.words(), &dst.words(), &[k, (k / 2), code]]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * src.h as u64 * src.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.dwconv,
            param,
            &[w, b, &src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn resize(
        &mut self,
        src: &View,
        dst: &View,
        scale: (f32, f32),
        mul: f32,
        mulc: u32,
    ) {
        let p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[scale.0.to_bits(), scale.1.to_bits(), mul.to_bits(), mulc],
        ]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * dst.h as u64 * dst.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.resize,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn scaled(&mut self, src: &View, dst: &View, factor: f32, mul: f32, mulc: u32) {
        self.resize(src, dst, (1.0 / factor, 1.0 / factor), mul, mulc);
    }

    pub(super) fn warp(&mut self, src: &View, flow: &View, dst: &View) {
        self.warp_at(src, flow, dst, 0);
    }

    pub(super) fn warp_at(&mut self, src: &View, flow: &View, dst: &View, oy: u32) {
        let p = words(&[&[dst.n], &src.words(), &dst.words(), &flow.words(), &[oy]]);
        let param = self.gpu.uniform(&p);
        let items = dst.n as u64 * dst.h as u64 * dst.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.warp,
            param,
            &[&src.buf, &flow.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn shuffle(&mut self, src: &View, dst: &View, r: u32) {
        let p = words(&[&[src.n], &src.words(), &dst.words(), &[r]]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * dst.h as u64 * dst.w as u64 * dst.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.shuffle,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn copy(&mut self, src: &View, dst: &View, mul: f32, add: f32, act: Act) {
        let (code, _) = act.code();
        let p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[mul.to_bits(), add.to_bits(), code],
        ]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * src.h as u64 * src.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.copy,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn add(&mut self, src: &View, dst: &View) {
        let p = words(&[&[src.n], &src.words(), &dst.words()]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * src.h as u64 * src.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.add,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn combine(&mut self, a: &View, b: &View, dst: &View, wa: f32, wb: f32) {
        let p = words(&[
            &[a.n],
            &a.words(),
            &b.words(),
            &dst.words(),
            &[wa.to_bits(), wb.to_bits()],
        ]);
        let param = self.gpu.uniform(&p);
        let items = a.n as u64 * a.h as u64 * a.w as u64 * a.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.combine,
            param,
            &[&a.buf, &b.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn norm(
        &mut self,
        gamma: &wgpu::Buffer,
        beta: &wgpu::Buffer,
        eps: f32,
        src: &View,
        dst: &View,
    ) {
        let p = words(&[&[src.n], &src.words(), &dst.words(), &[eps.to_bits()]]);
        let param = self.gpu.uniform(&p);
        let pixels = src.n * src.h * src.w;
        self.gpu.dispatch(
            self.pass,
            &self.k.norm,
            param,
            &[gamma, beta, &src.buf, &dst.buf],
            pixels.div_ceil(4),
        );
    }

    pub(super) fn fill(&mut self, dst: &View, v: f32) {
        let p = words(&[&[dst.n], &dst.words(), &[v.to_bits()]]);
        let param = self.gpu.uniform(&p);
        let items = dst.n as u64 * dst.h as u64 * dst.w as u64 * dst.c as u64;
        self.gpu
            .dispatch(self.pass, &self.k.fill, param, &[&dst.buf], groups(items));
    }

    pub(super) fn load(
        &mut self,
        rgb: &wgpu::Buffer,
        (sw, sh): (u32, u32),
        pad: (u32, u32),
        edge: bool,
        dst: &View,
    ) {
        self.load_turned(rgb, (sw, sh), pad, edge, false, dst);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn load_turned(
        &mut self,
        rgb: &wgpu::Buffer,
        (sw, sh): (u32, u32),
        pad: (u32, u32),
        edge: bool,
        turned: bool,
        dst: &View,
    ) {
        let p = words(&[
            &[1],
            &dst.words(),
            &[sw, sh, pad.0, pad.1, u32::from(edge), u32::from(turned)],
        ]);
        let param = self.gpu.uniform(&p);
        let items = dst.h as u64 * dst.w as u64 * 3;
        self.gpu.dispatch(
            self.pass,
            &self.k.load,
            param,
            &[rgb, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn store(
        &mut self,
        src: &View,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        self.store_both(src, None, (top, left), (ow, oh), out);
    }

    pub(super) fn store_both(
        &mut self,
        src: &View,
        turned: Option<&View>,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        let n = (ow * oh * 3).div_ceil(4);
        let p = words(&[
            &[1],
            &src.words(),
            &[top, left, ow, oh, n, u32::from(turned.is_some())],
        ]);
        let param = self.gpu.uniform(&p);
        let other = turned.map_or(&src.buf, |t| &t.buf);
        self.gpu.dispatch(
            self.pass,
            &self.k.store,
            param,
            &[&src.buf, out, other],
            groups(u64::from(n)),
        );
    }

    pub(super) fn predict(&mut self, w0: &View, w1: &View, mask: &View, res: &View, dst: &View) {
        let p = words(&[
            &[1],
            &w0.words(),
            &w1.words(),
            &mask.words(),
            &res.words(),
            &dst.words(),
        ]);
        let param = self.gpu.uniform(&p);
        let items = u64::from(w0.h) * u64::from(w0.w) * 3;
        self.gpu.dispatch(
            self.pass,
            &self.k.predict,
            param,
            &[&w0.buf, &w1.buf, &mask.buf, &res.buf, &dst.buf],
            groups(items),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish(
        &mut self,
        w0: &View,
        w1: &View,
        mask: &View,
        res: Option<&View>,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        let n = (ow * oh * 3).div_ceil(4);
        let r = res.cloned().unwrap_or_else(|| mask.clone());
        let p = words(&[
            &[1],
            &w0.words(),
            &w1.words(),
            &mask.words(),
            &r.words(),
            &[top, left, ow, n, u32::from(res.is_some()), oh],
        ]);
        let param = self.gpu.uniform(&p);
        self.gpu.dispatch(
            self.pass,
            &self.k.finish,
            param,
            &[&w0.buf, &w1.buf, &mask.buf, &r.buf, out],
            groups(u64::from(n)),
        );
    }
}

pub(super) struct Grid {
    pub(super) hp: u32,
    pub(super) wp: u32,
    pub(super) top: u32,
    pub(super) left: u32,
    pub(super) shift: u32,
    pub(super) pad: bool,
    pub(super) ids: wgpu::Buffer,
}

impl Grid {
    pub(super) fn new(gpu: &Gpu, h: u32, w: u32, shift: u32) -> Self {
        let hp = h.div_ceil(7) * 7;
        let wp = w.div_ceil(7) * 7;
        let (ph, pw) = (hp - h, wp - w);
        let (top, left) = (ph / 2, pw / 2);
        let band = |v: u32, lo: u32, hi: u32| u32::from(v >= lo) + u32::from(v >= hi);
        let mut ids = vec![0u32; (hp * wp) as usize];
        for y in 0..hp {
            for x in 0..wp {
                let pad = band(y, top, top + h) * 3 + band(x, left, left + w);
                let sh = if shift > 0 {
                    band(y, hp - 7, hp - shift) * 3 + band(x, wp - 7, wp - shift)
                } else {
                    0
                };
                ids[(y * wp + x) as usize] = pad | (sh << 8);
            }
        }
        Self {
            hp,
            wp,
            top,
            left,
            shift,
            pad: ph > 0 || pw > 0,
            ids: gpu.upload(&crate::compute::words(&ids)),
        }
    }

    pub(super) fn tokens(&self) -> u32 {
        self.hp * self.wp
    }
}

impl Ops<'_, '_> {
    pub(super) fn windows(&mut self, g: &Grid, src: &View, dst: &View) {
        let p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[g.hp, g.wp, g.top, g.left, g.shift, 0],
        ]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * u64::from(g.tokens()) * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.window,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(super) fn unwindows(&mut self, g: &Grid, src: &View, dst: &View) {
        let p = words(&[
            &[dst.n],
            &src.words(),
            &dst.words(),
            &[g.hp, g.wp, g.top, g.left, g.shift, 1],
        ]);
        let mut p = p;
        p[2] = dst.h;
        p[3] = dst.w;
        p[6] = dst.c;
        let param = self.gpu.uniform(&p);
        let items = dst.n as u64 * dst.h as u64 * dst.w as u64 * dst.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.window,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn cor(
        &mut self,
        g: &Grid,
        (h, w): (u32, u32),
        n: u32,
        wts: &wgpu::Buffer,
        bias: &wgpu::Buffer,
        dst: &View,
    ) {
        let p = words(&[
            &[n],
            &dst.words(),
            &[h, w, g.hp, g.wp, g.top, g.left, g.shift],
        ]);
        let param = self.gpu.uniform(&p);
        let items = n as u64 * u64::from(g.tokens()) * dst.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.cor,
            param,
            &[wts, bias, &dst.buf],
            groups(items),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn attend(
        &mut self,
        g: &Grid,
        heads: u32,
        q: &View,
        kv: &View,
        ce: &View,
        xo: &View,
        cr: &View,
    ) {
        let nwin = g.tokens() / 49;
        let scale = 1.0 / ((q.c / heads) as f32).sqrt();
        let p = [
            q.c,
            ce.c,
            heads,
            nwin,
            g.wp,
            u32::from(g.pad),
            u32::from(g.shift > 0),
            scale.to_bits(),
        ];
        let param = self.gpu.uniform(&p);
        self.gpu.dispatch(
            self.pass,
            &self.k.attend,
            param,
            &[&q.buf, &kv.buf, &ce.buf, &g.ids, &xo.buf, &cr.buf],
            2 * nwin * heads,
        );
    }
}

impl Kernels {
    fn dummy_view(&self) -> View {
        View {
            buf: self.dummy.clone(),
            base: 0,
            n: 1,
            h: 1,
            w: 1,
            c: 1,
            stride: 1,
            at: 0,
        }
    }
}
