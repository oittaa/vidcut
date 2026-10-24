use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::Instant;

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::VideoPiece;
use wgpu::util::DeviceExt;

use crate::compute::{self, bytes, group, halves, words};
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
    fusion: Conv,
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
        fusion,
        up1,
        up1_alpha,
        up2,
        up2_alpha,
        last,
    })
}

#[derive(Clone, Copy, PartialEq)]
struct Shape {
    cin: usize,
    cout: usize,
    size: usize,
    act: bool,
    join: bool,
    wino: bool,
    sum: bool,
    shuffle: bool,
}

impl Shape {
    fn new(cin: usize, cout: usize, size: usize, act: bool, join: bool, sum: bool) -> Self {
        let wino = size == 3 && cin == FEATURES;
        Self {
            cin,
            cout,
            size,
            act,
            join,
            wino,
            sum,
            shuffle: wino && cout != cin,
        }
    }
}

fn laid_out(c: &Conv, size: usize) -> (usize, usize, Vec<f32>, Vec<f32>) {
    let taps = size * size;
    let cin = c.cin.next_multiple_of(PADDED);
    let cout = c.cout.next_multiple_of(BLOCK);
    let mut weights = vec![0f32; cin * taps * cout];
    for co in 0..c.cout {
        for ci in 0..c.cin {
            for k in 0..taps {
                weights[(ci * taps + k) * cout + co] = c.weights[(co * c.cin + ci) * taps + k];
            }
        }
    }
    let mut bias = c.bias.clone();
    bias.resize(cout, 0.0);
    (cin, cout, weights, bias)
}

