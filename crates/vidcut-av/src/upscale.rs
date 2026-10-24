use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError, mpsc};

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::VideoPiece;
use wgpu::util::DeviceExt;

use crate::compute::{self, bytes, group, words};
use crate::error::{Error, Result};
use crate::models::{self, Fetch, File, Need};

const RELEASE: &str = "https://github.com/filippawlicki/nanovsr/releases/download/v1.0";

pub const WEIGHTS: File = File {
    name: "nanovsr/nanovsr_5.4m.pth",
    title: "NanoVSR 5.4M upscaler",
    repo: "filippawlicki/nanovsr",
    revision: "v1.0",
    bytes: 24_624_333,
    sha256: "42a53e1dc9b928947ea9cea85d7dd6f7bd82b325e92374dc1946d2cbcadf31bc",
    gpu_gb: 0,
    languages: &[],
};

pub const SCALE: u32 = 4;
pub const FRAMES: usize = 15;
const FEATURES: usize = 96;
const BLOCKS: usize = 30;
const PADDED: usize = 8;
const EPS: f32 = 1e-5;
const LEAK: f32 = 0.1;
const TILE: usize = 256;
const MARGIN: usize = 4;
const BLOCK: usize = 16;
const OUT_BLOCK: usize = 64;
const SHARED: u32 = ((8 * 18 * 18 + 8 * 9 * OUT_BLOCK) * 4) as u32;

fn fetch_of(f: &File) -> Fetch {
    let file = f.name.rsplit('/').next().unwrap_or(f.name);
    Fetch {
        url: format!("{RELEASE}/{file}"),
        name: f.name.to_string(),
        title: f.title.to_string(),
        bytes: f.bytes,
        sha256: f.sha256,
    }
}

pub fn needed(dir: &Path) -> Vec<Need> {
    [WEIGHTS]
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: f.bytes,
        })
        .collect()
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    if models::present(dir, &WEIGHTS) {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} isn't on this computer yet: pass download (--download) to download it into {}, \
             checked against its SHA-256",
            models::listed(&needed(dir)),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&fetch_of(&WEIGHTS), dir)?;
    Ok(())
}

pub fn unfit(m: &Media) -> Option<&'static str> {
    m.video_as_read()?.hdr.map(|_| "HDR pictures")
}

pub fn check<'a>(
    p: &Project,
    pieces: impl IntoIterator<Item = &'a VideoPiece>,
    dir: &Path,
    download: bool,
) -> Result<()> {
    let mut media: Vec<&str> = Vec::new();
    for piece in pieces {
        for (layer, _) in piece.layers() {
            if let VideoPiece::Source {
                media: id,
                super_scale,
                ..
            } = layer
                && super_scale.ai()
                && super_scale.upscaler == Upscaler::AiFast
                && !media.contains(&id.as_str())
            {
                media.push(id);
            }
        }
    }
    if media.is_empty() {
        return Ok(());
    }
    let refused: Vec<String> = media
        .iter()
        .filter_map(|id| p.media(id))
        .filter_map(|m| unfit(m).map(|why| format!("{} ({why})", m.name)))
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "the AI Fast upscaler of Super Scale can't take {}: set their Upscaler to Classic, \
             or their Super Scale to Off",
            refused.join(", ")
        )));
    }
    get_ready(dir, download)?;
    with_engine(|_| Ok(()))
}

