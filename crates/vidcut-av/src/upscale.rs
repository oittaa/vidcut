use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError, mpsc};

use vidcut_core::model::{Media, NoiseReduction, Project};
use vidcut_core::render::VideoPiece;
use wgpu::util::DeviceExt;

use crate::error::{Error, Result};
use crate::models::{self, Fetch, File, Need};

const RELEASE: &str = "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.5.0";

pub const GENERAL: File = File {
    name: "real-esrgan/realesr-general-x4v3.pth",
    title: "Real-ESRGAN upscaler",
    repo: "xinntao/Real-ESRGAN",
    revision: "v0.2.5.0",
    bytes: 4_885_111,
    sha256: "8dc7edb9ac80ccdc30c3a5dca6616509367f05fbc184ad95b731f05bece96292",
    gpu_gb: 0,
    languages: &[],
};

pub const GENTLE: File = File {
    name: "real-esrgan/realesr-general-wdn-x4v3.pth",
    title: "Real-ESRGAN upscaler, light denoise",
    repo: "xinntao/Real-ESRGAN",
    revision: "v0.2.5.0",
    bytes: 4_885_111,
    sha256: "1641f8c4464b9f097c9fdda5589273713f67cf59f3d909e0bd688f0cee269dca",
    gpu_gb: 0,
    languages: &[],
};

pub const SCALE: u32 = 4;
pub const LARGEST: u64 = 4096 * 2304;
const FEATURES: usize = 64;
const MIDDLE: usize = 32;
const LAYERS: usize = MIDDLE + 2;
const PADDED: usize = 8;
const OUT: usize = 3 * (SCALE * SCALE) as usize;
const REACH: usize = LAYERS;
const TILE: usize = 640;
const BLOCK: usize = 16;
const SHARED: u32 = ((8 * 18 * 18 + 8 * 9 * FEATURES) * 4) as u32;

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
    [GENERAL, GENTLE]
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: f.bytes,
        })
        .collect()
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    let missing: Vec<File> = [GENERAL, GENTLE]
        .into_iter()
        .filter(|f| !models::present(dir, f))
        .collect();
    if missing.is_empty() {
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
    for f in &missing {
        models::fetch(&fetch_of(f), dir)?;
    }
    Ok(())
}

pub fn unfit(m: &Media) -> Option<&'static str> {
    let v = m.video_as_read()?;
    if v.hdr.is_some() {
        return Some("HDR pictures");
    }
    let across = match v.sar {
        [n, d] if n > 0 && d > 0 => f64::from(n) / f64::from(d),
        _ => 1.0,
    };
    let shown = f64::from(v.width) * across * f64::from(v.height);
    (shown > LARGEST as f64).then_some("pictures larger than 4K")
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
            "the AI upscaler of Super Scale can't take {}: set their Upscaler to Classic, \
             or their Super Scale to Off",
            refused.join(", ")
        )));
    }
    get_ready(dir, download)?;
    with_engine(|_| Ok(()))
}

struct Weights {
    cin: usize,
    cout: usize,
    weights: Vec<f32>,
    bias: Vec<f32>,
    alpha: Option<Vec<f32>>,
}

fn tensors(path: &Path) -> Result<Vec<Vec<f32>>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    let mut out = Vec::new();
    while let Ok(mut entry) = zip.by_name(&format!("archive/data/{}", out.len())) {
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        out.push(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        );
    }
    Ok(out)
}

fn read(path: &Path) -> Result<Vec<Weights>> {
    let bad = || {
        Error::Invalid(format!(
            "{}: not the weights of Real-ESRGAN's compact network",
            path.display()
        ))
    };
    let mut it = tensors(path)?.into_iter();
    let mut out = Vec::new();
    for l in 0..LAYERS {
        let last = l == LAYERS - 1;
        let cin = if l == 0 { 3 } else { FEATURES };
        let cout = if last { OUT } else { FEATURES };
        let w = it
            .next()
            .filter(|w| w.len() == cout * cin * 9)
            .ok_or_else(bad)?;
        let bias = it.next().filter(|b| b.len() == cout).ok_or_else(bad)?;
        let alpha = if last {
            None
        } else {
            Some(it.next().filter(|a| a.len() == FEATURES).ok_or_else(bad)?)
        };
        let padded = if l == 0 { PADDED } else { cin };
        let mut weights = vec![0f32; padded * 9 * cout];
        for co in 0..cout {
            for ci in 0..cin {
                for k in 0..9 {
                    weights[(ci * 9 + k) * cout + co] = w[(co * cin + ci) * 9 + k];
                }
            }
        }
        out.push(Weights {
            cin: padded,
            cout,
            weights,
            bias,
            alpha,
        });
    }
    if it.next().is_some() {
        return Err(bad());
    }
    Ok(out)
}

