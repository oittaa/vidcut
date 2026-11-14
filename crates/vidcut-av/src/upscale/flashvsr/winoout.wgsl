struct P { w: u32, rows: u32, cout: u32, tw: u32, tr0: u32, tiles: u32, act: u32, skip: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> m: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;
@group(0) @binding(3) var<storage, read> aux: array<u32>;
@group(0) @binding(4) var<storage, read_write> dst: array<u32>;

fn put(y: u32, x: u32, c: u32, v: vec4<f32>) {
    if (y >= p.rows || x >= p.w) {
        return;
    }
    let o = ((y * p.w + x) * p.cout + c) / 2u;
    var s = v + vec4<f32>(bias[c], bias[c + 1u], bias[c + 2u], bias[c + 3u]);
    if (p.skip == 1u) {
        s = max(s + vec4<f32>(h2(aux[o]), h2(aux[o + 1u])), vec4<f32>(0.0));
    } else if (p.act == 1u) {
        s = max(s, vec4<f32>(0.0));
    }
    dst[o] = pack2x16float(s.xy);
    dst[o + 1u] = pack2x16float(s.zw);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let quads = p.cout / 4u;
    let id = gid.x + gid.y * nwg.x * 256u;
    if (id >= p.tiles * quads) {
        return;
    }
    let t = id / quads;
    let c = (id % quads) * 4u;
    var v: array<vec4<f32>, 16>;
    for (var i = 0u; i < 16u; i++) {
        v[i] = m[((i * p.tiles + t) * p.cout + c) / 4u];
    }
    var r0: array<vec4<f32>, 4>;
    var r1: array<vec4<f32>, 4>;
    for (var k = 0u; k < 4u; k++) {
        r0[k] = v[k] + v[4u + k] + v[8u + k];
        r1[k] = v[4u + k] - v[8u + k] - v[12u + k];
    }
    let y = (p.tr0 + t / p.tw) * 2u;
    let x = (t % p.tw) * 2u;
    put(y, x, c, r0[0] + r0[1] + r0[2]);
    put(y, x + 1u, c, r0[1] - r0[2] - r0[3]);
    put(y + 1u, x, c, r1[0] + r1[1] + r1[2]);
    put(y + 1u, x + 1u, c, r1[1] - r1[2] - r1[3]);
}
