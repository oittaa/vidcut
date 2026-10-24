struct P { qbs: u32, s: u32, nw: u32, nh: u32, range: u32, stride: u32, pad0: u32, pad1: u32, src: array<vec4<u32>, 2> }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> qp: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> kc: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> kn: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> dst: array<f32>;

var<workgroup> q: array<vec4<f32>, 32>;
var<workgroup> red: array<f32, 256>;

fn key_block(qb: u32, slot: u32) -> i32 {
    let area = p.range * p.range;
    let w = slot / area;
    let rest = slot % area;
    let sb = qb % p.s;
    let half = i32(p.range / 2u);
    let kr = i32(sb / p.nw) + i32(rest / p.range) - half;
    let kc = i32(sb % p.nw) + i32(rest % p.range) - half;
    if (kr < 0 || kc < 0 || kr >= i32(p.nh) || kc >= i32(p.nw)) {
        return -1;
    }
    return i32(w * p.s) + kr * i32(p.nw) + kc;
}

fn logit(h: u32, kb: u32) -> f32 {
    let w = kb / p.s;
    let sb = kb % p.s;
    let src = p.src[w / 4u][w % 4u];
    var acc = 0.0;
    if (src < 64u) {
        let base = ((src * p.s + sb) * 12u + h) * 32u;
        for (var i = 0u; i < 32u; i++) {
            acc += dot(q[i], kc[base + i]);
        }
    } else {
        let base = (((src - 64u) * p.s + sb) * 12u + h) * 32u;
        for (var i = 0u; i < 32u; i++) {
            acc += dot(q[i], kn[base + i]);
        }
    }
    return acc / sqrt(128.0);
}

fn reduce(li: u32, v: f32, is_max: bool) -> f32 {
    red[li] = v;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s = s / 2u) {
        if (li < s) {
            if (is_max) {
                red[li] = max(red[li], red[li + s]);
            } else {
                red[li] += red[li + s];
            }
        }
        workgroupBarrier();
    }
    let t = red[0];
    workgroupBarrier();
    return t;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let row = wg.x + wg.y * nwg.x;
    if (row >= p.qbs * 12u) {
        return;
    }
    let h = row / p.qbs;
    let qb = row % p.qbs;
    if (li < 32u) {
        q[li] = qp[(qb * 12u + h) * 32u + li];
    }
    workgroupBarrier();
    let base = row * p.stride;
    var top = -3.0e38;
    for (var slot = li; slot < p.stride; slot += 256u) {
        let kb = key_block(qb, slot);
        if (kb >= 0) {
            let v = logit(h, u32(kb));
            dst[base + slot] = v;
            top = max(top, v);
        }
    }
    let m = reduce(li, top, true);
    var sum = 0.0;
    for (var slot = li; slot < p.stride; slot += 256u) {
        if (key_block(qb, slot) >= 0) {
            sum += exp(dst[base + slot] - m);
        }
    }
    let lse = m + log(reduce(li, sum, false));
    for (var slot = li; slot < p.stride; slot += 256u) {
        if (key_block(qb, slot) >= 0) {
            dst[base + slot] = exp(dst[base + slot] - lse);
        } else {
            dst[base + slot] = -1.0;
        }
    }
}