struct Conv {
    cin: usize,
    cout: usize,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

struct Net {
    extract: Conv,
    forward: Vec<Conv>,
    backward: Vec<Conv>,
    fusion: Vec<f32>,
    fusion_bias: Vec<f32>,
    up1: Conv,
    up1_alpha: f32,
    up2: Conv,
    up2_alpha: f32,
    last: Conv,
}

struct Tensors {
    path: std::path::PathBuf,
    it: std::vec::IntoIter<Vec<u8>>,
}

impl Tensors {
    fn open(path: &Path) -> Result<Self> {
        let bad = |e: String| Error::Invalid(format!("{}: {e}", path.display()));
        let file = std::fs::File::open(path)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(e.to_string()))?;
        let prefix = zip
            .file_names()
            .filter_map(|n| n.ok())
            .find_map(|n| n.strip_suffix("data.pkl").map(str::to_string))
            .ok_or_else(|| bad("no data.pkl".into()))?;
        let mut all = Vec::new();
        while let Ok(mut entry) = zip.by_name(&format!("{prefix}data/{}", all.len())) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            all.push(bytes);
        }
        Ok(Self {
            path: path.to_path_buf(),
            it: all.into_iter(),
        })
    }

    fn bad(&self) -> Error {
        Error::Invalid(format!(
            "{}: not the weights of NanoVSR 5.4M",
            self.path.display()
        ))
    }

    fn floats(&mut self, n: usize) -> Result<Vec<f32>> {
        match self.it.next() {
            Some(b) if b.len() == n * 4 => Ok(b
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect()),
            _ => Err(self.bad()),
        }
    }

    fn count(&mut self) -> Result<()> {
        match self.it.next() {
            Some(b) if b.len() == 8 => Ok(()),
            _ => Err(self.bad()),
        }
    }

    fn norm(&mut self, c: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let gamma = self.floats(c)?;
        let beta = self.floats(c)?;
        let mean = self.floats(c)?;
        let var = self.floats(c)?;
        self.count()?;
        let scale: Vec<f32> = gamma
            .iter()
            .zip(&var)
            .map(|(g, v)| g / (v + EPS).sqrt())
            .collect();
        let shift = beta
            .iter()
            .zip(&mean)
            .zip(&scale)
            .map(|((b, m), s)| b - m * s)
            .collect();
        Ok((scale, shift))
    }

    fn block(&mut self, cin: usize, cout: usize) -> Result<Conv> {
        let identity = (cin == cout).then(|| self.norm(cin)).transpose()?;
        let dense = self.floats(cout * cin * 9)?;
        let (s3, b3) = self.norm(cout)?;
        let one = self.floats(cout * cin)?;
        let (s1, b1) = self.norm(cout)?;
        let mut weights = vec![0f32; cout * cin * 9];
        let mut bias = vec![0f32; cout];
        for co in 0..cout {
            for ci in 0..cin {
                for k in 0..9 {
                    weights[(co * cin + ci) * 9 + k] = dense[(co * cin + ci) * 9 + k] * s3[co];
                }
                weights[(co * cin + ci) * 9 + 4] += one[co * cin + ci] * s1[co];
            }
            bias[co] = b3[co] + b1[co];
            if let Some((si, bi)) = &identity {
                weights[(co * cin + co) * 9 + 4] += si[co];
                bias[co] += bi[co];
            }
        }
        Ok(Conv {
            cin,
            cout,
            weights,
            bias,
        })
    }

    fn conv(&mut self, cin: usize, cout: usize, size: usize) -> Result<Conv> {
        let weights = self.floats(cout * cin * size * size)?;
        let bias = self.floats(cout)?;
        Ok(Conv {
            cin,
            cout,
            weights,
            bias,
        })
    }

    fn alpha(&mut self) -> Result<f32> {
        Ok(self.floats(1)?[0])
    }
}

fn read(path: &Path) -> Result<Net> {
    let mut t = Tensors::open(path)?;
    let extract = t.block(3, FEATURES)?;
    let forward = (0..BLOCKS)
        .map(|_| t.block(FEATURES, FEATURES))
        .collect::<Result<Vec<_>>>()?;
    let backward = (0..BLOCKS)
        .map(|_| t.block(FEATURES, FEATURES))
        .collect::<Result<Vec<_>>>()?;
    let fusion = t.conv(2 * FEATURES, FEATURES, 1)?;
    let up1 = t.conv(FEATURES, 4 * FEATURES, 3)?;
    let up1_alpha = t.alpha()?;
    let up2 = t.conv(FEATURES, 4 * 32, 3)?;
    let up2_alpha = t.alpha()?;
    let last = t.conv(32, 3, 3)?;
    if t.it.next().is_some() {
        return Err(t.bad());
    }
    Ok(Net {
        extract,
        forward,
        backward,
        fusion: fusion.weights,
        fusion_bias: fusion.bias,
        up1,
        up1_alpha,
        up2,
        up2_alpha,
        last,
    })
}

fn laid_out(c: &Conv) -> (usize, usize, Vec<f32>, Vec<f32>) {
    let cin = c.cin.next_multiple_of(PADDED);
    let cout = c.cout.next_multiple_of(BLOCK);
    let mut weights = vec![0f32; cin * 9 * cout];
    for co in 0..c.cout {
        for ci in 0..c.cin {
            for k in 0..9 {
                weights[(ci * 9 + k) * cout + co] = c.weights[(co * c.cin + ci) * 9 + k];
            }
        }
    }
    let mut bias = c.bias.clone();
    bias.resize(cout, 0.0);
    (cin, cout, weights, bias)
}

