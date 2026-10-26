use crate::compute::Gpu;

pub(super) const TILE: u32 = 128;
const DISPATCH: u64 = 150_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    None,
    Gelu,
    Relu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Load {
    Rows,
    Patch,
    Causal,
    Plane { two: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Store {
    Half(Act),
    Full,
    Gated,
    Cond,
    Skip,
    Head,
}

pub(super) struct Gemm<'a> {
    pub(super) m: u32,
    pub(super) n: u32,
    pub(super) k: u32,
    pub(super) load: Load,
    pub(super) store: Store,
    pub(super) params: [u32; 16],
    pub(super) weights: &'a wgpu::Buffer,
    pub(super) bias: &'a wgpu::Buffer,
    pub(super) a: &'a wgpu::Buffer,
    pub(super) a2: Option<&'a wgpu::Buffer>,
    pub(super) out: &'a wgpu::Buffer,
    pub(super) aux: Option<&'a wgpu::Buffer>,
}

fn act(a: Act) -> &'static str {
    match a {
        Act::None => "v",
        Act::Gelu => {
            "0.5 * v * (1.0 + tanh(clamp(0.7978845608028654 * (v + 0.044715 * v * v * v), vec4<f32>(-15.0), vec4<f32>(15.0))))"
        }
        Act::Relu => "max(v, vec4<f32>(0.0))",
    }
}

const HEADER: &str = r#"
struct Shape { m: u32, n: u32, k: u32, tn: u32, p: array<vec4<u32>, 4> }
@group(0) @binding(0) var<uniform> shape: Shape;
@group(0) @binding(1) var<storage, read> wts: array<u32>;
@group(0) @binding(2) var<storage, read> bias: array<f32>;

var<workgroup> sb: array<vec4<f32>, 512>;

fn par(i: u32) -> u32 {
    return shape.p[i / 4u][i % 4u];
}

fn b_row(n: u32) -> vec2<u32> {
    return vec2<u32>(select(0u, 1u, n < shape.n), n * (shape.k / 2u));
}

fn b_word(r: vec2<u32>, k2: u32) -> u32 {
    if (r.x == 0u) {
        return 0u;
    }
    return wts[r.y + k2];
}

fn unpack_pairs(w: vec4<u32>) -> array<vec4<f32>, 2> {
    let x = unpack2x16float(w.x);
    let y = unpack2x16float(w.y);
    let z = unpack2x16float(w.z);
    let v = unpack2x16float(w.w);
    return array<vec4<f32>, 2>(vec4<f32>(x.x, y.x, z.x, v.x), vec4<f32>(x.y, y.y, z.y, v.y));
}

fn bias4(n: u32) -> vec4<f32> {
    var b = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        if (n + i < shape.n) {
            b[i] = bias[n + i];
        }
    }
    return b;
}
"#;

fn bindings(load: Load, store: Store) -> String {
    let mut s = String::new();
    let mut at = 3;
    let mut bind = |s: &mut String, decl: &str| {
        *s += &format!("@group(0) @binding({at}) {decl};\n");
        at += 1;
    };
    match load {
        Load::Patch => bind(&mut s, "var<storage, read> src: array<f32>"),
        Load::Plane { two: true } => {
            bind(&mut s, "var<storage, read> src: array<u32>");
            bind(&mut s, "var<storage, read> src2: array<u32>");
        }
        _ => bind(&mut s, "var<storage, read> src: array<u32>"),
    }
    match store {
        Store::Half(_) => bind(&mut s, "var<storage, read_write> dst: array<u32>"),
        Store::Full | Store::Gated | Store::Cond => {
            bind(&mut s, "var<storage, read_write> dst: array<f32>")
        }
        Store::Skip => {
            bind(&mut s, "var<storage, read_write> dst: array<u32>");
            bind(&mut s, "var<storage, read> aux: array<u32>");
        }
        Store::Head => {
            bind(&mut s, "var<storage, read_write> dst: array<f32>");
            bind(&mut s, "var<storage, read> aux: array<f32>");
        }
    }
    if store == Store::Gated {
        bind(&mut s, "var<storage, read> aux: array<f32>");
    }
    s
}

