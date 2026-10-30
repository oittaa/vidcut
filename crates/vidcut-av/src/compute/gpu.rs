use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};

use crate::compute;
use crate::error::Result;

const MOST: u32 = 65535;
const SLOT: u64 = 256;
const SLOTS: usize = 16384;

pub(crate) struct Param {
    buffer: wgpu::Buffer,
    at: u64,
}

type Key = (wgpu::ComputePipeline, wgpu::Buffer, Vec<wgpu::Buffer>);

struct Ring {
    buffer: wgpu::Buffer,
    mirror: Vec<u8>,
    next: usize,
    dirty: usize,
}

pub(crate) struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) id: u64,
    pub(crate) listed: Option<crate::gpus::Gpu>,
    pub(crate) name: String,
    pub(crate) what: &'static str,
    uploaded: AtomicU64,
    pipes: Mutex<HashMap<String, wgpu::ComputePipeline>>,
    ring: Mutex<Ring>,
    groups: Mutex<HashMap<Key, wgpu::BindGroup>>,
    scratch: Mutex<Option<(wgpu::Buffer, wgpu::Buffer)>>,
}

pub(crate) fn storage() -> wgpu::BufferUsages {
    use wgpu::BufferUsages as U;
    U::STORAGE | U::COPY_DST | U::COPY_SRC
}

fn layout(device: &wgpu::Device, source: &str) -> wgpu::PipelineLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("@group(0) @binding(")?;
            let (n, decl) = rest.split_once(')')?;
            Some(wgpu::BindGroupLayoutEntry {
                binding: n.parse().ok()?,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: binding_type(decl),
                count: None,
            })
        })
        .collect();
    let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vidcut-compute"),
        entries: &entries,
    });
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vidcut-compute"),
        bind_group_layouts: &[Some(&group)],
        immediate_size: 0,
    })
}

fn binding_type(decl: &str) -> wgpu::BindingType {
    if decl.contains("texture_storage_2d<rgba16float") {
        return wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: wgpu::TextureViewDimension::D2,
        };
    }
    if decl.contains("texture_2d<f32>") {
        return wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
    }
    let uniform = decl.contains("var<uniform>");
    let ty = if uniform {
        wgpu::BufferBindingType::Uniform
    } else {
        wgpu::BufferBindingType::Storage {
            read_only: !decl.contains("read_write"),
        }
    };
    wgpu::BindingType::Buffer {
        ty,
        has_dynamic_offset: uniform,
        min_binding_size: None,
    }
}

fn pipeline(device: &wgpu::Device, source: String) -> wgpu::ComputePipeline {
    let layout = layout(device, &source);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vidcut-compute"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("vidcut-compute"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn slots(device: &wgpu::Device) -> wgpu::Buffer {
    compute::buffer(
        device,
        "vidcut-compute-params",
        SLOTS * SLOT as usize,
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    )
}

impl Gpu {
    pub(crate) fn start(what: &'static str, shared: u32) -> std::result::Result<Self, String> {
        let on = crate::screen::offscreen().map_err(|e| format!("no GPU for {what}: {e}"))?;
        let (device, queue) = on.gpu.handles();
        let limits = device.limits();
        let listed = crate::gpus::of(on.adapter.vendor, on.adapter.device);
        let name = on.adapter.name.clone();
        if limits.max_compute_workgroup_storage_size < shared
            || limits.max_compute_invocations_per_workgroup < 256
            || limits.max_storage_buffers_per_shader_stage < 8
        {
            return Err(format!("{name} has too little workgroup memory for {what}"));
        }
        let buffer = slots(device);
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            id: on.gpu.id(),
            listed,
            name,
            what,
            uploaded: AtomicU64::new(0),
            pipes: Mutex::new(HashMap::new()),
            ring: Mutex::new(Ring {
                buffer,
                mirror: vec![0; SLOTS * SLOT as usize],
                next: 0,
                dirty: 0,
            }),
            groups: Mutex::new(HashMap::new()),
            scratch: Mutex::new(None),
        })
    }

    pub(crate) fn error(&self, e: impl std::fmt::Display) -> crate::error::Error {
        compute::gpu_error(self.what, e)
    }

    pub(crate) fn pipe(&self, key: &str, make: impl FnOnce() -> String) -> wgpu::ComputePipeline {
        let mut pipes = self.pipes.lock().unwrap_or_else(PoisonError::into_inner);
        pipes
            .entry(key.to_string())
            .or_insert_with(|| pipeline(&self.device, make()))
            .clone()
    }

    pub(crate) fn half(&self, body: &str) -> String {
        format!(
            "fn h2(w: u32) -> vec2<f32> {{
    return unpack2x16float(w);
}}