fn conv_shader(s: Shape) -> String {
    let Shape {
        cin,
        cout,
        size,
        act,
        join,
        ..
    } = s;
    let taps = size * size;
    let mut body = String::new();
    for ky in 0..size {
        body += &format!(
            "            {{\n            let row = ci * HALO * HALO + (py + {ky}u) * HALO + px;\n"
        );
        for j in 0..3 + size {
            body += &format!("            let i{j} = tile[row + {j}u];\n");
        }
        for kx in 0..size {
            body += &format!(
                "            {{\n            let wb = (ci * TAPS + {k}u) * 16u + g * 4u;\n",
                k = ky * size + kx
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
    let fetch = if join {
        "@group(0) @binding(6) var<storage, read> src2: array<u32>;

fn fetch(p: u32, c: u32) -> u32 {
    if (c < CIN / 2u) {
        return src[p * (CIN / 4u) + c / 2u];
    }
    return src2[p * (CIN / 4u) + (c - CIN / 2u) / 2u];
}"
    } else {
        "fn fetch(p: u32, c: u32) -> u32 {
    return src[p * (CIN / 2u) + c / 2u];
}"
    };
    let halo = 15 + size;
    let pad = size / 2;
    let tile = 8 * halo * halo;
    let wts = 8 * taps * 16;
    format!(
        r#"
const CIN: u32 = {cin}u;
const COUT: u32 = {cout}u;
const TILE: u32 = 16u;
const HALO: u32 = {halo}u;
const PAD: i32 = {pad};
const TAPS: u32 = {taps}u;
const CHUNK: u32 = 8u;

struct Size {{ w: u32, h: u32 }}
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> bias: array<f32>;
@group(0) @binding(4) var<storage, read> alpha: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<u32>;
{fetch}

var<workgroup> tile: array<f32, {tile}>;
var<workgroup> wts: array<vec4<f32>, {wts}>;

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
            let gx = i32(x0 + pos % HALO) - PAD;
            let gy = i32(y0 + pos / HALO) - PAD;
            var v = vec2<f32>(0.0);
            if (gx >= 0 && gy >= 0 && gx < i32(size.w) && gy < i32(size.h)) {{
                v = unpack2x16float(fetch(u32(gy) * size.w + u32(gx), c0 + pair * 2u));
            }}
            tile[(pair * 2u) * HALO * HALO + pos] = v.x;
            tile[(pair * 2u + 1u) * HALO * HALO + pos] = v.y;
        }}
        for (var i = li; i < CHUNK * TAPS * 16u; i += 256u) {{
            let ck = i / 16u;
            let q = i % 16u;
            if (q < quads) {{
                wts[i] = weights[(c0 * TAPS + ck) * (COUT / 4u) + base / 4u + q];
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

fn winograd(c: &Conv) -> Vec<f32> {
    let mut u = vec![0f32; c.cin * 16 * c.cout];
    for co in 0..c.cout {
        for ci in 0..c.cin {
            let g = |a: usize, b: usize| f64::from(c.weights[(co * c.cin + ci) * 9 + a * 3 + b]);
            let rows = [0, 1, 2].map(|b| {
                [
                    g(0, b),
                    (g(0, b) + g(1, b) + g(2, b)) / 2.0,
                    (g(0, b) - g(1, b) + g(2, b)) / 2.0,
                    g(2, b),
                ]
            });
            for r in 0..4 {
                let h = [rows[0][r], rows[1][r], rows[2][r]];
                let v = [
                    h[0],
                    (h[0] + h[1] + h[2]) / 2.0,
                    (h[0] - h[1] + h[2]) / 2.0,
                    h[2],
                ];
                for (k, v) in v.iter().enumerate() {
                    u[(ci * 16 + r * 4 + k) * c.cout + co] = *v as f32;
                }
            }
        }
    }
    u
}

fn wino_shader(cout: usize, act: bool, sum: bool, shuffle: bool, half: bool) -> String {
    let (acc, zero, lane) = if half {
        ("vec4<f16>", "0.0h", "h2")
    } else {
        ("vec4<f32>", "0.0", "unh")
    };
    let mut decl = String::new();
    for k in 0..8 {
        for h in 0..2 {
            decl += &format!("    var a{k}{h} = {acc}({zero});\n");
        }
    }
    let mut body = String::new();
    for ci in 0..8 {
        body += &format!(
            "        {{\n        let vq = sm[({ci}u * 16u + tv) * 4u + a];\n        let uq = sm[512u + ({ci}u * 16u + t) * 4u + b];\n        let v0 = {acc}({lane}(vq.x), {lane}(vq.y));\n        let v1 = {acc}({lane}(vq.z), {lane}(vq.w));\n        let u0 = {acc}({lane}(uq.x), {lane}(uq.y));\n        let u1 = {acc}({lane}(uq.z), {lane}(uq.w));\n"
        );
        for k in 0..8 {
            let v = format!("v{}.{}", k / 4, ["x", "y", "z", "w"][k % 4]);
            for h in 0..2 {
                body += &format!("        a{k}{h} = fma({acc}({v}), u{h}, a{k}{h});\n");
            }
        }
        body += "        }\n";
    }
    let mut store = String::new();
    for k in 0..8 {
        let (from, c) = (["va", "vb"][k / 4], ["x", "y", "z", "w"][k % 4]);
        store += &format!(
            "            {{\n            let t = th * 8u + {k}u;\n            sm[(tc * 16u + (t ^ (t >> 3u))) * 4u + tp / 4u][tp % 4u] = pah(vec2<f32>({from}0.{c}, {from}1.{c}));\n            }}\n"
        );
    }
    let mut epilogue = String::new();
    for r in 0..4 {
        for kk in 0..2 {
            for h in 0..2 {
                epilogue += &format!(
                    "    sm[(t * 8u + a * 2u + {kk}u) * 8u + ((b * 2u + {h}u) ^ (a & 1u))] = bitcast<vec4<u32>>(vec4<f32>(a{k}{h}));\n",
                    k = 2 * r + kk
                );
            }
        }
        let nine = |n: &str| {
            (0..9)
                .map(|i| format!("{n}{r}_{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if shuffle {
            epilogue += "    workgroupBarrier();\n    if (li < 128u) {\n";
            for (n, quad) in [("a", 0), ("b", 1)] {
                for i in 0..3 {
                    for k in 0..3 {
                        epilogue += &format!(
                            "        let {n}{r}_{} = bitcast<vec4<f32>>(sm[(((p + {i}u) * 4u + sq + {k}u) * 8u + ej) * 8u + ((2u * qp + {quad}u) ^ ((ej / 2u) & 1u))]);\n",
                            i * 3 + k
                        );
                    }
                }
            }
            epilogue += &format!(
                "        place(x0 + ({}u + ej % 2u) * 2u + sq, y0 + (ej / 2u) * 2u + p, base + qp * 8u, spixel(array<vec4<f32>, 9>({}), p, sq), spixel(array<vec4<f32>, 9>({}), p, sq));\n",
                r * 2,
                nine("a"),
                nine("b")
            );
        } else {
            epilogue += "    workgroupBarrier();\n    {\n";
            for i in 0..3 {
                for k in 0..3 {
                    epilogue += &format!(
                        "        let m{r}_{} = bitcast<vec4<f32>>(sm[(((pp + {i}u) * 4u + qq + {k}u) * 8u + ej) * 8u + (cq ^ ((ej / 2u) & 1u))]);\n",
                        i * 3 + k
                    );
                }
            }
            epilogue += &format!(
                "        put(x0 + ({}u + ej % 2u) * 2u + qq, y0 + (ej / 2u) * 2u + pp, base + cq * 4u, pixel(array<vec4<f32>, 9>({}), pp, qq));\n",
                r * 2,
                nine("m")
            );
        }
        epilogue += "    }\n    workgroupBarrier();\n";
    }
    let activate = if act {
        "v = select(v * vec4<f32>(alpha[co], alpha[co + 1u], alpha[co + 2u], alpha[co + 3u]), v, v >= vec4<f32>(0.0));"
    } else {
        "_ = alpha[0];"
    };
    let fetch = if sum {
        "@group(0) @binding(6) var<storage, read> src2: array<vec4<u32>>;

fn fetch(i: u32) -> array<vec2<f32>, 4> {
    let q = src[i];
    let r = src2[i];
    return array<vec2<f32>, 4>(unh(q.x) + unh(r.x), unh(q.y) + unh(r.y), unh(q.z) + unh(r.z), unh(q.w) + unh(r.w));
}"
    } else {
        "fn fetch(i: u32) -> array<vec2<f32>, 4> {
    let q = src[i];
    return array<vec2<f32>, 4>(unh(q.x), unh(q.y), unh(q.z), unh(q.w));
}"
    };
    let halves = if half {
        "enable f16;

fn unh(x: u32) -> vec2<f32> {
    return unpack2x16float(x);
}

fn pah(v: vec2<f32>) -> u32 {
    return pack2x16float(v);
}

fn h2(x: u32) -> vec2<f16> {
    return vec2<f16>(unpack2x16float(x));
}"
    } else {
        "fn unh(x: u32) -> vec2<f32> {
    return unpack2x16float(x);
}

fn pah(v: vec2<f32>) -> u32 {
    return pack2x16float(v);
}"
    };
    format!(
        r#"{halves}

const CIN: u32 = {FEATURES}u;
const COUT: u32 = {cout}u;
const CK: u32 = 8u;

struct Size {{ w: u32, h: u32, sw: u32, ox: u32, oy: u32, p0: u32, p1: u32, p2: u32 }}
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> src: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> bias: array<f32>;
@group(0) @binding(4) var<storage, read> alpha: array<f32>;
@group(0) @binding(5) var<storage, read_write> dst: array<u32>;
{fetch}

var<workgroup> sm: array<vec4<u32>, 1024>;
var<workgroup> raw: array<vec2<f32>, 960>;

fn act4(y: vec4<f32>, co: u32) -> vec4<f32> {{
    var v = y + vec4<f32>(bias[co], bias[co + 1u], bias[co + 2u], bias[co + 3u]);
    {activate}
    return v;
}}

fn put(px: u32, py: u32, co: u32, y: vec4<f32>) {{
    if (px < size.w && py < size.h) {{
        let v = act4(y, co);
        let o = (py * size.w + px) * (COUT / 2u) + co / 2u;
        dst[o] = pah(v.xy);
        dst[o + 1u] = pah(v.zw);
    }}
}}

fn place(px: u32, py: u32, co: u32, y0: vec4<f32>, y1: vec4<f32>) {{
    if (px < size.w && py < size.h) {{
        let a = act4(y0, co);
        let b = act4(y1, co + 4u);
        let ow = size.w * 2u;
        let o = (py * 2u * ow + px * 2u) * (COUT / 8u) + co / 8u;
        dst[o] = pah(vec2<f32>(a.x, b.x));
        dst[o + COUT / 8u] = pah(vec2<f32>(a.y, b.y));
        dst[o + ow * (COUT / 8u)] = pah(vec2<f32>(a.z, b.z));
        dst[o + (ow + 1u) * (COUT / 8u)] = pah(vec2<f32>(a.w, b.w));
    }}
}}

fn down(a: vec4<f32>, b: vec4<f32>, d: vec4<f32>, p: u32) -> vec4<f32> {{
    return select(a + b + d, a - b - d, p == 1u);
}}

fn pixel(m: array<vec4<f32>, 9>, p: u32, q: u32) -> vec4<f32> {{
    let u0 = down(m[0], m[3], m[6], p);
    let u1 = down(m[1], m[4], m[7], p);
    let u2 = down(m[2], m[5], m[8], p);
    return down(u0, u1, u2, q);
}}

fn spixel(m: array<vec4<f32>, 9>, p: u32, q: u32) -> vec4<f32> {{
    let s = select(1.0, -1.0, p == 1u);
    let u0 = m[0] + s * (m[3] + m[6]);
    let u1 = m[1] + s * (m[4] + m[7]);
    let u2 = m[2] + s * (m[5] + m[8]);
    return down(u0, u1, u2, q);
}}

fn spread(w: vec4<f32>) -> vec4<f32> {{
    return vec4<f32>(w.x - w.z, w.y + w.z, w.z - w.y, w.y - w.w);
}}

fn rows(e0: vec4<f32>, e1: vec4<f32>, e2: vec4<f32>, th: u32) -> array<vec4<f32>, 2> {{
    return array<vec4<f32>, 2>(spread(select(e0 - e2, e1 - e0, th == 1u)), spread(select(e1 + e2, e0 - e2, th == 1u)));
}}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let t = li / 16u;
    let tv = t ^ (t >> 3u);
    let a = (li / 4u) % 4u;
    let b = li % 4u;
    let x0 = wg.y * 16u;
    let y0 = wg.z * 8u;
    let base = wg.x * 32u;
    let rx = i32(x0 + li % 18u) - 1;
    let ry = i32(y0 + li / 18u) - 1;
    let rin = li < 180u && rx >= 0 && ry >= 0 && rx < i32(size.w) && ry < i32(size.h);
    var rsrc = 0u;
    if (rin) {{
        rsrc = ((u32(ry) + size.oy) * size.sw + u32(rx) + size.ox) * (CIN / 8u);
    }}
    let rk = (li / 18u) * 12u + (li % 18u) / 2u;
    let re = li & 1u;
    let tp = li % 16u;
    let th = (li / 16u) % 2u;
    let tc = li / 32u;
    let tb = tc * 120u + ((tp / 4u) * 2u + th) * 12u + (tp % 4u) * 2u;
    let ur = li / 4u;
    let uk = li % 4u;
{decl}
    for (var c0 = 0u; c0 < CIN; c0 += CK) {{
        if (li < 180u) {{
            var pix = array<vec2<f32>, 4>();
            if (rin) {{
                pix = fetch(rsrc + c0 / 8u);
            }}
            raw[rk][re] = pix[0].x;
            raw[120u + rk][re] = pix[0].y;
            raw[240u + rk][re] = pix[1].x;
            raw[360u + rk][re] = pix[1].y;
            raw[480u + rk][re] = pix[2].x;
            raw[600u + rk][re] = pix[2].y;
            raw[720u + rk][re] = pix[3].x;
            raw[840u + rk][re] = pix[3].y;
        }}
        workgroupBarrier();
        {{
            let q00 = raw[tb];
            let q01 = raw[tb + 1u];
            let q02 = raw[tb + 2u];
            let q10 = raw[tb + 12u];
            let q11 = raw[tb + 13u];
            let q12 = raw[tb + 14u];
            let q20 = raw[tb + 24u];
            let q21 = raw[tb + 25u];
            let q22 = raw[tb + 26u];
            let ra = rows(vec4<f32>(q00, q01), vec4<f32>(q10, q11), vec4<f32>(q20, q21), th);
            let rb = rows(vec4<f32>(q01, q02), vec4<f32>(q11, q12), vec4<f32>(q21, q22), th);
            let va0 = ra[0];
            let vb0 = ra[1];
            let va1 = rb[0];
            let vb1 = rb[1];
{store}
        }}
        sm[512u + ur * 4u + uk] = weights[(c0 * 16u + ur) * (COUT / 8u) + base / 8u + uk];
        sm[768u + ur * 4u + uk] = weights[(c0 * 16u + ur + 64u) * (COUT / 8u) + base / 8u + uk];
        workgroupBarrier();
{body}
    }}
    workgroupBarrier();
    let jb = li % 64u;
    let ej = jb / 8u;
    let cq = jb % 8u;
    let qp = (jb / 2u) % 4u;
    let p = jb % 2u;
    let sq = li / 64u;
    let pp = li / 128u;
    let qq = (li / 64u) % 2u;
{epilogue}
}}
"#
    )
}

const PACK: &str = r#"
struct Size { w: u32, h: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> rgb: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn byte(at: u32) -> u32 {
    return (rgb[at / 4u] >> (8u * (at % 4u))) & 255u;
}

fn texel(p: u32) -> u32 {
    let at = p * 3u;
    return byte(at) | (byte(at + 1u) << 8u) | (byte(at + 2u) << 16u) | (255u << 24u);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= size.w || id.y >= size.h) {
        return;
    }
    let p = id.y * size.w + id.x;
    let v = unpack4x8unorm(texel(p));
    dst[p * 4u] = pack2x16float(v.xy);
    dst[p * 4u + 1u] = pack2x16float(vec2<f32>(v.z, 0.0));
    dst[p * 4u + 2u] = 0u;
    dst[p * 4u + 3u] = 0u;
}
"#;

fn last_shader() -> String {
    let mut body = String::new();
    for ky in 0..3 {
        for j in 0..6usize {
            body += &format!(
                "    {{\n    let y = ly + {ky} - 1;\n    let x = lx + {j} - 1;\n    var q = array<vec4<u32>, 4>(vec4<u32>(0u), vec4<u32>(0u), vec4<u32>(0u), vec4<u32>(0u));\n    if (y >= 0 && y < h4 && x >= 0 && x < w4) {{\n        let p = (u32(y) * u32(w4) + u32(x)) * 4u;\n        q = array<vec4<u32>, 4>(src[p], src[p + 1u], src[p + 2u], src[p + 3u]);\n    }}\n"
            );
            for word in 0..16 {
                body += &format!(
                    "    let v{word} = unpack2x16float(q[{}].{});\n",
                    word / 4,
                    ["x", "y", "z", "w"][word % 4]
                );
            }
            for k in j.saturating_sub(2)..=j.min(3) {
                let tap = ky * 3 + j - k;
                for ci in 0..32 {
                    body += &format!(
                        "    s{k} = fma(vec3<f32>(v{}.{}), wts.w[{}].xyz, s{k});\n",
                        ci / 2,
                        ["x", "y"][ci % 2],
                        ci * 9 + tap
                    );
                }
            }
            body += "    }\n";
        }
    }
    format!(
        r#"
struct Place {{ fw: u32, fh: u32, rx: u32, ry: u32, tw: u32, th: u32, cx: u32, cy: u32, cw: u32, ch: u32, p0: u32, p1: u32 }}
struct Last {{ w: array<vec4<f32>, 288>, b: vec4<f32> }}
@group(0) @binding(0) var<uniform> place: Place;
@group(0) @binding(1) var<storage, read> src: array<vec4<u32>>;
@group(0) @binding(2) var<uniform> wts: Last;
@group(0) @binding(3) var<storage, read> pixels: array<u32>;
@group(0) @binding(4) var<storage, read_write> out: array<u32>;

fn byte(at: u32) -> u32 {{
    return (pixels[at / 4u] >> (8u * (at % 4u))) & 255u;
}}

fn px(x: u32, y: u32) -> vec3<f32> {{
    let at = (y * place.fw + x) * 3u;
    return unpack4x8unorm(byte(at) | (byte(at + 1u) << 8u) | (byte(at + 2u) << 16u) | (255u << 24u)).xyz;
}}

fn base(gx: u32, gy: u32) -> vec3<f32> {{
    let sx = max((f32(gx) + 0.5) / 4.0 - 0.5, 0.0);
    let sy = max((f32(gy) + 0.5) / 4.0 - 0.5, 0.0);
    let x0 = min(u32(sx), place.fw - 1u);
    let y0 = min(u32(sy), place.fh - 1u);
    let x1 = min(x0 + 1u, place.fw - 1u);
    let y1 = min(y0 + 1u, place.fh - 1u);
    let fx = sx - f32(x0);
    let fy = sy - f32(y0);
    return mix(mix(px(x0, y0), px(x1, y0), fx), mix(px(x0, y1), px(x1, y1), fx), fy);
}}

fn rgb(s: vec3<f32>, gx: u32, gy: u32) -> vec3<f32> {{
    return clamp(s + base(gx, gy), vec3<f32>(0.0), vec3<f32>(1.0));
}}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    if (id.x >= place.cw || id.y >= place.ch * 4u) {{
        return;
    }}
    let gx = (place.cx + id.x) * 4u;
    let gy = place.cy * 4u + id.y;
    let lx = i32(gx - place.rx * 4u);
    let ly = i32(gy - place.ry * 4u);
    let w4 = i32(place.tw * 4u);
    let h4 = i32(place.th * 4u);
    var s0 = wts.b.xyz;
    var s1 = wts.b.xyz;
    var s2 = wts.b.xyz;
    var s3 = wts.b.xyz;
{body}
    let a = rgb(s0, gx, gy);
    let b = rgb(s1, gx + 1u, gy);
    let c = rgb(s2, gx + 2u, gy);
    let d = rgb(s3, gx + 3u, gy);
    let o = (gy * place.fw + place.cx + id.x) * 3u;
    out[o] = pack4x8unorm(vec4<f32>(a, b.x));
    out[o + 1u] = pack4x8unorm(vec4<f32>(b.yz, c.xy));
    out[o + 2u] = pack4x8unorm(vec4<f32>(c.z, d));
}}
"#
    )
}

fn last_weights(c: &Conv) -> Vec<f32> {
    let mut w = vec![0f32; 289 * 4];
    for ci in 0..c.cin {
        for tap in 0..9 {
            for co in 0..c.cout {
                w[(ci * 9 + tap) * 4 + co] = c.weights[(co * c.cin + ci) * 9 + tap];
            }
        }
    }
    w[288 * 4..288 * 4 + c.cout].copy_from_slice(&c.bias);
    w
}

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
    fusion: Layer,
    up1: Layer,
    up2: Layer,
    last: wgpu::Buffer,
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
    convs: Vec<(Shape, wgpu::ComputePipeline)>,
    pack: wgpu::ComputePipeline,
    last: wgpu::ComputePipeline,
}

struct Frame {
    w: usize,
    h: usize,
    count: usize,
    size: wgpu::Buffer,
    rgb: Vec<wgpu::Buffer>,
    input: wgpu::Buffer,
    feats: wgpu::Buffer,
    xa: wgpu::Buffer,
    xb: wgpu::Buffer,
    zero: wgpu::Buffer,
    states: Vec<wgpu::Buffer>,
    back: [wgpu::Buffer; 2],
    fused: wgpu::Buffer,
}

struct Job {
    tw: usize,
    th: usize,
    cw: usize,
    ch: usize,
    size1: wgpu::Buffer,
    size2: wgpu::Buffer,
    place: wgpu::Buffer,
}

struct Tiles {
    size: (usize, usize),
    jobs: Vec<Job>,
    s1: wgpu::Buffer,
    s2: wgpu::Buffer,
    out: wgpu::Buffer,
    read: [wgpu::Buffer; 2],
    shown: u64,
}

struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Pipelines,
    none: wgpu::Buffer,
    net: Option<Gpu>,
    frame: Option<Frame>,
    tiles: Option<Tiles>,
    pending: Option<Pending>,
    groups: RefCell<Groups>,
}