fn loader(load: Load) -> &'static str {
    match load {
        Load::Rows => {
            r#"
fn a_row(m: u32) -> vec4<u32> {
    return vec4<u32>(select(0u, 1u, m < shape.m), (par(1u) + m) * par(0u), 0u, 0u);
}

fn a_word(r: vec4<u32>, k2: u32, tap: u32, ci: u32) -> u32 {
    if (r.x == 0u) {
        return 0u;
    }
    return src[r.y + k2];
}
"#
        }
        Load::Patch => {
            r#"
fn a_row(m: u32) -> vec4<u32> {
    if (m >= shape.m) {
        return vec4<u32>(0u);
    }
    let w8 = par(0u);
    let h8 = par(1u);
    let nh = par(2u);
    let nw = par(3u);
    let b = m / 128u;
    let l = m % 128u;
    let s = nh * nw;
    let f = (b / s) * 2u + l / 64u;
    let sb = b % s;
    let ht = (sb / nw) * 8u + (l / 8u) % 8u;
    let wt = (sb % nw) * 8u + l % 8u;
    return vec4<u32>(1u, f * (h8 * w8) + ht * 2u * w8 + wt * 2u, 0u, 0u);
}

fn a_get(r: vec4<u32>, k2: u32, tap: u32, ci: u32) -> vec2<f32> {
    if (r.x == 0u) {
        return vec2<f32>(0.0);
    }
    let w8 = par(0u);
    let plane = par(1u) * w8;
    let k = k2 * 2u;
    let t = k / 16u;
    let c = k % 16u;
    let i = c * 4u * plane + r.y + (t / 2u) * w8 + t % 2u;
    return vec2<f32>(src[i], src[i + 4u * plane]);
}
"#
        }
        Load::Causal => {
            r#"
fn a_row(m: u32) -> vec4<u32> {
    if (m >= shape.m) {
        return vec4<u32>(0u);
    }
    let pp = par(0u);
    let wt = par(1u);
    let p = m % pp;
    return vec4<u32>(1u, m / pp, p / wt, p % wt);
}

fn a_word(r: vec4<u32>, k2: u32, tap: u32, ci: u32) -> u32 {
    if (r.x == 0u) {
        return 0u;
    }
    let pp = par(0u);
    let wt = par(1u);
    let ht = par(2u);
    let cin = par(3u);
    let kt = tap / 9u;
    let ky = (tap / 3u) % 3u;
    let kx = tap % 3u;
    let y = u32(clamp(i32(r.z) + i32(ky) - 1, 0, i32(ht) - 1));
    let x = u32(clamp(i32(r.w) + i32(kx) - 1, 0, i32(wt) - 1));
    let frame = par(4u + r.y * 2u + kt);
    return src[((frame * pp + y * wt + x) * cin + ci) / 2u];
}
"#
        }
        Load::Plane { two } => {
            if two {
                PLANE_TWO
            } else {
                PLANE_ONE
            }
        }
    }
}

const PLANE_HEAD: &str = r#"
fn a_row(m: u32) -> vec4<u32> {
    if (m >= shape.m) {
        return vec4<u32>(0u);
    }
    let w = par(0u);
    return vec4<u32>(1u, par(2u) + m / w, m % w, 0u);
}

fn plane_at(r: vec4<u32>, tap: u32) -> vec2<u32> {
    let w = par(0u);
    let h = par(1u);
    let yy = i32(r.y) + i32(tap / 3u) - 1;
    let xx = i32(r.z) + i32(tap % 3u) - 1;
    if (yy < 0 || xx < 0 || yy >= i32(h) || xx >= i32(w)) {
        return vec2<u32>(0u, 0u);
    }
    var sy = u32(yy);
    var sx = u32(xx);
    var sw = w;
    if (par(7u) == 1u) {
        sy = sy / 2u;
        sx = sx / 2u;
        sw = w / 2u;
    }
    return vec2<u32>(1u, (sy - par(8u)) * sw + sx);
}
"#;

const PLANE_ONE: &str = r#"
fn a_word(r: vec4<u32>, k2: u32, tap: u32, ci: u32) -> u32 {
    if (r.x == 0u) {
        return 0u;
    }
    let at = plane_at(r, tap);
    if (at.x == 0u) {
        return 0u;
    }
    return src[at.y * par(4u) + par(5u) + ci / 2u];
}
"#;

const PLANE_TWO: &str = r#"
fn a_word(r: vec4<u32>, k2: u32, tap: u32, ci: u32) -> u32 {
    if (r.x == 0u) {
        return 0u;
    }
    let at = plane_at(r, tap);
    if (at.x == 0u) {
        return 0u;
    }
    if (ci < par(3u)) {
        return src[at.y * par(4u) + par(5u) + ci / 2u];
    }
    return src2[at.y * par(6u) + (ci - par(3u)) / 2u];
}
"#;

