use std::sync::Arc;

use super::{Gpu, Slot};
use crate::ffmpeg::{format, frame};
use crate::player::{Layout, Picture};

pub(crate) const STRIDE: usize = 1024;

const PEAKS: usize = 3 * STRIDE;

const PARAMS: u64 = 48;

pub(super) struct Pipes {
    peak: wgpu::ComputePipeline,
    sample: wgpu::ComputePipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut light"),
            source: wgpu::ShaderSource::Wgsl(include_str!("light.wgsl").into()),
        });
        let made = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut light"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            peak: made("peak"),
            sample: made("sample"),
        }
    }
}

pub(crate) struct Readout {
    pub(crate) peaks: Vec<u32>,
    pub(crate) samples: Vec<u32>,
}

struct Buffers {
    params: wgpu::Buffer,
    peaks: wgpu::Buffer,
    samples: wgpu::Buffer,
    staging: wgpu::Buffer,
    count: usize,
}

#[derive(Default)]
pub(crate) struct Meter {
    slot: Slot,
    takes: Option<crate::hw::GpuPictures>,
    rank: Option<(Arc<[u32]>, wgpu::Buffer)>,
    buffers: Option<Buffers>,
}

fn bits_of(f: &frame::Video) -> Option<(Layout, u32)> {
    match crate::hw::sw_format(f) {
        format::Pixel::NV12 => Some((Layout::Nv12, 8)),
        format::Pixel::P010LE => Some((Layout::P010, 10)),
        _ => None,
    }
}

pub(crate) fn bits(f: &frame::Video) -> Option<u32> {
    bits_of(f).map(|(_, bits)| bits)
}

impl Meter {
    pub(crate) fn takes(&mut self, gpu: &Gpu, f: &frame::Video) -> bool {
        let Some((layout, _)) = bits_of(f) else {
            return false;
        };
        let pictures = self.takes.get_or_insert_with(|| gpu.gpu_pictures());
        crate::hw::Kind::of_frame(f).is_some_and(|k| pictures.contains(&(k, layout)))
    }

    pub(crate) fn measure(
        &mut self,
        gpu: &Gpu,
        f: &frame::Video,
        step: u32,
        rank: &Arc<[u32]>,
    ) -> Result<Readout, String> {
        let (_, bits) = bits_of(f).ok_or("pictures of this layout")?;
        crate::cuda::share::wait_for(f).map_err(|e| e.to_string())?;
        let frame = crate::export::video::frame_ref(f).map_err(|e| e.to_string())?;
        let picture = Arc::new(Picture::new(frame, 0, None, None));
        let (w, h) = (f.width(), f.height());
        let sites = (w.div_ceil(2), h.div_ceil(2));
        let grid = (w.div_ceil(step), h.div_ceil(step));
        let count = grid.0 as usize * grid.1 as usize;
        let device = &gpu.device;
        if self
            .rank
            .as_ref()
            .is_none_or(|(r, _)| !Arc::ptr_eq(r, rank))
        {
            let buffer = super::checked(device, || {
                crate::compute::buffer(
                    device,
                    "vidcut light ranks",
                    rank.len() * 4,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                )
            })?;
            gpu.queue
                .write_buffer(&buffer, 0, &crate::compute::words(rank));
            self.rank = Some((rank.clone(), buffer));
        }
        if self.buffers.as_ref().is_none_or(|b| b.count != count) {
            let made = |label, size, usage| crate::compute::buffer(device, label, size, usage);
            self.buffers = Some(super::checked(device, || Buffers {
                params: made(
                    "vidcut light",
                    PARAMS as usize,
                    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                ),
                peaks: made(
                    "vidcut light peaks",
                    PEAKS * 4,
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                ),
                samples: made(
                    "vidcut light samples",
                    count * 4,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                ),
                staging: made(
                    "vidcut light readout",
                    (PEAKS + count) * 4,
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
                count,
            })?);
        }
        let b = self.buffers.as_ref().expect("made above");
        let (scale, shift) = if bits > 8 { (65535f32, 6) } else { (255f32, 0) };
        let params = [
            w,
            h,
            sites.0,
            sites.1,
            grid.0,
            grid.1,
            step,
            shift,
            scale.to_bits(),
            bits,
            0,
            0,
        ];
        gpu.queue
            .write_buffer(&b.params, 0, &crate::compute::words(&params));
        let empty: Vec<u32> = (0..PEAKS)
            .map(|i| if i < 2 * STRIDE { 0 } else { u32::MAX })
            .collect();
        gpu.queue
            .write_buffer(&b.peaks, 0, &crate::compute::words(&empty));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vidcut light"),
        });
        #[cfg(any(target_os = "linux", windows))]
        {
            self.slot.computes = true;
        }
        self.slot.upload(gpu, &picture, &mut encoder);
        let planes = self
            .slot
            .planes
            .as_ref()
            .ok_or("no planes for the picture")?;
        let (y, uv) = match self.slot.views.take() {
            Some((y, uv, _)) => (y, uv),
            None => (
                planes.y.create_view(&Default::default()),
                planes.u.create_view(&Default::default()),
            ),
        };
        let pipes = gpu.light.get_or_init(|| Pipes::new(device));
        let rank = &self.rank.as_ref().expect("made above").1;
        let bind = |pipeline: &wgpu::ComputePipeline, more: &[(u32, &wgpu::Buffer)]| {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: b.params.as_entire_binding(),
                },
            ];
            entries.extend(more.iter().map(|&(binding, buffer)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }));
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vidcut light"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            })
        };
        super::checked(device, || {
            let peaking = bind(&pipes.peak, &[(3, rank), (4, &b.peaks)]);
            let sampling = bind(&pipes.sample, &[(5, &b.samples)]);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("vidcut light"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipes.peak);
            pass.set_bind_group(0, &peaking, &[]);
            pass.dispatch_workgroups(sites.0.div_ceil(16), sites.1.div_ceil(64), 1);
            pass.set_pipeline(&pipes.sample);
            pass.set_bind_group(0, &sampling, &[]);
            pass.dispatch_workgroups(grid.0.div_ceil(16), grid.1.div_ceil(16), 1);
        })?;
        encoder.copy_buffer_to_buffer(&b.peaks, 0, &b.staging, 0, (PEAKS * 4) as u64);
        encoder.copy_buffer_to_buffer(
            &b.samples,
            0,
            &b.staging,
            (PEAKS * 4) as u64,
            (count * 4) as u64,
        );
        let acquired = self.slot.acquired.take();
        let released = self.slot.released.take();
        let done = gpu.submit(
            acquired
                .into_iter()
                .chain([encoder.finish()])
                .chain(released),
        );
        self.slot.submitted(gpu);
        let slice = b.staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let failed = |e: &dyn std::fmt::Display| format!("reading the light back: {e}");
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(done),
                timeout: None,
            })
            .map_err(|e| failed(&e))?;
        rx.recv().map_err(|e| failed(&e))?.map_err(|e| failed(&e))?;
        let words: Vec<u32> = slice
            .get_mapped_range()
            .map_err(|e| failed(&e))?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect();
        b.staging.unmap();
        let (peaks, samples) = words.split_at(PEAKS);
        Ok(Readout {
            peaks: peaks.to_vec(),
            samples: samples[..count].to_vec(),
        })
    }
}
