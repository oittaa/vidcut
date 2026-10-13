//! Memory the viewer's graphics device shares with CUDA, so pictures NVDEC
//! decoded show without leaving the GPU (docs/decisions/preview.md §3): a
//! buffer of wgpu's Vulkan device, exported as a file descriptor
//! (`VK_KHR_external_memory_fd`), or on Windows a Win32 handle
//! (`VK_KHR_external_memory_win32`), and imported by CUDA
//! (`vidcut_av::cuda::share`). CUDA copies a picture's planes into it, and
//! wgpu copies them on into the picture's textures.
//!
//! Linux and Windows on Vulkan only, and only when the display's GPU is
//! CUDA's: other pictures are copied down to memory and uploaded.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(target_os = "linux", windows))]
use vidcut_av::cuda::share::SharedBuffer;
use vidcut_av::cuda::{CudaDevice, PlaneCopy};
use vidcut_av::ffmpeg::frame;

/// Whether `device` can share memory with CUDA: Vulkan, with the extension
/// for it. Doesn't open CUDA.
pub fn possible(device: &wgpu::Device) -> bool {
    #[cfg(any(target_os = "linux", windows))]
    {
        // SAFETY: only reads what the device enabled.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() };
        hal.is_some_and(|hal| hal.enabled_device_extensions().contains(&EXTENSION))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = device;
        false
    }
}

