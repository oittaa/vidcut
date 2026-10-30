struct P { wm: u32, hm: u32, r0: u32, rows: u32, sy0: u32, w: u32, deep: u32, pad1: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> wts: array<u32>;
@group(0) @binding(3) var<storage, read> bias: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<atomic<u32>>;

const K: u32 = 1152u;

var<workgroup> wf: array<vec4<f32>, 1152>;

fn weight(n: u32, k: u32) -> f32 {
    let v = unpack2x16float(wts[(n * K + k) / 2u]);
    return select(v.x, v.y, k % 2u == 1u);
}

fn level(v: f32) -> u32 {
    return u32(round(clamp(v, 0.0, 1.0) * 255.0));
}

fn put(at: u32, v: f32) {
    if (p.deep == 1u) {
        let code = u32(round(clamp(v, 0.0, 1.0) * 65535.0));
        atomicOr(&dst[at / 2u], code << (16u * (at % 2u)));
        return;
    }
    atomicOr(&dst[at / 4u], level(v) << (8u * (at % 4u)));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var k = li; k < K; k += 256u) {
        wf[k] = vec4<f32>(weight(0u, k), weight(1u, k), weight(2u, k), 0.0);
    }
    workgroupBarrier();
    if (id.x >= p.w || id.y >= p.rows) {
        return;
    }
    let y = p.r0 + id.y;
    let x = id.x;
    var a0 = 0.0;
    var a1 = 0.0;
    var a2 = 0.0;
    for (var t = 0u; t < 9u; t++) {
        let yy = i32(y) + i32(t / 3u) - 1;
        let xx = i32(x) + i32(t % 3u) - 1;
        if (yy < 0 || xx < 0 || yy >= i32(p.hm) || xx >= i32(p.wm)) {
            continue;
        }
        let base = ((u32(yy) - p.sy0) * p.wm + u32(xx)) * 64u;
        for (var c = 0u; c < 64u; c++) {
            let v = unpack2x16float(src[base + c]);
            let w0 = wf[t * 128u + c * 2u];
            let w1 = wf[t * 128u + c * 2u + 1u];
            a0 = fma(v.x, w0.x, a0);
            a1 = fma(v.x, w0.y, a1);
            a2 = fma(v.x, w0.z, a2);
            a0 = fma(v.y, w1.x, a0);
            a1 = fma(v.y, w1.y, a1);
            a2 = fma(v.y, w1.z, a2);
        }
    }
    let at = (y * p.w + x) * 3u;
    put(at, a0 + bias[0]);
    put(at + 1u, a1 + bias[1]);
    put(at + 2u, a2 + bias[2]);
}
