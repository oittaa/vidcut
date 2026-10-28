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

fn byte(i: u32) -> f32 {
    return f32((rgb[i >> 2u] >> ((i & 3u) * 8u)) & 255u) / 255.0;
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

fn luma_at(i: u32) -> f32 {
    return 0.2126 * byte(i) + 0.7152 * byte(i + 1u) + 0.0722 * byte(i + 2u);
}

@compute @workgroup_size(64)
fn luma(@builtin(global_invocation_id) id: vec3<u32>) {
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
                sum += luma_at((y * w + x) * 3u);
            }
        }
        let mean = sum / f32((x1 - x0) * (y1 - y0));
        v = v | (u32(round(16.0 + 219.0 * mean)) << (k * 8u));
    }
    small[id.x] = v;
}