fn mix(a: Vec<Weights>, b: &[Weights], s: f32) -> Vec<Weights> {
    let m = |x: &mut [f32], y: &[f32]| {
        for (p, q) in x.iter_mut().zip(y) {
            *p = *p * s + q * (1.0 - s);
        }
    };
    a.into_iter()
        .zip(b)
        .map(|(mut x, y)| {
            m(&mut x.weights, &y.weights);
            m(&mut x.bias, &y.bias);
            if let (Some(p), Some(q)) = (&mut x.alpha, &y.alpha) {
                m(p, q);
            }
            x
        })
        .collect()
}

fn conv_shader(cin: usize, cout: usize, prelu: bool) -> String {
    let groups = cout / 16;
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
                "            {{\n            let wb = (ci * 9u + {k}u) * (COUT / 4u) + g * 4u;\n",
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
            "    if (x0 + px + {j}u < size.w && y0 + py < size.h) {{\n        let o = ((y0 + py) * size.w + x0 + px + {j}u) * (COUT / 2u) + g * 8u;\n"
        );
        for q in 0..4 {
            write += &format!(
                "        {{\n        let co = g * 16u + {q}u * 4u;\n        var v = a{j}{q} + vec4<f32>(bias[co], bias[co + 1u], bias[co + 2u], bias[co + 3u]);\n"
            );
            if prelu {
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
const GROUPS: u32 = {groups}u;
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
var<workgroup> wts: array<vec4<f32>, {wcount}>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {{
    let g = li / 64u;
    let p = li % 64u;
    let px = (p % 4u) * 4u;
    let py = p / 4u;
    let x0 = wg.x * TILE;
    let y0 = wg.y * TILE;
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
        for (var i = li; i < CHUNK * 9u * COUT / 4u; i += 256u) {{
            wts[i] = weights[c0 * 9u * COUT / 4u + i];
        }}
        workgroupBarrier();
        if (g < GROUPS) {{
            for (var ci = 0u; ci < CHUNK; ci++) {{
{body}
            }}
        }}
        workgroupBarrier();
    }}
    _ = alpha[0];
    if (g >= GROUPS) {{
        return;
    }}
{write}
}}
"#,
        wcount = 8 * 9 * cout / 4,
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

const SHUFFLE: &str = r#"
struct Size { w: u32, h: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var<storage, read> conv: array<u32>;
@group(0) @binding(2) var<storage, read> rgba: array<u32>;
@group(0) @binding(3) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let ow = size.w * 4u;
    if (id.x >= ow || id.y >= size.h * 4u) {
        return;
    }
    let p = (id.y / 4u) * size.w + id.x / 4u;
    let sub = (id.y % 4u) * 4u + id.x % 4u;
    var base = unpack4x8unorm(rgba[p]);
    var rgb = vec3<f32>(0.0);
    for (var c = 0u; c < 3u; c++) {
        let ch = c * 16u + sub;
        let pair = unpack2x16float(conv[p * 24u + ch / 2u]);
        rgb[c] = select(pair.x, pair.y, ch % 2u == 1u) + base[c];
    }
    out[id.y * ow + id.x] = pack4x8unorm(vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
"#;

struct Layer {
    pipeline: usize,
    weights: wgpu::Buffer,
    bias: wgpu::Buffer,
    alpha: Option<wgpu::Buffer>,
}

struct Room {
    pixels: usize,
    size: wgpu::Buffer,
    rgba: wgpu::Buffer,
    input: wgpu::Buffer,
    ping: wgpu::Buffer,
    pong: wgpu::Buffer,
    out: wgpu::Buffer,
    read: wgpu::Buffer,
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
                read: (core.0.saturating_sub(REACH), (core.1 + REACH).min(n)),
            }
        })
        .collect()
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn gpu_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("the AI upscaler on the GPU: {e}"))
}

