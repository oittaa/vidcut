struct P { per_head: u32, shift: u32, groups: u32, pad: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> probs: array<f32>;
@group(0) @binding(2) var<storage, read> state: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> hist: array<atomic<u32>>;

var<workgroup> local: array<atomic<u32>, 256>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let g = wg.x + wg.y * nwg.x;
    if (g >= p.groups * 12u) {
        return;
    }
    let h = g / p.groups;
    let part = g % p.groups;
    atomicStore(&local[li], 0u);
    workgroupBarrier();
    let st = state[h];
    let prefix = st.x;
    var keep = 0u;
    if (p.shift < 24u) {
        keep = 0xffffffffu << (p.shift + 8u);
    }
    let span = (p.per_head + p.groups - 1u) / p.groups;
    let first = part * span;
    let last = min(first + span, p.per_head);
    for (var i = first + li; i < last; i += 256u) {
        let v = probs[h * p.per_head + i];
        if (v >= 0.0) {
            let bits = bitcast<u32>(v);
            if ((bits & keep) == (prefix & keep)) {
                atomicAdd(&local[(bits >> p.shift) & 255u], 1u);
            }
        }
    }
    workgroupBarrier();
    let n = atomicLoad(&local[li]);
    if (n > 0u) {
        atomicAdd(&hist[h * 256u + li], n);
    }
}
