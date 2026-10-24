struct P { shift: u32, pad0: u32, pad1: u32, pad2: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read_write> state: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> hist: array<u32>;

@compute @workgroup_size(1)
fn main(@builtin(workgroup_id) wg: vec3<u32>) {
    let h = wg.x;
    var st = state[h];
    var seen = 0u;
    for (var b = 255i; b >= 0i; b--) {
        let n = hist[h * 256u + u32(b)];
        if (seen + n >= st.y) {
            st.x = st.x | (u32(b) << p.shift);
            st.y = st.y - seen;
            break;
        }
        seen += n;
    }
    state[h] = st;
    for (var b = 0u; b < 256u; b++) {
        hist[h * 256u + b] = 0u;
    }
}
