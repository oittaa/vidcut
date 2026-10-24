struct P { w8: u32, h8: u32, wm: u32, plane: u32, f: u32, pad0: u32, pad1: u32, pad2: u32, slots: vec4<u32> }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> frames: array<u32>;
@group(0) @binding(2) var<storage, read> latents: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<u32>;

fn level(pixel: u32, c: u32) -> f32 {
    return f32((pixel >> (8u * c)) & 255u) / 127.5 - 1.0;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let count = p.w8 * p.h8;
    let i = (wg.x + wg.y * nwg.x) * 256u + li;
    if (i >= count * 392u) {
        return;
    }
    let pos = i / 392u;
    let q = i % 392u;
    let ch = q * 2u;
    let y = pos / p.w8;
    let x = pos % p.w8;
    var v: vec2<f32>;
    if (ch < 768u) {
        let c = ch / 256u;
        let ff = (ch / 64u) % 4u;
        let hh = (ch / 8u) % 8u;
        let ww = ch % 8u;
        let at = p.slots[ff] * p.plane + (y * 8u + hh) * p.wm + x * 8u + ww;
        v = vec2<f32>(level(frames[at], c), level(frames[at + 1u], c));
    } else {
        let lc = ch - 768u;
        let at = (lc * 4u + p.f) * count + pos;
        v = vec2<f32>(latents[at], latents[at + 4u * count]);
    }
    dst[pos * 392u + q] = pack2x16float(tanh(v / 3.0) * 3.0);
}