type Rest = Box<dyn FnOnce(Result<Vec<Vec<u8>>>) + Send>;

type Groups = HashMap<u64, Vec<(wgpu::ComputePipeline, Vec<wgpu::Buffer>, wgpu::BindGroup)>>;

struct Pending {
    wanted: usize,
    size: (usize, usize),
    outs: Vec<Vec<u8>>,
    rest: Rest,
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
        let half_feature = adapter.features() & wgpu::Features::SHADER_F16;
        let half = !half_feature.is_empty();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vidcut-upscale"),
            required_limits: limits,
            required_features: half_feature,
            ..Default::default()
        }))
        .map_err(|e| format!("the GPU did not open for the AI Fast upscaler: {e}"))?;
        let made = checked(&device, || {
            let convs = [
                Shape::new(PADDED, FEATURES, 3, true, false, false),
                Shape::new(FEATURES, FEATURES, 3, true, false, false),
                Shape::new(FEATURES, FEATURES, 3, true, false, true),
                Shape::new(2 * FEATURES, FEATURES, 1, false, true, false),
                Shape::new(FEATURES, 4 * FEATURES, 3, true, false, false),
                Shape::new(FEATURES, 4 * 32, 3, true, false, false),
            ]
            .into_iter()
            .map(|shape| {
                let source = if shape.wino {
                    wino_shader(shape.cout, shape.act, shape.sum, shape.shuffle, half)
                } else {
                    conv_shader(shape)
                };
                (shape, pipeline(&device, source))
            })
            .collect();
            let pipelines = Pipelines {
                convs,
                pack: pipeline(&device, PACK.into()),
                last: pipeline(&device, last_shader()),
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
            pending: None,
            groups: RefCell::default(),
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

    fn layer(&self, c: &Conv, alpha: Option<f32>, sum: bool) -> Layer {
        let size = if c.weights.len() == c.cin * c.cout {
            1
        } else {
            3
        };
        let wino = size == 3 && c.cin == FEATURES;
        let (cin, cout, weights, bias) = if wino {
            (c.cin, c.cout, winograd(c), c.bias.clone())
        } else {
            laid_out(c, size)
        };
        let shape = Shape::new(cin, cout, size, alpha.is_some(), c.cin == 2 * FEATURES, sum);
        let pipeline = self
            .pipelines
            .convs
            .iter()
            .position(|(key, _)| *key == shape)
            .expect("a pipeline for each layer");
        Layer {
            pipeline,
            cout,
            weights: self.storage(&if wino {
                halves(&weights)
            } else {
                bytes(&weights)
            }),
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
            extract: self.layer(&net.extract, Some(LEAK), false),
            forward: net
                .forward
                .iter()
                .enumerate()
                .map(|(k, c)| self.layer(c, Some(LEAK), k == 0))
                .collect(),
            backward: net
                .backward
                .iter()
                .enumerate()
                .map(|(k, c)| self.layer(c, Some(LEAK), k == 0))
                .collect(),
            fusion: self.layer(&net.fusion, None, false),
            up1: self.layer(&net.up1, Some(net.up1_alpha), false),
            up2: self.layer(&net.up2, Some(net.up2_alpha), false),
            last: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("upscale-last"),
                    contents: &bytes(&last_weights(&net.last)),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
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
        self.groups.get_mut().clear();
        use wgpu::BufferUsages as U;
        let px = w * h;
        let feat = px * FEATURES * 2;
        let d = &self.device;
        self.frame = Some(Frame {
            w,
            h,
            count,
            size: buffer(d, 32, U::UNIFORM | U::COPY_DST),
            rgb: (0..count)
                .map(|_| buffer(d, px * 3, U::STORAGE | U::COPY_DST))
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
        self.groups.get_mut().clear();
        let f = self.frame.as_ref().expect("frame made");
        let (w, h) = (f.w, f.h);
        use wgpu::BufferUsages as U;
        let uniform = |v: &[usize]| {
            let v: Vec<u32> = v.iter().map(|&x| x as u32).collect();
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("upscale-tile"),
                    contents: &words(&v),
                    usage: U::UNIFORM,
                })
        };
        let mut jobs = Vec::new();
        for row in spans(h) {
            for col in spans(w) {
                let (tw, th) = (col.read.1 - col.read.0, row.read.1 - row.read.0);
                let (cw, ch) = (col.core.1 - col.core.0, row.core.1 - row.core.0);
                let (rx, ry) = (col.read.0, row.read.0);
                jobs.push(Job {
                    tw,
                    th,
                    cw,
                    ch,
                    size1: uniform(&[tw, th, w, rx, ry, 0, 0, 0]),
                    size2: uniform(&[tw * 2, th * 2, tw * 2, 0, 0, 0, 0, 0]),
                    place: uniform(&[w, h, rx, ry, tw, th, col.core.0, row.core.0, cw, ch, 0, 0]),
                });
            }
        }
        let side = TILE + 2 * MARGIN;
        let (p2, p4) = (side * side * 4, side * side * 16);
        let shown = w * h * (SCALE * SCALE) as usize * 3;
        let d = &self.device;
        self.tiles = Some(Tiles {
            size: (w, h),
            jobs,
            s1: buffer(d, p2 * FEATURES * 2, U::STORAGE),
            s2: buffer(d, p4 * 32 * 2, U::STORAGE),
            out: buffer(d, shown, U::STORAGE | U::COPY_SRC),
            read: [
                buffer(d, shown, U::MAP_READ | U::COPY_DST),
                buffer(d, shown, U::MAP_READ | U::COPY_DST),
            ],
            shown: shown as u64,
        });
    }

    fn conv(
        &self,
        pass: &mut wgpu::ComputePass,
        layer: &Layer,
        (size, src, dst): (&wgpu::Buffer, &[&wgpu::Buffer], &wgpu::Buffer),
        (w, h): (usize, usize),
    ) {
        let (shape, pipeline) = &self.pipelines.convs[layer.pipeline];
        let mut buffers = vec![
            size,
            src[0],
            &layer.weights,
            &layer.bias,
            layer.alpha.as_ref().unwrap_or(&self.none),
            dst,
        ];
        buffers.extend(&src[1..]);
        let bind = self.bound(pipeline, &buffers);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let (x, y, z) = if shape.wino {
            (layer.cout / 32, w.div_ceil(BLOCK), h.div_ceil(BLOCK / 2))
        } else {
            (
                w.div_ceil(BLOCK),
                h.div_ceil(BLOCK),
                layer.cout.div_ceil(OUT_BLOCK),
            )
        };
        pass.dispatch_workgroups(x as u32, y as u32, z as u32);
    }

    fn bound(
        &self,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
    ) -> wgpu::BindGroup {
        let mut hasher = DefaultHasher::new();
        pipeline.hash(&mut hasher);
        for b in buffers {
            b.hash(&mut hasher);
        }
        let mut groups = self.groups.borrow_mut();
        let same = groups.entry(hasher.finish()).or_default();
        let found = same.iter().find(|(p, bs, _)| {
            p == pipeline
                && bs.len() == buffers.len()
                && bs.iter().zip(buffers).all(|(a, b)| a == *b)
        });
        if let Some((_, _, bind)) = found {
            return bind.clone();
        }
        let bind = group(&self.device, pipeline, buffers);
        same.push((
            pipeline.clone(),
            buffers.iter().map(|b| (*b).clone()).collect(),
            bind.clone(),
        ));
        bind
    }

    fn simple(
        &self,
        pass: &mut wgpu::ComputePass,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        (x, y): (u32, u32),
    ) {
        let bind = self.bound(pipeline, buffers);
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
            &[&f.size, &f.rgb[i], &f.input],
            (blocks(w), blocks(h)),
        );
        self.conv(pass, &net.extract, (&f.size, &[&f.input], &f.feats), (w, h));
        let last = layers.len() - 1;
        for (k, layer) in layers.iter().enumerate() {
            let (src, out) = if k % 2 == 0 {
                (&f.xa, &f.xb)
            } else {
                (&f.xb, &f.xa)
            };
            let out = if k == last { dst } else { out };
            let srcs: &[&wgpu::Buffer] = if k == 0 { &[&f.feats, prev] } else { &[src] };
            self.conv(pass, layer, (&f.size, srcs, out), (w, h));
        }
    }

    fn wanted_first(
        &mut self,
        frames: &[Vec<u8>],
        (w, h): (usize, usize),
        wanted: usize,
    ) -> Result<Vec<u8>> {
        let device = self.device.clone();
        checked(&device, || {
            let clock = Instant::now();
            self.load()?;
            self.make_frame((w, h), frames.len());
            if self.tiles.as_ref().is_none_or(|t| t.size != (w, h)) {
                self.make_tiles();
            }
            let f = self.frame.as_ref().expect("frame made");
            self.queue.write_buffer(
                &f.size,
                0,
                &words(&[w as u32, h as u32, w as u32, 0, 0, 0, 0, 0]),
            );
            for (i, rgb) in frames.iter().enumerate() {
                self.queue.write_buffer(&f.rgb[i], 0, rgb);
            }
            let net = self.net.as_ref().expect("loaded");
            let n = frames.len();
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for i in 0..=wanted {
                    let prev = if i == 0 { &f.zero } else { &f.states[i - 1] };
                    self.chain(&mut pass, &net.forward, (i, prev, &f.states[i]));
                }
                for i in (wanted..n).rev() {
                    let prev = if i + 1 == n {
                        &f.zero
                    } else {
                        &f.states[i + 1]
                    };
                    let dst = if i == wanted {
                        &f.back[0]
                    } else {
                        &f.states[i]
                    };
                    self.chain(&mut pass, &net.backward, (i, prev, dst));
                }
                self.conv(
                    &mut pass,
                    &net.fusion,
                    (&f.size, &[&f.states[wanted], &f.back[0]], &f.fused),
                    (w, h),
                );
                self.enlarge(&mut pass, wanted);
            }
            let t = self.tiles.as_ref().expect("tiles made");
            enc.copy_buffer_to_buffer(&t.out, 0, &t.read[0], 0, t.shown);
            let done = self.queue.submit([enc.finish()]);
            let first = self.take(0, done)?;
            crate::debug!(
                "upscale",
                "{n} frames at {w}x{h}: frame {wanted} in {:.0} ms",
                clock.elapsed().as_secs_f64() * 1e3
            );
            Ok(first)
        })
    }

    fn finish(&mut self) {
        let Some(Pending {
            wanted,
            size,
            mut outs,
            rest,
        }) = self.pending.take()
        else {
            return;
        };
        let device = self.device.clone();
        let done = checked(&device, || self.others(wanted, size, &mut outs)).map(|()| outs);
        rest(done);
    }

    fn others(&self, wanted: usize, (w, h): (usize, usize), outs: &mut [Vec<u8>]) -> Result<()> {
        let clock = Instant::now();
        let f = self.frame.as_ref().expect("frame made");
        let t = self.tiles.as_ref().expect("tiles made");
        let net = self.net.as_ref().expect("loaded");
        let n = outs.len();
        let ahead = [&f.states[wanted], &f.back[1]];
        let later = (wanted + 1..n).map(|i| (i, true));
        let earlier = (0..wanted).rev().map(|i| (i, false));
        let mut waiting = None;
        for (k, (i, forward)) in later.chain(earlier).enumerate() {
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let (fwd, bwd) = if forward {
                    let j = i - wanted;
                    let (prev, dst) = (ahead[(j + 1) % 2], ahead[j % 2]);
                    self.chain(&mut pass, &net.forward, (i, prev, dst));
                    (dst, &f.states[i])
                } else {
                    let j = wanted - i;
                    let (prev, dst) = (&f.back[(j + 1) % 2], &f.back[j % 2]);
                    self.chain(&mut pass, &net.backward, (i, prev, dst));
                    (&f.states[i], dst)
                };
                self.conv(
                    &mut pass,
                    &net.fusion,
                    (&f.size, &[fwd, bwd], &f.fused),
                    (w, h),
                );
                self.enlarge(&mut pass, i);
            }
            enc.copy_buffer_to_buffer(&t.out, 0, &t.read[k % 2], 0, t.shown);
            let done = self.queue.submit([enc.finish()]);
            if let Some((j, slot, done)) = waiting.replace((i, k % 2, done)) {
                outs[j] = self.take(slot, done)?;
            }
        }
        if let Some((j, slot, done)) = waiting {
            outs[j] = self.take(slot, done)?;
        }
        crate::debug!(
            "upscale",
            "{n} frames at {w}x{h}: the other frames in {:.0} ms",
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(())
    }

    fn enlarge(&self, pass: &mut wgpu::ComputePass, i: usize) {
        let f = self.frame.as_ref().expect("frame made");
        let t = self.tiles.as_ref().expect("tiles made");
        let net = self.net.as_ref().expect("loaded");
        for job in &t.jobs {
            let (tw, th) = (job.tw, job.th);
            self.conv(pass, &net.up1, (&job.size1, &[&f.fused], &t.s1), (tw, th));
            self.conv(
                pass,
                &net.up2,
                (&job.size2, &[&t.s1], &t.s2),
                (tw * 2, th * 2),
            );
            self.simple(
                pass,
                &self.pipelines.last,
                &[&job.place, &t.s2, &net.last, &f.rgb[i], &t.out],
                (blocks(job.cw), blocks(job.ch * 4)),
            );
        }
    }

    fn take(&self, slot: usize, done: wgpu::SubmissionIndex) -> Result<Vec<u8>> {
        let t = self.tiles.as_ref().expect("tiles made");
        let slice = t.read[slot].slice(..t.shown);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(done),
                timeout: None,
            })
            .map_err(gpu_error)?;
        rx.recv().map_err(gpu_error)?.map_err(gpu_error)?;
        let out = slice.get_mapped_range().map_err(gpu_error)?.to_vec();
        t.read[slot].unmap();
        Ok(out)
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

pub(crate) fn upscale(
    frames: &[Vec<u8>],
    size: (usize, usize),
    wanted: usize,
    rest: impl FnOnce(Result<Vec<Vec<u8>>>) + Send + 'static,
) -> Result<Vec<u8>> {
    let first = with_engine(|e| {
        e.finish();
        let first = e.wanted_first(frames, size, wanted);
        let Ok(first) = first else {
            e.frame = None;
            e.tiles = None;
            e.groups.get_mut().clear();
            return first;
        };
        let mut outs = vec![Vec::new(); frames.len()];
        outs[wanted].clone_from(&first);
        e.pending = Some(Pending {
            wanted,
            size,
            outs,
            rest: Box::new(rest),
        });
        Ok(first)
    })?;
    let _ = std::thread::Builder::new()
        .name("vidcut-upscale-rest".into())
        .spawn(settle);
    Ok(first)
}

pub(crate) fn settle() {
    let mut engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(Ok(e)) = engine.as_mut() {
        e.finish();
    }
}
