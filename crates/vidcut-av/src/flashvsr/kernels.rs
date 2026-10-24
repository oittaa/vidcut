use super::gpu::Gpu;

const EPS: f32 = 1e-6;

fn threads(n: u32, size: u32) -> u32 {
    n.div_ceil(size)
}

pub(super) struct Norm<'a> {
    pub(super) x: &'a wgpu::Buffer,
    pub(super) mul: &'a wgpu::Buffer,
    pub(super) add: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) rows: u32,
    pub(super) first: u32,
}

pub(super) fn layer_norm(gpu: &Gpu, pass: &mut wgpu::ComputePass, n: &Norm) {
    let pipe = gpu.pipe("layernorm", || include_str!("layernorm.wgsl").into());
    let p = gpu.uniform(&[n.rows, n.first, EPS.to_bits(), 0]);
    gpu.dispatch(pass, &pipe, p, &[n.x, n.mul, n.add, n.out], n.rows);
}

pub(super) fn rms(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    x: &wgpu::Buffer,
    weight: &wgpu::Buffer,
    table: &wgpu::Buffer,
    count: u32,
    rope: Option<(u32, u32)>,
) {
    let pipe = gpu.pipe("rmsnorm", || include_str!("rmsnorm.wgsl").into());
    let (on, nh, nw) = rope.map_or((0, 0, 0), |(nh, nw)| (1, nh, nw));
    let p = gpu.uniform(&[count, on, nh, nw, EPS.to_bits(), 0, 0, 0]);
    gpu.dispatch(pass, &pipe, p, &[x, weight, table], count);
}

pub(super) fn chan_norm(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    (x, weight, out): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
    count: u32,
    c: u32,
    base: u32,
) {
    let pipe = gpu.pipe("channorm", || include_str!("channorm.wgsl").into());
    let p = gpu.uniform(&[count, c, base, 0]);
    gpu.dispatch(pass, &pipe, p, &[x, weight, out], count);
}

pub(super) struct Unshuffle<'a> {
    pub(super) frames: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) wt: u32,
    pub(super) count: u32,
    pub(super) wm: u32,
    pub(super) plane: u32,
    pub(super) slot: u32,
    pub(super) frame: u32,
}

pub(super) fn unshuffle(gpu: &Gpu, pass: &mut wgpu::ComputePass, u: &Unshuffle) {
    let pipe = gpu.pipe("unshuffle", || include_str!("unshuffle.wgsl").into());
    let p = gpu.uniform(&[u.wt, u.count, u.wm, u.slot, u.frame, u.plane, 0, 0]);
    gpu.dispatch(
        pass,
        &pipe,
        p,
        &[u.frames, u.out],
        threads(u.count * 384, 256),
    );
}

pub(super) struct DecInput<'a> {
    pub(super) frames: &'a wgpu::Buffer,
    pub(super) latents: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) w8: u32,
    pub(super) h8: u32,
    pub(super) wm: u32,
    pub(super) plane: u32,
    pub(super) f: u32,
    pub(super) slots: [u32; 4],
}

pub(super) fn dec_input(gpu: &Gpu, pass: &mut wgpu::ComputePass, d: &DecInput) {
    let pipe = gpu.pipe("decinput", || include_str!("decinput.wgsl").into());
    let s = d.slots;
    let p = gpu.uniform(&[
        d.w8, d.h8, d.wm, d.plane, d.f, 0, 0, 0, s[0], s[1], s[2], s[3],
    ]);
    gpu.dispatch(
        pass,
        &pipe,
        p,
        &[d.frames, d.latents, d.out],
        threads(d.w8 * d.h8 * 392, 256),
    );
}

pub(super) fn pool(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    src: &wgpu::Buffer,
    out: &wgpu::Buffer,
    blocks: u32,
) {
    let pipe = gpu.pipe("pool", || include_str!("pool.wgsl").into());
    let p = gpu.uniform(&[blocks, 0, 0, 0]);
    gpu.dispatch(pass, &pipe, p, &[src, out], blocks * 12);
}

#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub(super) qbs: u32,
    pub(super) s: u32,
    pub(super) nw: u32,
    pub(super) nh: u32,
    pub(super) range: u32,
    pub(super) stride: u32,
    pub(super) src: [u32; 8],
}

