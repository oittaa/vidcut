use std::ffi::{CStr, c_void};
use std::sync::Mutex;

use ash::vk;
use ash::vk::Handle as _;

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, frame};

const DECODED: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) fn keep(f: &frame::Video) -> Result<frame::Video> {
    // SAFETY: `f`'s device and `AVVkFrame`, from `parts`, live as long as `f`.
    unsafe {
        let (dev, _, vkf) = parts(f)?;
        crate::cuda::wait_signalled(dev, &[vkf.cast_const()], DECODED)
    }
    .map_err(|e| Error::Invalid(format!("decoding on Vulkan: {e}")))?;
    crate::export::video::frame_ref(f)
}

pub(crate) fn image_bytes(f: &frame::Video) -> Option<usize> {
    // SAFETY: a Vulkan frame's own AVVkFrame, alive as long as it.
    unsafe {
        let (_, _, vkf) = parts(f).ok()?;
        let vkf = &*vkf;
        let bytes: usize = vkf
            .mem
            .iter()
            .zip(vkf.size)
            .filter(|(m, _)| **m != 0)
            .map(|(_, size)| size)
            .sum();
        (bytes > 0).then_some(bytes)
    }
}

unsafe fn parts(
    f: &frame::Video,
) -> Result<(
    &ffi::AVVulkanDeviceContext,
    *mut ffi::AVHWFramesContext,
    *mut ffi::AVVkFrame,
)> {
    // SAFETY: a Vulkan frame's frames context's device `hwctx` is an `AVVulkanDeviceContext`.
    unsafe {
        let raw = f.as_ptr();
        if (*raw).format != ffi::AVPixelFormat::AV_PIX_FMT_VULKAN as i32
            || (*raw).hw_frames_ctx.is_null()
            || (*raw).data[0].is_null()
        {
            return Err(Error::Invalid("not a Vulkan frame".into()));
        }
        let fc = (*(*raw).hw_frames_ctx)
            .data
            .cast::<ffi::AVHWFramesContext>();
        let dev = &*((*(*fc).device_ctx).hwctx as *const ffi::AVVulkanDeviceContext);
        Ok((dev, fc, (*raw).data[0].cast::<ffi::AVVkFrame>()))
    }
}

#[cfg(windows)]
pub const HANDLE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
#[cfg(not(windows))]
pub const HANDLE: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD;

#[cfg(windows)]
pub type Handle = std::os::windows::io::OwnedHandle;
#[cfg(not(windows))]
pub type Handle = std::os::fd::OwnedFd;

static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

struct Shared {
    frames: usize,
    handles: Vec<(u64, Handle)>,
}

unsafe fn handle_of(
    fns: &Fns,
    fc: *mut ffi::AVHWFramesContext,
    memory: vk::DeviceMemory,
) -> Result<Handle> {
    let frames = fc as usize;
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    let at = match shared.iter().position(|s| s.frames == frames) {
        Some(at) => at,
        None => {
            // SAFETY: `fc` is live, and the held frame keeps FFmpeg from reading `free` meanwhile.
            unsafe {
                if (*fc).free.is_some() {
                    return Err(Error::Invalid(
                        "FFmpeg's Vulkan frames are freed by someone else".into(),
                    ));
                }
                (*fc).free = Some(close_shared);
            }
            shared.push(Shared {
                frames,
                handles: Vec::new(),
            });
            shared.len() - 1
        }
    };
    let handles = &mut shared[at].handles;
    let key = memory.as_raw();
    let held = match handles.iter().position(|(m, _)| *m == key) {
        Some(i) => &handles[i].1,
        None => {
            // SAFETY: as the caller says.
            let exported = unsafe { fns.export(memory)? };
            handles.push((key, exported));
            &handles[handles.len() - 1].1
        }
    };
    held.try_clone()
        .map_err(|e| Error::Invalid(format!("copying a frame's memory handle: {e}")))
}

