struct P { w: u32, h: u32, wm: u32, base: u32, deep: u32, stage: u32, step: u32, count: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> frames: array<u32>;
@group(0) @binding(2) var<storage, read> made: array<u32>;
@group(0) @binding(3) var<storage, read> src: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<f32>;
@group(0) @binding(5) var<storage, read_write> out: array<u32>;

fn source(x: u32, y: u32) -> vec3<f32> {
    let at = p.base + y * p.wm + x;
    if (p.deep == 1u) {
        return vec3<f32>(unpack2x16unorm(frames[at * 2u]), unpack2x16unorm(frames[at * 2u + 1u]).x);
    }
    return unpack4x8unorm(frames[at]).xyz;
}

fn value(at: u32) -> f32 {
    if (p.deep == 1u) {
        return f32((made[at / 2u] >> (16u * (at % 2u))) & 65535u) / 65535.0;
    }
    return f32((made[at / 4u] >> (8u * (at % 4u))) & 255u) / 255.0;
}

fn low(i: u32) -> vec3<f32> {
    return vec3<f32>(src[i * 3u], src[i * 3u + 1u], src[i * 3u + 2u]);
}

fn put(i: u32, v: vec3<f32>) {
    dst[i * 3u] = v.x;
    dst[i * 3u + 1u] = v.y;
    dst[i * 3u + 2u] = v.z;
}

fn code(at: u32) -> u32 {
    if (at >= p.w * p.h * 3u) {
        return 0u;
    }
    let v = clamp(value(at) + src[at], 0.0, 1.0);
    if (p.deep == 1u) {
        return u32(round(v * 65535.0));
    }
    return u32(round(v * 255.0));
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = (wg.x + wg.y * nwg.x) * 256u + li;
    if (i >= p.count) {
        return;
    }
    if (p.stage == 3u) {
        if (p.deep == 1u) {
            out[i] = code(i * 2u) | (code(i * 2u + 1u) << 16u);
            return;
        }
        out[i] = code(i * 4u) | (code(i * 4u + 1u) << 8u) | (code(i * 4u + 2u) << 16u) | (code(i * 4u + 3u) << 24u);
        return;
    }
    let x = i % p.w;
    let y = i / p.w;
    if (p.stage == 0u) {
        let at = i * 3u;
        put(i, source(x, y) - vec3<f32>(value(at), value(at + 1u), value(at + 2u)));
        return;
    }
    let d = i32(p.step);
    if (p.stage == 1u) {
        let a = u32(clamp(i32(x) - d, 0, i32(p.w) - 1));
        let b = u32(clamp(i32(x) + d, 0, i32(p.w) - 1));
        put(i, 0.25 * low(y * p.w + a) + 0.5 * low(i) + 0.25 * low(y * p.w + b));
        return;
    }
    let a = u32(clamp(i32(y) - d, 0, i32(p.h) - 1));
    let b = u32(clamp(i32(y) + d, 0, i32(p.h) - 1));
    put(i, 0.25 * low(a * p.w + x) + 0.5 * low(i) + 0.25 * low(b * p.w + x));
}
