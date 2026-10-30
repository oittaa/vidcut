struct P { w: u32, h: u32, sw: u32, sh: u32, stage: u32, step: u32, p0: u32, p1: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var made: texture_2d<f32>;
@group(0) @binding(3) var low: texture_2d<f32>;
@group(0) @binding(4) var out: texture_storage_2d<rgba16float, write>;

fn source_at(x: u32, y: u32) -> vec3<f32> {
    let fx = clamp((f32(x) + 0.5) * f32(p.sw) / f32(p.w) - 0.5, 0.0, f32(p.sw - 1u));
    let fy = clamp((f32(y) + 0.5) * f32(p.sh) / f32(p.h) - 0.5, 0.0, f32(p.sh - 1u));
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, p.sw - 1u);
    let y1 = min(y0 + 1u, p.sh - 1u);
    let ax = fx - f32(x0);
    let ay = fy - f32(y0);
    let a = textureLoad(source, vec2<u32>(x0, y0), 0).rgb;
    let b = textureLoad(source, vec2<u32>(x1, y0), 0).rgb;
    let c = textureLoad(source, vec2<u32>(x0, y1), 0).rgb;
    let d = textureLoad(source, vec2<u32>(x1, y1), 0).rgb;
    let top = a + (b - a) * ax;
    let bottom = c + (d - c) * ax;
    return top + (bottom - top) * ay;
}

fn low_at(x: u32, y: u32) -> vec3<f32> {
    return textureLoad(low, vec2<u32>(x, y), 0).rgb;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.w || id.y >= p.h) {
        return;
    }
    let x = id.x;
    let y = id.y;
    let at = vec2<u32>(x, y);
    var v: vec3<f32>;
    if (p.stage == 0u) {
        v = source_at(x, y) - textureLoad(made, at, 0).rgb;
    } else if (p.stage == 1u) {
        let a = u32(max(i32(x) - i32(p.step), 0));
        let b = min(x + p.step, p.w - 1u);
        v = 0.25 * low_at(a, y) + 0.5 * low_at(x, y) + 0.25 * low_at(b, y);
    } else if (p.stage == 2u) {
        let a = u32(max(i32(y) - i32(p.step), 0));
        let b = min(y + p.step, p.h - 1u);
        v = 0.25 * low_at(x, a) + 0.5 * low_at(x, y) + 0.25 * low_at(x, b);
    } else {
        v = clamp(textureLoad(made, at, 0).rgb + low_at(x, y), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    textureStore(out, at, vec4<f32>(v, 1.0));
}
