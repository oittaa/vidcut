use crate::compute::{self, Gpu};
use crate::error::{Error, Result};
use crate::ffmpeg::frame;

use super::{
    EDGE, FLAT, GRID, HALF, LEVELS, Pairs, QUALITY, REPEAT, RETURN, ROUNDS, SETTLED, Seen,
};

const WHAT: &str = "stabilization";
const SHARED: u32 = 16 * 257;
const GROUP: u32 = 256;
const CELLS: u32 = (GRID.0 * GRID.1) as u32;
const POINT: u64 = 16;

const GRAY: &str = "
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

fn byte(j: u32) -> f32 {
    return f32((src[j / 4u] >> (8u * (j % 4u))) & 255u);
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = gid(wg, nw) * 256u + li;
    let w = u(1u);
    if i >= u(0u) {
        return;
    }
    let b = (i / w) * u(2u) + (i % w) * 3u;
    dst[i] = 0.299 * byte(b) + 0.587 * byte(b + 1u) + 0.114 * byte(b + 2u);
}
";

const BLUR: &str = "
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = gid(wg, nw) * 256u + li;
    let w = u(0u);
    let h = u(1u);
    if i >= w * h {
        return;
    }
    let x = i % w;
    let y = i / w;
    var a = y * w + select(x - 1u, 0u, x == 0u);
    var c = y * w + min(x + 1u, w - 1u);
    if u(4u) == 1u {
        a = select(y - 1u, 0u, y == 0u) * w + x;
        c = min(y + 1u, h - 1u) * w + x;
    }
    let o = u(2u);
    dst[u(3u) + i] = (src[o + a] + 2.0 * src[o + i] + src[o + c]) / 4.0;
}
";

const SHRINK: &str = "
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = gid(wg, nw) * 256u + li;
    let sw = u(0u);
    let sh = u(1u);
    let w = u(3u);
    if i >= w * u(4u) {
        return;
    }
    let x0 = min(2u * (i % w), sw - 1u);
    let y0 = min(2u * (i / w), sh - 1u);
    let x1 = min(2u * (i % w) + 1u, sw - 1u);
    let y1 = min(2u * (i / w) + 1u, sh - 1u);
    let o = u(2u);
    dst[u(5u) + i] = (src[o + y0 * sw + x0] + src[o + y0 * sw + x1] + src[o + y1 * sw + x0] + src[o + y1 * sw + x1]) / 4.0;
}
";

const APART: &str = "
@group(0) @binding(1) var<storage, read> pyr: array<f32>;
@group(0) @binding(2) var<storage, read_write> parts: array<f32>;

var<workgroup> sums: array<f32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let g = gid(wg, nw);
    let i = g * 256u + li;
    var v = 0.0;
    if i < u(0u) {
        v = abs(pyr[u(1u) + i] - pyr[u(2u) + i]);
    }
    sums[li] = v;
    workgroupBarrier();
    for (var span = 128u; span > 0u; span = span >> 1u) {
        if li < span {
            sums[li] = sums[li] + sums[li + span];
        }
        workgroupBarrier();
    }
    if li == 0u {
        parts[g] = sums[0];
    }
}
";

const SCORE: &str = "
@group(0) @binding(1) var<storage, read> pyr: array<f32>;
@group(0) @binding(2) var<storage, read_write> scores: array<f32>;

fn px(x: u32, y: u32) -> f32 {
    return pyr[u(2u) + y * u(0u) + x];
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = gid(wg, nw) * 256u + li;
    let w = u(0u);
    let h = u(1u);
    let edge = u(3u);
    if i >= w * h {
        return;
    }
    let x = i % w;
    let y = i / w;
    if x < edge || y < edge || x >= w - edge || y >= h - edge {
        scores[i] = 0.0;
        return;
    }
    var a = 0.0;
    var b = 0.0;
    var c = 0.0;
    for (var dy = 0u; dy < 5u; dy++) {
        var ra = 0.0;
        var rb = 0.0;
        var rc = 0.0;
        for (var dx = 0u; dx < 5u; dx++) {
            let xx = x + dx - 2u;
            let yy = y + dy - 2u;
            let gx = (px(xx + 1u, yy) - px(xx - 1u, yy)) * 0.5;
            let gy = (px(xx, yy + 1u) - px(xx, yy - 1u)) * 0.5;
            ra += gx * gx;
            rb += gx * gy;
            rc += gy * gy;
        }
        a += ra;
        b += rb;
        c += rc;
    }
    let d = (a - c) / 2.0;
    scores[i] = (a + c) / 2.0 - sqrt(d * d + b * b);
}
";