fn checked<T>(device: &wgpu::Device, run: impl FnOnce() -> Result<T>) -> Result<T> {
    let scopes = [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Validation,
        wgpu::ErrorFilter::Internal,
    ]
    .map(|f| device.push_error_scope(f));
    let out = run();
    let mut first = None;
    for scope in scopes.into_iter().rev() {
        if let Some(e) = pollster::block_on(scope.pop()) {
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(gpu_error(e)),
        None => out,
    }
}

struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    convs: Vec<((usize, usize, bool), wgpu::ComputePipeline)>,
    pack: wgpu::ComputePipeline,
    shuffle: wgpu::ComputePipeline,
    none: wgpu::Buffer,
    nets: Vec<(NoiseReduction, Vec<Layer>)>,
    room: Option<Room>,
}

impl Engine {
    fn start() -> std::result::Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU for the AI upscaler: {e}"))?;
        let limits = adapter.limits();
        if limits.max_compute_workgroup_storage_size < SHARED
            || limits.max_compute_invocations_per_workgroup < 256
        {
            return Err(format!(
                "{} has too little workgroup memory for the AI upscaler",
                adapter.get_info().name
            ));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vidcut-upscale"),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| format!("the GPU did not open for the AI upscaler: {e}"))?;
        let made = checked(&device, || {
            let pipeline = |source: String| {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("upscale"),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("upscale"),
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let convs: Vec<_> = [
                (PADDED, FEATURES, true),
                (FEATURES, FEATURES, true),
                (FEATURES, OUT, false),
            ]
            .into_iter()
            .map(|key| (key, pipeline(conv_shader(key.0, key.1, key.2))))
            .collect();
            let none = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("upscale-none"),
                contents: &[0; 4],
                usage: wgpu::BufferUsages::STORAGE,
            });
            Ok((convs, pipeline(PACK.into()), pipeline(SHUFFLE.into()), none))
        });
        let (convs, pack, shuffle, none) = made.map_err(|e| e.to_string())?;
        Ok(Self {
            device,
            queue,
            convs,
            pack,
            shuffle,
            none,
            nets: Vec::new(),
            room: None,
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

    fn net(&mut self, noise: NoiseReduction) -> Result<usize> {
        if let Some(i) = self.nets.iter().position(|(n, _)| *n == noise) {
            return Ok(i);
        }
        let dir = vidcut_core::dirs::models();
        get_ready(&dir, false)?;
        let general = read(&dir.join(GENERAL.name))?;
        let gentle = read(&dir.join(GENTLE.name))?;
        let layers = mix(general, &gentle, noise.strength())
            .iter()
            .map(|w| Layer {
                pipeline: self
                    .convs
                    .iter()
                    .position(|(key, _)| *key == (w.cin, w.cout, w.alpha.is_some()))
                    .expect("a pipeline for each layer"),
                weights: self.storage(&bytes(&w.weights)),
                bias: self.storage(&bytes(&w.bias)),
                alpha: w.alpha.as_ref().map(|a| self.storage(&bytes(a))),
            })
            .collect();
        self.nets.push((noise, layers));
        Ok(self.nets.len() - 1)
    }

    fn make_room(&mut self, pixels: usize) {
        if self.room.as_ref().is_some_and(|r| r.pixels >= pixels) {
            return;
        }
        let buffer = |size: usize, usage| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("upscale"),
                size: size as u64,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let shown = pixels * (SCALE * SCALE) as usize * 4;
        self.room = Some(Room {
            pixels,
            size: buffer(8, U::UNIFORM | U::COPY_DST),
            rgba: buffer(pixels * 4, U::STORAGE | U::COPY_DST),
            input: buffer(pixels * PADDED * 2, U::STORAGE),
            ping: buffer(pixels * FEATURES * 2, U::STORAGE),
            pong: buffer(pixels * FEATURES * 2, U::STORAGE),
            out: buffer(shown, U::STORAGE | U::COPY_SRC),
            read: buffer(shown, U::MAP_READ | U::COPY_DST),
        });
    }

    fn binds(&self, net: usize) -> (wgpu::BindGroup, Vec<wgpu::BindGroup>, wgpu::BindGroup) {
        let room = self.room.as_ref().expect("room made");
        let group = |pipeline: &wgpu::ComputePipeline, buffers: &[&wgpu::Buffer]| {
            let entries: Vec<wgpu::BindGroupEntry> = buffers
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect();
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            })
        };
        let pack = group(&self.pack, &[&room.size, &room.rgba, &room.input]);
        let convs = self.nets[net]
            .1
            .iter()
            .enumerate()
            .map(|(l, layer)| {
                let src = match l {
                    0 => &room.input,
                    l if l % 2 == 1 => &room.ping,
                    _ => &room.pong,
                };
                let dst = if l % 2 == 0 { &room.ping } else { &room.pong };
                group(
                    &self.convs[layer.pipeline].1,
                    &[
                        &room.size,
                        src,
                        &layer.weights,
                        &layer.bias,
                        layer.alpha.as_ref().unwrap_or(&self.none),
                        dst,
                    ],
                )
            })
            .collect();
        let last = if LAYERS % 2 == 1 {
            &room.ping
        } else {
            &room.pong
        };
        let shuffle = group(&self.shuffle, &[&room.size, last, &room.rgba, &room.out]);
        (pack, convs, shuffle)
    }

