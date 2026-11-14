struct P { rows: u32, rope: u32, nh: u32, nw: u32, eps: f32, pad0: u32, pad1: u32, pad2: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read_write> x: array<u32>;
@group(0) @binding(2) var<storage, read> w: array<f32>;
@group(0) @binding(3) var<storage, read> table: array<vec2<f32>>;

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

fn rotate(v: vec2<f32>, token: u32, j: u32) -> vec2<f32> {
    let b = token / 128u;
    let l = token % 128u;
    let s = p.nh * p.nw;
    let f = (b / s) * 2u + l / 64u;
    let sb = b % s;
    let ht = (sb / p.nw) * 8u + (l / 8u) % 8u;
    let wt = (sb % p.nw) * 8u + l % 8u;
    var cs: vec2<f32>;
    if (j < 22u) {
        cs = table[f * 22u + j];
    } else if (j < 43u) {
        cs = table[88u + ht * 21u + (j - 22u)];
    } else {
        cs = table[88u + p.nh * 8u * 21u + wt * 21u + (j - 43u)];
    }
    return vec2<f32>(v.x * cs.x - v.y * cs.y, v.y * cs.x + v.x * cs.y);
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let r = wg.x + wg.y * nwg.x;
    if (r >= p.rows) {
        return;
    }
    let base = r * 768u;
    let v0 = unpack2x16float(x[base + li]);
    let v1 = unpack2x16float(x[base + li + 256u]);
    let v2 = unpack2x16float(x[base + li + 512u]);
    let ss = dot(v0, v0) + dot(v1, v1) + dot(v2, v2);
    let scale = inverseSqrt(total(li, ss) / 1536.0 + p.eps);
    for (var j = 0u; j < 3u; j++) {
        let q = li + j * 256u;
        var v = v0;
        if (j == 1u) {
            v = v1;
        } else if (j == 2u) {
            v = v2;
        }
        var o = v * scale * vec2<f32>(w[q * 2u], w[q * 2u + 1u]);
        if (p.rope == 1u) {
            o = rotate(o, r, q % 64u);
        }
        x[base + q] = pack2x16float(o);
    }
}
