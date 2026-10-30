struct P { w_in: u32, h_in: u32, w_out: u32, h_out: u32, ksize: u32, vertical: u32, wm: u32, hm: u32, base: u32, deep: u32, pad1: u32, pad2: u32 }
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> bounds: array<i32>;
@group(0) @binding(3) var<storage, read> coef: array<i32>;
@group(0) @binding(4) var<storage, read_write> dst: array<u32>;

fn channel(pixel: u32, c: u32) -> i32 {
    return i32((pixel >> (8u * c)) & 255u);
}

fn byte(at: u32) -> i32 {
    return i32((src[at / 4u] >> (8u * (at % 4u))) & 255u);
}

fn half(at: u32) -> f32 {
    return f32((src[at / 2u] >> (16u * (at % 2u))) & 65535u) / 65535.0;
}

fn weight(at: u32) -> f32 {
    return f32(coef[at]) / 4194304.0;
}

fn deep(id: vec3<u32>) {
    if (p.vertical == 0u) {
        if (id.x >= p.w_out || id.y >= p.h_in) {
            return;
        }
        let xmin = bounds[id.x * 2u];
        let count = bounds[id.x * 2u + 1u];
        var acc = vec3<f32>(0.0);
        for (var i = 0i; i < count; i++) {
            let at = (id.y * p.w_in + u32(xmin + i)) * 3u;
            acc += vec3<f32>(half(at), half(at + 1u), half(at + 2u)) * weight(id.x * p.ksize + u32(i));
        }
        let v = clamp(acc, vec3<f32>(0.0), vec3<f32>(1.0));
        let o = (id.y * p.w_out + id.x) * 2u;
        dst[o] = pack2x16unorm(v.xy);
        dst[o + 1u] = pack2x16unorm(vec2<f32>(v.z, 1.0));
        return;
    }
    if (id.x >= p.wm || id.y >= p.hm) {
        return;
    }
    let x = min(id.x, p.w_out - 1u);
    let y = min(id.y, p.h_out - 1u);
    let ymin = bounds[y * 2u];
    let count = bounds[y * 2u + 1u];
    var acc = vec3<f32>(0.0);
    for (var i = 0i; i < count; i++) {
        let q = (u32(ymin + i) * p.w_out + x) * 2u;
        let rgb = vec3<f32>(unpack2x16unorm(src[q]), unpack2x16unorm(src[q + 1u]).x);
        acc += rgb * weight(y * p.ksize + u32(i));
    }
    let v = clamp(acc, vec3<f32>(0.0), vec3<f32>(1.0));
    let o = (p.base + id.y * p.wm + id.x) * 2u;
    dst[o] = pack2x16unorm(v.xy);
    dst[o + 1u] = pack2x16unorm(vec2<f32>(v.z, 1.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (p.deep == 1u) {
        deep(id);
        return;
    }
    if (p.vertical == 0u) {
        if (id.x >= p.w_out || id.y >= p.h_in) {
            return;
        }
        let xmin = bounds[id.x * 2u];
        let count = bounds[id.x * 2u + 1u];
        var acc = vec3<i32>(1i << 21u);
        for (var i = 0i; i < count; i++) {
            let k = coef[id.x * p.ksize + u32(i)];
            let at = (id.y * p.w_in + u32(xmin + i)) * 3u;
            acc += vec3<i32>(byte(at), byte(at + 1u), byte(at + 2u)) * k;
        }
        let v = vec3<u32>(clamp(acc >> vec3<u32>(22u), vec3<i32>(0), vec3<i32>(255)));
        dst[id.y * p.w_out + id.x] = v.x | (v.y << 8u) | (v.z << 16u) | (255u << 24u);
        return;
    }
    if (id.x >= p.wm || id.y >= p.hm) {
        return;
    }
    let x = min(id.x, p.w_out - 1u);
    let y = min(id.y, p.h_out - 1u);
    let ymin = bounds[y * 2u];
    let count = bounds[y * 2u + 1u];
    var acc = vec3<i32>(1i << 21u);
    for (var i = 0i; i < count; i++) {
        let k = coef[y * p.ksize + u32(i)];
        let pixel = src[u32(ymin + i) * p.w_out + x];
        acc += vec3<i32>(channel(pixel, 0u), channel(pixel, 1u), channel(pixel, 2u)) * k;
    }
    let v = vec3<u32>(clamp(acc >> vec3<u32>(22u), vec3<i32>(0), vec3<i32>(255)));
    dst[p.base + id.y * p.wm + id.x] = v.x | (v.y << 8u) | (v.z << 16u) | (255u << 24u);
}
