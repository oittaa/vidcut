use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vidcut_core::catalog::{self, Measure};
use vidcut_core::model::{Media, Project, Retime};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::compute::Gpu;
use crate::error::{Error, Result};
use crate::models::{self, Fetch, Need};

mod ema;
mod mamba;
mod ops;
mod rife;

use ops::{Kernels, Ops, View, tensor};

const WHAT: &str = "frame interpolation";
const SHARED: u32 = 16 * 1024;
const UHD: (u32, u32) = (3840, 2160);

fn model(r: Retime) -> Option<&'static catalog::Model> {
    match r {
        Retime::Rife => Some(&catalog::RIFE),
        Retime::EmaVfi => Some(&catalog::EMA_VFI),
        Retime::Vfimamba => Some(&catalog::VFIMAMBA),
        Retime::Nearest | Retime::FrameBlend => None,
    }
}

pub fn of(id: &str) -> Option<Retime> {
    Retime::ALL
        .into_iter()
        .find(|r| model(*r).is_some_and(|m| m.id == id))
}

fn files(r: Retime) -> &'static [catalog::File] {
    model(r).map_or(&[], |m| m.files)
}

pub fn needed(dir: &Path, r: Retime) -> Vec<Need> {
    files(r)
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        })
        .collect()
}

pub fn get_ready(dir: &Path, r: Retime, download: bool) -> Result<()> {
    let missing = needed(dir, r);
    if missing.is_empty() {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} isn't on this computer yet: pass download (--download) to download it into {}, \
             checked against its SHA-256",
            models::listed(&missing),
            dir.display()
        )));
    }
    models::network();
    for f in files(r).iter().filter(|f| !models::present(dir, f)) {
        models::fetch(&Fetch::of(f), dir)?;
    }
    Ok(())
}

pub fn inspect(dir: &Path, r: Retime, download: bool) -> Result<()> {
    get_ready(dir, r, download)?;
    models::verify(dir, files(r))?;
    with_engine(|e| e.net(r).map(drop))
}

pub fn unfit(m: &Media, r: Retime) -> Option<&'static str> {
    if r.is_nearest() {
        return None;
    }
    let hdr = m.video_as_read()?.hdr.is_some();
    let refused = match model(r) {
        Some(model) => !catalog::check(model, &[(Measure::HdrPictures, f64::from(u8::from(hdr)))])
            .refusals
            .is_empty(),
        None => hdr,
    };
    refused.then_some("HDR pictures")
}

pub fn check<'a>(
    p: &Project,
    plans: impl IntoIterator<Item = &'a RenderPlan>,
    dir: &Path,
    download: bool,
) -> Result<()> {
    let mut used: Vec<(Retime, String)> = Vec::new();
    for plan in plans {
        for piece in &plan.video {
            for (layer, _) in piece.layers() {
                if let VideoPiece::Source { media, retime, .. } = layer
                    && !retime.is_nearest()
                    && !used.iter().any(|(r, m)| r == retime && m == media)
                {
                    used.push((*retime, media.clone()));
                }
            }
        }
    }
    let refused: Vec<String> = used
        .iter()
        .filter_map(|(r, id)| {
            let m = p.media(id)?;
            unfit(m, *r).map(|why| format!("{} ({why}, {})", m.name, r.title()))
        })
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "Retime Process takes SDR pictures for now, so it can't take {}: set their clips' \
             Retime Process to Nearest",
            refused.join(", ")
        )));
    }
    let mut models: Vec<Retime> = used.iter().map(|(r, _)| *r).filter(Retime::ai).collect();
    models.sort_by_key(|r| *r as u8);
    models.dedup();
    for r in models {
        get_ready(dir, r, download)?;
    }
    Ok(())
}

enum Net {
    Rife(Box<rife::Rife>),
    Ema(Box<ema::Ema>),
    Mamba(Box<mamba::Mamba>),
}

struct Side<F> {
    img: View,
    feats: F,
    down: Option<(View, F)>,
    pred: View,
}

struct Shared<S> {
    full: S,
    down: Option<S>,
    up: Option<(View, View)>,
    refine: ema::Refine,
    ds: f32,
    pad: (u32, u32),
}

struct EmaState {
    sides: Vec<Side<ema::Features>>,
    shared: Shared<ema::Scratch>,
    flow: ema::Flow,
}

struct MambaState {
    sides: Vec<Side<mamba::Features>>,
    shared: Shared<mamba::Scratch>,
    flow: mamba::Flow,
    locals: mamba::Locals,
}

enum State {
    Rife(Box<rife::Pair>),
    Ema(Box<EmaState>),
    Mamba(Box<MambaState>),
    Blend(View),
}