fn storer(store: Store) -> String {
    let body = match store {
        Store::Half(a) => format!(
            r#"
    let v = val + bias4(n);
    let w = {};
    let o = (row * shape.n + n) / 2u;
    dst[o] = pack2x16float(w.xy);
    dst[o + 1u] = pack2x16float(w.zw);
"#,
            act(a)
        ),
        Store::Skip => r#"
    let o = (row * shape.n + n) / 2u;
    let s = vec4<f32>(unpack2x16float(aux[o]), unpack2x16float(aux[o + 1u]));
    let w = max(val + bias4(n) + s, vec4<f32>(0.0));
    dst[o] = pack2x16float(w.xy);
    dst[o + 1u] = pack2x16float(w.zw);
"#
        .into(),
        Store::Full => r#"
    let v = val + bias4(n);
    for (var i = 0u; i < 4u; i++) {
        if (n + i < shape.n) {
            dst[row * shape.n + n + i] = v[i];
        }
    }
"#
        .into(),
        Store::Gated => r#"
    let v = val + bias4(n);
    let o = row * shape.n + n;
    for (var i = 0u; i < 4u; i++) {
        dst[o + i] += aux[n + i] * v[i];
    }
"#
        .into(),
        Store::Cond => r#"
    let v = val + bias4(n);
    let wt = par(12u);
    let nh = par(13u);
    let nw = par(14u);
    let f = par(15u);
    let y = m / wt;
    let x = m % wt;
    let token = (((f / 2u) * nh + y / 8u) * nw + x / 8u) * 128u + (f % 2u) * 64u + (y % 8u) * 8u + x % 8u;
    let o = token * shape.n + n;
    for (var i = 0u; i < 4u; i++) {
        dst[o + i] += v[i];
    }
"#
        .into(),
        Store::Head => r#"
    let v = val + bias4(n);
    let w8 = par(12u);
    let h8 = par(13u);
    let nh = par(14u);
    let nw = par(15u);
    let b = m / 128u;
    let l = m % 128u;
    let s = nh * nw;
    let f = (b / s) * 2u + l / 64u;
    let sb = b % s;
    let ht = (sb / nw) * 8u + (l / 8u) % 8u;
    let wt = (sb % nw) * 8u + l % 8u;
    for (var i = 0u; i < 4u; i++) {
        let c = (n + i) % 16u;
        let pw = ((n + i) / 16u) % 2u;
        let ph = (n + i) / 32u;
        let o = ((c * 4u + f) * h8 + ht * 2u + ph) * w8 + wt * 2u + pw;
        dst[o] = aux[o] - v[i];
    }
"#
        .into(),
    };
    let offset = if matches!(store, Store::Gated | Store::Full) {
        "par(11u) + m"
    } else {
        "m"
    };
    format!(
        r#"
fn emit(m: u32, n: u32, val: vec4<f32>) {{
    if (m >= shape.m || n >= shape.n) {{
        return;
    }}
    let row = {offset};
{body}
}}
"#
    )
}

fn fmas() -> String {
    let mut s = String::new();
    for i in 0..8 {
        let (vec, comp) = if i < 4 { ("a0", i) } else { ("a1", i - 4) };
        let c = ["x", "y", "z", "w"][comp];
        s += &format!(
            "        c{i}0 = fma(vec4<f32>({vec}.{c}), b0, c{i}0);\n        c{i}1 = fma(vec4<f32>({vec}.{c}), b1, c{i}1);\n"
        );
    }
    s
}

