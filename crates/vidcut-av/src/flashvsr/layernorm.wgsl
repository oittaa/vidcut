struct P { rows: u32, first: u32, eps: f32, pad: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> x: array<f32>;
@group(0) @binding(2) var<storage, read> mul: array<f32>;
@group(0) @binding(3) var<storage, read> add: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<u32>;

const D: u32 = 1536u;
var<workgroup> red: array<f32, 256>;

fn total(li: u32, v: f32) -> f32 {
    red[li] = v;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s = s / 2u) {
        if (li < s) {
            red[li] += red[li + s];
        }
        workgroupBarrier();
    }
    let t = red[0];
    workgroupBarrier();
    return t;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let r = wg.x + wg.y * nwg.x;
    if (r >= p.rows) {
        return;
    }
    let base = (p.first + r) * D;
    var s = 0.0;
    for (var j = 0u; j < 6u; j++) {
        s += x[base + li + j * 256u];
    }
    let mean = total(li, s) / f32(D);
    var q = 0.0;
    for (var j = 0u; j < 6u; j++) {
        let d = x[base + li + j * 256u] - mean;
        q += d * d;
    }
    let rstd = inverseSqrt(total(li, q) / f32(D) + p.eps);
    for (var j = 0u; j < 3u; j++) {
        let c = (li + j * 256u) * 2u;
        let a = (x[base + c] - mean) * rstd * mul[c] + add[c];
        let b = (x[base + c + 1u] - mean) * rstd * mul[c + 1u] + add[c + 1u];
        dst[r * (D / 2u) + c / 2u] = pack2x16float(vec2<f32>(a, b));
    }
}