fn motion_scale((w, h): (u32, u32)) -> f32 {
    if w >= UHD.0 || h >= UHD.1 {
        0.25
    } else if w >= 1920 || h >= 1080 {
        0.5
    } else {
        1.0
    }
}

fn centered((w, h): (u32, u32), step: u32) -> ((u32, u32), (u32, u32)) {
    let (pw, ph) = (w.next_multiple_of(step), h.next_multiple_of(step));
    ((pw, ph), ((ph - h) / 2, (pw - w) / 2))
}

struct Layout {
    padded: (u32, u32),
    small: (u32, u32),
    pad: (u32, u32),
    ds: f32,
}

fn layout(size: (u32, u32)) -> Layout {
    let ds = motion_scale(size);
    let (padded, pad) = centered(size, (32.0 / ds) as u32);
    let small = ((padded.0 as f32 * ds) as u32, (padded.1 as f32 * ds) as u32);
    Layout {
        padded,
        small,
        pad,
        ds,
    }
}

fn sides<F>(gpu: &Gpu, l: &Layout, feats: impl Fn((u32, u32)) -> F) -> Vec<Side<F>> {
    let (pw, ph) = l.padded;
    (0..2)
        .map(|_| Side {
            img: tensor(gpu, 2, ph, pw, 3),
            feats: feats(l.padded),
            down: (l.ds < 1.0).then(|| (tensor(gpu, 2, l.small.1, l.small.0, 3), feats(l.small))),
            pred: tensor(gpu, 1, ph, pw, 3),
        })
        .collect()
}

fn shared<S>(gpu: &Gpu, l: &Layout, scratch: impl Fn((u32, u32)) -> S) -> Shared<S> {
    let (pw, ph) = l.padded;
    Shared {
        full: scratch(l.padded),
        down: (l.ds < 1.0).then(|| scratch(l.small)),
        up: (l.ds < 1.0).then(|| (tensor(gpu, 1, ph, pw, 4), tensor(gpu, 1, ph, pw, 1))),
        refine: ema::refine_for(gpu, l.padded, 64),
        ds: l.ds,
        pad: l.pad,
    }
}

fn load_sides<F>(
    o: &mut Ops,
    sides: &[Side<F>],
    (a, b): (&wgpu::Buffer, &wgpu::Buffer),
    size: (u32, u32),
    pad: (u32, u32),
) {
    for (i, side) in sides.iter().enumerate() {
        o.load_turned(a, size, pad, true, i == 1, &side.img.frame(0));
        o.load_turned(b, size, pad, true, i == 1, &side.img.frame(1));
        if let Some((img, _)) = &side.down {
            o.scaled(&side.img, img, img.w as f32 / side.img.w as f32, 1.0, 0);
        }
    }
}

fn upscaled<S>(o: &mut Ops, sh: &Shared<S>, flow: &View, mask: &View) -> (View, View) {
    match &sh.up {
        Some((f, m)) => {
            o.scaled(flow, f, 1.0 / sh.ds, 1.0 / sh.ds, 4);
            o.scaled(mask, m, 1.0 / sh.ds, 1.0, 0);
            (f.clone(), m.clone())
        }
        None => (flow.clone(), mask.clone()),
    }
}

fn predicted<S>(o: &mut Ops, sh: &Shared<S>, mask: &View, side_pred: &View) {
    let r = &sh.refine;
    o.predict(
        &r.warped.part(0, 3),
        &r.warped.part(3, 3),
        mask,
        &r.out,
        side_pred,
    );
}

struct Held {
    key: (PathBuf, i64, i64),
    retime: Retime,
    size: (u32, u32),
    state: State,
    out: wgpu::Buffer,
}

struct Engine {
    gpu: Gpu,
    kernels: Kernels,
    nets: Vec<(Retime, Net)>,
    held: Option<Held>,
}

static ENGINE: Mutex<Option<std::result::Result<Engine, String>>> = Mutex::new(None);
static USERS: Mutex<usize> = Mutex::new(0);
static DUMPED: AtomicU64 = AtomicU64::new(0);

fn engine() -> MutexGuard<'static, Option<std::result::Result<Engine, String>>> {
    ENGINE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn with_engine<T>(run: impl FnOnce(&mut Engine) -> Result<T>) -> Result<T> {
    let mut slot = engine();
    let e = match slot.get_or_insert_with(Engine::start) {
        Ok(e) => e,
        Err(why) => return Err(Error::Invalid(why.clone())),
    };
    let device = e.gpu.device.clone();
    crate::compute::checked(&device, WHAT, || run(e))
}

pub(crate) struct User;

