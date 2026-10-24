struct P { shift: u32, pad0: u32, pad1: u32, pad2: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read_write> state: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> hist: array<u32>;

var<workgroup> above: array<u32, 257>;
var<workgroup> goal: vec4<u32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let h = wg.x;
    let b = 255u - li;
    above[li + 1u] = hist[h * 256u + b];
    hist[h * 256u + b] = 0u;
    if (li == 0u) {
        above[0] = 0u;
        goal = state[h];
    }
    workgroupBarrier();
    for (var d = 1u; d < 256u; d = d * 2u) {
        var v = above[li + 1u];
        if (li >= d) {
            v += above[li + 1u - d];
        }
        workgroupBarrier();
        above[li + 1u] = v;
        workgroupBarrier();
    }
    let st = goal;
    if (above[li + 1u] >= st.y && (li == 0u || above[li] < st.y)) {
        state[h] = vec4<u32>(st.x | (b << p.shift), st.y - above[li], st.z, st.w);
    }
}
