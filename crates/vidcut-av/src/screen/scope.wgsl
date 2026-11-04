struct Params {
    canvas: vec2<u32>,
    size: vec2<u32>,
    luma: vec4<f32>,
    chroma: vec4<f32>,
    range: vec4<f32>,
    rows: array<vec4<f32>, 2>,
    hues: array<vec4<f32>, 3>,
    how: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var canvas: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> hits: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read> counted: array<u32>;

const GRID: vec3<f32> = vec3<f32>(0.3, 0.3, 0.3);

fn row(v: f32) -> u32 {
    let up = (v - params.range.x) / params.range.y;
    let down = (1.0 - up) * f32(params.size.y);
    return min(u32(clamp(down, 0.0, f32(params.size.y))), params.size.y - 1u);
}

fn add(x: u32, y: u32) {
    atomicAdd(&hits[y * params.size.x + x], 1u);
}

fn outside(id: vec3<u32>) -> bool {
    return id.x >= params.canvas.x || id.y >= params.canvas.y;
}

fn color_at(id: vec3<u32>) -> vec3<f32> {
    return textureLoad(canvas, id.xy, 0).rgb;
}

@compute @workgroup_size(16, 16)
fn waveform(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let x = id.x * params.size.x / params.canvas.x;
    add(x, row(dot(color_at(id), params.luma.xyz)));
}

@compute @workgroup_size(16, 16)
fn parade(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let part = params.size.x / 3u;
    let x = id.x * part / params.canvas.x;
    let c = color_at(id);
    add(x, row(c.r));
    add(x + part, row(c.g));
    add(x + 2u * part, row(c.b));
}

@compute @workgroup_size(16, 16)
fn vectorscope(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let c = color_at(id);
    let y = dot(c, params.luma.xyz);
    let middle = vec2<f32>(params.size) * 0.5;
    let at = middle + vec2<f32>((c.b - y) * params.chroma.x, (y - c.r) * params.chroma.y) * params.range.w;
    let last = params.size - vec2<u32>(1u);
    let p = min(vec2<u32>(clamp(at, vec2<f32>(0.0), vec2<f32>(last))), last);
    add(p.x, p.y);
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn hit(x: i32, y: i32) -> f32 {
    if x < 0 || y < 0 || x >= i32(params.size.x) || y >= i32(params.size.y) {
        return 0.0;
    }
    return f32(counted[u32(y) * params.size.x + u32(x)]);
}

fn level(hits_here: f32) -> f32 {
    return sqrt(min(hits_here * params.range.z, 1.0));
}

fn evened(x: u32, across: u32) -> f32 {
    let first = (x * params.canvas.x + across - 1u) / across;
    let after = ((x + 1u) * params.canvas.x + across - 1u) / across;
    return f32(params.canvas.x) / f32(across) / max(f32(after - first), 1.0);
}

fn ruled(y: f32) -> bool {
    for (var i = 0u; i < params.how.y; i++) {
        if y == params.rows[i / 4u][i % 4u] {
            return true;
        }
    }
    return false;
}

fn on_graticule(p: vec2<f32>) -> bool {
    let middle = floor(vec2<f32>(params.size) * 0.5);
    let d = p - middle;
    let circle = params.chroma.z;
    let far = length(d);
    if abs(far - circle) < 0.75 {
        return true;
    }
    if far < circle && (d.x == 0.0 || d.y == 0.0) {
        return true;
    }
    let half_box = params.chroma.w;
    for (var i = 0u; i < 6u; i++) {
        let hue = params.hues[i / 2u];
        let at = select(hue.xy, hue.zw, i % 2u == 1u);
        let off = abs(p - at);
        let out = max(off.x, off.y);
        if out <= half_box && out > half_box - 1.0 {
            return true;
        }
    }
    return false;
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = floor(at.xy);
    let x = i32(p.x);
    let y = i32(p.y);
    if params.how.x == 2u {
        var most = hit(x, y);
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                most = max(most, hit(x + dx, y + dy) * 0.5);
            }
        }
        let grid = select(vec3<f32>(0.0), GRID, on_graticule(p));
        return vec4<f32>(max(grid, vec3<f32>(level(most))), 1.0);
    }
    var tint = vec3<f32>(1.0);
    var edge = false;
    var even = evened(u32(x), params.size.x);
    if params.how.x == 1u {
        let part = params.size.x / 3u;
        let section = u32(x) / part;
        even = evened(u32(x) % part, part);
        edge = u32(x) % part == 0u && section > 0u && section < 3u;
        if section == 0u {
            tint = vec3<f32>(1.0, 0.25, 0.25);
        } else if section == 1u {
            tint = vec3<f32>(0.25, 1.0, 0.25);
        } else if section == 2u {
            tint = vec3<f32>(0.35, 0.45, 1.0);
        } else {
            tint = vec3<f32>(0.0);
        }
    }
    let grid = select(vec3<f32>(0.0), GRID, edge || ruled(p.y));
    return vec4<f32>(max(grid, tint * level(hit(x, y) * even)), 1.0);
}
