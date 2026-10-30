@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(3) var<storage, read> low: array<f32>;
@group(0) @binding(4) var<storage, read_write> lower: array<f32>;

fn low_at(i: u32) -> vec3<f32> {
    return vec3<f32>(low[i * 3u], low[i * 3u + 1u], low[i * 3u + 2u]);
}

fn put(i: u32, v: vec3<f32>) {
    lower[i * 3u] = v.x;
    lower[i * 3u + 1u] = v.y;
    lower[i * 3u + 2u] = v.z;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = (wg.x + wg.y * nwg.x) * 256u + li;
    if (i >= p.count) {
        return;
    }
    if (p.stage == 3u) {
        finish(i);
        return;
    }
    let x = i % p.w;
    let y = i / p.w;
    if (p.stage == 0u) {
        put(i, source(x, y) - made_at(i));
        return;
    }
    let d = i32(p.step);
    if (p.stage == 1u) {
        let a = u32(clamp(i32(x) - d, 0, i32(p.w) - 1));
        let b = u32(clamp(i32(x) + d, 0, i32(p.w) - 1));
        put(i, 0.25 * low_at(y * p.w + a) + 0.5 * low_at(i) + 0.25 * low_at(y * p.w + b));
        return;
    }
    let a = u32(clamp(i32(y) - d, 0, i32(p.h) - 1));
    let b = u32(clamp(i32(y) + d, 0, i32(p.h) - 1));
    put(i, 0.25 * low_at(a * p.w + x) + 0.5 * low_at(i) + 0.25 * low_at(b * p.w + x));
}