const BEST: &str = "
@group(0) @binding(1) var<storage, read> scores: array<f32>;
@group(0) @binding(2) var<storage, read_write> cells: array<vec4<f32>>;

var<workgroup> best: array<f32, 256>;
var<workgroup> at: array<u32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let g = gid(wg, nw);
    let w = u(0u);
    let h = u(1u);
    let edge = u(2u);
    let cols = u(3u);
    let rows = u(4u);
    let iw = w - 2u * edge;
    let ih = h - 2u * edge;
    let c = g % cols;
    let r = g / cols;
    let x0 = edge + c * iw / cols;
    let x1 = edge + (c + 1u) * iw / cols;
    let y0 = edge + r * ih / rows;
    let y1 = edge + (r + 1u) * ih / rows;
    let cw = x1 - x0;
    let n = cw * (y1 - y0);
    var bs = 0.0;
    var bi = 4294967295u;
    for (var k = li; k < n; k += 256u) {
        let j = (y0 + k / cw) * w + x0 + k % cw;
        let v = scores[j];
        if v > bs {
            bs = v;
            bi = j;
        }
    }
    best[li] = bs;
    at[li] = bi;
    workgroupBarrier();
    for (var span = 128u; span > 0u; span = span >> 1u) {
        if li < span {
            let o = best[li + span];
            let oi = at[li + span];
            if o > best[li] || (o == best[li] && oi < at[li]) {
                best[li] = o;
                at[li] = oi;
            }
        }
        workgroupBarrier();
    }
    if li == 0u {
        cells[g] = vec4<f32>(best[0], f32(at[0] % w), f32(at[0] / w), 0.0);
    }
}
";

const FOLLOW: &str = "
@group(0) @binding(1) var<storage, read> pyr: array<f32>;
@group(0) @binding(2) var<storage, read> cells: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> points: array<vec4<f32>>;

var<workgroup> sums: array<vec4<f32>, 256>;
var<workgroup> one: vec4<f32>;

fn pick(o: u32, w: u32, h: u32, x: u32, y: u32) -> f32 {
    return pyr[o + min(y, h - 1u) * w + min(x, w - 1u)];
}

fn at(o: u32, w: u32, h: u32, x: f32, y: f32) -> f32 {
    let cx = clamp(x, 0.0, f32(w - 1u));
    let cy = clamp(y, 0.0, f32(h - 1u));
    let x0 = u32(floor(cx));
    let y0 = u32(floor(cy));
    let fx = cx - f32(x0);
    let fy = cy - f32(y0);
    let a = pick(o, w, h, x0, y0);
    let b = pick(o, w, h, x0 + 1u, y0);
    let c = pick(o, w, h, x0, y0 + 1u);
    let d = pick(o, w, h, x0 + 1u, y0 + 1u);
    let top = a + (b - a) * fx;
    let low = c + (d - c) * fx;
    return top + (low - top) * fy;
}

fn total(v: vec4<f32>, li: u32) -> vec4<f32> {
    sums[li] = v;
    workgroupBarrier();
    for (var span = 128u; span > 0u; span = span >> 1u) {
        if li < span {
            sums[li] = sums[li] + sums[li + span];
        }
        workgroupBarrier();
    }
    let out = workgroupUniformLoad(&sums[0]);
    workgroupBarrier();
    return out;
}