unsafe extern "C" fn close_shared(fc: *mut ffi::AVHWFramesContext) {
    let frames = fc as usize;
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    shared.retain(|s| s.frames != frames);
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    pub frames: usize,
    pub memory: Vec<u64>,
}

impl Identity {
    pub fn of(f: &frame::Video) -> Result<Self> {
        // SAFETY: `f`'s `AVVkFrame` lives as long as `f`; FFmpeg set its images and memory once.
        unsafe {
            let (_, fc, vkf) = parts(f)?;
            let vkf = &*vkf;
            let images = vkf.img.iter().take_while(|&&i| i != 0).count();
            Ok(Self {
                frames: fc as usize,
                memory: vkf.mem[..images].to_vec(),
            })
        }
    }
}

pub fn layout(f: &frame::Video) -> Result<vk::ImageLayout> {
    // SAFETY: `f`'s frames context and `AVVkFrame` live as long as `f`; read under their lock.
    unsafe {
        let (_, fc, vkf) = parts(f)?;
        let _locked = Locked::new(fc, vkf);
        Ok(vk::ImageLayout::from_raw((*vkf).layout[0]))
    }
}

pub struct Plane {
    pub memory: Handle,
    pub size: u64,
    pub memory_type: u32,
    pub dedicated: bool,
    pub format: vk::Format,
    pub extent: (u32, u32),
    pub layers: u32,
    pub tiling: vk::ImageTiling,
    pub usage: vk::ImageUsageFlags,
    pub flags: vk::ImageCreateFlags,
    pub handle_types: vk::ExternalMemoryHandleTypeFlags,
    pub families: Vec<u32>,
}