pub(super) fn scores(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    g: &Grid,
    (qp, kc, kn, out): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
) {
    let pipe = gpu.pipe("scores", || include_str!("scores.wgsl").into());
    let mut v = vec![g.qbs, g.s, g.nw, g.nh, g.range, g.stride, 0, 0];
    v.extend_from_slice(&g.src);
    let p = gpu.uniform(&v);
    gpu.dispatch(pass, &pipe, p, &[qp, kc, kn, out], g.qbs * 12);
}

pub(super) fn hist(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    (probs, state, hist): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
    per_head: u32,
    shift: u32,
) {
    let pipe = gpu.pipe("hist", || include_str!("hist.wgsl").into());
    let groups = per_head.div_ceil(256 * 16).max(1);
    let p = gpu.uniform(&[per_head, shift, groups, 0]);
    gpu.dispatch(pass, &pipe, p, &[probs, state, hist], groups * 12);
}

pub(super) fn pick(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    (state, hist): (&wgpu::Buffer, &wgpu::Buffer),
    shift: u32,
) {
    let pipe = gpu.pipe("pick", || include_str!("pick.wgsl").into());
    let p = gpu.uniform(&[shift, 0, 0, 0]);
    gpu.dispatch(pass, &pipe, p, &[state, hist], 12);
}

pub(super) fn select(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    g: &Grid,
    all: bool,
    (probs, state, list, counts): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
) {
    let pipe = gpu.pipe("select", || include_str!("select.wgsl").into());
    let p = gpu.uniform(&[g.qbs, g.s, g.nw, g.nh, g.range, g.stride, u32::from(all), 0]);
    gpu.dispatch(
        pass,
        &pipe,
        p,
        &[probs, state, list, counts],
        threads(g.qbs * 12, 64),
    );
}

pub(super) struct Attend<'a> {
    pub(super) q: &'a wgpu::Buffer,
    pub(super) kc: &'a wgpu::Buffer,
    pub(super) vc: &'a wgpu::Buffer,
    pub(super) kn: &'a wgpu::Buffer,
    pub(super) vn: &'a wgpu::Buffer,
    pub(super) list: &'a wgpu::Buffer,
    pub(super) counts: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) qbs: u32,
    pub(super) s: u32,
    pub(super) stride: u32,
    pub(super) dense: u32,
    pub(super) src: [u32; 8],
}

pub(super) fn attend(gpu: &Gpu, pass: &mut wgpu::ComputePass, a: &Attend, first: u32, count: u32) {
    let pipe = gpu.pipe("attend", || include_str!("attend.wgsl").into());
    let mut v = vec![a.qbs, first, count, a.s, a.stride, a.dense, 0, 0];
    v.extend_from_slice(&a.src);
    let p = gpu.uniform(&v);
    gpu.dispatch(
        pass,
        &pipe,
        p,
        &[a.q, a.kc, a.vc, a.kn, a.vn, a.list, a.counts, a.out],
        count * 24,
    );
}

pub(super) struct Resize<'a> {
    pub(super) src: &'a wgpu::Buffer,
    pub(super) bounds: &'a wgpu::Buffer,
    pub(super) coef: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) values: [u32; 12],
}

pub(super) fn resize(gpu: &Gpu, pass: &mut wgpu::ComputePass, r: &Resize, (x, y): (u32, u32)) {
    let pipe = gpu.pipe("resize", || include_str!("resize.wgsl").into());
    let p = gpu.uniform(&r.values);
    let bind = gpu.bind(&pipe, p, &[r.src, r.bounds, r.coef, r.out]);
    pass.set_pipeline(&pipe);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(x.div_ceil(16), y.div_ceil(16), 1);
}

pub(super) struct Last<'a> {
    pub(super) src: &'a wgpu::Buffer,
    pub(super) weights: &'a wgpu::Buffer,
    pub(super) bias: &'a wgpu::Buffer,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) wm: u32,
    pub(super) hm: u32,
    pub(super) r0: u32,
    pub(super) rows: u32,
    pub(super) sy0: u32,
    pub(super) w: u32,
}

pub(super) fn last(gpu: &Gpu, pass: &mut wgpu::ComputePass, l: &Last) {
    let pipe = gpu.pipe("last", || include_str!("last.wgsl").into());
    let p = gpu.uniform(&[l.wm, l.hm, l.r0, l.rows, l.sy0, l.w, 0, 0]);
    let bind = gpu.bind(&pipe, p, &[l.src, l.weights, l.bias, l.out]);
    pass.set_pipeline(&pipe);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(l.w.div_ceil(16), l.rows.div_ceil(16), 1);
}
