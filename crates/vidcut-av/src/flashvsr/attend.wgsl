struct P { qbs: u32, first: u32, count: u32, s: u32, stride: u32, dense: u32, pad0: u32, pad1: u32, src: array<vec4<u32>, 2> }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> q: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> kc: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> vc: array<vec4<u32>>;
@group(0) @binding(4) var<storage, read> kn: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read> vn: array<vec4<u32>>;
@group(0) @binding(6) var<storage, read> list: array<u32>;
@group(0) @binding(7) var<storage, read> counts: array<u32>;
@group(0) @binding(8) var<storage, read_write> dst: array<vec4<u32>>;

var<workgroup> qs: array<vec4<u32>, 1024>;
var<workgroup> ks: array<vec4<u32>, 512>;
var<workgroup> ps: array<u32, 1024>;
var<workgroup> mx: array<f32, 512>;
var<workgroup> blocks: u32;

fn half8(v: vec4<u32>) -> array<vec4<f32>, 2> {
    return array<vec4<f32>, 2>(
        vec4<f32>(unpack2x16float(v.x), unpack2x16float(v.y)),
        vec4<f32>(unpack2x16float(v.z), unpack2x16float(v.w)),
    );
}

fn row_of(kb: u32) -> vec2<u32> {
    if (p.dense > 0u) {
        return vec2<u32>(0u, kb * 128u);
    }
    let w = kb / p.s;
    let sb = kb % p.s;
    let src = p.src[w / 4u][w % 4u];
    if (src < 64u) {
        return vec2<u32>(0u, (src * p.s + sb) * 128u);
    }
    return vec2<u32>(1u, ((src - 64u) * p.s + sb) * 128u);
}

fn key(at: vec2<u32>, i: u32) -> vec4<u32> {
    if (at.x == 0u) {
        return kc[i];
    }
    return kn[i];
}

