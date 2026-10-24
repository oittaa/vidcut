struct P { w: u32, h: u32, y0: u32, rows: u32, cin: u32, stride: u32, offset: u32, stride2: u32, up: u32, sy0: u32, ctot: u32, tw: u32, tr0: u32, tiles: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> src2: array<u32>;
@group(0) @binding(3) var<storage, read_write> dst: array<u32>;

fn tap(yy: i32, xx: i32, ci: u32) -> vec2<f32> {
    if (yy < 0 || xx < 0 || yy >= i32(p.h) || xx >= i32(p.w)) {
        return vec2<f32>(0.0);
    }
    if (yy < i32(p.y0) - 1 || yy > i32(p.y0 + p.rows)) {
        return vec2<f32>(0.0);
    }
    var sy = u32(yy);
    var sx = u32(xx);
    var sw = p.w;
    if (p.up == 1u) {
        sy = sy / 2u;
        sx = sx / 2u;
        sw = p.w / 2u;
    }
    let at = (sy - p.sy0) * sw + sx;
    if (ci < p.cin) {
        return h2(src[at * p.stride + p.offset + ci / 2u]);
    }
    return h2(src2[at * p.stride2 + (ci - p.cin) / 2u]);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let pairs = p.ctot / 2u;
    let id = gid.x + gid.y * nwg.x * 256u;
    if (id >= p.tiles * pairs) {
        return;
    }
    let t = id / pairs;
    let cp = id % pairs;
    let oy = i32(p.y0 + (p.tr0 + t / p.tw) * 2u) - 1;
    let ox = i32((t % p.tw) * 2u) - 1;
    var d: array<vec2<f32>, 16>;
    for (var i = 0u; i < 16u; i++) {
        d[i] = tap(oy + i32(i / 4u), ox + i32(i % 4u), cp * 2u);
    }
    var e: array<vec2<f32>, 16>;
    for (var c = 0u; c < 4u; c++) {
        e[c] = d[c] - d[8u + c];
        e[4u + c] = d[4u + c] + d[8u + c];
        e[8u + c] = d[8u + c] - d[4u + c];
        e[12u + c] = d[4u + c] - d[12u + c];
    }
    for (var r = 0u; r < 4u; r++) {
        let a = e[r * 4u];
        let b = e[r * 4u + 1u];
        let c = e[r * 4u + 2u];
        let q = e[r * 4u + 3u];
        let v = array<vec2<f32>, 4>(a - c, b + c, c - b, b - q);
        for (var k = 0u; k < 4u; k++) {
            dst[((r * 4u + k) * p.tiles + t) * pairs + cp] = pack2x16float(v[k]);
        }
    }
}
