@group(0) @binding(0) var encoded: texture_2d<u32>;
@group(0) @binding(1) var<storage, read_write> packed: array<u32>;
@group(0) @binding(2) var<storage, read> rgb: array<u32>;
@group(0) @binding(3) var picture: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(256)
fn pack(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let size = textureDimensions(encoded);
    let bytes = size.x * size.y * 3u;
    let word = id.y * groups.x * 256u + id.x;
    if word * 4u >= bytes {
        return;
    }
    var v = 0u;
    for (var k = 0u; k < 4u; k++) {
        let b = word * 4u + k;
        if b < bytes {
            let p = b / 3u;
            let texel = textureLoad(encoded, vec2<u32>(p % size.x, p / size.x), 0);
            v = v | (texel[b % 3u] << (k * 8u));
        }
    }
    packed[word] = v;
}

@compute @workgroup_size(256)
fn pack16(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let size = textureDimensions(encoded);
    let halves = size.x * size.y * 3u;
    let word = id.y * groups.x * 256u + id.x;
    if word * 2u >= halves {
        return;
    }
    var v = 0u;
    for (var k = 0u; k < 2u; k++) {
        let b = word * 2u + k;
        if b < halves {
            let p = b / 3u;
            let texel = textureLoad(encoded, vec2<u32>(p % size.x, p / size.x), 0);
            v = v | ((texel[b % 3u] & 65535u) << (k * 16u));
        }
    }
    packed[word] = v;
}

fn byte(i: u32) -> f32 {
    return f32((rgb[i >> 2u] >> ((i & 3u) * 8u)) & 255u) / 255.0;
}

fn half(i: u32) -> f32 {
    return f32((rgb[i >> 1u] >> ((i & 1u) * 16u)) & 65535u) / 65535.0;
}

@group(0) @binding(6) var deep_picture: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn unpack16(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(deep_picture);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let i = (id.y * size.x + id.x) * 3u;
    textureStore(deep_picture, id.xy, vec4<f32>(half(i), half(i + 1u), half(i + 2u), 1.0));
}

@compute @workgroup_size(16, 16)
fn unpack(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(picture);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let i = (id.y * size.x + id.x) * 3u;
    textureStore(picture, id.xy, vec4<f32>(byte(i), byte(i + 1u), byte(i + 2u), 1.0));
}

@group(0) @binding(4) var<storage, read_write> small: array<u32>;
@group(0) @binding(5) var<uniform> sizes: vec4<u32>;

fn channel(i: u32, deep: bool) -> f32 {
    if deep {
        return half(i);
    }
    return byte(i);
}

fn luma_at(i: u32, deep: bool) -> f32 {
    return 0.2126 * channel(i, deep) + 0.7152 * channel(i + 1u, deep) + 0.0722 * channel(i + 2u, deep);
}

@compute @workgroup_size(64)
fn luma(@builtin(global_invocation_id) id: vec3<u32>) {
    small_luma(id, false);
}

@compute @workgroup_size(64)
fn luma16(@builtin(global_invocation_id) id: vec3<u32>) {
    small_luma(id, true);
}

fn small_luma(id: vec3<u32>, deep: bool) {
    let w = sizes.x;
    let h = sizes.y;
    let ow = sizes.z;
    let oh = sizes.w;
    let count = ow * oh;
    if id.x * 4u >= count {
        return;
    }
    var v = 0u;
    for (var k = 0u; k < 4u; k++) {
        let o = id.x * 4u + k;
        if o >= count {
            break;
        }
        let ox = o % ow;
        let oy = o / ow;
        let x0 = ox * w / ow;
        let x1 = max((ox + 1u) * w / ow, x0 + 1u);
        let y0 = oy * h / oh;
        let y1 = max((oy + 1u) * h / oh, y0 + 1u);
        var sum = 0.0;
        for (var y = y0; y < y1; y++) {
            for (var x = x0; x < x1; x++) {
                sum += luma_at((y * w + x) * 3u, deep);
            }
        }
        let mean = sum / f32((x1 - x0) * (y1 - y0));
        v = v | (u32(round(16.0 + 219.0 * mean)) << (k * 8u));
    }
    small[id.x] = v;
}
