use std::collections::HashMap;
use std::sync::{Mutex, PoisonError, mpsc};

use wgpu::util::DeviceExt;

use crate::compute;
use crate::error::Result;

pub(super) const WHAT: &str = "the AI Best upscaler";
pub(super) const SHARED: u32 = 32 * 1024;
const MOST: u32 = 65535;

pub(super) fn gpu_error(e: impl std::fmt::Display) -> crate::error::Error {
    compute::gpu_error(WHAT, e)
}

pub(super) struct Gpu {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pipes: Mutex<HashMap<String, wgpu::ComputePipeline>>,
}

pub(super) fn storage() -> wgpu::BufferUsages {
    use wgpu::BufferUsages as U;
    U::STORAGE | U::COPY_DST | U::COPY_SRC
}

impl Gpu {
    pub(super) fn start() -> std::result::Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU for the AI Best upscaler: {e}"))?;
        let limits = adapter.limits();
        let name = adapter.get_info().name;
        if limits.max_compute_workgroup_storage_size < SHARED
            || limits.max_compute_invocations_per_workgroup < 256
            || limits.max_storage_buffers_per_shader_stage < 8
        {
            return Err(format!(
                "{name} has too little workgroup memory for the AI Best upscaler"
            ));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vidcut-flashvsr"),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| format!("the GPU did not open for the AI Best upscaler: {e}"))?;
        Ok(Self {
            device,
            queue,
            pipes: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn pipe(&self, key: &str, make: impl FnOnce() -> String) -> wgpu::ComputePipeline {
        let mut pipes = self.pipes.lock().unwrap_or_else(PoisonError::into_inner);
        pipes
            .entry(key.to_string())
            .or_insert_with(|| compute::pipeline(&self.device, "flashvsr", make()))
            .clone()
    }

    pub(super) fn buffer(&self, size: usize) -> wgpu::Buffer {
        compute::buffer(&self.device, "flashvsr", size, storage())
    }

    pub(super) fn upload(&self, data: &[u8]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("flashvsr"),
                contents: data,
                usage: storage(),
            })
    }

    pub(super) fn uniform(&self, values: &[u32]) -> wgpu::Buffer {
        let mut padded = values.to_vec();
        padded.resize(values.len().next_multiple_of(4).max(4), 0);
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("flashvsr-uniform"),
                contents: &compute::words(&padded),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub(super) fn dispatch(
        &self,
        pass: &mut wgpu::ComputePass,
        pipe: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        groups: u32,
    ) {
        if groups == 0 {
            return;
        }
        let bind = compute::group(&self.device, pipe, buffers);
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bind, &[]);
        let x = groups.min(MOST);
        pass.dispatch_workgroups(x, groups.div_ceil(x), 1);
    }

    pub(super) fn read(&self, src: &wgpu::Buffer, offset: u64, size: u64) -> Result<Vec<u8>> {
        let staging = compute::buffer(
            &self.device,
            "flashvsr-read",
            size as usize,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(src, offset, &staging, 0, size);
        self.queue.submit([enc.finish()]);
        let slice = staging.slice(..size);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(gpu_error)?;
        rx.recv().map_err(gpu_error)?.map_err(gpu_error)?;
        let out = slice.get_mapped_range().map_err(gpu_error)?.to_vec();
        staging.unmap();
        Ok(out)
    }

    pub(super) fn wait(&self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(gpu_error)?;
        Ok(())
    }
}

pub(super) fn floats(raw: &[u8]) -> Vec<f32> {
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}