fn follow(ka: u32, kb: u32, pt: vec2<f32>, li: u32) -> vec3<f32> {
    let live = li < SIDE * SIDE;
    let dx = f32(i32(li % SIDE) - HALF);
    let dy = f32(i32(li / SIDE) - HALF);
    var g = vec2<f32>(0.0);
    for (var l = i32(LEVELS) - 1; l >= 0; l--) {
        let lu = u32(l);
        let w = u(lu);
        let h = u(LEVELS + lu);
        let oa = u(ka + lu);
        let ob = u(kb + lu);
        let k = f32(1u << lu);
        let px = (pt.x + 0.5) / k - 0.5;
        let py = (pt.y + 0.5) / k - 0.5;
        var t = 0.0;
        var ix = 0.0;
        var iy = 0.0;
        if live {
            let x = px + dx;
            let y = py + dy;
            t = at(oa, w, h, x, y);
            ix = (at(oa, w, h, x + 1.0, y) - at(oa, w, h, x - 1.0, y)) * 0.5;
            iy = (at(oa, w, h, x, y + 1.0) - at(oa, w, h, x, y - 1.0)) * 0.5;
        }
        let m = total(vec4<f32>(ix * ix, ix * iy, iy * iy, 0.0), li);
        let d = (m.x - m.z) / 2.0;
        let least = (m.x + m.z) / 2.0 - sqrt(d * d + m.y * m.y);
        let det = m.x * m.z - m.y * m.y;
        if least < FLAT * f32(SIDE * SIDE) || det <= 0.0 {
            return vec3<f32>(0.0);
        }
        var v = vec2<f32>(0.0);
        for (var r = 0u; r < ROUNDS; r++) {
            var e = vec4<f32>(0.0);
            if live {
                let diff = t - at(ob, w, h, px + dx + g.x + v.x, py + dy + g.y + v.y);
                e = vec4<f32>(diff * ix, diff * iy, 0.0, 0.0);
            }
            let b = total(e, li);
            let dvx = (m.z * b.x - m.y * b.y) / det;
            let dvy = (m.x * b.y - m.y * b.x) / det;
            v = v + vec2<f32>(dvx, dvy);
            if abs(dvx) < SETTLED && abs(dvy) < SETTLED {
                break;
            }
        }
        if l == 0 {
            g = g + v;
        } else {
            g = 2.0 * (g + v);
        }
    }
    let q = pt + g;
    let inside = q.x >= 0.0 && q.x <= f32(u(0u) - 1u) && q.y >= 0.0 && q.y <= f32(u(LEVELS) - 1u);
    return vec3<f32>(q, select(0.0, 1.0, inside));
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let g = gid(wg, nw);
    var top = 0.0;
    for (var c = li; c < CELLS; c += 256u) {
        top = max(top, cells[c].x);
    }
    sums[li] = vec4<f32>(top, 0.0, 0.0, 0.0);
    workgroupBarrier();
    for (var span = 128u; span > 0u; span = span >> 1u) {
        if li < span {
            sums[li].x = max(sums[li].x, sums[li + span].x);
        }
        workgroupBarrier();
    }
    if li == 0u {
        let c = cells[g];
        one = vec4<f32>(c.x, c.y, c.z, sums[0].x);
    }
    let c = workgroupUniformLoad(&one);
    var out = vec4<f32>(-1.0);
    if c.x > QUALITY * c.w && c.x > 0.0 {
        let pt = c.yz;
        let ahead = follow(2u * LEVELS, 3u * LEVELS, pt, li);
        if ahead.z > 0.0 {
            let back = follow(3u * LEVELS, 2u * LEVELS, ahead.xy, li);
            if back.z > 0.0 && length(back.xy - pt) <= RETURN {
                out = vec4<f32>(pt, ahead.xy);
            }
        }
    }
    if li == 0u {
        points[g] = out;
    }
}
";

struct Pipes {
    gray: wgpu::ComputePipeline,
    blur: wgpu::ComputePipeline,
    shrink: wgpu::ComputePipeline,
    apart: wgpu::ComputePipeline,
    score: wgpu::ComputePipeline,
    best: wgpu::ComputePipeline,
    follow: wgpu::ComputePipeline,
}

impl Pipes {
    fn new(gpu: &Gpu) -> Self {
        let make = |key: &str, body: &str| {
            gpu.pipe(key, || {
                format!("{}{}{body}", compute::ops::HEADER, constants())
            })
        };
        Self {
            gray: make("steady-gray", GRAY),
            blur: make("steady-blur", BLUR),
            shrink: make("steady-shrink", SHRINK),
            apart: make("steady-apart", APART),
            score: make("steady-score", SCORE),
            best: make("steady-best", BEST),
            follow: make("steady-follow", FOLLOW),
        }
    }
}