pub struct Exported {
    pub planes: Vec<Plane>,
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

impl Exported {
    pub fn of(f: &frame::Video) -> Result<Self> {
        // SAFETY: `f`'s device, frames and `AVVkFrame` outlive this; `fns` is that device's loader.
        unsafe {
            let (dev, fc, vkf) = parts(f)?;
            let vkf = &*vkf;
            let vkfc = &*((*fc).hwctx as *const ffi::AVVulkanFramesContext);
            let planes = ffi::av_pix_fmt_count_planes((*fc).sw_format).max(0) as usize;
            let images = vkf.img.iter().take_while(|&&i| i != 0).count();
            if planes == 0 || images != planes {
                return Err(Error::Invalid(format!(
                    "a Vulkan frame of {images} images for {planes} planes"
                )));
            }
            let tiling = vk::ImageTiling::from_raw(vkf.tiling);
            if tiling == vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT {
                return Err(Error::Invalid(
                    "a Vulkan frame shared as DMA-BUFs, not as opaque memory".into(),
                ));
            }
            let fns = Fns::of(dev)?;
            let usage = vk::ImageUsageFlags::from_raw(vkfc.usage as u32);
            let handle_types =
                fns.handle_types(vk::Format::from_raw(vkfc.format[0]), tiling, usage)?;
            let kinds = fns.memory_properties();
            let qf = &dev.qf[..dev.nb_qf.clamp(0, dev.qf.len() as i32) as usize];
            let families = families(qf.iter().map(|q| q.idx as u32));
            let mut out = Vec::with_capacity(planes);
            for i in 0..planes {
                if vkf.mem[i] == 0 || vkf.offset[i] != 0 {
                    return Err(Error::Invalid(
                        "a Vulkan frame whose planes share memory".into(),
                    ));
                }
                let (allowed, dedicated) = fns.requirements(vk::Image::from_raw(vkf.img[i]));
                let size = vkf.size[i] as u64;
                let memory_type = memory_type(&kinds, allowed, tiling, size).ok_or_else(|| {
                    Error::Invalid("no memory type for a Vulkan frame's plane".into())
                })?;
                out.push(Plane {
                    memory: handle_of(&fns, fc, vk::DeviceMemory::from_raw(vkf.mem[i]))?,
                    size,
                    memory_type,
                    dedicated,
                    format: vk::Format::from_raw(vkfc.format[i]),
                    extent: plane_size((*fc).sw_format, (*fc).width, (*fc).height, i),
                    layers: vkfc.nb_layers.max(1) as u32,
                    tiling,
                    usage,
                    flags: vk::ImageCreateFlags::from_raw(vkfc.img_flags),
                    handle_types,
                    families: families.clone(),
                });
            }
            let (device_uuid, driver_uuid) = fns.uuids();
            Ok(Self {
                planes: out,
                device_uuid,
                driver_uuid,
            })
        }
    }
}

pub fn acquired_outside(f: &frame::Video, layout: vk::ImageLayout) -> Result<()> {
    // SAFETY: `f`'s frames context and `AVVkFrame` live as long as `f`; changed under their lock.
    unsafe {
        let (_, fc, vkf) = parts(f)?;
        let _locked = Locked::new(fc, vkf);
        let images = (*vkf).img.iter().take_while(|&&i| i != 0).count();
        for i in 0..images {
            (*vkf).layout[i] = layout.as_raw();
            (*vkf).queue_family[i] = vk::QUEUE_FAMILY_EXTERNAL;
            (*vkf).access[i] = 0;
        }
    }
    Ok(())
}

struct Locked {
    fc: *mut ffi::AVHWFramesContext,
    vkf: *mut ffi::AVVkFrame,
}

impl Locked {
    unsafe fn new(fc: *mut ffi::AVHWFramesContext, vkf: *mut ffi::AVVkFrame) -> Self {
        // SAFETY: as the caller says; FFmpeg's own lock.
        unsafe {
            let vkfc = &*((*fc).hwctx as *const ffi::AVVulkanFramesContext);
            if let Some(lock) = vkfc.lock_frame {
                lock(fc, vkf);
            }
        }
        Self { fc, vkf }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: locked in `new`, by the same frames context.
        unsafe {
            let vkfc = &*((*self.fc).hwctx as *const ffi::AVVulkanFramesContext);
            if let Some(unlock) = vkfc.unlock_frame {
                unlock(self.fc, self.vkf);
            }
        }
    }
}

fn families(families: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for f in families {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

fn plane_size(format: ffi::AVPixelFormat, width: i32, height: i32, plane: usize) -> (u32, u32) {
    let (w, h) = (width.max(0) as u32, height.max(0) as u32);
    // SAFETY: FFmpeg's static description of a format, or null.
    let Some(desc) = (unsafe { ffi::av_pix_fmt_desc_get(format).as_ref() }) else {
        return (w, h);
    };
    let planar = desc.flags & ffi::AV_PIX_FMT_FLAG_PLANAR as u64 != 0;
    let rgb = desc.flags & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0;
    if plane == 0 || plane == 3 || rgb || !planar {
        return (w, h);
    }
    (
        w.div_ceil(1 << desc.log2_chroma_w),
        h.div_ceil(1 << desc.log2_chroma_h),
    )
}

fn memory_type(
    kinds: &vk::PhysicalDeviceMemoryProperties,
    allowed: u32,
    tiling: vk::ImageTiling,
    size: u64,
) -> Option<u32> {
    let needs = if tiling == vk::ImageTiling::LINEAR {
        vk::MemoryPropertyFlags::HOST_VISIBLE
    } else {
        vk::MemoryPropertyFlags::DEVICE_LOCAL
    };
    let count = kinds.memory_type_count.min(vk::MAX_MEMORY_TYPES as u32);
    (0..count).find(|&i| {
        let kind = kinds.memory_types[i as usize];
        let heap = kinds.memory_heaps.get(kind.heap_index as usize);
        allowed & (1 << i) != 0
            && kind.property_flags.contains(needs)
            && heap.is_some_and(|h| size <= h.size)
    })
}

unsafe fn enabled(dev: &ffi::AVVulkanDeviceContext, name: &CStr) -> bool {
    let count = dev.nb_enabled_dev_extensions.max(0) as usize;
    if dev.enabled_dev_extensions.is_null() {
        return false;
    }
    // SAFETY: as the caller says: an array of that many C strings.
    unsafe {
        std::slice::from_raw_parts(dev.enabled_dev_extensions, count)
            .iter()
            .any(|&e| !e.is_null() && CStr::from_ptr(e) == name)
    }
}

struct Fns {
    instance: ash::InstanceFnV1_0,
    instance_1_1: ash::InstanceFnV1_1,
    device_1_1: ash::DeviceFnV1_1,
    #[cfg(windows)]
    memory: ash::khr::external_memory_win32::DeviceFn,
    #[cfg(not(windows))]
    memory: ash::khr::external_memory_fd::DeviceFn,
    physical: vk::PhysicalDevice,
    device: vk::Device,
}

impl Fns {
    unsafe fn of(dev: &ffi::AVVulkanDeviceContext) -> Result<Self> {
        #[cfg(windows)]
        let extension = ash::khr::external_memory_win32::NAME;
        #[cfg(not(windows))]
        let extension = ash::khr::external_memory_fd::NAME;
        // SAFETY: as the caller says.
        if !unsafe { enabled(dev, extension) } {
            return Err(Error::Invalid(format!(
                "FFmpeg's Vulkan device shares no memory (no {})",
                extension.to_string_lossy()
            )));
        }
        let get = dev
            .get_proc_addr
            .ok_or_else(|| Error::Invalid("Vulkan has no loader".into()))?;
        let inst = dev.inst;
        // SAFETY: `get` gives the instance's calls, each as vulkan.h declares it, or null.
        let instance_fn = |name: &CStr| unsafe {
            std::mem::transmute::<ffi::PFN_vkVoidFunction, *const c_void>(get(inst, name.as_ptr()))
        };
        let instance = ash::InstanceFnV1_0::load(instance_fn);
        let instance_1_1 = ash::InstanceFnV1_1::load(instance_fn);
        let device = vk::Device::from_raw(dev.act_dev as u64);
        let get_device = instance.get_device_proc_addr;
        // SAFETY: the device's own lookup of its calls, or null.
        let device_fn = |name: &CStr| unsafe {
            std::mem::transmute::<vk::PFN_vkVoidFunction, *const c_void>(get_device(
                device,
                name.as_ptr(),
            ))
        };
        Ok(Self {
            device_1_1: ash::DeviceFnV1_1::load(device_fn),
            #[cfg(windows)]
            memory: ash::khr::external_memory_win32::DeviceFn::load(device_fn),
            #[cfg(not(windows))]
            memory: ash::khr::external_memory_fd::DeviceFn::load(device_fn),
            instance,
            instance_1_1,
            physical: vk::PhysicalDevice::from_raw(dev.phys_dev as u64),
            device,
        })
    }

    unsafe fn handle_types(
        &self,
        format: vk::Format,
        tiling: vk::ImageTiling,
        usage: vk::ImageUsageFlags,
    ) -> Result<vk::ExternalMemoryHandleTypeFlags> {
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(HANDLE);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(tiling)
            .usage(usage)
            .flags(vk::ImageCreateFlags::ALIAS)
            .push_next(&mut external);
        let mut found = vk::ExternalImageFormatProperties::default();
        let mut props = vk::ImageFormatProperties2::default().push_next(&mut found);
        // SAFETY: as the caller says; structures as vulkan.h declares them.
        let result = unsafe {
            (self
                .instance_1_1
                .get_physical_device_image_format_properties2)(
                self.physical, &info, &mut props
            )
        };
        match result {
            vk::Result::SUCCESS => Ok(found.external_memory_properties.compatible_handle_types),
            e => Err(Error::Invalid(format!(
                "FFmpeg's Vulkan frames aren't shared: {e}"
            ))),
        }
    }

    unsafe fn memory_properties(&self) -> vk::PhysicalDeviceMemoryProperties {
        let mut props = vk::PhysicalDeviceMemoryProperties::default();
        // SAFETY: as the caller says.
        unsafe { (self.instance.get_physical_device_memory_properties)(self.physical, &mut props) };
        props
    }

    unsafe fn requirements(&self, image: vk::Image) -> (u32, bool) {
        let info = vk::ImageMemoryRequirementsInfo2::default().image(image);
        let mut dedicated = vk::MemoryDedicatedRequirements::default();
        let mut req = vk::MemoryRequirements2::default().push_next(&mut dedicated);
        // SAFETY: as the caller says.
        unsafe { (self.device_1_1.get_image_memory_requirements2)(self.device, &info, &mut req) };
        let allowed = req.memory_requirements.memory_type_bits;
        let own = dedicated.prefers_dedicated_allocation != 0
            || dedicated.requires_dedicated_allocation != 0;
        (allowed, own)
    }

    unsafe fn uuids(&self) -> ([u8; 16], [u8; 16]) {
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        // SAFETY: as the caller says.
        unsafe { (self.instance_1_1.get_physical_device_properties2)(self.physical, &mut props) };
        (id.device_uuid, id.driver_uuid)
    }

    #[cfg(windows)]
    unsafe fn export(&self, memory: vk::DeviceMemory) -> Result<Handle> {
        use std::os::windows::io::FromRawHandle;
        let info = vk::MemoryGetWin32HandleInfoKHR::default()
            .memory(memory)
            .handle_type(HANDLE);
        let mut handle: vk::HANDLE = 0;
        // SAFETY: `memory` is the device's, exportable as `HANDLE`; the handle is ours to close.
        unsafe {
            match (self.memory.get_memory_win32_handle_khr)(self.device, &info, &mut handle) {
                vk::Result::SUCCESS if handle != 0 => Ok(
                    std::os::windows::io::OwnedHandle::from_raw_handle(handle as _),
                ),
                e => Err(Error::Invalid(format!("exporting a frame's memory: {e}"))),
            }
        }
    }

    #[cfg(not(windows))]
    unsafe fn export(&self, memory: vk::DeviceMemory) -> Result<Handle> {
        use std::os::fd::FromRawFd;
        let info = vk::MemoryGetFdInfoKHR::default()
            .memory(memory)
            .handle_type(HANDLE);
        let mut fd = -1;
        // SAFETY: as the caller says; the descriptor is a new one, ours.
        unsafe {
            match (self.memory.get_memory_fd_khr)(self.device, &info, &mut fd) {
                vk::Result::SUCCESS if fd >= 0 => Ok(std::os::fd::OwnedFd::from_raw_fd(fd)),
                e => Err(Error::Invalid(format!("exporting a frame's memory: {e}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{memory_type, plane_size};
    use crate::ffmpeg::ffi::AVPixelFormat;
    use ash::vk;

    #[test]
    fn a_4_2_2_frames_chroma_planes_are_half_as_wide_and_as_tall() {
        let size = plane_size(AVPixelFormat::AV_PIX_FMT_YUV422P10LE, 3840, 2160, 1);
        assert_eq!(size, (1920, 2160));
    }

    #[test]
    fn an_optimal_images_memory_is_the_first_device_local_type_it_may_be_in() {
        let mut kinds = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: 3,
            memory_heap_count: 2,
            ..Default::default()
        };
        let local = vk::MemoryPropertyFlags::DEVICE_LOCAL;
        let host = vk::MemoryPropertyFlags::HOST_VISIBLE;
        kinds.memory_types[0] = vk::MemoryType {
            property_flags: host,
            heap_index: 1,
        };
        kinds.memory_types[1] = vk::MemoryType {
            property_flags: local,
            heap_index: 0,
        };
        kinds.memory_types[2] = vk::MemoryType {
            property_flags: local,
            heap_index: 0,
        };
        kinds.memory_heaps[0].size = 16 << 30;
        kinds.memory_heaps[1].size = 32 << 30;
        let found = memory_type(&kinds, 0b101, vk::ImageTiling::OPTIMAL, 1 << 24);
        assert_eq!(found, Some(2));
    }
}