{body}"
        )
    }

    pub(crate) fn scratch(&self, first: u64, second: u64) -> (wgpu::Buffer, wgpu::Buffer) {
        let mut s = self.scratch.lock().unwrap_or_else(PoisonError::into_inner);
        let fits = s
            .as_ref()
            .is_some_and(|(a, b)| a.size() >= first && b.size() >= second);
        if !fits {
            let (a, b) = s.take().map_or((first, second), |(a, b)| {
                (a.size().max(first), b.size().max(second))
            });
            *s = Some((self.buffer(a as usize), self.buffer(b as usize)));
        }
        s.clone().expect("made")
    }

    pub(crate) fn free_scratch(&self) {
        *self.scratch.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub(crate) fn buffer(&self, size: usize) -> wgpu::Buffer {
        compute::buffer(&self.device, "vidcut-compute", size, storage())
    }

    pub(crate) fn uploaded(&self) -> u64 {
        self.uploaded.load(Ordering::Relaxed)
    }

    pub(crate) fn upload(&self, data: &[u8]) -> wgpu::Buffer {
        self.uploaded
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        compute::buffer_with(&self.device, "vidcut-compute", data, storage())
    }

    fn write_dirty(&self, ring: &mut Ring) {
        let (from, to) = (ring.dirty * SLOT as usize, ring.next * SLOT as usize);
        if to > from {
            self.queue
                .write_buffer(&ring.buffer, from as u64, &ring.mirror[from..to]);
        }
        ring.dirty = ring.next;
    }

    pub(crate) fn uniform(&self, values: &[u32]) -> Param {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.next == SLOTS {
            self.write_dirty(&mut ring);
            ring.buffer = slots(&self.device);
            ring.next = 0;
            ring.dirty = 0;
        }
        let at = ring.next * SLOT as usize;
        let data = compute::words(values);
        ring.mirror[at..at + data.len()].copy_from_slice(&data);
        ring.next += 1;
        Param {
            buffer: ring.buffer.clone(),
            at: at as u64,
        }
    }

    pub(crate) fn flush(&self) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        self.write_dirty(&mut ring);
        self.groups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn set(
        &self,
        pass: &mut wgpu::ComputePass,
        pipe: &wgpu::ComputePipeline,
        param: Param,
        buffers: &[&wgpu::Buffer],
    ) {
        let key = (
            pipe.clone(),
            param.buffer.clone(),
            buffers.iter().map(|b| (*b).clone()).collect(),
        );
        let bind = self
            .groups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_insert_with(|| {
                let resources = buffers.iter().map(|b| b.as_entire_binding());
                self.group(pipe, &param, resources)
            })
            .clone();
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bind, &[param.at as u32]);
    }

    fn group<'a>(
        &self,
        pipe: &wgpu::ComputePipeline,
        param: &Param,
        resources: impl IntoIterator<Item = wgpu::BindingResource<'a>>,
    ) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &param.buffer,
                offset: 0,
                size: wgpu::BufferSize::new(SLOT),
            }),
        }];
        entries.extend(resources.into_iter().enumerate().map(|(i, resource)| {
            wgpu::BindGroupEntry {
                binding: i as u32 + 1,
                resource,
            }
        }));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &entries,
        })
    }

    pub(crate) fn dispatch(
        &self,
        pass: &mut wgpu::ComputePass,
        pipe: &wgpu::ComputePipeline,
        param: Param,
        buffers: &[&wgpu::Buffer],
        groups: u32,
    ) {
        if groups == 0 {
            return;
        }
        self.set(pass, pipe, param, buffers);
        let x = groups.min(MOST);
        pass.dispatch_workgroups(x, groups.div_ceil(x), 1);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn dispatch_bound(
        &self,
        pass: &mut wgpu::ComputePass,
        pipe: &wgpu::ComputePipeline,
        param: Param,
        resources: Vec<wgpu::BindingResource<'_>>,
        groups: u32,
    ) {
        if groups == 0 {
            return;
        }
        let bind = self.group(pipe, &param, resources);
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bind, &[param.at as u32]);
        let x = groups.min(MOST);
        pass.dispatch_workgroups(x, groups.div_ceil(x), 1);
    }

    pub(crate) fn staging(&self, size: u64) -> wgpu::Buffer {
        compute::buffer(
            &self.device,
            "vidcut-compute-read",
            size as usize,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        )
    }

    pub(crate) fn fetch(
        &self,
        staging: &wgpu::Buffer,
        size: u64,
        done: Option<wgpu::SubmissionIndex>,
    ) -> Result<Vec<u8>> {
        let slice = staging.slice(..size);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: done,
                timeout: None,
            })
            .map_err(|e| self.error(e))?;
        rx.recv()
            .map_err(|e| self.error(e))?
            .map_err(|e| self.error(e))?;
        let out = slice
            .get_mapped_range()
            .map_err(|e| self.error(e))?
            .to_vec();
        staging.unmap();
        Ok(out)
    }

    pub(crate) fn read(&self, src: &wgpu::Buffer, offset: u64, size: u64) -> Result<Vec<u8>> {
        let staging = self.staging(size);
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(src, offset, &staging, 0, size);
        self.flush();
        self.queue.submit([enc.finish()]);
        self.fetch(&staging, size, None)
    }

    pub(crate) fn wait_for(&self, done: wgpu::SubmissionIndex) -> Result<()> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(done),
                timeout: None,
            })
            .map_err(|e| self.error(e))?;
        Ok(())
    }

    pub(crate) fn wait(&self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| self.error(e))?;
        Ok(())
    }
}

pub(crate) fn floats(raw: &[u8]) -> Vec<f32> {
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}