fn constants() -> String {
    format!(
        "const LEVELS: u32 = {LEVELS}u;
const SIDE: u32 = {side}u;
const HALF: i32 = {HALF};
const ROUNDS: u32 = {ROUNDS}u;
const SETTLED: f32 = {SETTLED:?};
const FLAT: f32 = {FLAT:?};
const RETURN: f32 = {RETURN:?};
const QUALITY: f32 = {QUALITY:?};
const CELLS: u32 = {CELLS}u;
",
        side = 2 * HALF + 1,
    )
}

#[derive(Clone, Copy)]
struct Level {
    w: u32,
    h: u32,
    at: u32,
}

pub(super) struct Measuring {
    gpu: Gpu,
    pipes: Pipes,
    levels: [Level; LEVELS],
    slot: u32,
    now: u32,
    first: bool,
    frame: Option<wgpu::Buffer>,
    one: wgpu::Buffer,
    two: wgpu::Buffer,
    pyr: wgpu::Buffer,
    scores: wgpu::Buffer,
    cells: wgpu::Buffer,
    points: wgpu::Buffer,
    parts: wgpu::Buffer,
    staging: wgpu::Buffer,
}

fn groups(n: u32) -> u32 {
    n.div_ceil(GROUP)
}

impl Measuring {
    pub(super) fn start((w, h): (u32, u32)) -> Result<Self> {
        let gpu = Gpu::start(WHAT, SHARED).map_err(|why| {
            Error::Invalid(format!(
                "{why}; with gpu.acceleration off, the CPU measures the shake"
            ))
        })?;
        let mut levels = [Level { w, h, at: 0 }; LEVELS];
        for l in 1..LEVELS {
            let up = levels[l - 1];
            levels[l] = Level {
                w: (up.w / 2).max(1),
                h: (up.h / 2).max(1),
                at: up.at + up.w * up.h,
            };
        }
        let last = levels[LEVELS - 1];
        let slot = last.at + last.w * last.h;
        let n = (w * h) as usize;
        let read = CELLS as u64 * POINT + u64::from(groups(w * h)) * 4;
        let made = compute::checked(&gpu.device, WHAT, || {
            Ok((
                Pipes::new(&gpu),
                gpu.buffer(n * 4),
                gpu.buffer(n * 4),
                gpu.buffer(2 * slot as usize * 4),
                gpu.buffer(n * 4),
                gpu.buffer(CELLS as usize * POINT as usize),
                gpu.buffer(CELLS as usize * POINT as usize),
                gpu.buffer(groups(w * h) as usize * 4),
                gpu.staging(read),
            ))
        })?;
        let (pipes, one, two, pyr, scores, cells, points, parts, staging) = made;
        Ok(Self {
            gpu,
            pipes,
            levels,
            slot,
            now: 0,
            first: true,
            frame: None,
            one,
            two,
            pyr,
            scores,
            cells,
            points,
            parts,
            staging,
        })
    }

    fn upload(&mut self, frame: &frame::Video) -> u32 {
        let (h, stride) = (frame.height() as usize, frame.stride(0));
        let used = stride * h;
        let data = &frame.data(0)[..used];
        let size = used.next_multiple_of(4) as u64;
        if self.frame.as_ref().is_none_or(|b| b.size() < size) {
            self.frame = Some(self.gpu.buffer(size as usize));
        }
        let buffer = self.frame.as_ref().expect("made");
        match used % 4 {
            0 => self.gpu.queue.write_buffer(buffer, 0, data),
            _ => {
                let mut padded = data.to_vec();
                padded.resize(size as usize, 0);
                self.gpu.queue.write_buffer(buffer, 0, &padded);
            }
        }
        stride as u32
    }

    pub(super) fn seen(&mut self, frame: &frame::Video) -> Result<Seen> {
        let device = self.gpu.device.clone();
        compute::checked(&device, WHAT, || self.run(frame))
    }

