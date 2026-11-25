struct Params {
    canvas: vec2<u32>,
    size: vec2<u32>,
    luma: vec4<f32>,
    chroma: vec4<f32>,
    range: vec4<f32>,
    levels: vec4<f32>,
    rows: array<vec4<f32>, 3>,
    hues: array<vec4<f32>, 3>,
    how: vec4<u32>,
    parts: vec4<u32>,
    skin: vec4<f32>,
    plot: vec4<f32>,
    to_xyz: array<vec4<f32>, 3>,
    from_xyz: array<vec4<f32>, 3>,
    chart: vec4<u32>,
    curve: array<vec4<f32>, 78>,
    grid: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var canvas: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> hits: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read> counted: array<u32>;

const DASH: u32 = 4u;

const WHITE: vec3<f32> = vec3<f32>(1.0, 1.0, 1.0);

const RED: vec3<f32> = vec3<f32>(1.0, 0.25, 0.25);

const GREEN: vec3<f32> = vec3<f32>(0.25, 1.0, 0.25);

const BLUE: vec3<f32> = vec3<f32>(0.35, 0.45, 1.0);

const CHART_LINE: f32 = 190.0 / 255.0;

const GAMUT_LINE: f32 = 76.0 / 255.0;

const WHITE_POINT: f32 = 229.0 / 255.0;

const WHITE_RADIUS: f32 = 0.0047;

const LEAST_RADIUS: f32 = 2.0;

const AXIS_STEP: f32 = 0.025;

const AXIS_TICK: f32 = 0.011;

fn shown(v: f32) -> f32 {
    return params.levels.x + params.levels.y * v;
}

fn shown_difference(v: f32) -> f32 {
    return params.levels.z + params.levels.w * v;
}

fn differences(c: vec3<f32>) -> vec2<f32> {
    let y = dot(c, params.luma.xyz);
    return vec2<f32>((c.b - y) * params.chroma.x, (c.r - y) * params.chroma.y);
}

fn row(v: f32) -> u32 {
    let up = (v - params.range.x) / params.range.y;
    let down = (1.0 - up) * f32(params.size.y);
    return min(u32(clamp(down, 0.0, f32(params.size.y))), params.size.y - 1u);
}

fn add_in(plane: u32, x: u32, y: u32) {
    atomicAdd(&hits[(plane * params.size.y + y) * params.size.x + x], 1u);
}

fn add(x: u32, y: u32) {
    add_in(0u, x, y);
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
    let c = color_at(id);
    if params.how.w == 0u {
        add(x, row(shown(dot(c, params.luma.xyz))));
    } else if params.how.w == 1u {
        let d = differences(c);
        add(x, row(shown_difference(d.x)));
        add(x, row(shown_difference(d.y)));
    } else {
        for (var i = 0u; i < 3u; i++) {
            if (params.parts.y & (1u << i)) != 0u {
                add_in(i, x, row(shown(c[i])));
            }
        }
    }
}

fn parade_value(c: vec3<f32>, section: u32) -> f32 {
    if params.how.w == 0u {
        return shown(c[section]);
    }
    if section == 0u {
        return shown(dot(c, params.luma.xyz));
    }
    if params.how.w == 1u {
        return shown(c[section - 1u]);
    }
    return shown_difference(differences(c)[section - 1u]);
}

@compute @workgroup_size(16, 16)
fn parade(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let part = params.size.x / params.parts.x;
    let x = id.x * part / params.canvas.x;
    let c = color_at(id);
    for (var s = 0u; s < params.parts.x; s++) {
        add(x + s * part, row(parade_value(c, s)));
    }
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

fn bin(v: f32) -> u32 {
    return min(u32(clamp(v, 0.0, 1.0) * f32(params.size.x)), params.size.x - 1u);
}

fn band_value(c: vec3<f32>, band: u32) -> f32 {
    if params.how.w == 0u {
        return shown(c[band]);
    }
    if band == 0u {
        return shown(dot(c, params.luma.xyz));
    }
    return shown(c[band - 1u]);
}

@compute @workgroup_size(16, 16)
fn histogram(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let c = color_at(id);
    for (var b = 0u; b < params.parts.x; b++) {
        add(bin(band_value(c, b)), b);
    }
}

fn linear(c: vec3<f32>) -> vec3<f32> {
    if params.chart.x == 0u {
        return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.4));
    }
    return decode(c, params.chart.x, 1.0);
}

fn uv_of(xy: vec2<f32>) -> vec2<f32> {
    let d = -2.0 * xy.x + 12.0 * xy.y + 3.0;
    return vec2<f32>(4.0 * xy.x, 9.0 * xy.y) / d;
}

