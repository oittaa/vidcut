use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vidcut_core::model::Hdr;

use super::shared::HeldMemory;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, frame};

#[derive(Clone)]
pub(crate) struct Held {
    buffer: wgpu::Buffer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    gpu: u64,
    memory: Option<Arc<HeldMemory>>,
}

static MARK: u8 = 0;

static SHARES: AtomicBool = AtomicBool::new(true);

impl Held {
    pub(crate) fn new(
        buffer: wgpu::Buffer,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
        gpu: u64,
    ) -> Self {
        Self {
            buffer,
            device: device.clone(),
            queue: queue.clone(),
            gpu,
            memory: None,
        }
    }

    pub(crate) fn imported_on(
        &self,
        device: &wgpu::Device,
    ) -> Option<std::result::Result<wgpu::Buffer, String>> {
        let memory = self.memory.as_ref()?;
        Some(super::shared::held_on(device, &self.device, memory))
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn on(&self, gpu: &super::Gpu) -> bool {
        self.gpu == gpu.id
    }

    pub(crate) fn read(&self, len: usize) -> Result<Vec<u8>> {
        let size = self.buffer.size();
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut held picture"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging, 0, size);
        let done = self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let failed = |e: &dyn std::fmt::Display| {
            Error::Invalid(format!("a picture didn't come down from the GPU: {e}"))
        };
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(done),
                timeout: None,
            })
            .map_err(|e| failed(&e))?;
        rx.recv().map_err(|e| failed(&e))?.map_err(|e| failed(&e))?;
        let mut out = staging
            .slice(..)
            .get_mapped_range()
            .map_err(|e| failed(&e))?
            .to_vec();
        staging.unmap();
        out.truncate(len);
        Ok(out)
    }
}

pub(crate) fn picture(
    held: Held,
    (w, h): (u32, u32),
    pts: Option<i64>,
    (deep, hdr): (bool, Option<Hdr>),
) -> Result<frame::Video> {
    let (pixel, bytes) = rgb_pixel(deep);
    let mut f = frame::Video::empty();
    // SAFETY: fresh `f` takes one zeroed row at no stride.
    unsafe {
        let raw = f.as_mut_ptr();
        (*raw).format = ffi::AVPixelFormat::from(pixel) as i32;
        (*raw).width = w as i32;
        (*raw).height = h as i32;
        let row = ffi::av_buffer_allocz(w as usize * bytes);
        if row.is_null() {
            return Err(Error::Invalid("no memory for a picture on the GPU".into()));
        }
        (*raw).buf[0] = row;
        (*raw).data[0] = (*row).data;
        (*raw).linesize[0] = 0;
        (*raw).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
    }
    super::carried::carry(&mut f, held, &MARK)?;
    tag(&mut f, hdr);
    f.set_pts(pts);
    Ok(f)
}

pub(crate) fn rgb_pixel(deep: bool) -> (ffmpeg::format::Pixel, usize) {
    match deep {
        true => (ffmpeg::format::Pixel::RGB48LE, 6),
        false => (ffmpeg::format::Pixel::RGB24, 3),
    }
}

pub(crate) fn eight(rgb: &[u8], deep: bool) -> Cow<'_, [u8]> {
    match deep {
        true => Cow::Owned(
            rgb.as_chunks::<2>()
                .0
                .iter()
                .map(|c| ((u32::from(u16::from_le_bytes(*c)) + 128) / 257) as u8)
                .collect(),
        ),
        false => Cow::Borrowed(rgb),
    }
}

pub(crate) fn sixteen(rgb: &[u8]) -> Vec<u8> {
    rgb.iter()
        .flat_map(|v| (u16::from(*v) * 257).to_le_bytes())
        .collect()
}

pub(crate) fn tag(f: &mut frame::Video, hdr: Option<Hdr>) {
    use ffmpeg::util::color::{Primaries, Range, Space, TransferCharacteristic};
    let (primaries, transfer) = match hdr {
        None => (Primaries::BT709, TransferCharacteristic::BT709),
        Some(Hdr::Pq) => (Primaries::BT2020, TransferCharacteristic::SMPTE2084),
        Some(Hdr::Hlg) => (Primaries::BT2020, TransferCharacteristic::ARIB_STD_B67),
    };
    f.set_color_range(Range::JPEG);
    f.set_color_space(Space::RGB);
    f.set_color_primaries(primaries);
    f.set_color_transfer_characteristic(transfer);
}

