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

fn dots(a: array<vec4<f32>, 2>, k: vec4<u32>) -> f32 {
    let v = half8(k);
    return dot(a[0], v[0]) + dot(a[1], v[1]);
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
    let r = li / 4u;
    let kq = li % 4u;
    let q0 = qb * 128u + part * 64u;
    var qr: array<vec4<u32>, 16>;
    for (var c = 0u; c < 16u; c++) {
        qr[c] = q[(q0 + r) * 192u + h * 16u + c];
    }
    if (li == 0u) {
        if (p.dense > 0u) {
            blocks = p.dense;
        } else {
            blocks = counts[h * p.qbs + qb];
        }
    }
    let n = workgroupUniformLoad(&blocks);
    var m = -3.0e38;
    var la = 0.0;
    var lb = 0.0;
    var o0 = vec4<f32>(0.0);
    var o1 = vec4<f32>(0.0);
    var o2 = vec4<f32>(0.0);
    var o3 = vec4<f32>(0.0);
    var o4 = vec4<f32>(0.0);
    var o5 = vec4<f32>(0.0);
    var o6 = vec4<f32>(0.0);
    var o7 = vec4<f32>(0.0);
    let scale = 1.0 / sqrt(128.0);
    let k0 = kq * 8u;
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
            var sa = vec4<f32>(0.0);
            var sb = vec4<f32>(0.0);
            for (var c = 0u; c < 16u; c++) {
                let a = half8(qr[c]);
                sa += vec4<f32>(
                    dots(a, ks[k0 * 16u + c]),
                    dots(a, ks[(k0 + 1u) * 16u + c]),
                    dots(a, ks[(k0 + 2u) * 16u + c]),
                    dots(a, ks[(k0 + 3u) * 16u + c]),
                );
                sb += vec4<f32>(
                    dots(a, ks[(k0 + 4u) * 16u + c]),
                    dots(a, ks[(k0 + 5u) * 16u + c]),
                    dots(a, ks[(k0 + 6u) * 16u + c]),
                    dots(a, ks[(k0 + 7u) * 16u + c]),
                );
            }
            sa *= scale;
            sb *= scale;
            mx[r * 4u + kq] = max(max(max(sa.x, sa.y), max(sa.z, sa.w)), max(max(sb.x, sb.y), max(sb.z, sb.w)));
            workgroupBarrier();
            var tm = m;
            for (var i = 0u; i < 4u; i++) {
                tm = max(tm, mx[r * 4u + i]);
            }
            let f = exp(m - tm);
            m = tm;
            let ea = exp(sa - vec4<f32>(m));
            let eb = exp(sb - vec4<f32>(m));
            la = la * f + ea.x + ea.y + ea.z + ea.w;
            lb = lb * f + eb.x + eb.y + eb.z + eb.w;
            o0 *= f;
            o1 *= f;
            o2 *= f;
            o3 *= f;
            o4 *= f;
            o5 *= f;
            o6 *= f;
            o7 *= f;
            ps[r * 16u + kq * 4u] = pack2x16float(ea.xy);
            ps[r * 16u + kq * 4u + 1u] = pack2x16float(ea.zw);
            ps[r * 16u + kq * 4u + 2u] = pack2x16float(eb.xy);
            ps[r * 16u + kq * 4u + 3u] = pack2x16float(eb.zw);
            for (var j = 0u; j < 2u; j++) {
                let e = li + j * 256u;
                ks[e] = value(at, (at.y + t0 + e / 16u) * 192u + h * 16u + e % 16u);
            }
            workgroupBarrier();
            for (var kk = 0u; kk < 16u; kk++) {
                let w2 = unpack2x16float(ps[r * 16u + kk]);
                for (var e = 0u; e < 2u; e++) {
                    let row = (kk * 2u + e) * 16u + kq * 4u;
                    let va = half8(ks[row]);
                    let vb = half8(ks[row + 1u]);
                    let vc = half8(ks[row + 2u]);
                    let vd = half8(ks[row + 3u]);
                    let w = vec4<f32>(w2[e]);
                    o0 = fma(w, va[0], o0);
                    o1 = fma(w, va[1], o1);
                    o2 = fma(w, vb[0], o2);
                    o3 = fma(w, vb[1], o3);
                    o4 = fma(w, vc[0], o4);
                    o5 = fma(w, vc[1], o5);
                    o6 = fma(w, vd[0], o6);
                    o7 = fma(w, vd[1], o7);
                }
            }
            workgroupBarrier();
        }
    }
    mx[r * 8u + kq * 2u] = la;
    mx[r * 8u + kq * 2u + 1u] = lb;
    workgroupBarrier();
    var sum = 0.0;
    for (var i = 0u; i < 8u; i++) {
        sum += mx[r * 8u + i];
    }
    let place = (q0 + r) * 192u + h * 16u + kq * 4u;
    let inv = 1.0 / sum;
    dst[place] = vec4<u32>(
        pack2x16float(o0.xy * inv),
        pack2x16float(o0.zw * inv),
        pack2x16float(o1.xy * inv),
        pack2x16float(o1.zw * inv),
    );
    dst[place + 1u] = vec4<u32>(
        pack2x16float(o2.xy * inv),
        pack2x16float(o2.zw * inv),
        pack2x16float(o3.xy * inv),
        pack2x16float(o3.zw * inv),
    );
    dst[place + 2u] = vec4<u32>(
        pack2x16float(o4.xy * inv),
        pack2x16float(o4.zw * inv),
        pack2x16float(o5.xy * inv),
        pack2x16float(o5.zw * inv),
    );
    dst[place + 3u] = vec4<u32>(
        pack2x16float(o6.xy * inv),
        pack2x16float(o6.zw * inv),
        pack2x16float(o7.xy * inv),
        pack2x16float(o7.zw * inv),
    );
}