fn main_source(load: Load) -> String {
    let full = matches!(load, Load::Patch);
    let mut decl = String::new();
    for i in 0..8 {
        decl += &format!("    var c{i}0 = vec4<f32>(0.0);\n    var c{i}1 = vec4<f32>(0.0);\n");
    }
    let mut rows = String::new();
    let mut fill = String::new();
    for j in 0..4 {
        rows += &format!(
            "    let ar{j} = a_row(m0 + lr + {r}u);\n    let br{j} = b_row(n0 + lr + {r}u);\n",
            r = 32 * j
        );
        let a = if full {
            format!(
                r#"            let a = a_get(ar{j}, k0 / 2u + pair, tap, ci);
            sa[(pair * 2u) * 32u + r / 4u][r % 4u] = a.x;
            sa[(pair * 2u + 1u) * 32u + r / 4u][r % 4u] = a.y;
"#
            )
        } else {
            format!(
                r#"            let a = unpack2x16float(a_word(ar{j}, k0 / 2u + pair, tap, ci));
            sa[(pair * 2u) * 32u + r / 4u][r % 4u] = a.x;
            sa[(pair * 2u + 1u) * 32u + r / 4u][r % 4u] = a.y;
"#
            )
        };
        fill += &format!(
            r#"
        {{
            let r = lr + {r}u;
{a}            let b = unpack2x16float(b_word(br{j}, k0 / 2u + pair));
            sb[(pair * 2u) * 32u + r / 4u][r % 4u] = b.x;
            sb[(pair * 2u + 1u) * 32u + r / 4u][r % 4u] = b.y;
        }}
"#,
            r = 32 * j
        );
    }
    let step = match load {
        Load::Plane { .. } => {
            "        ci += 16u;\n        if (ci >= par(9u)) {\n            ci -= par(9u);\n            tap += 1u;\n        }\n"
        }
        Load::Causal => {
            "        ci += 16u;\n        if (ci >= par(3u)) {\n            ci -= par(3u);\n            tap += 1u;\n        }\n"
        }
        Load::Rows | Load::Patch => "",
    };
    let mut inner = String::new();
    for kk in 0..16 {
        inner += &format!(
            "        {{\n        let a0 = sa[{a0}u + ty];\n        let a1 = sa[{a1}u + ty];\n        let b0 = sb[{a0}u + tx];\n        let b1 = sb[{a1}u + tx];\n",
            a0 = kk * 32,
            a1 = kk * 32 + 16
        );
        inner += &fmas();
        inner += "        }\n";
    }
    let shared = "var<workgroup> sa: array<vec4<f32>, 512>;";
    let mut out = String::new();
    for i in 0..8 {
        let row = if i < 4 {
            format!("ty * 4u + {i}u")
        } else {
            format!("64u + ty * 4u + {}u", i - 4)
        };
        out += &format!(
            "    emit(m0 + {row}, n0 + tx * 4u, c{i}0);\n    emit(m0 + {row}, n0 + 64u + tx * 4u, c{i}1);\n"
        );
    }
    format!(
        r#"
{shared}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let idx = par(10u) + wg.x + wg.y * nwg.x;
    if (idx >= ((shape.m + 127u) / 128u) * shape.tn) {{
        return;
    }}
    let m0 = (idx / shape.tn) * 128u;
    let n0 = (idx % shape.tn) * 128u;
    let tx = li % 16u;
    let ty = li / 16u;
    let pair = li % 8u;
    let lr = li / 8u;
{decl}
{rows}
    var tap = 0u;
    var ci = pair * 2u;
    for (var k0 = 0u; k0 < shape.k; k0 += 16u) {{
{fill}
{step}
        workgroupBarrier();
{inner}
        workgroupBarrier();
    }}
{out}
}}
"#
    )
}

fn source(load: Load, store: Store) -> String {
    let mut s = String::from(HEADER);
    s += &bindings(load, store);
    if matches!(load, Load::Plane { .. }) {
        s += PLANE_HEAD;
    }
    s += loader(load);
    s += &storer(store);
    s += &main_source(load);
    s
}

pub(super) fn run(gpu: &Gpu, pass: &mut wgpu::ComputePass, g: &Gemm) {
    assert!(
        g.k.is_multiple_of(16),
        "a matrix multiply needs K in steps of 16"
    );
    let key = format!("gemm {:?} {:?}", g.load, g.store);
    let pipe = gpu.pipe(&key, || source(g.load, g.store));
    let tn = g.n.div_ceil(TILE);
    let tiles = g.m.div_ceil(TILE) * tn;
    let per_tile = u64::from(TILE * TILE) * u64::from(g.k);
    let most = (DISPATCH / per_tile).clamp(1, u64::from(tiles.max(1))) as u32;
    for first in (0..tiles).step_by(most as usize) {
        let mut params = g.params;
        params[10] = first;
        let mut values = vec![g.m, g.n, g.k, tn];
        values.extend_from_slice(&params);
        let shape = gpu.uniform(&values);
        let mut buffers: Vec<&wgpu::Buffer> = vec![g.weights, g.bias, g.a];
        if let Some(a2) = g.a2 {
            buffers.push(a2);
        }
        buffers.push(g.out);
        if let Some(aux) = g.aux {
            buffers.push(aux);
        }
        gpu.dispatch(pass, &pipe, shape, &buffers, most.min(tiles - first));
    }
}