    fn run(
        &mut self,
        rgb: &[u8],
        (w, h): (usize, usize),
        noise: NoiseReduction,
    ) -> Result<Vec<u8>> {
        let device = self.device.clone();
        checked(&device, || {
            let net = self.net(noise)?;
            let (cols, rows) = (spans(w), spans(h));
            let widest = cols.iter().map(|s| s.read.1 - s.read.0).max().unwrap_or(0);
            let tallest = rows.iter().map(|s| s.read.1 - s.read.0).max().unwrap_or(0);
            self.make_room(widest * tallest);
            let binds = self.binds(net);
            let scale = SCALE as usize;
            let mut out = vec![0u8; w * scale * h * scale * 3];
            for row in &rows {
                for col in &cols {
                    self.tile(rgb, w, (col, row), (net, &binds), &mut out)?;
                }
            }
            Ok(out)
        })
    }

    fn tile(
        &self,
        rgb: &[u8],
        w: usize,
        (col, row): (&Span, &Span),
        (net, (pack, convs, shuffle)): (
            usize,
            &(wgpu::BindGroup, Vec<wgpu::BindGroup>, wgpu::BindGroup),
        ),
        out: &mut [u8],
    ) -> Result<()> {
        let room = self.room.as_ref().expect("room made");
        let (tw, th) = (col.read.1 - col.read.0, row.read.1 - row.read.0);
        let mut rgba = Vec::with_capacity(tw * th * 4);
        for y in row.read.0..row.read.1 {
            let line = &rgb[(y * w + col.read.0) * 3..(y * w + col.read.1) * 3];
            for px in line.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        let size: Vec<u8> = [tw as u32, th as u32]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        self.queue.write_buffer(&room.size, 0, &size);
        self.queue.write_buffer(&room.rgba, 0, &rgba);
        let scale = SCALE as usize;
        let (ow, oh) = (tw * scale, th * scale);
        let shown = (ow * oh * 4) as u64;
        let blocks = |n: usize| n.div_ceil(BLOCK) as u32;
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pack);
            pass.set_bind_group(0, pack, &[]);
            pass.dispatch_workgroups(blocks(tw), blocks(th), 1);
            for (layer, bind) in self.nets[net].1.iter().zip(convs) {
                pass.set_pipeline(&self.convs[layer.pipeline].1);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(blocks(tw), blocks(th), 1);
            }
            pass.set_pipeline(&self.shuffle);
            pass.set_bind_group(0, shuffle, &[]);
            pass.dispatch_workgroups(blocks(ow), blocks(oh), 1);
        }
        enc.copy_buffer_to_buffer(&room.out, 0, &room.read, 0, shown);
        self.queue.submit([enc.finish()]);
        let slice = room.read.slice(..shown);
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
            let into = w * scale * 3;
            for y in (row.core.0 - row.read.0) * scale..(row.core.1 - row.read.0) * scale {
                let to = (row.read.0 * scale + y) * into + col.core.0 * scale * 3;
                let first = (col.core.0 - col.read.0) * scale;
                let count = (col.core.1 - col.core.0) * scale;
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
        room.read.unmap();
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

pub(crate) fn upscale(rgb: &[u8], size: (usize, usize), noise: NoiseReduction) -> Result<Vec<u8>> {
    with_engine(|e| e.run(rgb, size, noise))
}
