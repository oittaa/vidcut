@group(0) @binding(0) var encoded: texture_2d<u32>;
@group(0) @binding(1) var<storage, read_write> packed: array<u32>;
@group(0) @binding(2) var<storage, read> rgb: array<u32>;
@group(0) @binding(3) var picture: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(256)
fn pack(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let size = textureDimensions(encoded);
    let bytes = size.x * size.y;
    let word = id.y * groups.x * 256u + id.x;
    if word * 4u >= bytes {
        return;
    }
    var v = 0u;
    for (var k = 0u; k < 4u; k++) {
        let b = word * 4u + k;
        if b < bytes {
            v = v | (textureLoad(encoded, vec2<u32>(b % size.x, b / size.x), 0).r << (k * 8u));
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
