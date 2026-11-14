struct P { qbs: u32, s: u32, nw: u32, nh: u32, range: u32, stride: u32, all: u32, pad: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> probs: array<f32>;
@group(0) @binding(2) var<storage, read> state: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> list: array<u32>;
@group(0) @binding(4) var<storage, read_write> counts: array<u32>;

fn key_block(qb: u32, slot: u32) -> i32 {
    let area = p.range * p.range;
    let w = slot / area;
    let rest = slot % area;
    let sb = qb % p.s;
    let half = i32(p.range / 2u);
    let kr = i32(sb / p.nw) + i32(rest / p.range) - half;
    let kc = i32(sb % p.nw) + i32(rest % p.range) - half;
    if (kr < 0 || kc < 0 || kr >= i32(p.nh) || kc >= i32(p.nw)) {
        return -1;
    }
    return i32(w * p.s) + kr * i32(p.nw) + kc;
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let row = (wg.x + wg.y * nwg.x) * 64u + li;
    if (row >= p.qbs * 12u) {
        return;
    }
    let h = row / p.qbs;
    let qb = row % p.qbs;
    let threshold = bitcast<f32>(state[h].x);
    let base = row * p.stride;
    var n = 0u;
    var first = -1;
    for (var slot = 0u; slot < p.stride; slot++) {
        let kb = key_block(qb, slot);
        if (kb < 0) {
            continue;
        }
        if (first < 0) {
            first = kb;
        }
        if (p.all == 1u || probs[base + slot] > threshold) {
            list[base + n] = u32(kb);
            n += 1u;
        }
    }
    if (n == 0u) {
        list[base] = u32(first);
        n = 1u;
    }
    counts[row] = n;
}
