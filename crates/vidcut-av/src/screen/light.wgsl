struct Params {
    size: vec2<u32>,
    sites: vec2<u32>,
    grid: vec2<u32>,
    step: u32,
    shift: u32,
    scale: f32,
    bits: u32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> rank: array<u32>;
@group(0) @binding(4) var<storage, read_write> peaks: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> samples: array<u32>;

const STRIDE: u32 = 1024u;
const ROWS: u32 = 4u;
const NONE: u32 = 0xffffffffu;

var<workgroup> local: array<atomic<u32>, 3072>;

fn code(v: f32) -> u32 {
    return u32(round(v * params.scale)) >> params.shift;
}

fn luma_at(x: u32, y: u32) -> u32 {
    return code(textureLoad(luma, vec2<u32>(x, y), 0).r);
}

fn chroma_at(x: u32, y: u32) -> vec2<u32> {
    let c = textureLoad(chroma, vec2<u32>(x, y), 0);
    return vec2<u32>(code(c.r), code(c.g));
}

fn top(cx: u32, cy: u32) -> u32 {
    let x = 2u * cx;
    let y = 2u * cy;
    let right = min(x + 1u, params.size.x - 1u);
    let below = min(y + 1u, params.size.y - 1u);
    return max(max(luma_at(x, y), luma_at(right, y)), max(luma_at(x, below), luma_at(right, below)));
}

@compute @workgroup_size(16, 16)
fn peak(
    @builtin(local_invocation_index) i: u32,
    @builtin(local_invocation_id) l: vec3<u32>,
    @builtin(workgroup_id) g: vec3<u32>,
) {
    for (var k = i; k < 3u * STRIDE; k += 256u) {
        atomicStore(&local[k], select(0u, NONE, k >= 2u * STRIDE));
    }
    workgroupBarrier();
    let cx = g.x * 16u + l.x;
    for (var r = 0u; r < ROWS; r++) {
        let cy = (g.y * ROWS + r) * 16u + l.y;
        if cx < params.sites.x && cy < params.sites.y {
            let y = top(cx, cy);
            let c = chroma_at(cx, cy);
            atomicMax(&local[c.y], y + 1u);
            atomicMax(&local[STRIDE + c.x], y + 1u);
            atomicMin(&local[2u * STRIDE + y], rank[(c.x << params.bits) | c.y]);
        }
    }
    workgroupBarrier();
    for (var k = i; k < 3u * STRIDE; k += 256u) {
        let v = atomicLoad(&local[k]);
        if k < 2u * STRIDE {
            if v != 0u {
                atomicMax(&peaks[k], v);
            }
        } else if v != NONE {
            atomicMin(&peaks[k], v);
        }
    }
}

@compute @workgroup_size(16, 16)
fn sample(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.grid.x || id.y >= params.grid.y {
        return;
    }
    let x = id.x * params.step;
    let y = id.y * params.step;
    let c = chroma_at(x / 2u, y / 2u);
    samples[id.y * params.grid.x + id.x] = luma_at(x, y) | (c.x << 10u) | (c.y << 20u);
}