fn conv_shader(cin: usize, cout: usize, act: bool) -> String {
    let mut body = String::new();
    for ky in 0..3 {
        body += &format!(
            "            {{\n            let row = ci * HALO * HALO + (py + {ky}u) * HALO + px;\n"
        );
        for j in 0..6 {
            body += &format!("            let i{j} = tile[row + {j}u];\n");
        }
        for kx in 0..3 {
            body += &format!(
                "            {{\n            let wb = (ci * 9u + {k}u) * 16u + g * 4u;\n",
                k = ky * 3 + kx
            );
            for q in 0..4 {
                body += &format!("            let w{q} = wts[wb + {q}u];\n");
            }
            for j in 0..4 {
                for q in 0..4 {
                    body += &format!(
                        "            a{j}{q} = fma(vec4<f32>(i{i}), w{q}, a{j}{q});\n",
                        i = j + kx
                    );
                }
            }
            body += "            }\n";
        }
        body += "            }\n";
    }
    let mut decl = String::new();
    for j in 0..4 {
        for q in 0..4 {
            decl += &format!("    var a{j}{q} = vec4<f32>(0.0);\n");
        }
    }
    let mut write = String::new();
    for j in 0..4 {
        write += &format!(
            "    if (x0 + px + {j}u < size.w && y0 + py < size.h) {{\n        let o = ((y0 + py) * size.w + x0 + px + {j}u) * (COUT / 2u) + (base + g * 16u) / 2u;\n"
        );
        for q in 0..4 {
            write += &format!(
                "        {{\n        let co = base + g * 16u + {q}u * 4u;\n        var v = a{j}{q} + vec4<f32>(bias[co], bias[co + 1u], bias[co + 2u], bias[co + 3u]);\n"
            );
            if act {
                write += "        let al = vec4<f32>(alpha[co], alpha[co + 1u], alpha[co + 2u], alpha[co + 3u]);\n        v = select(v * al, v, v >= vec4<f32>(0.0));\n";
            }
            write += &format!(
                "        dst[o + {a}u] = pack2x16float(v.xy);\n        dst[o + {b}u] = pack2x16float(v.zw);\n        }}\n",
                a = q * 2,
                b = q * 2 + 1
            );
        }
        write += "    }\n";
    }
    format!(
        r#"
const CIN: u32 = {cin}u;
const COUT: u32 = {cout}u;
const TILE: u32 = 16u;
const HALO: u32 = 18u;
const CHUNK: u32 = 8u;

struct Size {{ w: u32, h: u32 }}
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> bias: array<f32>;
@group(0) @binding(4) var<storage, read> alpha: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<u32>;

var<workgroup> tile: array<f32, 2592>;
var<workgroup> wts: array<vec4<f32>, 1152>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let g = li / 64u;
    let p = li % 64u;
    let px = (p % 4u) * 4u;
    let py = p / 4u;
    let x0 = wg.x * TILE;
    let y0 = wg.y * TILE;
    let base = wg.z * 64u;
    let groups = min(4u, (COUT - base) / 16u);
    let quads = groups * 4u;
{decl}
    for (var c0 = 0u; c0 < CIN; c0 += CHUNK) {{
        for (var i = li; i < HALO * HALO * (CHUNK / 2u); i += 256u) {{
            let pair = i / (HALO * HALO);
            let pos = i % (HALO * HALO);
            let gx = i32(x0 + pos % HALO) - 1;
            let gy = i32(y0 + pos / HALO) - 1;
            var v = vec2<f32>(0.0);
            if (gx >= 0 && gy >= 0 && gx < i32(size.w) && gy < i32(size.h)) {{
                v = unpack2x16float(src[(u32(gy) * size.w + u32(gx)) * (CIN / 2u) + c0 / 2u + pair]);
            }}
            tile[(pair * 2u) * HALO * HALO + pos] = v.x;
            tile[(pair * 2u + 1u) * HALO * HALO + pos] = v.y;
        }}
        for (var i = li; i < CHUNK * 9u * 16u; i += 256u) {{
            let ck = i / 16u;
            let q = i % 16u;
            if (q < quads) {{
                wts[i] = weights[(c0 * 9u + ck) * (COUT / 4u) + base / 4u + q];
            }}
        }}
        workgroupBarrier();
        if (g < groups) {{
            for (var ci = 0u; ci < CHUNK; ci++) {{
{body}
            }}
        }}
        workgroupBarrier();
    }}
    _ = alpha[0];
    if (g >= groups) {{
        return;
    }}
{write}
}}
"#
    )
}

const PACK: &str = r#"
struct Size { w: u32, h: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> rgba: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= size.w || id.y >= size.h) {
        return;
    }
    let p = id.y * size.w + id.x;
    let v = unpack4x8unorm(rgba[p]);
    dst[p * 4u] = pack2x16float(v.xy);
    dst[p * 4u + 1u] = pack2x16float(vec2<f32>(v.z, 0.0));
    dst[p * 4u + 2u] = 0u;
    dst[p * 4u + 3u] = 0u;
}
"#;