    fn run(&mut self, frame: &frame::Video) -> Result<Seen> {
        let stride = self.upload(frame);
        let first = std::mem::replace(&mut self.first, false);
        let (before, after) = ((1 - self.now) * self.slot, self.now * self.slot);
        let l0 = self.levels[0];
        let n = l0.w * l0.h;
        let wide = l0.w > 2 * EDGE as u32 && l0.h > 2 * EDGE as u32;
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        {
            let g = &self.gpu;
            let p = &self.pipes;
            let mut pass = enc.begin_compute_pass(&Default::default());
            let source = self.frame.as_ref().expect("uploaded");
            g.dispatch(
                &mut pass,
                &p.gray,
                g.uniform(&[n, l0.w, stride]),
                &[source, &self.one],
                groups(n),
            );
            let blur = |pass: &mut wgpu::ComputePass,
                        (w, h): (u32, u32),
                        (src, from): (&wgpu::Buffer, u32),
                        (dst, to): (&wgpu::Buffer, u32),
                        down: u32| {
                g.dispatch(
                    pass,
                    &p.blur,
                    g.uniform(&[w, h, from, to, down]),
                    &[src, dst],
                    groups(w * h),
                );
            };
            let size = (l0.w, l0.h);
            blur(&mut pass, size, (&self.one, 0), (&self.two, 0), 0);
            blur(&mut pass, size, (&self.two, 0), (&self.pyr, after), 1);
            for pair in self.levels.windows(2) {
                let (up, l) = (pair[0], pair[1]);
                let size = (up.w, up.h);
                blur(
                    &mut pass,
                    size,
                    (&self.pyr, after + up.at),
                    (&self.one, 0),
                    0,
                );
                blur(&mut pass, size, (&self.one, 0), (&self.two, 0), 1);
                g.dispatch(
                    &mut pass,
                    &p.shrink,
                    g.uniform(&[up.w, up.h, 0, l.w, l.h, after + l.at]),
                    &[&self.two, &self.pyr],
                    groups(l.w * l.h),
                );
            }
            if !first {
                g.dispatch(
                    &mut pass,
                    &p.apart,
                    g.uniform(&[n, before, after]),
                    &[&self.pyr, &self.parts],
                    groups(n),
                );
            }
            if !first && wide {
                g.dispatch(
                    &mut pass,
                    &p.score,
                    g.uniform(&[l0.w, l0.h, before, EDGE as u32]),
                    &[&self.pyr, &self.scores],
                    groups(n),
                );
                let (cols, rows) = (GRID.0 as u32, GRID.1 as u32);
                g.dispatch(
                    &mut pass,
                    &p.best,
                    g.uniform(&[l0.w, l0.h, EDGE as u32, cols, rows]),
                    &[&self.scores, &self.cells],
                    CELLS,
                );
                let mut values = Vec::with_capacity(4 * LEVELS);
                values.extend(self.levels.iter().map(|l| l.w));
                values.extend(self.levels.iter().map(|l| l.h));
                values.extend(self.levels.iter().map(|l| before + l.at));
                values.extend(self.levels.iter().map(|l| after + l.at));
                g.dispatch(
                    &mut pass,
                    &p.follow,
                    g.uniform(&values),
                    &[&self.pyr, &self.cells, &self.points],
                    CELLS,
                );
            }
        }
        self.now = 1 - self.now;
        if first {
            self.gpu.flush();
            self.gpu.queue.submit([enc.finish()]);
            return Ok(Seen::First);
        }
        let points = CELLS as u64 * POINT;
        let parts = u64::from(groups(n)) * 4;
        if wide {
            enc.copy_buffer_to_buffer(&self.points, 0, &self.staging, 0, points);
        }
        enc.copy_buffer_to_buffer(&self.parts, 0, &self.staging, points, parts);
        self.gpu.flush();
        let done = self.gpu.queue.submit([enc.finish()]);
        let raw = self.gpu.fetch(&self.staging, points + parts, Some(done))?;
        let read = compute::floats(&raw);
        let (found, sums) = read.split_at(CELLS as usize * 4);
        let apart: f32 = sums.iter().sum();
        if apart / (n.max(1) as f32) < REPEAT {
            return Ok(Seen::Same);
        }
        if !wide {
            return Ok(Seen::Moved(Vec::new()));
        }
        let pairs: Pairs = found
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|v| v[0] >= 0.0)
            .map(|v| ([v[0], v[1]], [v[2], v[3]]))
            .collect();
        Ok(Seen::Moved(pairs))
    }
}
