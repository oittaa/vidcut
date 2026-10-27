struct P { blocks: u32, pad0: u32, pad1: u32, pad2: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec2<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let g = wg.x + wg.y * nwg.x;
    if (g >= p.blocks * 12u) {
        return;
    }
    let b = g / 12u;
    let h = g % 12u;
    var s = vec2<f32>(0.0);
    for (var t = 0u; t < 128u; t++) {
        s += unpack2x16float(src[(b * 128u + t) * 768u + h * 64u + li]);
    }
    dst[(b * 12u + h) * 64u + li] = s / 128.0;
}