impl User {
    pub(crate) fn of(r: Retime) -> Option<Self> {
        if r.is_nearest() {
            return None;
        }
        *USERS.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Some(Self)
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let last = {
            let mut n = USERS.lock().unwrap_or_else(PoisonError::into_inner);
            *n = n.saturating_sub(1);
            *n == 0
        };
        if last {
            *engine() = None;
        }
    }
}

impl Engine {
    fn start() -> std::result::Result<Self, String> {
        let gpu = Gpu::start(WHAT, SHARED)?;
        let kernels = crate::compute::checked(&gpu.device, WHAT, || Ok(Kernels::new(&gpu)))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            gpu,
            kernels,
            nets: Vec::new(),
            held: None,
        })
    }

    fn loaded(&self, r: Retime) -> Option<&Net> {
        self.nets.iter().find(|(k, _)| *k == r).map(|(_, n)| n)
    }

    fn net(&mut self, r: Retime) -> Result<&Net> {
        if self.loaded(r).is_none() {
            let dir = vidcut_core::dirs::models();
            get_ready(&dir, r, false)?;
            let m =
                model(r).ok_or_else(|| Error::Invalid(format!("{} has no model", r.title())))?;
            let path = dir.join(m.files[0].name);
            let net = match r {
                Retime::Rife => Net::Rife(Box::new(rife::load(&self.gpu, &path, m.title)?)),
                Retime::EmaVfi => Net::Ema(Box::new(ema::load(&self.gpu, &path, m.title)?)),
                _ => Net::Mamba(Box::new(mamba::load(&self.gpu, &path, m.title)?)),
            };
            self.nets.push((r, net));
        }
        Ok(self.loaded(r).expect("loaded"))
    }

    fn submit(&self, run: impl FnOnce(&mut Ops)) {
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            let mut o = Ops {
                gpu: &self.gpu,
                k: &self.kernels,
                pass: &mut pass,
            };
            run(&mut o);
        }
        self.gpu.flush();
        self.gpu.queue.submit([enc.finish()]);
    }

    fn upload(&self, rgb: &[u8]) -> wgpu::Buffer {
        let mut data = rgb.to_vec();
        data.resize(rgb.len().next_multiple_of(4), 0);
        self.gpu.upload(&data)
    }

    fn hold(
        &mut self,
        r: Retime,
        key: &(PathBuf, i64, i64),
        a: &[u8],
        b: &[u8],
        size: (u32, u32),
    ) -> Result<()> {
        if self
            .held
            .as_ref()
            .is_some_and(|h| h.key == *key && h.retime == r && h.size == size)
        {
            return Ok(());
        }
        self.held = None;
        if r.ai() {
            self.net(r)?;
        }
        let (w, h) = size;
        let ua = self.upload(a);
        let ub = self.upload(b);
        let out = self
            .gpu
            .buffer((w as usize * h as usize * 3).next_multiple_of(4));
        let gpu = &self.gpu;
        let state = match (r, self.loaded(r)) {
            (Retime::FrameBlend, _) => {
                let img = tensor(gpu, 2, h, w, 3);
                self.submit(|o| {
                    o.load(&ua, size, (0, 0), false, &img.frame(0));
                    o.load(&ub, size, (0, 0), false, &img.frame(1));
                });
                State::Blend(img)
            }
            (_, Some(Net::Rife(net))) => {
                let scale = if w >= UHD.0 || h >= UHD.1 { 0.5 } else { 1.0 };
                let pair = net.pair(gpu, rife::padded(size, scale), scale);
                self.submit(|o| {
                    o.load(&ua, size, (0, 0), false, &pair.img.frame(0));
                    o.load(&ub, size, (0, 0), false, &pair.img.frame(1));
                    net.features(o, &pair);
                });
                State::Rife(Box::new(pair))
            }
            (_, Some(Net::Ema(net))) => {
                let l = layout(size);
                let sides = sides(gpu, &l, |s| ema::features_for(gpu, s));
                let sh = shared(gpu, &l, |s| ema::scratch_for(gpu, s));
                let flow = ema::flow_for(gpu, l.small);
                self.submit(|o| {
                    load_sides(o, &sides, (&ua, &ub), size, l.pad);
                    for side in &sides {
                        net.features(o, &side.img, &side.feats, &sh.full);
                        if let (Some((img, feats)), Some(x)) = (&side.down, &sh.down) {
                            net.features(o, img, feats, x);
                        }
                    }
                });
                State::Ema(Box::new(EmaState {
                    sides,
                    shared: sh,
                    flow,
                }))
            }
            (_, Some(Net::Mamba(net))) => {
                let l = layout(size);
                let sides = sides(gpu, &l, |s| mamba::features_for(gpu, s));
                let sh = shared(gpu, &l, |s| mamba::scratch_for(gpu, s));
                let flow = mamba::flow_for(gpu, l.small);
                let locals = mamba::locals_for(gpu, l.padded);
                self.submit(|o| {
                    load_sides(o, &sides, (&ua, &ub), size, l.pad);
                    for side in &sides {
                        net.features(o, &side.img, &side.feats, &sh.full);
                        if let (Some((img, feats)), Some(x)) = (&side.down, &sh.down) {
                            net.features(o, img, feats, x);
                        }
                    }
                });
                State::Mamba(Box::new(MambaState {
                    sides,
                    shared: sh,
                    flow,
                    locals,
                }))
            }
            _ => return Err(Error::Invalid(format!("{} has no model", r.title()))),
        };
        self.held = Some(Held {
            key: key.clone(),
            retime: r,
            size,
            state,
            out,
        });
        Ok(())
    }

    fn frame(&self, r: Retime, t: f32) -> Result<Vec<u8>> {
        let held = self.held.as_ref().expect("held");
        let size = held.size;
        self.submit(|o| match (&held.state, self.loaded(r)) {
            (State::Blend(img), _) => {
                let mixed = tensor(o.gpu, 1, size.1, size.0, 3);
                o.combine(&img.frame(0), &img.frame(1), &mixed, 1.0 - t, t);
                o.store(&mixed, (0, 0), size, &held.out);
            }
            (State::Rife(pair), Some(Net::Rife(net))) => {
                net.frame(o, pair, t);
                net.finish(o, pair, size, &held.out);
            }
            (State::Ema(state), Some(Net::Ema(net))) => {
                let (sides, sh, fl) = (&state.sides, &state.shared, &state.flow);
                for side in sides {
                    let (img, feats) = match &side.down {
                        Some((img, feats)) => (img, feats),
                        None => (&side.img, &side.feats),
                    };
                    net.flow(o, img, feats, fl, t);
                    let (flow, mask) = upscaled(o, sh, &fl.flow, &fl.mask);
                    net.refine(o, &side.img, &side.feats.af, &flow, &mask, &sh.refine);
                    predicted(o, sh, &mask, &side.pred);
                }
                o.store_both(
                    &sides[0].pred,
                    Some(&sides[1].pred),
                    sh.pad,
                    size,
                    &held.out,
                );
            }
            (State::Mamba(state), Some(Net::Mamba(net))) => {
                let (sides, sh, fl, locals) =
                    (&state.sides, &state.shared, &state.flow, &state.locals);
                for side in sides {
                    let (img, af) = match &side.down {
                        Some((img, feats)) => (img, &feats.af),
                        None => (&side.img, &side.feats.af),
                    };
                    net.flow(o, img, af, fl, t);
                    let (flow, mask) = upscaled(o, sh, &fl.flow, &fl.mask);
                    net.locals(o, &side.img, &flow, &mask, locals);
                    ema::refine(
                        o,
                        &net.unet,
                        &side.img,
                        &side.feats.af,
                        &flow,
                        &mask,
                        &sh.refine,
                    );
                    predicted(o, sh, &mask, &side.pred);
                }
                o.store_both(
                    &sides[0].pred,
                    Some(&sides[1].pred),
                    sh.pad,
                    size,
                    &held.out,
                );
            }
            _ => {}
        });
        let bytes = size.0 as u64 * size.1 as u64 * 3;
        let mut out = self.gpu.read(&held.out, 0, bytes.next_multiple_of(4))?;
        out.truncate(bytes as usize);
        Ok(out)
    }
}

fn dump(name: &str, size: (u32, u32), rgb: &[u8]) -> Result<()> {
    let Some(dir) = std::env::var_os("VIDCUT_INTERPOLATE_DUMP") else {
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    crate::npy::write_bytes(
        &dir.join(format!("{name}.npy")),
        &[size.1 as usize, size.0 as usize, 3],
        rgb,
    )
}

pub(crate) fn between(
    r: Retime,
    key: &(PathBuf, i64, i64),
    a: &[u8],
    b: &[u8],
    size: (u32, u32),
    t: f64,
) -> Result<Vec<u8>> {
    let made = with_engine(|e| {
        e.hold(r, key, a, b, size)?;
        e.frame(r, t as f32)
    })?;
    if std::env::var_os("VIDCUT_INTERPOLATE_DUMP").is_some() {
        let n = DUMPED.fetch_add(1, Ordering::Relaxed);
        let name = format!("{n:04}");
        dump(&format!("{name}-a"), size, a)?;
        dump(&format!("{name}-b"), size, b)?;
        dump(
            &format!("{name}-out-{t:.4}-{}", r.title().replace(' ', "")),
            size,
            &made,
        )?;
    }
    Ok(made)
}
