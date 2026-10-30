struct P { wt: u32, count: u32, wm: u32, slot: u32, frame: u32, plane: u32, deep: u32, pad1: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> frames: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn level(pixel: u32, c: u32) -> f32 {
    return f32((pixel >> (8u * c)) & 255u) / 127.5 - 1.0;
}

fn value(at: u32, c: u32) -> f32 {
    if (p.deep == 1u) {
        let word = frames[at * 2u + c / 2u];
        return f32((word >> (16u * (c % 2u))) & 65535u) / 32767.5 - 1.0;
    }
    return level(frames[at], c);
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = (wg.x + wg.y * nwg.x) * 256u + li;
    if (i >= p.count * 384u) {
        return;
    }
    let pos = i / 384u;
    let q = i % 384u;
    let ch = q * 2u;
    let c = ch / 256u;
    let row = (ch / 16u) % 16u;
    let col = ch % 16u;
    let y = (pos / p.wt) * 16u + row;
    let x = (pos % p.wt) * 16u + col;
    let at = p.slot * p.plane + y * p.wm + x;
    let v = vec2<f32>(value(at, c), value(at + 1u, c));
    dst[(p.frame * p.count + pos) * 384u + q] = pack2x16float(v);
}