const ADD: &str = r#"
struct Count { n: u32, pad: u32 }
@group(0) @binding(0) var<uniform> count: Count;
@group(0) @binding(1) var<storage, read> a: array<u32>;
@group(0) @binding(2) var<storage, read> b: array<u32>;
@group(0) @binding(3) var<storage, read_write> dst: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = id.y * n.x * 256u + id.x;
    if (i >= count.n) {
        return;
    }
    dst[i] = pack2x16float(unpack2x16float(a[i]) + unpack2x16float(b[i]));
}
"#;

const FUSE: &str = r#"
const F: u32 = 96u;
struct Size { w: u32, h: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> fwd: array<u32>;
@group(0) @binding(2) var<storage, read> bwd: array<u32>;
@group(0) @binding(3) var<storage, read> weights: array<f32>;
@group(0) @binding(4) var<storage, read> bias: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<u32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= size.w || id.y >= size.h) {
        return;
    }
    let p = (id.y * size.w + id.x) * (F / 2u);
    var acc: array<f32, 96>;
    for (var co = 0u; co < F; co++) {
        acc[co] = bias[co];
    }
    for (var c = 0u; c < F / 2u; c++) {
        let a = unpack2x16float(fwd[p + c]);
        let b = unpack2x16float(bwd[p + c]);
        for (var co = 0u; co < F; co++) {
            let row = co * 2u * F;
            acc[co] += weights[row + c * 2u] * a.x + weights[row + c * 2u + 1u] * a.y
                + weights[row + F + c * 2u] * b.x + weights[row + F + c * 2u + 1u] * b.y;
        }
    }
    for (var c = 0u; c < F / 2u; c++) {
        dst[p + c] = pack2x16float(vec2<f32>(acc[c * 2u], acc[c * 2u + 1u]));
    }
}
"#;

fn shuffle_shader(cin: usize) -> String {
    format!(
        r#"
const CIN: u32 = {cin}u;
const COUT: u32 = {cout}u;
struct Size {{ w: u32, h: u32 }}
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn at(p: u32, c: u32) -> f32 {{
    let v = unpack2x16float(src[p * (CIN / 2u) + c / 2u]);
    return select(v.x, v.y, c % 2u == 1u);
}}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let ow = size.w * 2u;
    if (id.x >= ow || id.y >= size.h * 2u) {{
        return;
    }}
    let p = (id.y / 2u) * size.w + id.x / 2u;
    let sub = (id.y % 2u) * 2u + id.x % 2u;
    let o = (id.y * ow + id.x) * (COUT / 2u);
    for (var c = 0u; c < COUT; c += 2u) {{
        dst[o + c / 2u] = pack2x16float(vec2<f32>(at(p, c * 4u + sub), at(p, (c + 1u) * 4u + sub)));
    }}
}}
"#,
        cout = cin / 4
    )
}

const CROP: &str = r#"
struct Crop { src_w: u32, x0: u32, y0: u32, w: u32, h: u32, words: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<uniform> crop: Crop;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= crop.w * crop.words || id.y >= crop.h) {
        return;
    }
    let x = id.x / crop.words;
    let k = id.x % crop.words;
    let s = ((crop.y0 + id.y) * crop.src_w + crop.x0 + x) * crop.words + k;
    dst[(id.y * crop.w + x) * crop.words + k] = src[s];
}
"#;

