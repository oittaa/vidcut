use std::ffi::c_void;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, frame};

#[derive(Clone)]
pub(crate) struct Held {
    buffer: wgpu::Buffer,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

static MARK: u8 = 0;

fn mark() -> *mut c_void {
    (&raw const MARK).cast_mut().cast()
}

impl Held {
    pub(crate) fn new(buffer: wgpu::Buffer, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            buffer,
            device: device.clone(),
            queue: queue.clone(),
        }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn on(&self, device: &wgpu::Device) -> bool {
        self.device == *device
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

unsafe extern "C" fn let_go(_: *mut c_void, data: *mut u8) {
    // SAFETY: `data` is the box that `picture` made, freed once by its buffer.
    drop(unsafe { Box::from_raw(data.cast::<Held>()) });
}

pub(crate) fn picture(held: Held, (w, h): (u32, u32), pts: Option<i64>) -> Result<frame::Video> {
    let mut f = frame::Video::empty();
    // SAFETY: fresh `f` takes one zeroed row at no stride, and `held` as its opaque buffer.
    unsafe {
        let raw = f.as_mut_ptr();
        (*raw).format = ffi::AVPixelFormat::AV_PIX_FMT_RGB24 as i32;
        (*raw).width = w as i32;
        (*raw).height = h as i32;
        let row = ffi::av_buffer_allocz(w as usize * 3);
        if row.is_null() {
            return Err(Error::Invalid("no memory for a picture on the GPU".into()));
        }
        (*raw).buf[0] = row;
        (*raw).data[0] = (*row).data;
        (*raw).linesize[0] = 0;
        let boxed = Box::into_raw(Box::new(held));
        let opaque = ffi::av_buffer_create(
            boxed.cast(),
            std::mem::size_of::<Held>(),
            Some(let_go),
            mark(),
            ffi::AV_BUFFER_FLAG_READONLY,
        );
        if opaque.is_null() {
            drop(Box::from_raw(boxed));
            return Err(Error::Invalid("no memory for a picture on the GPU".into()));
        }
        (*raw).opaque_ref = opaque;
        (*raw).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
    }
    f.set_color_range(ffmpeg::util::color::Range::JPEG);
    f.set_color_space(ffmpeg::util::color::Space::RGB);
    f.set_color_primaries(ffmpeg::util::color::Primaries::BT709);
    f.set_color_transfer_characteristic(ffmpeg::util::color::TransferCharacteristic::BT709);
    f.set_pts(pts);
    Ok(f)
}

pub(crate) fn of(f: &frame::Video) -> Option<&Held> {
    // SAFETY: the frame's own reference; a marked one holds a `Held` while the frame lives.
    unsafe {
        let opaque = (*f.as_ptr()).opaque_ref;
        if opaque.is_null() || ffi::av_buffer_get_opaque(opaque) != mark() {
            return None;
        }
        Some(&*(*opaque).data.cast::<Held>())
    }
}

pub(crate) fn held(f: &frame::Video) -> Result<&Held> {
    of(f).ok_or_else(|| Error::Invalid("the picture isn't held on the GPU".into()))
}

pub(crate) fn rgb(f: &frame::Video) -> Result<Vec<u8>> {
    let bytes = held(f)?.read(f.width() as usize * f.height() as usize * 3)?;
    crate::copies::down(f);
    Ok(bytes)
}

pub(crate) fn in_memory(f: &frame::Video) -> Result<frame::Video> {
    let rgb = rgb(f)?;
    let mut out = crate::buffers::unpacked(
        ffmpeg::format::Pixel::RGB24,
        (f.width(), f.height()),
        3,
        &rgb,
    );
    // SAFETY: both frames are valid; `out` drops the reference to `f`'s held picture.
    unsafe {
        let ret = ffi::av_frame_copy_props(out.as_mut_ptr(), f.as_ptr());
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "copying a frame's properties".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        ffi::av_buffer_unref(&mut (*out.as_mut_ptr()).opaque_ref);
    }
    Ok(out)
}

pub(super) struct Pipes {
    pack: wgpu::ComputePipeline,
    unpack: wgpu::ComputePipeline,
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
            unpack: made("unpack"),
        }
    }

    pub(super) fn pack(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        encoded: &wgpu::Texture,
    ) -> wgpu::Buffer {
        let words = (u64::from(encoded.width()) * u64::from(encoded.height())).div_ceil(4);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut held picture"),
            size: words * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = encoded.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut rgb"),
            layout: &self.pack.get_bind_group_layout(0),
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
            pass.set_pipeline(&self.pack);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(across, groups.div_ceil(across), 1);
        }
        buffer
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
            layout: &self.unpack.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rgb.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&self.unpack);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(
            picture.width().div_ceil(16),
            picture.height().div_ceil(16),
            1,
        );
    }
}
