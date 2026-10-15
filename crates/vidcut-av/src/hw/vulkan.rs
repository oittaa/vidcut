//! Vulkan's frames (ProRes decoded on NVIDIA's GPU, [`super::Kind::Vulkan`])
//! for the viewer: kept once the GPU is done decoding them, and handed over
//! as each plane's memory with how FFmpeg made the plane's image and
//! allocated its memory (hwcontext_vulkan.c `create_frame`, `alloc_mem`).
//! An opaque handle imports only into an image made the same way, into
//! memory allocated the same way, on the same GPU and driver, so the
//! viewer's device repeats what this gives. FFmpeg's frames say where their
//! images are (`AVVkFrame`: layout, queue family, "updated after every
//! barrier"): the viewer acquires them from there ([`layout`]) and says
//! where it left them ([`acquired_outside`]). It keeps its imports of a
//! frame's images by their [`Identity`].
//!
//! Each memory object is exported once, and its handle kept until FFmpeg
//! frees the frames context, after the memory (`SHARED`); each import gets
//! a duplicate. NVIDIA's Windows driver gives the same NT handle for every
//! export of a memory object: closed, it is gone for the exports after it,
//! and open, it keeps the memory after FFmpeg frees it.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;

use ash::vk;
use ash::vk::Handle as _;

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, frame};

/// How long the GPU may take to decode a frame the player keeps.
const DECODED: std::time::Duration = std::time::Duration::from_secs(5);

/// `f`, a Vulkan frame, kept for the viewer: another reference, once the GPU
/// is done decoding it. The viewer's device doesn't wait on FFmpeg's
/// semaphores, so the decode thread waits for them here.
pub(crate) fn keep(f: &frame::Video) -> Result<frame::Video> {
    // SAFETY: a Vulkan frame's own device and AVVkFrame, alive as long as
    // it.
    unsafe {
        let (dev, _, vkf) = parts(f)?;
        crate::cuda::wait_signalled(dev, &[vkf.cast_const()], DECODED)
    }
    .map_err(|e| Error::Invalid(format!("decoding on Vulkan: {e}")))?;
    crate::export::video::frame_ref(f)
}

/// The Vulkan device, the frames context and the `AVVkFrame` of `f`. The
/// last two are FFmpeg's to change, so they stay pointers.
///
/// # Safety
/// `f` is a frame of FFmpeg's; what is returned lives as long as it.
unsafe fn parts(
    f: &frame::Video,
) -> Result<(
    &ffi::AVVulkanDeviceContext,
    *mut ffi::AVHWFramesContext,
    *mut ffi::AVVkFrame,
)> {
    // SAFETY: the frame's own fields; a Vulkan frames context's device is a
    // Vulkan device context.
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

/// The handle type FFmpeg makes its frames' memory exportable as: an
/// opaque Win32 handle (an NT handle) on Windows, an opaque file descriptor
/// elsewhere.
#[cfg(windows)]
pub const HANDLE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
#[cfg(not(windows))]
pub const HANDLE: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD;

/// A handle of exported memory: an NT handle, or a file descriptor.
#[cfg(windows)]
pub type Handle = std::os::windows::io::OwnedHandle;
#[cfg(not(windows))]
pub type Handle = std::os::fd::OwnedFd;

/// The handles of the memory exported from each frames context so far: one
/// for each memory object, closed when FFmpeg frees the context
/// ([`close_shared`]).
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

/// A frames context's exported handles, by memory object.
struct Shared {
    frames: usize,
    handles: Vec<(u64, Handle)>,
}

/// A new handle of `memory`, FFmpeg's, of the frames context `fc`: a
/// duplicate of the one exported the first time, which `SHARED` keeps.
/// The first export from `fc` has FFmpeg close them when it frees `fc`.
///
/// # Safety
/// `fc` is live, a frame of it held, and `memory` its and `fns`'s
/// device's, allocated exportable as [`HANDLE`].
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
            // SAFETY: as the caller says. FFmpeg reads `free` when it frees
            // the context, which the frame held holds off.
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

/// Closes the handles of `fc`'s memory: FFmpeg calls it when it frees
/// `fc`, once the memory is freed ("The pool will be freed strictly before
/// this struct's free() callback is invoked").
unsafe extern "C" fn close_shared(fc: *mut ffi::AVHWFramesContext) {
    let frames = fc as usize;
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    shared.retain(|s| s.frames != frames);
}

/// Which images a Vulkan frame has: its frames context and each image's
/// memory object. FFmpeg's pool hands the same frames out again while the
/// frames context lives, and frees them only with it, so while it lives
/// the same identity names the same images.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    /// The frames context, by its address.
    pub frames: usize,
    pub memory: Vec<u64>,
}