pub(crate) fn deep(f: &frame::Video) -> bool {
    f.format() == ffmpeg::format::Pixel::RGB48LE
}

fn bytes(f: &frame::Video) -> usize {
    f.width() as usize * f.height() as usize * rgb_pixel(deep(f)).1
}

pub(crate) fn of(f: &frame::Video) -> Option<&Held> {
    super::carried::carried(f, &MARK)
}

pub(crate) fn held(f: &frame::Video) -> Result<&Held> {
    of(f).ok_or_else(|| Error::Invalid("the picture isn't held on the GPU".into()))
}

pub(crate) fn rgb(f: &frame::Video) -> Result<Vec<u8>> {
    let bytes = held(f)?.read(bytes(f))?;
    crate::copies::down(f);
    Ok(bytes)
}

pub(crate) fn stop_sharing(why: &str) {
    if SHARES.swap(false, Ordering::Relaxed) {
        eprintln!("vidcut: AI pictures come down from the GPU to be shown: {why}");
    }
}

pub(crate) fn share_again() {
    SHARES.store(true, Ordering::Relaxed);
}

pub(crate) fn shared(f: &frame::Video) -> Option<frame::Video> {
    let held = of(f)?;
    if !SHARES.load(Ordering::Relaxed) || !for_the_viewer(held) {
        return None;
    }
    match share(f, held) {
        Ok(out) => Some(out),
        Err(e) => {
            stop_sharing(&e.to_string());
            None
        }
    }
}

fn for_the_viewer(held: &Held) -> bool {
    super::offscreen().is_ok_and(|o| {
        let adapter = crate::gpus::adapter_id(
            o.adapter.vendor,
            o.adapter.device,
            &o.adapter.device_pci_bus_id,
        );
        held.on(&o.gpu)
            && super::shared::possible(&o.gpu.device)
            && crate::player::viewer_on(&adapter)
    })
}

fn share(f: &frame::Video, held: &Held) -> Result<frame::Video> {
    let size = held.buffer.size();
    let (buffer, memory) =
        super::shared::held_buffer(&held.device, size).map_err(Error::Invalid)?;
    let mut encoder = held.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&held.buffer, 0, &buffer, 0, size);
    let done = held.queue.submit([encoder.finish()]);
    held.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(done),
            timeout: None,
        })
        .map_err(|e| Error::Invalid(format!("the GPU did not finish a picture: {e}")))?;
    let shared = Held {
        buffer,
        device: held.device.clone(),
        queue: held.queue.clone(),
        gpu: held.gpu,
        memory: Some(Arc::new(memory)),
    };
    let mut out = crate::export::video::frame_ref(f)?;
    super::carried::carry(&mut out, shared, &MARK)?;
    Ok(out)
}

pub(crate) fn in_memory(f: &frame::Video) -> Result<frame::Video> {
    let rgb = rgb(f)?;
    let (pixel, size) = rgb_pixel(deep(f));
    let mut out = crate::buffers::unpacked(pixel, (f.width(), f.height()), size, &rgb);
    // SAFETY: both frames are valid.
    let ret = unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), f.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "copying a frame's properties".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    super::carried::let_go_of(&mut out);
    Ok(out)
}

pub(crate) fn small_luma(
    gpu: &super::Gpu,
    f: &frame::Video,
    (ow, oh): (u32, u32),
) -> Result<Vec<u8>> {
    let held = held(f)?;
    let size = (f.width(), f.height());
    if !held.on(gpu) || held.buffer().size() < bytes(f) as u64 {
        return Err(Error::Invalid(
            "the picture to measure isn't held on the drawing's GPU".into(),
        ));
    }
    let small = gpu
        .checked(|| {
            gpu.held.luma(
                (&gpu.device, &gpu.queue),
                (held.buffer(), deep(f)),
                size,
                (ow, oh),
            )
        })
        .map_err(Error::Invalid)?;
    Held::new(small, (&gpu.device, &gpu.queue), gpu.id).read(ow as usize * oh as usize)
}

fn half_float(picture: &wgpu::Texture) -> bool {
    picture.format() == wgpu::TextureFormat::Rgba16Float
}

fn sixteen_bit(encoded: &wgpu::Texture) -> bool {
    encoded.format() == wgpu::TextureFormat::Rgba16Uint
}

