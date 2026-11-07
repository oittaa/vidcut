struct Params {
    size: vec2<u32>,
    at: u32,
    parity: u32,
    weights: vec3<f32>,
    strength: f32,
    keep: f32,
    gain: f32,
    pad0: f32,
    pad1: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

@group(0) @binding(1) var taken: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> new_guide: array<f32>;
@group(0) @binding(3) var<storage, read_write> new_solved: array<f32>;

@group(0) @binding(1) var<storage, read> guide: array<f32>;
@group(0) @binding(2) var<storage, read_write> new_across: array<f32>;
@group(0) @binding(3) var<storage, read_write> new_down: array<f32>;

@group(0) @binding(2) var<storage, read> across: array<f32>;
@group(0) @binding(3) var<storage, read> down: array<f32>;
@group(0) @binding(4) var<storage, read_write> solved: array<f32>;

@group(0) @binding(2) var<storage, read_write> held: array<f32>;
@group(0) @binding(3) var<storage, read_write> made: array<f32>;

const POWER: f32 = 1.2;
const LEAST: f32 = 1e-4;
const EDGE: f32 = 1e4;
const OVER: f32 = 1.8;

fn outside(id: vec3<u32>) -> bool {
    return id.x >= params.size.x || id.y >= params.size.y;
}

@compute @workgroup_size(16, 16)
fn split(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let i = id.y * params.size.x + id.x;
    let value = dot(textureLoad(taken, id.xy, 0).rgb, params.weights);
    new_guide[i] = value;
    new_solved[i] = value;
}

fn link(a: f32, b: f32) -> f32 {
    return 1.0 / (pow(abs(a - b), POWER) + LEAST);
}

@compute @workgroup_size(16, 16)
fn linked(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let n = params.size;
    let i = id.y * n.x + id.x;
    if id.x + 1u < n.x {
        new_across[i] = link(guide[i], guide[i + 1u]);
    }
    if id.y + 1u < n.y {
        new_down[i] = link(guide[i], guide[i + n.x]);
    }
}

@compute @workgroup_size(16, 16)
fn sweep(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) || (id.x + id.y) % 2u != params.parity {
        return;
    }
    let n = params.size;
    let i = id.y * n.x + id.x;
    let here = solved[i];
    var total = 0.0;
    var pulled = 0.0;
    if id.x > 0u {
        total += across[i - 1u];
        pulled += across[i - 1u] * (solved[i - 1u] - here);
    } else {
        total += EDGE;
    }
    if id.x + 1u < n.x {
        total += across[i];
        pulled += across[i] * (solved[i + 1u] - here);
    } else {
        total += EDGE;
    }
    if id.y > 0u {
        total += down[i - n.x];
        pulled += down[i - n.x] * (solved[i - n.x] - here);
    } else {
        total += EDGE;
    }
    if id.y + 1u < n.y {
        total += down[i];
        pulled += down[i] * (solved[i + n.x] - here);
    } else {
        total += EDGE;
    }
    let change = (guide[i] - here + params.strength * pulled) / (1.0 + params.strength * total);
    solved[i] = here + OVER * change;
}

@compute @workgroup_size(16, 16)
fn collect(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let i = id.y * params.size.x + id.x;
    let before = select(0.0, made[params.at + i], params.keep > 0.5);
    made[params.at + i] = before + params.gain * (held[i] - guide[i]);
    held[i] = guide[i];
}