impl Identity {
    /// `f`'s, a Vulkan frame.
    pub fn of(f: &frame::Video) -> Result<Self> {
        // SAFETY: the frame's own frames context and AVVkFrame, alive as
        // long as it; set when FFmpeg made the frame.
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

/// The layout FFmpeg's last barrier left `f`'s images in, a Vulkan frame's
/// (its barriers take the first image's for all).
pub fn layout(f: &frame::Video) -> Result<vk::ImageLayout> {
    // SAFETY: the frame's own frames context and AVVkFrame, alive as long
    // as it, read while locked as FFmpeg changes them.
    unsafe {
        let (_, fc, vkf) = parts(f)?;
        let _locked = Locked::new(fc, vkf);
        Ok(vk::ImageLayout::from_raw((*vkf).layout[0]))
    }
}

/// One plane of a Vulkan frame, for another device on the same GPU to
/// import: its memory, exported, how FFmpeg allocated it, and how FFmpeg
/// made the image bound to it (`VkImageCreateInfo`), which the importer
/// repeats.
pub struct Plane {
    /// The memory, as a handle of ours to close.
    pub memory: Handle,
    /// Its size and memory type, and whether it is the image's own
    /// (dedicated), as FFmpeg allocated it.
    pub size: u64,
    pub memory_type: u32,
    pub dedicated: bool,
    pub format: vk::Format,
    pub extent: (u32, u32),
    pub layers: u32,
    pub tiling: vk::ImageTiling,
    pub usage: vk::ImageUsageFlags,
    pub flags: vk::ImageCreateFlags,
    /// The handle types it was made exportable as
    /// (`VkExternalMemoryImageCreateInfo`).
    pub handle_types: vk::ExternalMemoryHandleTypeFlags,
    /// The queue families that share it: `CONCURRENT` where more than one.
    pub families: Vec<u32>,
}

/// A Vulkan frame's planes, their memory exported, and the GPU and driver
/// they are on, which an opaque handle is imported on only.
pub struct Exported {
    pub planes: Vec<Plane>,
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

impl Exported {
    /// The planes of `f`, a Vulkan frame whose planes are images of their
    /// own with memory of their own, as FFmpeg's ProRes decoder makes them
    /// (vulkan_decode.c `ff_vk_frame_params`). Each export gives new
    /// handles.
    pub fn of(f: &frame::Video) -> Result<Self> {
        // SAFETY: the frame's own device, frames context and AVVkFrame,
        // alive as long as it; Vulkan's calls through FFmpeg's loader on its
        // device's objects.
        unsafe {
            let (dev, fc, vkf) = parts(f)?;
            // Set when FFmpeg made the frame.
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

/// Records in `f`, a Vulkan frame, that another device on the GPU acquired
/// its images from the external queue family into `layout`: FFmpeg's next
/// barrier on them acquires them back from there, as it does a frame it
/// gave CUDA.
pub fn acquired_outside(f: &frame::Video, layout: vk::ImageLayout) -> Result<()> {
    // SAFETY: the frame's own frames context and AVVkFrame, alive as long
    // as it, changed only while locked as FFmpeg changes them.
    unsafe {
        let (_, fc, vkf) = parts(f)?;
        let _locked = Locked::new(fc, vkf);
        let images = (*vkf).img.iter().take_while(|&&i| i != 0).count();
        for i in 0..images {
            (*vkf).layout[i] = layout.as_raw();
            (*vkf).queue_family[i] = vk::QUEUE_FAMILY_EXTERNAL;
            // None: FFmpeg 8.0 keeps 32-bit access flags here, 9.0 64-bit.
            (*vkf).access[i] = 0;
        }
    }
    Ok(())
}

/// A Vulkan frame locked as its frames context locks it, so FFmpeg's
/// threads don't change it meanwhile; unlocked when dropped.
struct Locked {
    fc: *mut ffi::AVHWFramesContext,
    vkf: *mut ffi::AVVkFrame,
}

impl Locked {
    /// # Safety
    /// `vkf` is a frame of `fc`'s, alive while this is.
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

/// Each of `families` once, in order, as FFmpeg lists its images' queue
/// families (`img_qfs`).
fn families(families: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for f in families {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// The size of plane `plane` of a `width`×`height` frame of `format`, as
/// FFmpeg makes its image (hwcontext_vulkan.c `get_plane_wh`): a planar YUV
/// frame's chroma planes subsampled, the rest the frame's size.
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

/// The memory type FFmpeg allocates an image's memory in
/// (hwcontext_vulkan.c `alloc_mem`): the first of those `allowed` with the
/// properties its tiling needs (device-local; host-visible for linear),
/// in a heap that holds `size`.
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

/// Whether FFmpeg's device `dev` turned on the extension `name`.
///
/// # Safety
/// `dev` is FFmpeg's live Vulkan device context.
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

/// The calls the export makes on FFmpeg's Vulkan device, through its
/// loader.
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
    /// `dev`'s, where it exports memory as [`HANDLE`].
    ///
    /// # Safety
    /// `dev` is FFmpeg's live Vulkan device context.
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
        // SAFETY: the loader's lookup of the instance's calls, each as
        // vulkan.h declares it, or null.
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

    /// The handle types FFmpeg made its images exportable as, as it asks
    /// for them (hwcontext_vulkan.c `try_export_flags`): those the driver
    /// finds compatible with [`HANDLE`] for images of `format` (the first
    /// plane's), `tiling` and `usage`.
    ///
    /// # Safety
    /// The device is live.
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

    /// The GPU's memory types and heaps.
    ///
    /// # Safety
    /// The device is live.
    unsafe fn memory_properties(&self) -> vk::PhysicalDeviceMemoryProperties {
        let mut props = vk::PhysicalDeviceMemoryProperties::default();
        // SAFETY: as the caller says.
        unsafe { (self.instance.get_physical_device_memory_properties)(self.physical, &mut props) };
        props
    }

    /// The memory types `image` may be in, and whether the driver prefers
    /// or requires memory of its own for it, which FFmpeg then gives it
    /// (`alloc_bind_mem`).
    ///
    /// # Safety
    /// `image` is the device's, alive.
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

    /// The GPU's UUID and its driver's.
    ///
    /// # Safety
    /// The device is live.
    unsafe fn uuids(&self) -> ([u8; 16], [u8; 16]) {
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        // SAFETY: as the caller says.
        unsafe { (self.instance_1_1.get_physical_device_properties2)(self.physical, &mut props) };
        (id.device_uuid, id.driver_uuid)
    }

    /// `memory`, FFmpeg's, exported as an NT handle of ours to close. The
    /// same memory gives the same handle each time (`SHARED`).
    ///
    /// # Safety
    /// `memory` is the device's, allocated exportable as [`HANDLE`].
    #[cfg(windows)]
    unsafe fn export(&self, memory: vk::DeviceMemory) -> Result<Handle> {
        use std::os::windows::io::FromRawHandle;
        let info = vk::MemoryGetWin32HandleInfoKHR::default()
            .memory(memory)
            .handle_type(HANDLE);
        let mut handle: vk::HANDLE = 0;
        // SAFETY: as the caller says; the handle is a new one, ours to
        // close.
        unsafe {
            match (self.memory.get_memory_win32_handle_khr)(self.device, &info, &mut handle) {
                vk::Result::SUCCESS if handle != 0 => Ok(
                    std::os::windows::io::OwnedHandle::from_raw_handle(handle as _),
                ),
                e => Err(Error::Invalid(format!("exporting a frame's memory: {e}"))),
            }
        }
    }

    /// `memory`, FFmpeg's, exported as a new file descriptor of ours.
    ///
    /// # Safety
    /// `memory` is the device's, allocated exportable as [`HANDLE`].
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
        // Types 0 and 2 allowed: 0 isn't device-local.
        let found = memory_type(&kinds, 0b101, vk::ImageTiling::OPTIMAL, 1 << 24);
        assert_eq!(found, Some(2));
    }
}