/// CUDA's device (the one NVDEC decodes on), when it is on `device`'s GPU:
/// the same UUID as Vulkan's physical device.
pub fn cuda_on(device: &wgpu::Device) -> Result<CudaDevice, String> {
    #[cfg(any(target_os = "linux", windows))]
    {
        use ash::vk;
        if !possible(device) {
            return Err("the display's device can't share memory".into());
        }
        let cuda = CudaDevice::open().map_err(|e| e.to_string())?;
        let uuid = cuda.uuid().map_err(|e| e.to_string())?;
        // SAFETY: reads the physical device's properties.
        let ours = unsafe {
            let hal = device
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("not Vulkan")?;
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_properties2(hal.raw_physical_device(), &mut props);
            id.device_uuid
        };
        if ours == uuid {
            Ok(cuda)
        } else {
            Err("CUDA's GPU isn't the display's".into())
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = device;
        Err("sharing memory with CUDA needs Linux or Windows".into())
    }
}

/// A buffer of the display's device that CUDA writes pictures into.
pub struct Shared {
    /// CUDA's view of it: let go before the memory is.
    #[cfg(any(target_os = "linux", windows))]
    cuda: SharedBuffer,
    buffer: wgpu::Buffer,
    size: u64,
    /// A submission copying out of it hasn't finished: CUDA mustn't write
    /// it. The queue clears it once the work is done
    /// ([`wgpu::Queue::on_submitted_work_done`]), so nothing waits on the
    /// GPU here: waiting holds the device, and the UI's frames with it.
    busy: Arc<AtomicBool>,
}

impl Shared {
    /// A buffer of `size` bytes, shared with `cuda`.
    pub fn new(device: &wgpu::Device, cuda: &CudaDevice, size: u64) -> Result<Self, String> {
        #[cfg(any(target_os = "linux", windows))]
        {
            let (cuda, buffer) = make(device, cuda, size)?;
            Ok(Self {
                cuda,
                buffer,
                size,
                busy: Arc::default(),
            })
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (device, cuda, size);
            Err("sharing memory with CUDA needs Linux or Windows".into())
        }
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Free to fill: no copy out of it is in flight.
    pub fn free(&self) -> bool {
        !self.busy.load(Ordering::Acquire)
    }

    /// Copies the planes of `frame` (in CUDA memory) in as `planes` place
    /// them. Only while it is [`free`](Self::free).
    pub fn fill(&mut self, frame: &frame::Video, planes: &[PlaneCopy]) -> Result<(), String> {
        #[cfg(any(target_os = "linux", windows))]
        {
            self.cuda
                .copy_planes(frame, planes)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (frame, planes);
            Err("sharing memory with CUDA needs Linux or Windows".into())
        }
    }

    /// The work `queue` was just given copies out of it: busy until that
    /// is done.
    pub fn used(&mut self, queue: &wgpu::Queue) {
        self.busy.store(true, Ordering::Release);
        let busy = self.busy.clone();
        queue.on_submitted_work_done(move || busy.store(false, Ordering::Release));
    }
}

/// Makes the Vulkan buffer, with its own exportable memory, and imports it
/// into CUDA.
#[cfg(any(target_os = "linux", windows))]
fn make(
    device: &wgpu::Device,
    cuda: &CudaDevice,
    size: u64,
) -> Result<(SharedBuffer, wgpu::Buffer), String> {
    use ash::vk;
    use wgpu::hal::api::Vulkan;

    #[cfg(target_os = "linux")]
    let handle = vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD;
    #[cfg(windows)]
    let handle = vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
    // SAFETY: Vulkan objects made on wgpu's device, bound and exported as
    // the specification has it; each failure frees what was made. wgpu
    // takes the buffer and its memory over (`from_raw_managed`) once CUDA
    // has imported the memory.
    let (cuda_buffer, hal_buffer) = unsafe {
        let hal = device.as_hal::<Vulkan>().ok_or("not Vulkan")?;
        let raw = hal.raw_device();
        let instance = hal.shared_instance().raw_instance();
        let mut external = vk::ExternalMemoryBufferCreateInfo::default().handle_types(handle);
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external);
        let buffer = raw
            .create_buffer(&info, None)
            .map_err(|e| format!("making a shared buffer: {e}"))?;
        let req = raw.get_buffer_memory_requirements(buffer);
        let props = instance.get_physical_device_memory_properties(hal.raw_physical_device());
        let kind = (0..props.memory_type_count).find(|&i| {
            req.memory_type_bits & (1 << i) != 0
                && props.memory_types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        });
        let Some(kind) = kind else {
            raw.destroy_buffer(buffer, None);
            return Err("no video memory to share".into());
        };
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(handle);
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(kind)
            .push_next(&mut export)
            .push_next(&mut dedicated);
        let memory = match raw.allocate_memory(&alloc, None) {
            Ok(m) => m,
            Err(e) => {
                raw.destroy_buffer(buffer, None);
                return Err(format!("allocating shared memory: {e}"));
            }
        };
        let free = || {
            raw.destroy_buffer(buffer, None);
            raw.free_memory(memory, None);
        };
        if let Err(e) = raw.bind_buffer_memory(buffer, memory, 0) {
            free();
            return Err(format!("binding shared memory: {e}"));
        }
        let exported = match export_memory(instance, raw, memory, handle) {
            Ok(e) => e,
            Err(e) => {
                free();
                return Err(format!("exporting shared memory: {e}"));
            }
        };
        let cuda_buffer = match SharedBuffer::import(cuda, exported, req.size, true) {
            Ok(b) => b,
            Err(e) => {
                free();
                return Err(e.to_string());
            }
        };
        (
            cuda_buffer,
            wgpu::hal::vulkan::Buffer::from_raw_managed(buffer, memory, 0, req.size),
        )
    };
    // SAFETY: made on this device, for `size` bytes, its contents CUDA's.
    let buffer = unsafe {
        device.create_buffer_from_hal::<Vulkan>(
            hal_buffer,
            &wgpu::BufferDescriptor {
                label: Some("vidcut shared picture"),
                size,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    };
    Ok((cuda_buffer, buffer))
}

/// The extension wgpu's device needs to export memory: for file descriptors,
/// or on Windows for Win32 handles.
#[cfg(target_os = "linux")]
const EXTENSION: &std::ffi::CStr = ash::khr::external_memory_fd::NAME;
#[cfg(windows)]
const EXTENSION: &std::ffi::CStr = ash::khr::external_memory_win32::NAME;

/// `memory` exported as a file descriptor.
///
/// # Safety
/// `memory` is `raw`'s, allocated exportable as `handle`.
#[cfg(target_os = "linux")]
unsafe fn export_memory(
    instance: &ash::Instance,
    raw: &ash::Device,
    memory: ash::vk::DeviceMemory,
    handle: ash::vk::ExternalMemoryHandleTypeFlags,
) -> ash::prelude::VkResult<vidcut_av::cuda::share::Exported> {
    use std::os::fd::FromRawFd;
    let fds = ash::khr::external_memory_fd::Device::new(instance, raw);
    let info = ash::vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(handle);
    // SAFETY: the descriptor Vulkan returns is a new one, ours to close.
    unsafe {
        fds.get_memory_fd(&info)
            .map(|fd| std::os::fd::OwnedFd::from_raw_fd(fd))
    }
}

/// `memory` exported as a Win32 handle.
///
/// # Safety
/// `memory` is `raw`'s, allocated exportable as `handle`.
#[cfg(windows)]
unsafe fn export_memory(
    instance: &ash::Instance,
    raw: &ash::Device,
    memory: ash::vk::DeviceMemory,
    handle: ash::vk::ExternalMemoryHandleTypeFlags,
) -> ash::prelude::VkResult<vidcut_av::cuda::share::Exported> {
    use std::os::windows::io::FromRawHandle;
    let handles = ash::khr::external_memory_win32::Device::new(instance, raw);
    let info = ash::vk::MemoryGetWin32HandleInfoKHR::default()
        .memory(memory)
        .handle_type(handle);
    // SAFETY: the handle Vulkan returns is a new one, ours to close.
    unsafe {
        handles
            .get_memory_win32_handle(&info)
            .map(|h| std::os::windows::io::OwnedHandle::from_raw_handle(h as _))
    }
}