const FINISH: &str = r#"
struct Size { w: u32, h: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> last: array<u32>;
@group(0) @binding(2) var<storage, read> rgba: array<u32>;
@group(0) @binding(3) var<storage, read_write> out: array<u32>;

fn px(x: u32, y: u32) -> vec3<f32> {
    return unpack4x8unorm(rgba[y * size.w + x]).xyz;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let ow = size.w * 4u;
    if (id.x >= ow || id.y >= size.h * 4u) {
        return;
    }
    let sx = max((f32(id.x) + 0.5) / 4.0 - 0.5, 0.0);
    let sy = max((f32(id.y) + 0.5) / 4.0 - 0.5, 0.0);
    let x0 = min(u32(sx), size.w - 1u);
    let y0 = min(u32(sy), size.h - 1u);
    let x1 = min(x0 + 1u, size.w - 1u);
    let y1 = min(y0 + 1u, size.h - 1u);
    let fx = sx - f32(x0);
    let fy = sy - f32(y0);
    let base = mix(mix(px(x0, y0), px(x1, y0), fx), mix(px(x0, y1), px(x1, y1), fx), fy);
    let o = (id.y * ow + id.x) * 8u;
    let rg = unpack2x16float(last[o]);
    let ba = unpack2x16float(last[o + 1u]);
    let rgb = clamp(vec3<f32>(rg.x, rg.y, ba.x) + base, vec3<f32>(0.0), vec3<f32>(1.0));
    out[id.y * ow + id.x] = pack4x8unorm(vec4<f32>(rgb, 1.0));
}
"#;

struct Layer {
    pipeline: usize,
    cout: usize,
    weights: wgpu::Buffer,
    bias: wgpu::Buffer,
    alpha: Option<wgpu::Buffer>,
}

struct Gpu {
    extract: Layer,
    forward: Vec<Layer>,
    backward: Vec<Layer>,
    fusion: wgpu::Buffer,
    fusion_bias: wgpu::Buffer,
    up1: Layer,
    up2: Layer,
    last: Layer,
}

struct Span {
    core: (usize, usize),
    read: (usize, usize),
}

fn spans(n: usize) -> Vec<Span> {
    let count = n.div_ceil(TILE).max(1);
    let each = n.div_ceil(count);
    (0..count)
        .map(|k| {
            let core = (k * each, ((k + 1) * each).min(n));
            Span {
                core,
                read: (core.0.saturating_sub(MARGIN), (core.1 + MARGIN).min(n)),
            }
        })
        .collect()
}

const WHAT: &str = "the AI Fast upscaler";

fn gpu_error(e: impl std::fmt::Display) -> Error {
    compute::gpu_error(WHAT, e)
}

fn checked<T>(device: &wgpu::Device, run: impl FnOnce() -> Result<T>) -> Result<T> {
    compute::checked(device, WHAT, run)
}

struct Pipelines {
    convs: Vec<((usize, usize, bool), wgpu::ComputePipeline)>,
    pack: wgpu::ComputePipeline,
    add: wgpu::ComputePipeline,
    fuse: wgpu::ComputePipeline,
    shuffle1: wgpu::ComputePipeline,
    shuffle2: wgpu::ComputePipeline,
    crop: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
}

struct Frame {
    w: usize,
    h: usize,
    count: usize,
    size: wgpu::Buffer,
    pixels: wgpu::Buffer,
    rgba: Vec<wgpu::Buffer>,
    input: wgpu::Buffer,
    feats: wgpu::Buffer,
    xa: wgpu::Buffer,
    xb: wgpu::Buffer,
    zero: wgpu::Buffer,
    states: Vec<wgpu::Buffer>,
    back: [wgpu::Buffer; 2],
    fused: wgpu::Buffer,
}

struct Tiles {
    size1: wgpu::Buffer,
    size2: wgpu::Buffer,
    size4: wgpu::Buffer,
    crop_feat: wgpu::Buffer,
    crop_rgba: wgpu::Buffer,
    feat: wgpu::Buffer,
    rgba: wgpu::Buffer,
    up1: wgpu::Buffer,
    s1: wgpu::Buffer,
    up2: wgpu::Buffer,
    s2: wgpu::Buffer,
    last: wgpu::Buffer,
    out: wgpu::Buffer,
    read: wgpu::Buffer,
}

struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Pipelines,
    none: wgpu::Buffer,
    net: Option<Gpu>,
    frame: Option<Frame>,
    tiles: Option<Tiles>,
}

fn pipeline(device: &wgpu::Device, source: String) -> wgpu::ComputePipeline {
    compute::pipeline(device, "upscale", source)
}

fn buffer(device: &wgpu::Device, size: usize, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    compute::buffer(device, "upscale", size, usage)
}

fn blocks(n: usize) -> u32 {
    n.div_ceil(BLOCK) as u32
}