fn value(at: vec2<u32>, i: u32) -> vec4<u32> {
    if (at.x == 0u) {
        return vc[i];
    }
    return vn[i];
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let idx = wg.x + wg.y * nwg.x;
    if (idx >= p.count * 24u) {
        return;
    }
    let part = idx % 2u;
    let h = (idx / 2u) % 12u;
    let qb = p.first + idx / 24u;
    let g = li / 8u;
    let kq = li % 8u;
    let q0 = qb * 128u + part * 64u;
    for (var j = 0u; j < 4u; j++) {
        let e = li + j * 256u;
        qs[e] = q[(q0 + e / 16u) * 192u + h * 16u + e % 16u];
    }
    if (li == 0u) {
        if (p.dense > 0u) {
            blocks = p.dense;
        } else {
            blocks = counts[h * p.qbs + qb];
        }
    }
    let n = workgroupUniformLoad(&blocks);
    var m0 = -3.0e38;
    var m1 = -3.0e38;
    var l0 = 0.0;
    var l1 = 0.0;
    var o0a = vec4<f32>(0.0);
    var o0b = vec4<f32>(0.0);
    var o0c = vec4<f32>(0.0);
    var o0d = vec4<f32>(0.0);
    var o1a = vec4<f32>(0.0);
    var o1b = vec4<f32>(0.0);
    var o1c = vec4<f32>(0.0);
    var o1d = vec4<f32>(0.0);
    let scale = 1.0 / sqrt(128.0);
    for (var b = 0u; b < n; b++) {
        var kb = b;
        if (p.dense == 0u) {
            kb = list[(h * p.qbs + qb) * p.stride + b];
        }
        let at = row_of(kb);
        for (var t0 = 0u; t0 < 128u; t0 += 32u) {
            for (var j = 0u; j < 2u; j++) {
                let e = li + j * 256u;
                ks[e] = key(at, (at.y + t0 + e / 16u) * 192u + h * 16u + e % 16u);
            }
            workgroupBarrier();
            var s0 = vec4<f32>(0.0);
            var s1 = vec4<f32>(0.0);
            for (var c = 0u; c < 16u; c++) {
                let a = half8(qs[(g * 2u) * 16u + c]);
                let z = half8(qs[(g * 2u + 1u) * 16u + c]);
                let k0 = half8(ks[(kq * 4u) * 16u + c]);
                let k1 = half8(ks[(kq * 4u + 1u) * 16u + c]);
                let k2 = half8(ks[(kq * 4u + 2u) * 16u + c]);
                let k3 = half8(ks[(kq * 4u + 3u) * 16u + c]);
                s0 += vec4<f32>(
                    dot(a[0], k0[0]) + dot(a[1], k0[1]),
                    dot(a[0], k1[0]) + dot(a[1], k1[1]),
                    dot(a[0], k2[0]) + dot(a[1], k2[1]),
                    dot(a[0], k3[0]) + dot(a[1], k3[1]),
                );
                s1 += vec4<f32>(
                    dot(z[0], k0[0]) + dot(z[1], k0[1]),
                    dot(z[0], k1[0]) + dot(z[1], k1[1]),
                    dot(z[0], k2[0]) + dot(z[1], k2[1]),
                    dot(z[0], k3[0]) + dot(z[1], k3[1]),
                );
            }
            s0 *= scale;
            s1 *= scale;
            mx[(g * 2u) * 8u + kq] = max(max(s0.x, s0.y), max(s0.z, s0.w));
            mx[(g * 2u + 1u) * 8u + kq] = max(max(s1.x, s1.y), max(s1.z, s1.w));
            workgroupBarrier();
            var t0m = m0;
            var t1m = m1;
            for (var i = 0u; i < 8u; i++) {
                t0m = max(t0m, mx[(g * 2u) * 8u + i]);
                t1m = max(t1m, mx[(g * 2u + 1u) * 8u + i]);
            }
            let a0 = exp(m0 - t0m);
            let a1 = exp(m1 - t1m);
            m0 = t0m;
            m1 = t1m;
            let e0 = exp(s0 - vec4<f32>(m0));
            let e1 = exp(s1 - vec4<f32>(m1));
            l0 = l0 * a0 + e0.x + e0.y + e0.z + e0.w;
            l1 = l1 * a1 + e1.x + e1.y + e1.z + e1.w;
            o0a *= a0;
            o0b *= a0;
            o0c *= a0;
            o0d *= a0;
            o1a *= a1;
            o1b *= a1;
            o1c *= a1;
            o1d *= a1;
            ps[(g * 2u) * 16u + kq * 2u] = pack2x16float(e0.xy);
            ps[(g * 2u) * 16u + kq * 2u + 1u] = pack2x16float(e0.zw);
            ps[(g * 2u + 1u) * 16u + kq * 2u] = pack2x16float(e1.xy);
            ps[(g * 2u + 1u) * 16u + kq * 2u + 1u] = pack2x16float(e1.zw);
            for (var j = 0u; j < 2u; j++) {
                let e = li + j * 256u;
                ks[e] = value(at, (at.y + t0 + e / 16u) * 192u + h * 16u + e % 16u);
            }
            workgroupBarrier();
            for (var kk = 0u; kk < 16u; kk++) {
                let r0 = unpack2x16float(ps[(g * 2u) * 16u + kk]);
                let r1 = unpack2x16float(ps[(g * 2u + 1u) * 16u + kk]);
                for (var e = 0u; e < 2u; e++) {
                    let row = (kk * 2u + e) * 16u + kq * 2u;
                    let va = half8(ks[row]);
                    let vb = half8(ks[row + 1u]);
                    let w0 = vec4<f32>(r0[e]);
                    let w1 = vec4<f32>(r1[e]);
                    o0a = fma(w0, va[0], o0a);
                    o0b = fma(w0, va[1], o0b);
                    o0c = fma(w0, vb[0], o0c);
                    o0d = fma(w0, vb[1], o0d);
                    o1a = fma(w1, va[0], o1a);
                    o1b = fma(w1, va[1], o1b);
                    o1c = fma(w1, vb[0], o1c);
                    o1d = fma(w1, vb[1], o1d);
                }
            }
            workgroupBarrier();
        }
    }
    mx[(g * 2u) * 8u + kq] = l0;
    mx[(g * 2u + 1u) * 8u + kq] = l1;
    workgroupBarrier();
    var sum0 = 0.0;
    var sum1 = 0.0;
    for (var i = 0u; i < 8u; i++) {
        sum0 += mx[(g * 2u) * 8u + i];
        sum1 += mx[(g * 2u + 1u) * 8u + i];
    }
    let r0 = (q0 + g * 2u) * 192u + h * 16u + kq * 2u;
    let r1 = r0 + 192u;
    let i0 = 1.0 / sum0;
    let i1 = 1.0 / sum1;
    dst[r0] = vec4<u32>(
        pack2x16float(o0a.xy * i0),
        pack2x16float(o0a.zw * i0),
        pack2x16float(o0b.xy * i0),
        pack2x16float(o0b.zw * i0),
    );
    dst[r0 + 1u] = vec4<u32>(
        pack2x16float(o0c.xy * i0),
        pack2x16float(o0c.zw * i0),
        pack2x16float(o0d.xy * i0),
        pack2x16float(o0d.zw * i0),
    );
    dst[r1] = vec4<u32>(
        pack2x16float(o1a.xy * i1),
        pack2x16float(o1a.zw * i1),
        pack2x16float(o1b.xy * i1),
        pack2x16float(o1b.zw * i1),
    );
    dst[r1 + 1u] = vec4<u32>(
        pack2x16float(o1c.xy * i1),
        pack2x16float(o1c.zw * i1),
        pack2x16float(o1d.xy * i1),
        pack2x16float(o1d.zw * i1),
    );
}
