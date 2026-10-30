struct P { w: u32, h: u32, sw: u32, sh: u32, unused: u32, stage: u32, step: u32, count: u32 }
@group(0) @binding(1) var frames: texture_2d<f32>;
@group(0) @binding(2) var made: texture_2d<f32>;
@group(0) @binding(5) var out: texture_storage_2d<rgba16float, write>;

fn at(i: u32) -> vec2<u32> {
    return vec2<u32>(i % p.w, i / p.w);
}

fn source(x: u32, y: u32) -> vec3<f32> {
    let fx = clamp((f32(x) + 0.5) * f32(p.sw) / f32(p.w) - 0.5, 0.0, f32(p.sw - 1u));
    let fy = clamp((f32(y) + 0.5) * f32(p.sh) / f32(p.h) - 0.5, 0.0, f32(p.sh - 1u));
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, p.sw - 1u);
    let y1 = min(y0 + 1u, p.sh - 1u);
    let a = textureLoad(frames, vec2<u32>(x0, y0), 0).rgb;
    let b = textureLoad(frames, vec2<u32>(x1, y0), 0).rgb;
    let c = textureLoad(frames, vec2<u32>(x0, y1), 0).rgb;
    let d = textureLoad(frames, vec2<u32>(x1, y1), 0).rgb;
    let top = mix(a, b, fx - f32(x0));
    let bottom = mix(c, d, fx - f32(x0));
    return mix(top, bottom, fy - f32(y0));
}

fn made_at(i: u32) -> vec3<f32> {
    return textureLoad(made, at(i), 0).rgb;
}

fn finish(i: u32) {
    let v = clamp(made_at(i) + low_at(i), vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(out, at(i), vec4<f32>(v, 1.0));
}