impl Engine {
    fn start() -> std::result::Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU for the AI Fast upscaler: {e}"))?;
        let limits = adapter.limits();
        if limits.max_compute_workgroup_storage_size < SHARED
            || limits.max_compute_invocations_per_workgroup < 256
        {
            return Err(format!(
                "{} has too little workgroup memory for the AI Fast upscaler",
                adapter.get_info().name
            ));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vidcut-upscale"),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| format!("the GPU did not open for the AI Fast upscaler: {e}"))?;
        let made = checked(&device, || {
            let convs = [
                (PADDED, FEATURES, true),
                (FEATURES, FEATURES, true),
                (FEATURES, 4 * FEATURES, true),
                (FEATURES, 4 * 32, true),
                (32, BLOCK, false),
            ]
            .into_iter()
            .map(|key| (key, pipeline(&device, conv_shader(key.0, key.1, key.2))))
            .collect();
            let pipelines = Pipelines {
                convs,
                pack: pipeline(&device, PACK.into()),
                add: pipeline(&device, ADD.into()),
                fuse: pipeline(&device, FUSE.into()),
                shuffle1: pipeline(&device, shuffle_shader(4 * FEATURES)),
                shuffle2: pipeline(&device, shuffle_shader(4 * 32)),
                crop: pipeline(&device, CROP.into()),
                finish: pipeline(&device, FINISH.into()),
            };
            let none = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("upscale-none"),
                contents: &[0; 4],
                usage: wgpu::BufferUsages::STORAGE,
            });
            Ok((pipelines, none))
        });
        let (pipelines, none) = made.map_err(|e| e.to_string())?;
        Ok(Self {
            device,
            queue,
            pipelines,
            none,
            net: None,
            frame: None,
            tiles: None,
        })
    }

    fn storage(&self, data: &[u8]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("upscale-weights"),
                contents: data,
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    fn layer(&self, c: &Conv, alpha: Option<f32>) -> Layer {
        let (cin, cout, weights, bias) = laid_out(c);
        let pipeline = self
            .pipelines
            .convs
            .iter()
            .position(|(key, _)| *key == (cin, cout, alpha.is_some()))
            .expect("a pipeline for each layer");
        Layer {
            pipeline,
            cout,
            weights: self.storage(&bytes(&weights)),
            bias: self.storage(&bytes(&bias)),
            alpha: alpha.map(|a| self.storage(&bytes(&vec![a; cout]))),
        }
    }

    fn load(&mut self) -> Result<()> {
        if self.net.is_some() {
            return Ok(());
        }
        let dir = vidcut_core::dirs::models();
        get_ready(&dir, false)?;
        let net = read(&dir.join(WEIGHTS.name))?;
        let gpu = Gpu {
            extract: self.layer(&net.extract, Some(LEAK)),
            forward: net
                .forward
                .iter()
                .map(|c| self.layer(c, Some(LEAK)))
                .collect(),
            backward: net
                .backward
                .iter()
                .map(|c| self.layer(c, Some(LEAK)))
                .collect(),
            fusion: self.storage(&bytes(&net.fusion)),
            fusion_bias: self.storage(&bytes(&net.fusion_bias)),
            up1: self.layer(&net.up1, Some(net.up1_alpha)),
            up2: self.layer(&net.up2, Some(net.up2_alpha)),
            last: self.layer(&net.last, None),
        };
        self.net = Some(gpu);
        Ok(())
    }

    fn make_frame(&mut self, (w, h): (usize, usize), count: usize) {
        if self
            .frame
            .as_ref()
            .is_some_and(|f| f.w == w && f.h == h && f.count >= count)
        {
            return;
        }
        self.frame = None;
        use wgpu::BufferUsages as U;
        let px = w * h;
        let feat = px * FEATURES * 2;
        let d = &self.device;
        self.frame = Some(Frame {
            w,
            h,
            count,
            size: buffer(d, 8, U::UNIFORM | U::COPY_DST),
            pixels: buffer(d, 8, U::UNIFORM | U::COPY_DST),
            rgba: (0..count)
                .map(|_| buffer(d, px * 4, U::STORAGE | U::COPY_DST))
                .collect(),
            input: buffer(d, px * PADDED * 2, U::STORAGE),
            feats: buffer(d, feat, U::STORAGE),
            xa: buffer(d, feat, U::STORAGE),
            xb: buffer(d, feat, U::STORAGE),
            zero: buffer(d, feat, U::STORAGE),
            states: (0..count).map(|_| buffer(d, feat, U::STORAGE)).collect(),
            back: [buffer(d, feat, U::STORAGE), buffer(d, feat, U::STORAGE)],
            fused: buffer(d, feat, U::STORAGE),
        });
    }

    fn make_tiles(&mut self) {
        if self.tiles.is_some() {
            return;
        }
        use wgpu::BufferUsages as U;
        let side = TILE + 2 * MARGIN;
        let (p1, p2, p4) = (side * side, side * side * 4, side * side * 16);
        let d = &self.device;
        self.tiles = Some(Tiles {
            size1: buffer(d, 8, U::UNIFORM | U::COPY_DST),
            size2: buffer(d, 8, U::UNIFORM | U::COPY_DST),
            size4: buffer(d, 8, U::UNIFORM | U::COPY_DST),
            crop_feat: buffer(d, 32, U::UNIFORM | U::COPY_DST),
            crop_rgba: buffer(d, 32, U::UNIFORM | U::COPY_DST),
            feat: buffer(d, p1 * FEATURES * 2, U::STORAGE),
            rgba: buffer(d, p1 * 4, U::STORAGE),
            up1: buffer(d, p1 * 4 * FEATURES * 2, U::STORAGE),
            s1: buffer(d, p2 * FEATURES * 2, U::STORAGE),
            up2: buffer(d, p2 * 4 * 32 * 2, U::STORAGE),
            s2: buffer(d, p4 * 32 * 2, U::STORAGE),
            last: buffer(d, p4 * BLOCK * 2, U::STORAGE),
            out: buffer(d, p4 * 4, U::STORAGE | U::COPY_SRC),
            read: buffer(d, p4 * 4, U::MAP_READ | U::COPY_DST),
        });
    }

    fn conv(
        &self,
        pass: &mut wgpu::ComputePass,
        layer: &Layer,
        (size, src, dst): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
        (w, h): (usize, usize),
    ) {
        let pipeline = &self.pipelines.convs[layer.pipeline].1;
        let bind = group(
            &self.device,
            pipeline,
            &[
                size,
                src,
                &layer.weights,
                &layer.bias,
                layer.alpha.as_ref().unwrap_or(&self.none),
                dst,
            ],
        );
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(blocks(w), blocks(h), layer.cout.div_ceil(OUT_BLOCK) as u32);
    }

    fn simple(
        &self,
        pass: &mut wgpu::ComputePass,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        (x, y): (u32, u32),
    ) {
        let bind = group(&self.device, pipeline, buffers);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(x, y, 1);
    }

    fn chain(
        &self,
        pass: &mut wgpu::ComputePass,
        layers: &[Layer],
        (i, prev, dst): (usize, &wgpu::Buffer, &wgpu::Buffer),
    ) {
        let f = self.frame.as_ref().expect("frame made");
        let (w, h) = (f.w, f.h);
        let net = self.net.as_ref().expect("loaded");
        self.simple(
            pass,
            &self.pipelines.pack,
            &[&f.size, &f.rgba[i], &f.input],
            (blocks(w), blocks(h)),
        );
        self.conv(pass, &net.extract, (&f.size, &f.input, &f.feats), (w, h));
        let n = (w * h * FEATURES / 2) as u32;
        let groups = n.div_ceil(256);
        let (gx, gy) = (groups.min(65535), groups.div_ceil(65535));
        self.simple(
            pass,
            &self.pipelines.add,
            &[&f.pixels, &f.feats, prev, &f.xa],
            (gx, gy),
        );
        let last = layers.len() - 1;
        for (k, layer) in layers.iter().enumerate() {
            let (src, out) = if k % 2 == 0 {
                (&f.xa, &f.xb)
            } else {
                (&f.xb, &f.xa)
            };
            let out = if k == last { dst } else { out };
            self.conv(pass, layer, (&f.size, src, out), (w, h));
        }
    }

    fn run(&mut self, frames: &[Vec<u8>], (w, h): (usize, usize)) -> Result<Vec<Vec<u8>>> {
        let device = self.device.clone();
        checked(&device, || {
            self.load()?;
            self.make_frame((w, h), frames.len());
            self.make_tiles();
            let f = self.frame.as_ref().expect("frame made");
            let n = (w * h * FEATURES / 2) as u32;
            self.queue
                .write_buffer(&f.size, 0, &words(&[w as u32, h as u32]));
            self.queue.write_buffer(&f.pixels, 0, &words(&[n, 0]));
            for (i, rgb) in frames.iter().enumerate() {
                let rgba: Vec<u32> = rgb
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| u32::from_le_bytes([p[0], p[1], p[2], 255]))
                    .collect();
                self.queue.write_buffer(&f.rgba[i], 0, &words(&rgba));
            }
            let net = self.net.as_ref().expect("loaded");
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for i in 0..frames.len() {
                    let prev = if i == 0 { &f.zero } else { &f.states[i - 1] };
                    self.chain(&mut pass, &net.forward, (i, prev, &f.states[i]));
                }
            }
            self.queue.submit([enc.finish()]);
            let mut outs = vec![Vec::new(); frames.len()];
            for i in (0..frames.len()).rev() {
                let prev = if i + 1 == frames.len() {
                    &f.zero
                } else {
                    &f.back[(i + 1) % 2]
                };
                let mut enc = self.device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_compute_pass(&Default::default());
                    self.chain(&mut pass, &net.backward, (i, prev, &f.back[i % 2]));
                    self.simple(
                        &mut pass,
                        &self.pipelines.fuse,
                        &[
                            &f.size,
                            &f.states[i],
                            &f.back[i % 2],
                            &net.fusion,
                            &net.fusion_bias,
                            &f.fused,
                        ],
                        (blocks(w), blocks(h)),
                    );
                }
                self.queue.submit([enc.finish()]);
                outs[i] = self.enlarge(i)?;
            }
            Ok(outs)
        })
    }

    fn enlarge(&self, i: usize) -> Result<Vec<u8>> {
        let f = self.frame.as_ref().expect("frame made");
        let (w, h) = (f.w, f.h);
        let scale = SCALE as usize;
        let mut out = vec![0u8; w * scale * h * scale * 3];
        for row in spans(h) {
            for col in spans(w) {
                self.tile(i, (&col, &row), &mut out)?;
            }
        }
        Ok(out)
    }

    fn tile(&self, i: usize, (col, row): (&Span, &Span), out: &mut [u8]) -> Result<()> {
        let f = self.frame.as_ref().expect("frame made");
        let t = self.tiles.as_ref().expect("tiles made");
        let net = self.net.as_ref().expect("loaded");
        let (tw, th) = (col.read.1 - col.read.0, row.read.1 - row.read.0);
        let (x0, y0) = (col.read.0 as u32, row.read.0 as u32);
        let q = &self.queue;
        q.write_buffer(&t.size1, 0, &words(&[tw as u32, th as u32]));
        q.write_buffer(&t.size2, 0, &words(&[tw as u32 * 2, th as u32 * 2]));
        q.write_buffer(&t.size4, 0, &words(&[tw as u32 * 4, th as u32 * 4]));
        let feat_words = (FEATURES / 2) as u32;
        let crop =
            |words_per: u32| words(&[f.w as u32, x0, y0, tw as u32, th as u32, words_per, 0, 0]);
        q.write_buffer(&t.crop_feat, 0, &crop(feat_words));
        q.write_buffer(&t.crop_rgba, 0, &crop(1));
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            self.simple(
                &mut pass,
                &self.pipelines.crop,
                &[&t.crop_feat, &f.fused, &t.feat],
                (blocks(tw * FEATURES / 2), blocks(th)),
            );
            self.simple(
                &mut pass,
                &self.pipelines.crop,
                &[&t.crop_rgba, &f.rgba[i], &t.rgba],
                (blocks(tw), blocks(th)),
            );
            self.conv(&mut pass, &net.up1, (&t.size1, &t.feat, &t.up1), (tw, th));
            self.simple(
                &mut pass,
                &self.pipelines.shuffle1,
                &[&t.size1, &t.up1, &t.s1],
                (blocks(tw * 2), blocks(th * 2)),
            );
            self.conv(
                &mut pass,
                &net.up2,
                (&t.size2, &t.s1, &t.up2),
                (tw * 2, th * 2),
            );
            self.simple(
                &mut pass,
                &self.pipelines.shuffle2,
                &[&t.size2, &t.up2, &t.s2],
                (blocks(tw * 4), blocks(th * 4)),
            );
            self.conv(
                &mut pass,
                &net.last,
                (&t.size4, &t.s2, &t.last),
                (tw * 4, th * 4),
            );
            self.simple(
                &mut pass,
                &self.pipelines.finish,
                &[&t.size1, &t.last, &t.rgba, &t.out],
                (blocks(tw * 4), blocks(th * 4)),
            );
        }
        let scale = SCALE as usize;
        let (ow, oh) = (tw * scale, th * scale);
        let shown = (ow * oh * 4) as u64;
        enc.copy_buffer_to_buffer(&t.out, 0, &t.read, 0, shown);
        self.queue.submit([enc.finish()]);
        let slice = t.read.slice(..shown);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(gpu_error)?;
        rx.recv().map_err(gpu_error)?.map_err(gpu_error)?;
        {
            let data = slice.get_mapped_range().map_err(gpu_error)?;
            let into = f.w * scale * 3;
            let first = (col.core.0 - col.read.0) * scale;
            let count = (col.core.1 - col.core.0) * scale;
            for y in (row.core.0 - row.read.0) * scale..(row.core.1 - row.read.0) * scale {
                let to = (row.read.0 * scale + y) * into + col.core.0 * scale * 3;
                let from = &data[(y * ow + first) * 4..(y * ow + first + count) * 4];
                for (d, s) in out[to..to + count * 3]
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(from.as_chunks::<4>().0)
                {
                    *d = [s[0], s[1], s[2]];
                }
            }
        }
        t.read.unmap();
        Ok(())
    }
}

static ENGINE: Mutex<Option<std::result::Result<Engine, String>>> = Mutex::new(None);

fn with_engine<T>(run: impl FnOnce(&mut Engine) -> Result<T>) -> Result<T> {
    let mut engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    match engine.get_or_insert_with(Engine::start) {
        Ok(e) => run(e),
        Err(why) => Err(Error::Invalid(why.clone())),
    }
}

pub(crate) fn upscale(frames: &[Vec<u8>], size: (usize, usize)) -> Result<Vec<Vec<u8>>> {
    with_engine(|e| {
        let out = e.run(frames, size);
        e.frame = None;
        e.tiles = None;
        out
    })
}
