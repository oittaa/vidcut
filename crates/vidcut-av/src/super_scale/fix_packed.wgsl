struct P { w: u32, h: u32, wm: u32, base: u32, deep: u32, stage: u32, step: u32, count: u32 }
@group(0) @binding(1) var<storage, read> frames: array<u32>;
@group(0) @binding(2) var<storage, read> made: array<u32>;
@group(0) @binding(5) var<storage, read_write> out: array<u32>;

fn source(x: u32, y: u32) -> vec3<f32> {
    let at = p.base + y * p.wm + x;
    if (p.deep == 1u) {
        return vec3<f32>(unpack2x16unorm(frames[at * 2u]), unpack2x16unorm(frames[at * 2u + 1u]).x);
    }
    return unpack4x8unorm(frames[at]).xyz;
}

fn value(at: u32) -> f32 {
    if (p.deep == 1u) {
        return f32((made[at / 2u] >> (16u * (at % 2u))) & 65535u) / 65535.0;
    }
    return f32((made[at / 4u] >> (8u * (at % 4u))) & 255u) / 255.0;
}

fn made_at(i: u32) -> vec3<f32> {
    let at = i * 3u;
    return vec3<f32>(value(at), value(at + 1u), value(at + 2u));
}

fn code(at: u32) -> u32 {
    if (at >= p.w * p.h * 3u) {
        return 0u;
    }
    let v = clamp(value(at) + low[at], 0.0, 1.0);
    if (p.deep == 1u) {
        return u32(round(v * 65535.0));
    }
    return u32(round(v * 255.0));
}

fn finish(i: u32) {
    if (p.deep == 1u) {
        out[i] = code(i * 2u) | (code(i * 2u + 1u) << 16u);
        return;
    }
    out[i] = code(i * 4u) | (code(i * 4u + 1u) << 8u) | (code(i * 4u + 2u) << 16u) | (code(i * 4u + 3u) << 24u);
}