fn packed_words(encoded: &wgpu::Texture) -> u64 {
    let per_word = if sixteen_bit(encoded) { 2 } else { 4 };
    (u64::from(encoded.width()) * u64::from(encoded.height()) * 3).div_ceil(per_word)
}

pub(super) struct Pipes {
    pack: wgpu::ComputePipeline,
    pack16: wgpu::ComputePipeline,
    unpack: wgpu::ComputePipeline,
    unpack16: wgpu::ComputePipeline,
    luma: wgpu::ComputePipeline,
    luma16: wgpu::ComputePipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut rgb"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rgb.wgsl").into()),
        });
        let made = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut rgb"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            pack: made("pack"),
            pack16: made("pack16"),
            unpack: made("unpack"),
            unpack16: made("unpack16"),
            luma: made("luma"),
            luma16: made("luma16"),
        }
    }

    pub(super) fn pack(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        encoded: &wgpu::Texture,
    ) -> wgpu::Buffer {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut held picture"),
            size: packed_words(encoded) * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.pack_into(device, encoder, encoded, &buffer);
        buffer
    }

    pub(super) fn pack_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        encoded: &wgpu::Texture,
        buffer: &wgpu::Buffer,
    ) {
        let words = packed_words(encoded);
        let view = encoded.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut rgb"),
            layout: &self.packer(encoded).get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        let groups = words.div_ceil(256) as u32;
        let across = groups.clamp(1, 65535);
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(self.packer(encoded));
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(across, groups.div_ceil(across), 1);
        }
    }

    pub(super) fn unpack(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        rgb: &wgpu::Buffer,
        picture: &wgpu::Texture,
    ) {
        let view = picture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut rgb"),
            layout: &self.unpacker(picture).get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rgb.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: if half_float(picture) { 6 } else { 3 },
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(self.unpacker(picture));
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(
            picture.width().div_ceil(16),
            picture.height().div_ceil(16),
            1,
        );
    }

    fn packer(&self, encoded: &wgpu::Texture) -> &wgpu::ComputePipeline {
        if sixteen_bit(encoded) {
            &self.pack16
        } else {
            &self.pack
        }
    }

    fn unpacker(&self, picture: &wgpu::Texture) -> &wgpu::ComputePipeline {
        if half_float(picture) {
            &self.unpack16
        } else {
            &self.unpack
        }
    }

    fn lumas(&self, deep: bool) -> &wgpu::ComputePipeline {
        if deep { &self.luma16 } else { &self.luma }
    }

    fn luma(
        &self,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
        (rgb, deep): (&wgpu::Buffer, bool),
        (w, h): (u32, u32),
        (ow, oh): (u32, u32),
    ) -> wgpu::Buffer {
        let words = (u64::from(ow) * u64::from(oh)).div_ceil(4);
        let small = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut small luma"),
            size: words * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sizes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut small luma"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&sizes, 0, &[w, h, ow, oh].map(u32::to_le_bytes).concat());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut rgb"),
            layout: &self.lumas(deep).get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rgb.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: small.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: sizes.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(self.lumas(deep));
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(words.div_ceil(64) as u32, 1, 1);
        }
        queue.submit([encoder.finish()]);
        small
    }
}

#[cfg(test)]
mod tests {
    use super::{eight, rgb_pixel, sixteen};
    use crate::ffmpeg::format::Pixel;

    #[test]
    fn sixteen_bit_white_middle_and_black_become_255_128_and_0() {
        assert_eq!(
            *eight(&[0xFF, 0xFF, 0x80, 0x80, 0x00, 0x00], true),
            [255, 128, 0]
        );
    }

    #[test]
    fn an_8_bit_picture_stays_as_it_is() {
        assert_eq!(*eight(&[1, 2, 3], false), [1, 2, 3]);
    }

    #[test]
    fn eight_bit_levels_widen_to_16_bits_by_257() {
        assert_eq!(sixteen(&[1, 255]), [1, 1, 255, 255]);
    }

    #[test]
    fn a_16_bit_picture_is_rgb48_at_6_bytes_a_pixel() {
        assert_eq!(rgb_pixel(true), (Pixel::RGB48LE, 6));
    }

    #[test]
    fn an_8_bit_picture_is_rgb24_at_3_bytes_a_pixel() {
        assert_eq!(rgb_pixel(false), (Pixel::RGB24, 3));
    }
}
