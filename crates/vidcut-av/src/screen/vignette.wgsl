struct Params {
    cells: vec2<u32>,
    width: u32,
    down: u32,
    radius: f32,
    stretch: f32,
    cos: f32,
    sin: f32,
    shape: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> fresh: array<f32>;

@group(0) @binding(1) var<storage, read> from_cells: array<f32>;
@group(0) @binding(2) var<storage, read_write> to_cells: array<f32>;

const SIN30: f32 = 0.5;
const COS30: f32 = 0.8660254037844386;
const SIN45: f32 = 0.7071067811865476;

fn inside(u: f32, w: f32) -> bool {
    let r = params.radius;
    let a = abs(u);
    let b = abs(w);
    switch params.shape {
        case 1u: {
            return max(-w, COS30 * a + SIN30 * w) <= r;
        }
        case 2u: {
            return SIN45 * (a + b) <= r;
        }
        case 3u: {
            return max(a, b) <= r;
        }
        case 4u: {
            return max(b, COS30 * a + SIN30 * b) <= r;
        }
        case 5u: {
            return max(max(a, b), SIN45 * (a + b)) <= r;
        }
        default: {
            return u * u + w * w <= r * r;
        }
    }
}

@compute @workgroup_size(16, 16)
fn mask(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = params.cells;
    if id.x >= n.x || id.y >= n.y {
        return;
    }
    let x = (2.0 * f32(id.x) - f32(n.x)) / params.stretch;
    let y = 2.0 * f32(id.y) - f32(n.y);
    let u = params.cos * x - params.sin * y;
    let w = params.sin * x + params.cos * y;
    fresh[id.y * n.x + id.x] = select(0.0, 1.0, inside(u, w));
}

fn at(line: u32, i: u32) -> u32 {
    if params.down == 1u {
        return i * params.cells.x + line;
    }
    return line * params.cells.x + i;
}

@compute @workgroup_size(16)
fn run(@builtin(global_invocation_id) id: vec3<u32>) {
    let down = params.down == 1u;
    let lines = select(params.cells.y, params.cells.x, down);
    let long = select(params.cells.x, params.cells.y, down);
    let line = id.x;
    if line >= lines {
        return;
    }
    let reach = params.width / 2u;
    let width = f32(params.width);
    var sum = 0.0;
    for (var i = 0u; i < min(reach + 1u, long); i++) {
        sum += from_cells[at(line, i)];
    }
    for (var i = 0u; i < long; i++) {
        to_cells[at(line, i)] = sum / width;
        if i + reach + 1u < long {
            sum += from_cells[at(line, i + reach + 1u)];
        }
        if i >= reach {
            sum -= from_cells[at(line, i - reach)];
        }
    }
}
