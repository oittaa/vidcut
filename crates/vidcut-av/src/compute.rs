use std::sync::{Mutex, PoisonError};

use crate::error::{Error, Result};

mod gpu;

pub(crate) use gpu::{Gpu, floats};

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
    let found: Vec<wgpu::Error> = scopes
        .into_iter()
        .rev()
        .filter_map(|scope| pollster::block_on(scope.pop()))
        .collect();
    if found
        .iter()
        .any(|e| matches!(e, wgpu::Error::OutOfMemory { .. }))
    {
        return Err(gpu_error(what, "not enough GPU memory"));
    }
    match found.into_iter().next() {
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

pub(crate) fn buffer_with(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (contents.len().max(4) as u64).next_multiple_of(4),
        usage,
        mapped_at_creation: true,
    });
    if let Ok(mut mapped) = buffer.get_mapped_range_mut(..) {
        mapped.slice(..contents.len()).copy_from_slice(contents);
    }
    buffer.unmap();
    buffer
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

pub(crate) fn half(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 112;
    let man = b & 0x7f_ffff;
    let (bits, rest, shift) = if exp >= 31 {
        return sign | 0x7c00;
    } else if exp > 0 {
        (((exp as u32) << 10) | (man >> 13), man & 0x1fff, 13)
    } else if exp > -11 {
        let m = man | 0x80_0000;
        let shift = (14 - exp) as u32;
        (m >> shift, m & ((1 << shift) - 1), shift)
    } else {
        return sign;
    };
    let middle = 1 << (shift - 1);
    let up = rest > middle || (rest == middle && bits & 1 == 1);
    sign | (bits + u32::from(up)) as u16
}

const SERIAL: usize = 1 << 16;

pub(crate) fn halves(v: &[f32]) -> Vec<u8> {
    let mut out = vec![0u8; v.len() * 2];
    let parts = if v.len() < SERIAL {
        1
    } else {
        crate::pool::threads()
    };
    let each = v.len().div_ceil(parts.max(1)).max(1);
    let slices: Vec<Mutex<(&[f32], &mut [u8])>> = v
        .chunks(each)
        .zip(out.chunks_mut(each * 2))
        .map(Mutex::new)
        .collect();
    crate::pool::run(slices.len(), &|i| {
        let mut part = slices[i].lock().unwrap_or_else(PoisonError::into_inner);
        let (src, dst) = &mut *part;
        for (f, d) in src.iter().zip(dst.as_chunks_mut::<2>().0) {
            *d = half(*f).to_le_bytes();
        }
    });
    drop(slices);
    out
}