fn xy_of(uv: vec2<f32>) -> vec2<f32> {
    let d = 6.0 * uv.x - 16.0 * uv.y + 12.0;
    return vec2<f32>(9.0 * uv.x, 4.0 * uv.y) / d;
}

fn chart_place(xy: vec2<f32>) -> vec2<f32> {
    let c = select(xy, uv_of(xy), params.how.w == 1u);
    let r = params.plot;
    return vec2<f32>(r.x + c.x * (r.z - r.x), r.w - c.y * (r.w - r.y));
}

@compute @workgroup_size(16, 16)
fn chromaticity(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside(id) {
        return;
    }
    let l = linear(color_at(id));
    let xyz = vec3<f32>(dot(params.to_xyz[0].xyz, l), dot(params.to_xyz[1].xyz, l), dot(params.to_xyz[2].xyz, l));
    let sum = xyz.x + xyz.y + xyz.z;
    if sum <= 1e-6 {
        return;
    }
    let at = chart_place(xyz.xy / sum);
    if at.x < 0.0 || at.y < 0.0 || at.x >= f32(params.size.x) || at.y >= f32(params.size.y) {
        return;
    }
    add(u32(at.x), u32(at.y));
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn hit_in(plane: u32, x: i32, y: i32) -> f32 {
    if x < 0 || y < 0 || x >= i32(params.size.x) || y >= i32(params.size.y) {
        return 0.0;
    }
    return f32(counted[(plane * params.size.y + u32(y)) * params.size.x + u32(x)]);
}

fn hit(x: i32, y: i32) -> f32 {
    return hit_in(0u, x, y);
}

fn channel_tint(i: u32) -> vec3<f32> {
    if i == 0u {
        return RED;
    }
    if i == 1u {
        return GREEN;
    }
    return BLUE;
}

fn parade_tint(section: u32) -> vec3<f32> {
    if params.how.w == 0u {
        return channel_tint(section);
    }
    if params.how.w == 1u && section > 0u {
        return channel_tint(section - 1u);
    }
    return WHITE;
}

fn level(hits_here: f32) -> f32 {
    return sqrt(min(hits_here * params.range.z, 1.0));
}

fn evened(x: u32, across: u32) -> f32 {
    let first = (x * params.canvas.x + across - 1u) / across;
    let after = ((x + 1u) * params.canvas.x + across - 1u) / across;
    return f32(params.canvas.x) / f32(across) / max(f32(after - first), 1.0);
}

fn ruled(p: vec2<f32>) -> bool {
    let across = params.how.x == 3u;
    let at = select(p.y, p.x, across);
    let along = select(p.x, p.y, across);
    for (var i = 0u; i < params.how.y; i++) {
        if at == params.rows[i / 4u][i % 4u] {
            return i < params.how.z || (u32(along) / DASH) % 2u == 0u;
        }
    }
    return false;
}

fn band_tint(band: u32) -> vec3<f32> {
    if params.how.w == 0u {
        return channel_tint(band);
    }
    if band == 0u {
        return WHITE;
    }
    return channel_tint(band - 1u);
}

fn bar(band: u32, x: i32, room: f32) -> f32 {
    if x < 0 || x >= i32(params.size.x) {
        return 0.0;
    }
    let count = f32(counted[band * params.size.x + u32(x)]);
    return min(count * params.range.z, 1.0) * room;
}

fn histogram_color(x: i32, y: i32) -> vec3<f32> {
    let bands = params.parts.x;
    let tall = params.size.y / bands;
    let band = u32(y) / tall;
    if band >= bands {
        return vec3<f32>(0.0);
    }
    let up = f32(tall - 1u - u32(y) % tall);
    let room = f32(tall) - 2.0;
    let here = bar(band, x, room);
    let left = bar(band, x - 1, room);
    let top = up >= max(here - 1.0, 0.0) && up < max(here, 1.0);
    let side = up >= min(here, left) && up < max(here, left);
    let tint = band_tint(band);
    if top || side {
        return tint;
    }
    if up < here {
        return tint * 0.35;
    }
    return vec3<f32>(0.0);
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
    if params.parts.w == 1u {
        let along = dot(d, params.skin.xy);
        let off = abs(d.x * params.skin.y - d.y * params.skin.x);
        if along > 0.0 && along < circle && off < 0.75 {
            return true;
        }
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

fn spread(x: i32, y: i32) -> f32 {
    var most = hit(x, y);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            most = max(most, hit(x + dx, y + dy) * 0.5);
        }
    }
    return most;
}

fn chart_color(p: vec2<f32>) -> vec3<f32> {
    let r = params.plot;
    let c = vec2<f32>((p.x + 0.5 - r.x) / (r.z - r.x), (r.w - p.y - 0.5) / (r.w - r.y));
    let xy = select(c, xy_of(c), params.how.w == 1u);
    let y = max(xy.y, 1e-4);
    let xyz = vec3<f32>(xy.x / y, 1.0, (1.0 - xy.x - xy.y) / y);
    let rgb = max(vec3<f32>(dot(params.from_xyz[0].xyz, xyz), dot(params.from_xyz[1].xyz, xyz), dot(params.from_xyz[2].xyz, xyz)), vec3<f32>(0.0));
    let peak = max(max(rgb.r, rgb.g), max(rgb.b, 1e-6));
    return pow(rgb / peak, vec3<f32>(1.0 / 2.2));
}

fn point(i: u32) -> vec2<f32> {
    let v = params.curve[i / 2u];
    return select(v.xy, v.zw, i % 2u == 1u);
}

fn near(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> bool {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - (a + t * ab)) < 0.75;
}

fn chart_line(p: vec2<f32>) -> f32 {
    let locus = params.chart.y;
    let planck = params.chart.z;
    let gamut = locus + planck;
    let side = params.plot.z - params.plot.x;
    let white = length(p - point(gamut + 3u));
    if abs(white - max(side * WHITE_RADIUS, LEAST_RADIUS)) < 0.75 {
        return WHITE_POINT;
    }
    for (var i = 0u; i < locus; i++) {
        if near(p, point(i), point((i + 1u) % locus)) {
            return CHART_LINE;
        }
    }
    let ticks = gamut + 4u;
    for (var i = 0u; i < params.chart.w; i++) {
        if near(p, point(ticks + 2u * i), point(ticks + 2u * i + 1u)) {
            return CHART_LINE;
        }
    }
    for (var i = 1u; i < planck; i++) {
        if near(p, point(locus + i - 1u), point(locus + i)) {
            return CHART_LINE;
        }
    }
    for (var i = 0u; i < 3u; i++) {
        if near(p, point(gamut + i), point(gamut + (i + 1u) % 3u)) {
            return GAMUT_LINE;
        }
    }
    return 0.0;
}

fn on_chart_grid(p: vec2<f32>) -> bool {
    let r = params.plot;
    if p.x < floor(r.x) || p.x > r.z || p.y < floor(r.y) || p.y > r.w {
        return false;
    }
    let side = r.z - r.x;
    let g = vec2<f32>(p.x - r.x, r.w - p.y);
    let tenth = round(g * 10.0 / side);
    let line = abs(g - tenth * side / 10.0);
    if (line.x < 0.5 && tenth.x > 0.0) || (line.y < 0.5 && tenth.y > 0.0) {
        return true;
    }
    let step = side * AXIS_STEP;
    let tick = abs(g - round(g / step) * step);
    let reach = side * AXIS_TICK;
    return (tick.x < 0.5 && g.y < reach) || (tick.y < 0.5 && g.x < reach);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = floor(at.xy);
    let x = i32(p.x);
    let y = i32(p.y);
    if params.how.x == 2u {
        let grid = select(vec3<f32>(0.0), params.grid.rgb, on_graticule(p));
        return vec4<f32>(max(grid, vec3<f32>(level(spread(x, y)))), 1.0);
    }
    if params.how.x == 4u {
        let grid = select(vec3<f32>(0.0), params.grid.rgb, on_chart_grid(p));
        let lines = max(grid, vec3<f32>(chart_line(p)));
        return vec4<f32>(max(lines, chart_color(p) * level(spread(x, y))), 1.0);
    }
    if params.how.x == 3u {
        let grid = select(vec3<f32>(0.0), params.grid.rgb, ruled(p));
        return vec4<f32>(max(grid, histogram_color(x, y)), 1.0);
    }
    var color = vec3<f32>(0.0);
    var edge = false;
    if params.how.x == 1u {
        let sections = params.parts.x;
        let part = params.size.x / sections;
        let section = u32(x) / part;
        edge = u32(x) % part == 0u && section > 0u && section < sections;
        if section < sections {
            let even = evened(u32(x) % part, part);
            color = parade_tint(section) * level(hit(x, y) * even);
        }
    } else {
        let even = evened(u32(x), params.size.x);
        if params.parts.z == 3u {
            for (var i = 0u; i < 3u; i++) {
                color += channel_tint(i) * level(hit_in(i, x, y) * even);
            }
        } else {
            color = WHITE * level(hit(x, y) * even);
        }
    }
    let grid = select(vec3<f32>(0.0), params.grid.rgb, edge || ruled(p));
    return vec4<f32>(max(grid, min(color, WHITE)), 1.0);
}
