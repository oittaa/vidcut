use crate::compute::{Gpu, bytes};
use crate::error::{Error, Result};

pub(crate) const SHARED: u32 = 16 * 1024;

#[derive(Clone)]
pub(crate) struct View {
    pub(crate) buf: wgpu::Buffer,
    pub(crate) base: u32,
    pub(crate) n: u32,
    pub(crate) h: u32,
    pub(crate) w: u32,
    pub(crate) c: u32,
    pub(crate) stride: u32,
    pub(crate) at: u32,
}

impl View {
    pub(crate) fn part(&self, at: u32, c: u32) -> View {
        View {
            at: self.at + at,
            c,
            ..self.clone()
        }
    }

    pub(crate) fn frame(&self, b: u32) -> View {
        View {
            base: self.base + b * self.h * self.w * self.stride,
            n: 1,
            ..self.clone()
        }
    }

    pub(crate) fn rows(&self, y: u32, h: u32) -> View {
        debug_assert_eq!(self.n, 1);
        View {
            base: self.base + y * self.w * self.stride,
            h,
            ..self.clone()
        }
    }

    pub(crate) fn shaped(&self, n: u32, h: u32, w: u32, c: u32) -> View {
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

pub(crate) struct Carve<'a> {
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
    pub(crate) fn view(&mut self, n: u32, h: u32, w: u32, c: u32) -> View {
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

    pub(crate) fn mark(&self) -> usize {
        self.live.len()
    }

    pub(crate) fn reset(&mut self, mark: usize) {
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

pub(crate) fn arena<T>(
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
        return Err(Error::Invalid(format!(
            "{} on the GPU: the frame is larger than the GPU's buffers",
            gpu.what
        )));
    }
    let (sizes, slots) = slots(&c.asked);
    let bufs: Vec<wgpu::Buffer> = sizes.iter().map(|&f| gpu.buffer(f as usize * 4)).collect();
    let mut c = carve(&bufs, &slots);
    let t = plan(&mut c);
    Ok((bufs, t))
}

pub(crate) fn tensor(gpu: &Gpu, n: u32, h: u32, w: u32, c: u32) -> View {
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
pub(crate) enum Act {
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

pub(crate) struct Conv {
    pub(crate) cin: u32,
    pub(crate) cout: u32,
    pub(crate) kh: u32,
    pub(crate) kw: u32,
    pub(crate) stride: (u32, u32),
    pub(crate) pad: (i32, i32),
    pub(crate) dil: (u32, u32),
    pub(crate) cols: u32,
    pub(crate) weights: wgpu::Buffer,
    pub(crate) bias: wgpu::Buffer,
    pub(crate) alpha: wgpu::Buffer,
    pub(crate) act: Act,
}

pub(crate) struct Shape {
    pub(crate) cin: u32,
    pub(crate) cout: u32,
    pub(crate) k: (u32, u32),
    pub(crate) stride: u32,
    pub(crate) pad: (i32, i32),
    pub(crate) dil: u32,
}

impl Shape {
    pub(crate) fn square(cin: u32, cout: u32, k: u32, stride: u32, pad: i32) -> Self {
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

pub(crate) fn conv(
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

pub(crate) const WINO: usize = 36;

pub(crate) struct Wino {
    u: wgpu::Buffer,
    bias: wgpu::Buffer,
    alpha: wgpu::Buffer,
    act: Act,
    cin: u32,
    cout: u32,
    cols: u32,
}

pub(crate) fn wino(
    gpu: &Gpu,
    cin: u32,
    cout: u32,
    weights: &[f32],
    bias: &[f32],
    act: Act,
) -> Wino {
    let g = [
        [0.25, 0.0, 0.0],
        [-1.0 / 6.0, -1.0 / 6.0, -1.0 / 6.0],
        [-1.0 / 6.0, 1.0 / 6.0, -1.0 / 6.0],
        [1.0 / 24.0, 1.0 / 12.0, 1.0 / 6.0],
        [1.0 / 24.0, -1.0 / 12.0, 1.0 / 6.0],
        [0.0, 0.0, 1.0f32],
    ];
    let cols = cout.next_multiple_of(128);
    let (ci_n, co_n, wide) = (cin as usize, cout as usize, cols as usize);
    let mut laid = vec![0f32; WINO * ci_n * wide];
    for co in 0..co_n {
        for ci in 0..ci_n {
            let w = &weights[(co * ci_n + ci) * 9..][..9];
            let gw: Vec<[f32; 3]> = g
                .iter()
                .map(|gi| std::array::from_fn(|b| (0..3).map(|a| gi[a] * w[a * 3 + b]).sum()))
                .collect();
            for (i, row) in gw.iter().enumerate() {
                for (j, gj) in g.iter().enumerate() {
                    let v: f32 = (0..3).map(|b| row[b] * gj[b]).sum();
                    laid[((i * 6 + j) * ci_n + ci) * wide + co] = v;
                }
            }
        }
    }
    let zeros = vec![0f32; wide];
    let mut b = zeros.clone();
    b[..co_n].copy_from_slice(bias);
    Wino {
        u: gpu.upload(&bytes(&laid)),
        bias: gpu.upload(&bytes(&b)),
        alpha: gpu.upload(&bytes(&zeros)),
        act,
        cin,
        cout,
        cols,
    }
}

pub(crate) enum Layer {
    Plain(Conv),
    Wino(Wino),
}

#[derive(PartialEq)]
enum Allowed {
    Plain,
    Direct,
    All,
}

fn allowed() -> Allowed {
    match std::env::var("VIDCUT_GPU_CONV").as_deref() {
        Ok("plain") => Allowed::Plain,
        Ok("direct") => Allowed::Direct,
        _ => Allowed::All,
    }
}

pub(crate) fn layer(gpu: &Gpu, s: Shape, weights: &[f32], bias: &[f32], act: Act) -> Layer {
    if allowed() == Allowed::All
        && s.k == (3, 3)
        && s.stride == 1
        && s.pad == (1, 1)
        && s.dil == 1
        && s.cin.is_multiple_of(16)
        && s.cout.is_multiple_of(4)
    {
        Layer::Wino(wino(gpu, s.cin, s.cout, weights, bias, act))
    } else {
        Layer::Plain(conv(gpu, s, weights, Some(bias), act, None))
    }
}

pub(crate) struct Deconv {
    pub(crate) parts: Vec<Conv>,
}

pub(crate) fn deconv(
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

pub(crate) struct Kernels {
    conv: wgpu::ComputePipeline,
    conv4: wgpu::ComputePipeline,
    wino_in: wgpu::ComputePipeline,
    wino_gemm: wgpu::ComputePipeline,
    wino_out: wgpu::ComputePipeline,
    dwconv: wgpu::ComputePipeline,
    resize: wgpu::ComputePipeline,
    pool: wgpu::ComputePipeline,
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
    pub(crate) dummy: wgpu::Buffer,
    four: bool,
}

pub(crate) const HEADER: &str = r#"
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

var<workgroup> sa: array<vec4<f32>, 528>;
var<workgroup> sb: array<vec4<f32>, 256>;

struct Rows {
    y: vec4<i32>,
    x: vec4<i32>,
    b: vec4<u32>,
    ok: vec4<bool>,
}

struct Tap {
    oy: i32,
    ox: i32,
    ci: u32,
    kin: bool,
}

fn rows_of(m: vec4<u32>) -> Rows {
    let per = u(13u) * u(14u);
    let qw = u(14u);
    let r = m % per;
    var out: Rows;
    out.ok = m < vec4<u32>(u(38u));
    out.b = m / per;
    out.y = vec4<i32>((r / qw) * u(21u)) - s(23u);
    out.x = vec4<i32>((r % qw) * u(22u)) - s(24u);
    return out;
}

fn tap_of(k0: u32, kq: u32) -> Tap {
    let cin = u(6u); let kw = u(20u); let kk = u(36u);
    let k = k0 + kq;
    var t: Tap;
    t.kin = k < kk;
    var tap = 0u;
    if (cin % 16u == 0u) {
        tap = k0 / cin;
        t.ci = k0 % cin + kq;
    } else if (t.kin) {
        tap = k / cin;
        t.ci = k % cin;
    }
    t.oy = i32((tap / kw) * u(25u));
    t.ox = i32((tap % kw) * u(26u));
    return t;
}

fn fetch(t: Tap, y0: i32, x0: i32, b: u32, ok: bool) -> f32 {
    let ih = u(2u); let iw = u(3u);
    let y = y0 + t.oy;
    let x = x0 + t.ox;
    if (t.kin && ok && y >= 0 && x >= 0 && y < i32(ih) && x < i32(iw)) {
        return src[u(1u) + ((b * ih + u32(y)) * iw + u32(x)) * u(4u) + u(5u) + t.ci];
    }
    return 0.0;
}

fn load4(t: Tap, r: Rows) -> vec4<f32> {
    return vec4<f32>(
        fetch(t, r.y.x, r.x.x, r.b.x, r.ok.x),
        fetch(t, r.y.y, r.x.y, r.b.y, r.ok.y),
        fetch(t, r.y.z, r.x.z, r.b.z, r.ok.z),
        fetch(t, r.y.w, r.x.w, r.b.w, r.ok.w),
    );
}

fn put(m: u32, n0: u32, v4: vec4<f32>) {
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u); let cout = u(12u);
    let qh = u(13u); let qw = u(14u);
    let oym = u(15u); let oya = u(16u); let oxm = u(17u); let oxa = u(18u);
    let code = u(27u); let la = f(28u); let rmode = u(29u);
    let rb = u(30u); let rstr = u(33u); let rat = u(34u);
    let shuffle = u(39u);
    if (m >= u(38u)) {
        return;
    }
    let per = qh * qw;
    let b = m / per;
    let r = m % per;
    let qy = r / qw;
    let qx = r % qw;
    for (var q = 0u; q < 4u; q++) {
        let co = n0 + q;
        if (co < cout) {
            var y = qy * oym + oya;
            var x = qx * oxm + oxa;
            var c = co;
            if (shuffle > 1u) {
                let sq = shuffle * shuffle;
                y = y * shuffle + (co % sq) / shuffle;
                x = x * shuffle + co % shuffle;
                c = co / sq;
            }
            var v = v4[q] + bias[co];
            if (rmode == 1u) {
                v += res[rb + ((b * oh + y) * ow + x) * rstr + rat + c];
            }
            v = act(v, code, la, alpha[co]);
            dst[ob + ((b * oh + y) * ow + x) * ostr + oat + c] = v;
        }
    }
}

fn b_at(k0: u32, i: u32, tn: u32) -> vec4<f32> {
    let kk = u(36u); let cols = u(37u);
    let k = k0 + i / 16u;
    if (k < kk) {
        return wts[(k * cols + tn * 64u + (i % 16u) * 4u) / 4u];
    }
    return vec4<f32>(0.0);
}

@compute @workgroup_size(128)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let kk = u(36u); let m_all = u(38u); let cout = u(12u);
    let g = gid(wg, nw);
    let tiles_m = (m_all + 127u) / 128u;
    let tiles_n = (cout + 63u) / 64u;
    if (g >= tiles_m * tiles_n) {
        return;
    }
    let tm = g % tiles_m;
    let tn = g / tiles_m;
    let kq = li % 16u;
    let mg = li / 16u;
    let base = tm * 128u + mg * 4u;
    let four = vec4<u32>(0u, 1u, 2u, 3u);
    let r0 = rows_of(base + four);
    let r1 = rows_of(base + 32u + four);
    let r2 = rows_of(base + 64u + four);
    let r3 = rows_of(base + 96u + four);
    let tx = li % 8u;
    let ty = li / 8u;
    var c00 = vec4<f32>(0.0); var c01 = vec4<f32>(0.0); var c10 = vec4<f32>(0.0); var c11 = vec4<f32>(0.0);
    var c20 = vec4<f32>(0.0); var c21 = vec4<f32>(0.0); var c30 = vec4<f32>(0.0); var c31 = vec4<f32>(0.0);
    var c40 = vec4<f32>(0.0); var c41 = vec4<f32>(0.0); var c50 = vec4<f32>(0.0); var c51 = vec4<f32>(0.0);
    var c60 = vec4<f32>(0.0); var c61 = vec4<f32>(0.0); var c70 = vec4<f32>(0.0); var c71 = vec4<f32>(0.0);
    var t = tap_of(0u, kq);
    var a0 = load4(t, r0);
    var a1 = load4(t, r1);
    var a2 = load4(t, r2);
    var a3 = load4(t, r3);
    var w0 = b_at(0u, li, tn);
    var w1 = b_at(0u, li + 128u, tn);
    for (var k0 = 0u; k0 < kk; k0 += 16u) {
        let at = kq * 33u + mg;
        sa[at] = a0;
        sa[at + 8u] = a1;
        sa[at + 16u] = a2;
        sa[at + 24u] = a3;
        sb[li] = w0;
        sb[li + 128u] = w1;
        workgroupBarrier();
        if (k0 + 16u < kk) {
            t = tap_of(k0 + 16u, kq);
            a0 = load4(t, r0);
            a1 = load4(t, r1);
            a2 = load4(t, r2);
            a3 = load4(t, r3);
            w0 = b_at(k0 + 16u, li, tn);
            w1 = b_at(k0 + 16u, li + 128u, tn);
        }
        for (var q = 0u; q < 16u; q += 4u) {
            {
                let x0 = sa[(q + 0u) * 33u + ty];
                let x1 = sa[(q + 0u) * 33u + 16u + ty];
                let y0 = sb[(q + 0u) * 16u + tx];
                let y1 = sb[(q + 0u) * 16u + 8u + tx];
                c00 = fma(vec4<f32>(x0.x), y0, c00); c01 = fma(vec4<f32>(x0.x), y1, c01);
                c10 = fma(vec4<f32>(x0.y), y0, c10); c11 = fma(vec4<f32>(x0.y), y1, c11);
                c20 = fma(vec4<f32>(x0.z), y0, c20); c21 = fma(vec4<f32>(x0.z), y1, c21);
                c30 = fma(vec4<f32>(x0.w), y0, c30); c31 = fma(vec4<f32>(x0.w), y1, c31);
                c40 = fma(vec4<f32>(x1.x), y0, c40); c41 = fma(vec4<f32>(x1.x), y1, c41);
                c50 = fma(vec4<f32>(x1.y), y0, c50); c51 = fma(vec4<f32>(x1.y), y1, c51);
                c60 = fma(vec4<f32>(x1.z), y0, c60); c61 = fma(vec4<f32>(x1.z), y1, c61);
                c70 = fma(vec4<f32>(x1.w), y0, c70); c71 = fma(vec4<f32>(x1.w), y1, c71);
            }
            {
                let x0 = sa[(q + 1u) * 33u + ty];
                let x1 = sa[(q + 1u) * 33u + 16u + ty];
                let y0 = sb[(q + 1u) * 16u + tx];
                let y1 = sb[(q + 1u) * 16u + 8u + tx];
                c00 = fma(vec4<f32>(x0.x), y0, c00); c01 = fma(vec4<f32>(x0.x), y1, c01);
                c10 = fma(vec4<f32>(x0.y), y0, c10); c11 = fma(vec4<f32>(x0.y), y1, c11);
                c20 = fma(vec4<f32>(x0.z), y0, c20); c21 = fma(vec4<f32>(x0.z), y1, c21);
                c30 = fma(vec4<f32>(x0.w), y0, c30); c31 = fma(vec4<f32>(x0.w), y1, c31);
                c40 = fma(vec4<f32>(x1.x), y0, c40); c41 = fma(vec4<f32>(x1.x), y1, c41);
                c50 = fma(vec4<f32>(x1.y), y0, c50); c51 = fma(vec4<f32>(x1.y), y1, c51);
                c60 = fma(vec4<f32>(x1.z), y0, c60); c61 = fma(vec4<f32>(x1.z), y1, c61);
                c70 = fma(vec4<f32>(x1.w), y0, c70); c71 = fma(vec4<f32>(x1.w), y1, c71);
            }
            {
                let x0 = sa[(q + 2u) * 33u + ty];
                let x1 = sa[(q + 2u) * 33u + 16u + ty];
                let y0 = sb[(q + 2u) * 16u + tx];
                let y1 = sb[(q + 2u) * 16u + 8u + tx];
                c00 = fma(vec4<f32>(x0.x), y0, c00); c01 = fma(vec4<f32>(x0.x), y1, c01);
                c10 = fma(vec4<f32>(x0.y), y0, c10); c11 = fma(vec4<f32>(x0.y), y1, c11);
                c20 = fma(vec4<f32>(x0.z), y0, c20); c21 = fma(vec4<f32>(x0.z), y1, c21);
                c30 = fma(vec4<f32>(x0.w), y0, c30); c31 = fma(vec4<f32>(x0.w), y1, c31);
                c40 = fma(vec4<f32>(x1.x), y0, c40); c41 = fma(vec4<f32>(x1.x), y1, c41);
                c50 = fma(vec4<f32>(x1.y), y0, c50); c51 = fma(vec4<f32>(x1.y), y1, c51);
                c60 = fma(vec4<f32>(x1.z), y0, c60); c61 = fma(vec4<f32>(x1.z), y1, c61);
                c70 = fma(vec4<f32>(x1.w), y0, c70); c71 = fma(vec4<f32>(x1.w), y1, c71);
            }
            {
                let x0 = sa[(q + 3u) * 33u + ty];
                let x1 = sa[(q + 3u) * 33u + 16u + ty];
                let y0 = sb[(q + 3u) * 16u + tx];
                let y1 = sb[(q + 3u) * 16u + 8u + tx];
                c00 = fma(vec4<f32>(x0.x), y0, c00); c01 = fma(vec4<f32>(x0.x), y1, c01);
                c10 = fma(vec4<f32>(x0.y), y0, c10); c11 = fma(vec4<f32>(x0.y), y1, c11);
                c20 = fma(vec4<f32>(x0.z), y0, c20); c21 = fma(vec4<f32>(x0.z), y1, c21);
                c30 = fma(vec4<f32>(x0.w), y0, c30); c31 = fma(vec4<f32>(x0.w), y1, c31);
                c40 = fma(vec4<f32>(x1.x), y0, c40); c41 = fma(vec4<f32>(x1.x), y1, c41);
                c50 = fma(vec4<f32>(x1.y), y0, c50); c51 = fma(vec4<f32>(x1.y), y1, c51);
                c60 = fma(vec4<f32>(x1.z), y0, c60); c61 = fma(vec4<f32>(x1.z), y1, c61);
                c70 = fma(vec4<f32>(x1.w), y0, c70); c71 = fma(vec4<f32>(x1.w), y1, c71);
            }
        }
        workgroupBarrier();
    }
    let m0 = tm * 128u + ty * 4u;
    let n0 = tn * 64u + tx * 4u;
    let n1 = n0 + 32u;
    put(m0, n0, c00); put(m0, n1, c01);
    put(m0 + 1u, n0, c10); put(m0 + 1u, n1, c11);
    put(m0 + 2u, n0, c20); put(m0 + 2u, n1, c21);
    put(m0 + 3u, n0, c30); put(m0 + 3u, n1, c31);
    put(m0 + 64u, n0, c40); put(m0 + 64u, n1, c41);
    put(m0 + 65u, n0, c50); put(m0 + 65u, n1, c51);
    put(m0 + 66u, n0, c60); put(m0 + 66u, n1, c61);
    put(m0 + 67u, n0, c70); put(m0 + 67u, n1, c71);
}
"#;

const CONV4: &str = r#"
@group(0) @binding(1) var<storage, read> wts: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> bias: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> alpha: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> res: array<vec4<f32>>;

var<workgroup> sa: array<vec4<f32>, 512>;
var<workgroup> sb: array<vec4<f32>, 256>;

var<private> c00: vec4<f32>;
var<private> c01: vec4<f32>;
var<private> c10: vec4<f32>;
var<private> c11: vec4<f32>;
var<private> c20: vec4<f32>;
var<private> c21: vec4<f32>;
var<private> c30: vec4<f32>;
var<private> c31: vec4<f32>;
var<private> c40: vec4<f32>;
var<private> c41: vec4<f32>;
var<private> c50: vec4<f32>;
var<private> c51: vec4<f32>;
var<private> c60: vec4<f32>;
var<private> c61: vec4<f32>;
var<private> c70: vec4<f32>;
var<private> c71: vec4<f32>;
var<private> tx: u32;
var<private> ty: u32;

struct Rows {
    y: vec4<i32>,
    x: vec4<i32>,
    b: vec4<u32>,
    ok: vec4<bool>,
}

fn rows_of(m: vec4<u32>) -> Rows {
    let per = u(13u) * u(14u);
    let qw = u(14u);
    let r = m % per;
    var out: Rows;
    out.ok = m < vec4<u32>(u(38u));
    out.b = m / per;
    out.y = vec4<i32>((r / qw) * u(21u)) - s(23u);
    out.x = vec4<i32>((r % qw) * u(22u)) - s(24u);
    return out;
}

fn fetch(y: i32, x: i32, b: u32, ok: bool, at: u32) -> vec4<f32> {
    let ih = u(2u); let iw = u(3u);
    if (ok && y >= 0 && x >= 0 && y < i32(ih) && x < i32(iw)) {
        return src[(u(1u) + ((b * ih + u32(y)) * iw + u32(x)) * u(4u) + at) / 4u];
    }
    return vec4<f32>(0.0);
}

fn add8(k: u32, r0: f32, r1: f32, r2: f32, r3: f32, r4: f32, r5: f32, r6: f32, r7: f32) {
    let b0 = sb[k * 16u + tx];
    let b1 = sb[k * 16u + 8u + tx];
    c00 += r0 * b0;
    c01 += r0 * b1;
    c10 += r1 * b0;
    c11 += r1 * b1;
    c20 += r2 * b0;
    c21 += r2 * b1;
    c30 += r3 * b0;
    c31 += r3 * b1;
    c40 += r4 * b0;
    c41 += r4 * b1;
    c50 += r5 * b0;
    c51 += r5 * b1;
    c60 += r6 * b0;
    c61 += r6 * b1;
    c70 += r7 * b0;
    c71 += r7 * b1;
}

fn block(g: u32) {
    let at = ty * 16u + g;
    let a0 = sa[at];
    let a1 = sa[at + 4u];
    let a2 = sa[at + 8u];
    let a3 = sa[at + 12u];
    let a4 = sa[at + 256u];
    let a5 = sa[at + 260u];
    let a6 = sa[at + 264u];
    let a7 = sa[at + 268u];
    let k = g * 4u;
    add8(k, a0.x, a1.x, a2.x, a3.x, a4.x, a5.x, a6.x, a7.x);
    add8(k + 1u, a0.y, a1.y, a2.y, a3.y, a4.y, a5.y, a6.y, a7.y);
    add8(k + 2u, a0.z, a1.z, a2.z, a3.z, a4.z, a5.z, a6.z, a7.z);
    add8(k + 3u, a0.w, a1.w, a2.w, a3.w, a4.w, a5.w, a6.w, a7.w);
}

fn act4(v: vec4<f32>, pa: vec4<f32>) -> vec4<f32> {
    let code = u(27u); let la = f(28u);
    return vec4<f32>(act(v.x, code, la, pa.x), act(v.y, code, la, pa.y), act(v.z, code, la, pa.z), act(v.w, code, la, pa.w));
}

fn put(m: u32, co: u32, acc: vec4<f32>) {
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u); let cout = u(12u);
    let qh = u(13u); let qw = u(14u);
    if (m >= u(38u) || co >= cout) {
        return;
    }
    let per = qh * qw;
    let b = m / per;
    let r = m % per;
    let y = (r / qw) * u(15u) + u(16u);
    let x = (r % qw) * u(17u) + u(18u);
    let pix = (b * oh + y) * ow + x;
    var v = acc + bias[co / 4u];
    if (u(29u) == 1u) {
        v += res[(u(30u) + pix * u(33u) + u(34u) + co) / 4u];
    }
    dst[(ob + pix * ostr + oat + co) / 4u] = act4(v, alpha[co / 4u]);
}

@compute @workgroup_size(128)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let cin = u(6u); let kw = u(20u); let kk = u(36u); let cols = u(37u);
    let tiles_m = (u(38u) + 127u) / 128u;
    let g = gid(wg, nw);
    if (g >= tiles_m * ((u(12u) + 63u) / 64u)) {
        return;
    }
    let m0 = (g % tiles_m) * 128u;
    let n0 = (g / tiles_m) * 64u;
    tx = li % 8u;
    ty = li / 8u;
    let ar = li / 4u;
    let aq = li % 4u;
    let rows = rows_of(vec4<u32>(m0 + ar, m0 + ar + 32u, m0 + ar + 64u, m0 + ar + 96u));
    let c4 = cols / 4u;
    let bk = li / 16u;
    let bq = li % 16u;
    let bi = bk * c4 + n0 / 4u + bq;
    let nk = kk / 16u;
    var tap = 0u;
    var ci = aq * 4u + u(5u);
    var oy = 0;
    var ox = 0;
    var p0 = fetch(rows.y.x, rows.x.x, rows.b.x, rows.ok.x, ci);
    var p1 = fetch(rows.y.y, rows.x.y, rows.b.y, rows.ok.y, ci);
    var p2 = fetch(rows.y.z, rows.x.z, rows.b.z, rows.ok.z, ci);
    var p3 = fetch(rows.y.w, rows.x.w, rows.b.w, rows.ok.w, ci);
    var pb0 = wts[bi];
    var pb1 = wts[bi + 8u * c4];
    for (var ks = 0u; ks < nk; ks++) {
        sa[ar * 4u + aq] = p0;
        sa[(ar + 32u) * 4u + aq] = p1;
        sa[(ar + 64u) * 4u + aq] = p2;
        sa[(ar + 96u) * 4u + aq] = p3;
        sb[bk * 16u + bq] = pb0;
        sb[(bk + 8u) * 16u + bq] = pb1;
        workgroupBarrier();
        if (ks + 1u < nk) {
            let k0 = (ks + 1u) * 16u;
            tap = k0 / cin;
            ci = k0 % cin + aq * 4u + u(5u);
            oy = i32((tap / kw) * u(25u));
            ox = i32((tap % kw) * u(26u));
            p0 = fetch(rows.y.x + oy, rows.x.x + ox, rows.b.x, rows.ok.x, ci);
            p1 = fetch(rows.y.y + oy, rows.x.y + ox, rows.b.y, rows.ok.y, ci);
            p2 = fetch(rows.y.z + oy, rows.x.z + ox, rows.b.z, rows.ok.z, ci);
            p3 = fetch(rows.y.w + oy, rows.x.w + ox, rows.b.w, rows.ok.w, ci);
            let ob = k0 * c4;
            pb0 = wts[bi + ob];
            pb1 = wts[bi + ob + 8u * c4];
        }
        block(0u);
        block(1u);
        block(2u);
        block(3u);
        workgroupBarrier();
    }
    let m = m0 + ty * 4u;
    let n = n0 + tx * 4u;
    put(m, n, c00);
    put(m, n + 32u, c01);
    put(m + 1u, n, c10);
    put(m + 1u, n + 32u, c11);
    put(m + 2u, n, c20);
    put(m + 2u, n + 32u, c21);
    put(m + 3u, n, c30);
    put(m + 3u, n + 32u, c31);
    put(m + 64u, n, c40);
    put(m + 64u, n + 32u, c41);
    put(m + 65u, n, c50);
    put(m + 65u, n + 32u, c51);
    put(m + 66u, n, c60);
    put(m + 66u, n + 32u, c61);
    put(m + 67u, n, c70);
    put(m + 67u, n + 32u, c71);
}
"#;

const WINO_IN: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> v: array<f32>;

struct Six {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

fn bt(x: Six) -> Six {
    return Six(
        4.0 * x.a - 5.0 * x.c + x.e,
        -4.0 * x.b - 4.0 * x.c + x.d + x.e,
        4.0 * x.b - 4.0 * x.c - x.d + x.e,
        -2.0 * x.b - x.c + 2.0 * x.d + x.e,
        2.0 * x.b - x.c - 2.0 * x.d + x.e,
        4.0 * x.b - 5.0 * x.d + x.f,
    );
}

fn d(b: u32, y: i32, x: i32, ci: u32) -> f32 {
    let ih = u(2u); let iw = u(3u);
    if (y < 0 || x < 0 || y >= i32(ih) || x >= i32(iw)) {
        return 0.0;
    }
    return src[u(1u) + ((b * ih + u32(y)) * iw + u32(x)) * u(4u) + u(5u) + ci];
}

fn row(b: u32, y: i32, x: i32, ci: u32) -> Six {
    return bt(Six(d(b, y, x, ci), d(b, y, x + 1, ci), d(b, y, x + 2, ci), d(b, y, x + 3, ci), d(b, y, x + 4, ci), d(b, y, x + 5, ci)));
}

fn put(at: u32, step: u32, j: u32, col: Six) {
    v[at + j * step] = col.a;
    v[at + (6u + j) * step] = col.b;
    v[at + (12u + j) * step] = col.c;
    v[at + (18u + j) * step] = col.d;
    v[at + (24u + j) * step] = col.e;
    v[at + (30u + j) * step] = col.f;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let c = u(6u); let vb = u(7u); let t_all = u(8u); let th = u(9u); let tw = u(10u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= t_all * c) {
        return;
    }
    let ci = i % c;
    let t = i / c;
    let tx = t % tw;
    let ty = (t / tw) % th;
    let b = t / (tw * th);
    let y = i32(ty * 4u) - 1;
    let x = i32(tx * 4u) - 1;
    let r0 = row(b, y, x, ci);
    let r1 = row(b, y + 1, x, ci);
    let r2 = row(b, y + 2, x, ci);
    let r3 = row(b, y + 3, x, ci);
    let r4 = row(b, y + 4, x, ci);
    let r5 = row(b, y + 5, x, ci);
    let step = t_all * c;
    let at = vb + t * c + ci;
    put(at, step, 0u, bt(Six(r0.a, r1.a, r2.a, r3.a, r4.a, r5.a)));
    put(at, step, 1u, bt(Six(r0.b, r1.b, r2.b, r3.b, r4.b, r5.b)));
    put(at, step, 2u, bt(Six(r0.c, r1.c, r2.c, r3.c, r4.c, r5.c)));
    put(at, step, 3u, bt(Six(r0.d, r1.d, r2.d, r3.d, r4.d, r5.d)));
    put(at, step, 4u, bt(Six(r0.e, r1.e, r2.e, r3.e, r4.e, r5.e)));
    put(at, step, 5u, bt(Six(r0.f, r1.f, r2.f, r3.f, r4.f, r5.f)));
}
"#;

const WINO_GEMM: &str = r#"
@group(0) @binding(1) var<storage, read> va: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> ub: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> mc: array<vec4<f32>>;

var<workgroup> sa: array<vec4<f32>, 512>;
var<workgroup> sb: array<vec4<f32>, 512>;

var<private> c00: vec4<f32>;
var<private> c01: vec4<f32>;
var<private> c10: vec4<f32>;
var<private> c11: vec4<f32>;
var<private> c20: vec4<f32>;
var<private> c21: vec4<f32>;
var<private> c30: vec4<f32>;
var<private> c31: vec4<f32>;
var<private> c40: vec4<f32>;
var<private> c41: vec4<f32>;
var<private> c50: vec4<f32>;
var<private> c51: vec4<f32>;
var<private> c60: vec4<f32>;
var<private> c61: vec4<f32>;
var<private> c70: vec4<f32>;
var<private> c71: vec4<f32>;
var<private> tx: u32;
var<private> ty: u32;

fn la(ok: bool, i: u32) -> vec4<f32> {
    if (ok) {
        return va[i];
    }
    return vec4<f32>(0.0);
}

fn add8(k: u32, r0: f32, r1: f32, r2: f32, r3: f32, r4: f32, r5: f32, r6: f32, r7: f32) {
    let b0 = sb[k * 32u + tx];
    let b1 = sb[k * 32u + 16u + tx];
    c00 += r0 * b0;
    c01 += r0 * b1;
    c10 += r1 * b0;
    c11 += r1 * b1;
    c20 += r2 * b0;
    c21 += r2 * b1;
    c30 += r3 * b0;
    c31 += r3 * b1;
    c40 += r4 * b0;
    c41 += r4 * b1;
    c50 += r5 * b0;
    c51 += r5 * b1;
    c60 += r6 * b0;
    c61 += r6 * b1;
    c70 += r7 * b0;
    c71 += r7 * b1;
}

fn block(g: u32) {
    let at = ty * 16u + g;
    let a0 = sa[at];
    let a1 = sa[at + 4u];
    let a2 = sa[at + 8u];
    let a3 = sa[at + 12u];
    let a4 = sa[at + 256u];
    let a5 = sa[at + 260u];
    let a6 = sa[at + 264u];
    let a7 = sa[at + 268u];
    let k = g * 4u;
    add8(k, a0.x, a1.x, a2.x, a3.x, a4.x, a5.x, a6.x, a7.x);
    add8(k + 1u, a0.y, a1.y, a2.y, a3.y, a4.y, a5.y, a6.y, a7.y);
    add8(k + 2u, a0.z, a1.z, a2.z, a3.z, a4.z, a5.z, a6.z, a7.z);
    add8(k + 3u, a0.w, a1.w, a2.w, a3.w, a4.w, a5.w, a6.w, a7.w);
}

fn out2(at: u32, m: u32, n4: u32, v0: vec4<f32>, v1: vec4<f32>) {
    let t = u(0u); let o4 = u(3u) / 4u;
    if (m >= t) {
        return;
    }
    if (n4 < o4) {
        mc[at + m * o4 + n4] = v0;
    }
    if (n4 + 16u < o4) {
        mc[at + m * o4 + n4 + 16u] = v1;
    }
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let t = u(0u); let cin = u(1u); let cols = u(2u); let cout = u(3u);
    let tiles_m = u(6u);
    let tiles = tiles_m * u(7u);
    let g = gid(wg, nw);
    let grp = g / tiles;
    if (grp >= 36u) {
        return;
    }
    let r = g % tiles;
    let m0 = (r % tiles_m) * 128u;
    let n0 = (r / tiles_m) * 128u;
    let k4 = cin / 4u;
    let c4 = cols / 4u;
    let abase = u(4u) + grp * t * k4;
    let bbase = grp * cin * c4;
    tx = li % 16u;
    ty = li / 16u;
    let ar0 = li / 4u;
    let ar1 = ar0 + 64u;
    let aq = li % 4u;
    let ok0 = m0 + ar0 < t;
    let ok1 = m0 + ar1 < t;
    let ai0 = abase + (m0 + ar0) * k4 + aq;
    let ai1 = abase + (m0 + ar1) * k4 + aq;
    let bk = li / 32u;
    let bq = li % 32u;
    let bi0 = bbase + bk * c4 + n0 / 4u + bq;
    let bi1 = bi0 + 8u * c4;
    let nk = cin / 16u;
    var pa0 = la(ok0, ai0);
    var pa1 = la(ok1, ai1);
    var pb0 = ub[bi0];
    var pb1 = ub[bi1];
    for (var ks = 0u; ks < nk; ks++) {
        sa[ar0 * 4u + aq] = pa0;
        sa[ar1 * 4u + aq] = pa1;
        sb[bk * 32u + bq] = pb0;
        sb[(bk + 8u) * 32u + bq] = pb1;
        workgroupBarrier();
        if (ks + 1u < nk) {
            let oa = (ks + 1u) * 4u;
            pa0 = la(ok0, ai0 + oa);
            pa1 = la(ok1, ai1 + oa);
            let ob = (ks + 1u) * 16u * c4;
            pb0 = ub[bi0 + ob];
            pb1 = ub[bi1 + ob];
        }
        block(0u);
        block(1u);
        block(2u);
        block(3u);
        workgroupBarrier();
    }
    let at = u(5u) + grp * t * (cout / 4u);
    let n4 = n0 / 4u + tx;
    let m = m0 + ty * 4u;
    out2(at, m, n4, c00, c01);
    out2(at, m + 1u, n4, c10, c11);
    out2(at, m + 2u, n4, c20, c21);
    out2(at, m + 3u, n4, c30, c31);
    out2(at, m + 64u, n4, c40, c41);
    out2(at, m + 65u, n4, c50, c51);
    out2(at, m + 66u, n4, c60, c61);
    out2(at, m + 67u, n4, c70, c71);
}
"#;

const WINO_OUT: &str = r#"
@group(0) @binding(1) var<storage, read> mo: array<f32>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;
@group(0) @binding(3) var<storage, read> alpha: array<f32>;
@group(0) @binding(4) var<storage, read> res: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<f32>;

fn across(at: u32, step: u32, i: u32) -> vec4<f32> {
    let a = mo[at + (i * 6u) * step];
    let b = mo[at + (i * 6u + 1u) * step];
    let c = mo[at + (i * 6u + 2u) * step];
    let d = mo[at + (i * 6u + 3u) * step];
    let e = mo[at + (i * 6u + 4u) * step];
    let f = mo[at + (i * 6u + 5u) * step];
    return vec4<f32>(
        a + b + c + d + e,
        b - c + 2.0 * d - 2.0 * e,
        b + c + 4.0 * d + 4.0 * e,
        b - c + 8.0 * d - 8.0 * e + f,
    );
}

fn put(b: u32, y: u32, x: u32, co: u32, v0: f32) {
    let ob = u(1u); let oh = u(2u); let ow = u(3u); let ostr = u(4u); let oat = u(5u);
    if (y >= oh || x >= ow) {
        return;
    }
    var v = v0 + bias[co];
    if (u(13u) == 1u) {
        v += res[u(14u) + ((b * oh + y) * ow + x) * u(17u) + u(18u) + co];
    }
    v = act(v, u(11u), f(12u), alpha[co]);
    dst[ob + ((b * oh + y) * ow + x) * ostr + oat + co] = v;
}

fn put4(b: u32, y: u32, x: u32, co: u32, v: vec4<f32>) {
    put(b, y, x, co, v.x);
    put(b, y, x + 1u, co, v.y);
    put(b, y, x + 2u, co, v.z);
    put(b, y, x + 3u, co, v.w);
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let c = u(6u); let mb = u(7u); let t_all = u(8u); let th = u(9u); let tw = u(10u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= t_all * c) {
        return;
    }
    let co = i % c;
    let t = i / c;
    let tx = t % tw;
    let ty = (t / tw) % th;
    let b = t / (tw * th);
    let step = t_all * c;
    let at = mb + t * c + co;
    let r0 = across(at, step, 0u);
    let r1 = across(at, step, 1u);
    let r2 = across(at, step, 2u);
    let r3 = across(at, step, 3u);
    let r4 = across(at, step, 4u);
    let r5 = across(at, step, 5u);
    let y = ty * 4u;
    let x = tx * 4u;
    put4(b, y, x, co, r0 + r1 + r2 + r3 + r4);
    put4(b, y + 1u, x, co, r1 - r2 + 2.0 * r3 - 2.0 * r4);
    put4(b, y + 2u, x, co, r1 + r2 + 4.0 * r3 + 4.0 * r4);
    put4(b, y + 3u, x, co, r1 - r2 + 8.0 * r3 - 8.0 * r4 + r5);
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
    var v = 0.0;
    if (u(17u) == 1u) {
        let ny = min(u32(f32(y) * scy), ih - 1u);
        let nx = min(u32(f32(x) * scx), iw - 1u);
        v = src[ib + ((b * ih + ny) * iw + nx) * istr + iat + ch];
    } else {
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
        v = (1.0 - ly) * ((1.0 - lx) * a + lx * bb) + ly * ((1.0 - lx) * cc + lx * d);
    }
    if (ch < mulc) {
        v *= mul;
    }
    dst[ob + ((b * oh + y) * ow + x) * ostr + oat + ch] = v;
}
"#;

const POOL: &str = r#"
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u);
    let ib = u(1u); let ih = u(2u); let iw = u(3u); let istr = u(4u); let iat = u(5u); let c = u(6u);
    let ob = u(7u); let oh = u(8u); let ow = u(9u); let ostr = u(10u); let oat = u(11u);
    let k = u(13u); let st = u(14u); let pad = s(15u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * oh * ow * c) {
        return;
    }
    let ch = i % c;
    let px = i / c;
    let x = px % ow;
    let y = (px / ow) % oh;
    let b = px / (ow * oh);
    var v = -3.4028235e38;
    for (var ky = 0u; ky < k; ky++) {
        let yy = i32(y * st + ky) - pad;
        if (yy < 0 || yy >= i32(ih)) {
            continue;
        }
        for (var kx = 0u; kx < k; kx++) {
            let xx = i32(x * st + kx) - pad;
            if (xx < 0 || xx >= i32(iw)) {
                continue;
            }
            v = max(v, src[ib + ((b * ih + u32(yy)) * iw + u32(xx)) * istr + iat + ch]);
        }
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
    return clamp(v, 0.0, 1.0);
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
    for (var j = 0u; j < 2u; j++) {
        let at = i * 2u + j;
        if (at < total) {
            let v = u32(round(value(at / 3u, at % 3u) * 65535.0));
            out |= v << (16u * j);
        }
    }
    dst[i] = out;
}
"#;

const LOAD: &str = r#"
@group(0) @binding(1) var<storage, read> rgb: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

fn level(i: u32) -> f32 {
    return f32((rgb[i / 2u] >> (16u * (i % 2u))) & 65535u) / 65535.0;
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
        v = level((cy * sw + cx) * 3u + ch);
    } else if (yy >= 0 && xx >= 0 && yy < i32(sh) && xx < i32(sw)) {
        v = level((u32(yy) * sw + u32(xx)) * 3u + ch);
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
    for (var j = 0u; j < 2u; j++) {
        let at = i * 2u + j;
        if (at < total) {
            let p = at / 3u;
            let y = p / ow + top;
            let x = p % ow + left;
            var v = clamp(src[ib + (y * w + x) * istr + iat + at % 3u], 0.0, 1.0);
            if (two == 1u) {
                let f = clamp(other[(((h - 1u - y) * w) + (w - 1u - x)) * istr + iat + at % 3u], 0.0, 1.0);
                v = (v + f) / 2.0;
            }
            out |= u32(round(v * 65535.0)) << (16u * j);
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
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let make = |key: &str, body: &str| gpu.pipe(key, || format!("{HEADER}{body}"));
        Self {
            conv: make("ops-conv", CONV),
            conv4: make("ops-conv4", CONV4),
            wino_in: make("ops-wino-in", WINO_IN),
            wino_gemm: make("ops-wino-gemm", WINO_GEMM),
            wino_out: make("ops-wino-out", WINO_OUT),
            dwconv: make("ops-dwconv", DWCONV),
            resize: make("ops-resize", RESIZE),
            pool: make("ops-pool", POOL),
            warp: make("ops-warp", WARP),
            shuffle: make("ops-shuffle", SHUFFLE),
            copy: make("ops-copy", COPY),
            add: make("ops-add", ADD),
            norm: make("ops-norm", NORM),
            fill: make("ops-fill", FILL),
            finish: make("ops-finish", FINISH),
            load: make("ops-load", LOAD),
            store: make("ops-store", STORE),
            predict: make("ops-predict", PREDICT),
            combine: make("ops-combine", COMBINE),
            window: make("ops-window", WINDOW),
            cor: make("ops-cor", COR),
            attend: make("ops-attend", ATTEND),
            dummy: gpu.upload(&[0u8; 16]),
            four: allowed() != Allowed::Plain,
        }
    }
}

pub(crate) struct Ops<'a, 'p> {
    pub(crate) gpu: &'a Gpu,
    pub(crate) k: &'a Kernels,
    pub(crate) pass: &'a mut wgpu::ComputePass<'p>,
}

pub(crate) enum Res<'v> {
    None,
    Before(&'v View),
}

fn words(parts: &[&[u32]]) -> Vec<u32> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

impl Ops<'_, '_> {
    pub(crate) fn conv(&mut self, c: &Conv, src: &View, dst: &View, res: Res) {
        let qh = (src.h as i32 + 2 * c.pad.0 - (c.dil.0 * (c.kh - 1)) as i32 - 1)
            / c.stride.0 as i32
            + 1;
        let qw = (src.w as i32 + 2 * c.pad.1 - (c.dil.1 * (c.kw - 1)) as i32 - 1)
            / c.stride.1 as i32
            + 1;
        self.conv_mapped(c, src, dst, res, (qh as u32, qw as u32), (1, 0, 1, 0), 1);
    }

    pub(crate) fn layer(
        &mut self,
        l: &Layer,
        src: &View,
        dst: &View,
        res: Res,
        scratch: (&View, &View),
    ) {
        match l {
            Layer::Plain(c) => self.conv(c, src, dst, res),
            Layer::Wino(w) => self.wino(w, src, dst, res, scratch),
        }
    }

    pub(crate) fn wino(
        &mut self,
        w: &Wino,
        src: &View,
        dst: &View,
        res: Res,
        (v, m): (&View, &View),
    ) {
        let (th, tw) = (src.h.div_ceil(4), src.w.div_ceil(4));
        let t = src.n * th * tw;
        let p = words(&[&[src.n], &src.words(), &[v.base, t, th, tw]]);
        let param = self.gpu.uniform(&p);
        let items = u64::from(t) * u64::from(src.c);
        self.gpu.dispatch(
            self.pass,
            &self.k.wino_in,
            param,
            &[&src.buf, &v.buf],
            groups(items),
        );
        let (tiles_m, tiles_n) = (t.div_ceil(128), w.cols / 128);
        let p = [
            t,
            w.cin,
            w.cols,
            w.cout,
            v.base / 4,
            m.base / 4,
            tiles_m,
            tiles_n,
        ];
        let param = self.gpu.uniform(&p);
        self.gpu.dispatch(
            self.pass,
            &self.k.wino_gemm,
            param,
            &[&v.buf, &w.u, &m.buf],
            WINO as u32 * tiles_m * tiles_n,
        );
        let (mode, r) = match res {
            Res::None => (0, self.k.dummy_view()),
            Res::Before(r) => (1, r.clone()),
        };
        let (code, leak) = w.act.code();
        let p = words(&[
            &[dst.n],
            &dst.words(),
            &[m.base, t, th, tw, code, leak.to_bits(), mode],
            &r.words(),
        ]);
        let param = self.gpu.uniform(&p);
        let items = u64::from(t) * u64::from(w.cout);
        let bufs = [&m.buf, &w.bias, &w.alpha, &r.buf, &dst.buf];
        self.gpu
            .dispatch(self.pass, &self.k.wino_out, param, &bufs, groups(items));
    }

    pub(crate) fn deconv(&mut self, d: &Deconv, src: &View, dst: &View, shuffle: u32) {
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
        let four = |v: &View| [v.base, v.stride, v.at].iter().all(|x| x.is_multiple_of(4));
        let fast = self.k.four
            && shuffle == 1
            && c.cin.is_multiple_of(16)
            && c.cout.is_multiple_of(4)
            && four(src)
            && four(dst)
            && (mode == 0 || four(&r));
        let tiles = m.div_ceil(128) * c.cout.div_ceil(64);
        let kernel = if fast { &self.k.conv4 } else { &self.k.conv };
        self.gpu.dispatch(
            self.pass,
            kernel,
            param,
            &[&c.weights, &c.bias, &c.alpha, &src.buf, &dst.buf, &r.buf],
            tiles,
        );
    }

    pub(crate) fn dwconv(
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

    pub(crate) fn resize(
        &mut self,
        src: &View,
        dst: &View,
        scale: (f32, f32),
        mul: f32,
        mulc: u32,
    ) {
        self.sample(src, dst, scale, (mul, mulc), 0);
    }

    pub(crate) fn nearest(&mut self, src: &View, dst: &View) {
        let scale = (src.h as f32 / dst.h as f32, src.w as f32 / dst.w as f32);
        self.sample(src, dst, scale, (1.0, 0), 1);
    }

    fn sample(
        &mut self,
        src: &View,
        dst: &View,
        scale: (f32, f32),
        (mul, mulc): (f32, u32),
        nearest: u32,
    ) {
        let p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[
                scale.0.to_bits(),
                scale.1.to_bits(),
                mul.to_bits(),
                mulc,
                nearest,
            ],
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

    pub(crate) fn pool(&mut self, src: &View, dst: &View, k: u32, stride: u32, pad: i32) {
        let p = words(&[
            &[src.n],
            &src.words(),
            &dst.words(),
            &[k, stride, pad as u32],
        ]);
        let param = self.gpu.uniform(&p);
        let items = src.n as u64 * dst.h as u64 * dst.w as u64 * src.c as u64;
        self.gpu.dispatch(
            self.pass,
            &self.k.pool,
            param,
            &[&src.buf, &dst.buf],
            groups(items),
        );
    }

    pub(crate) fn scaled(&mut self, src: &View, dst: &View, factor: f32, mul: f32, mulc: u32) {
        self.resize(src, dst, (1.0 / factor, 1.0 / factor), mul, mulc);
    }

    pub(crate) fn warp(&mut self, src: &View, flow: &View, dst: &View) {
        self.warp_at(src, flow, dst, 0);
    }

    pub(crate) fn warp_at(&mut self, src: &View, flow: &View, dst: &View, oy: u32) {
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

    pub(crate) fn shuffle(&mut self, src: &View, dst: &View, r: u32) {
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

    pub(crate) fn copy(&mut self, src: &View, dst: &View, mul: f32, add: f32, act: Act) {
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

    pub(crate) fn add(&mut self, src: &View, dst: &View) {
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

    pub(crate) fn combine(&mut self, a: &View, b: &View, dst: &View, wa: f32, wb: f32) {
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

    pub(crate) fn norm(
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

    pub(crate) fn fill(&mut self, dst: &View, v: f32) {
        let p = words(&[&[dst.n], &dst.words(), &[v.to_bits()]]);
        let param = self.gpu.uniform(&p);
        let items = dst.n as u64 * dst.h as u64 * dst.w as u64 * dst.c as u64;
        self.gpu
            .dispatch(self.pass, &self.k.fill, param, &[&dst.buf], groups(items));
    }

    pub(crate) fn load(
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
    pub(crate) fn load_turned(
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

    pub(crate) fn store(
        &mut self,
        src: &View,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        self.store_both(src, None, (top, left), (ow, oh), out);
    }

    pub(crate) fn store_both(
        &mut self,
        src: &View,
        turned: Option<&View>,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        let n = (ow * oh * 3).div_ceil(2);
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

    pub(crate) fn predict(&mut self, w0: &View, w1: &View, mask: &View, res: &View, dst: &View) {
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
    pub(crate) fn finish(
        &mut self,
        w0: &View,
        w1: &View,
        mask: &View,
        res: Option<&View>,
        (top, left): (u32, u32),
        (ow, oh): (u32, u32),
        out: &wgpu::Buffer,
    ) {
        let n = (ow * oh * 3).div_ceil(2);
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

pub(crate) struct Grid {
    pub(crate) hp: u32,
    pub(crate) wp: u32,
    pub(crate) top: u32,
    pub(crate) left: u32,
    pub(crate) shift: u32,
    pub(crate) pad: bool,
    pub(crate) ids: wgpu::Buffer,
}

impl Grid {
    pub(crate) fn new(gpu: &Gpu, h: u32, w: u32, shift: u32) -> Self {
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

    pub(crate) fn tokens(&self) -> u32 {
        self.hp * self.wp
    }
}

impl Ops<'_, '_> {
    pub(crate) fn windows(&mut self, g: &Grid, src: &View, dst: &View) {
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

    pub(crate) fn unwindows(&mut self, g: &Grid, src: &View, dst: &View) {
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
    pub(crate) fn cor(
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
    pub(crate) fn attend(
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
