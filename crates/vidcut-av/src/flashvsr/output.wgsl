struct P { w: u32, rows: u32, y0: u32, wm: u32, pad0: u32, pad1: u32, pad2: u32, pad3: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn level(v: f32) -> u32 {
    return u32(round(clamp(v, 0.0, 1.0) * 255.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.w || id.y >= p.rows) {
        return;
    }
    let i = (id.y * p.wm + id.x) * 3u;
    let r = level(src[i]);
    let g = level(src[i + 1u]);
    let b = level(src[i + 2u]);
    dst[(p.y0 + id.y) * p.w + id.x] = r | (g << 8u) | (b << 16u) | (255u << 24u);
}
