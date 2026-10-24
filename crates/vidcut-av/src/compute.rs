use crate::error::{Error, Result};

pub(crate) fn gpu_error(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{what} on the GPU: {e}"))
}

pub(crate) fn checked<T>(
    device: &wgpu::Device,
    what: &str,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
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
        Some(e) => Err(gpu_error(what, e)),
        None => out,
    }
}

pub(crate) fn pipeline(
    device: &wgpu::Device,
    label: &str,
    source: String,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

pub(crate) fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: usize,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (size.max(4) as u64).next_multiple_of(4),
        usage,
        mapped_at_creation: false,
    })
}

pub(crate) fn group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}

pub(crate) fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub(crate) fn words(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
