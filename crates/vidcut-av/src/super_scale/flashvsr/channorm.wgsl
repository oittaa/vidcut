struct P { rows: u32, c: u32, base: u32, pad: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> x: array<f32>;
@group(0) @binding(2) var<storage, read> w: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<u32>;

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
    let base = r * p.c;
    var ss = 0.0;
    for (var i = li; i < p.c; i += 256u) {
        let v = x[base + i];
        ss += v * v;
    }
    let norm = sqrt(total(li, ss));
    let scale = sqrt(f32(p.c)) / max(norm, 1e-12);
    for (var i = li; i < p.c / 2u; i += 256u) {
        let a = x[base + i * 2u] * scale * w[i * 2u];
        let b = x[base + i * 2u + 1u] * scale * w[i * 2u + 1u];
        let v = vec2<f32>(a, b);
        dst[p.base + base / 2u + i] = pack2x16float(v / (1.0 + exp(-v)));
    }
}
