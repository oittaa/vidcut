//! Direct3D 11's frames and libplacebo on a Vulkan device of the same GPU
//! (Windows). FFmpeg maps no Direct3D 11 frames into Vulkan or back, so
//! textures are opened on the Vulkan device by their NT handles here, as
//! the viewer's are on wgpu's (gui/acquire.rs):
//! - [`Importer`]: a decoded frame's picture, copied into a texture of its
//!   own (`hw::Keeper`), opened and taken over from Direct3D 11 with what it
//!   holds, an acquire from the external queue family in `GENERAL`.
//! - [`Exporter`]: libplacebo's picture copied into a frame of an encoder's
//!   own Direct3D 11 frames (AMF takes those alone), handed back to it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ::windows::Win32::Foundation::{CloseHandle, HANDLE};
use ::windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use ::windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1,
};
use ::windows::core::Interface;
use ash::vk;
use ash::vk::Handle;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};
use crate::hw::Device;

/// How long the CPU waits for the GPU with a picture before giving up.
const DONE: std::time::Duration = std::time::Duration::from_secs(5);

/// FFmpeg's Vulkan device, its calls, and what opening textures on it needs.
struct Vk {
    shared: Arc<Shared>,
    win32: ash::khr::external_memory_win32::Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    /// The device's queue families, which the images are shared among, and
    /// the graphics queue work is submitted on, as libplacebo renders.
    families: Vec<u32>,
    queue: (u32, vk::Queue),
}

/// What outlives the frames a [`Vk`] hands out: the device, and the
/// imports libplacebo let go of, freed once the GPU is done with them.
struct Shared {
    vulkan: Device,
    device: ash::Device,
    /// Where command buffers come from.
    pool: vk::CommandPool,
    retired: Mutex<Vec<Imported>>,
}

/// A frame opened on the Vulkan device.
struct Imported {
    image: vk::Image,
    memory: vk::DeviceMemory,
    semaphore: vk::Semaphore,
    /// The value its semaphore reaches once the GPU is done with it, as
    /// its frame said when FFmpeg let go of it.
    last: u64,
    command: vk::CommandBuffer,
    /// The texture it opens, held as long as the image.
    kept: frame::Video,
}

// SAFETY: Vulkan handles and a frame, freed on whichever thread sweeps.
unsafe impl Send for Imported {}

/// A texture of an encoder's frames, opened on the Vulkan device.
struct Opened {
    image: vk::Image,
    memory: vk::DeviceMemory,
}

impl Shared {
    /// Frees `imported`'s Vulkan objects once its semaphore says the GPU is
    /// done with it ([`DONE`] at most); the texture goes with it.
    ///
    /// # Safety
    /// Nothing submits work on it any more.
    unsafe fn free(&self, imported: Imported) {
        // SAFETY: as the caller says; the objects are this device's.
        unsafe {
            if self.wait(imported.semaphore, imported.last).is_err() {
                // A GPU that never finishes it: its objects are left, not
                // freed under work that may still read them.
                std::mem::forget(imported);
                return;
            }
            self.device
                .free_command_buffers(self.pool, &[imported.command]);
            self.device.destroy_semaphore(imported.semaphore, None);
            self.device.destroy_image(imported.image, None);
            self.device.free_memory(imported.memory, None);
        }
        drop(imported.kept);
    }

    /// Waits ([`DONE`] at most) until `semaphore` reaches `value`.
    ///
    /// # Safety
    /// `semaphore` is a timeline semaphore of this device.
    unsafe fn wait(&self, semaphore: vk::Semaphore, value: u64) -> Result<()> {
        let semaphores = [semaphore];
        let values = [value];
        // SAFETY: as the caller says.
        unsafe {
            self.device.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&semaphores)
                    .values(&values),
                DONE.as_nanos() as u64,
            )
        }
        .map_err(|e| Error::Invalid(format!("waiting for the GPU's Vulkan work: {e}")))
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // The last frame is gone: nothing submits work on the imports. The
        // device itself goes after (`vulkan`).
        // SAFETY: the device is alive; its own objects.
        unsafe {
            let retired =
                std::mem::take(&mut *self.retired.lock().unwrap_or_else(|e| e.into_inner()));
            for imported in retired {
                self.free(imported);
            }
            self.device.destroy_command_pool(self.pool, None);
        }
    }
}

/// The Vulkan device's context.
///
/// # Safety
/// `vulkan` is a Vulkan device context.
unsafe fn hwctx(vulkan: &Device) -> (*mut ffi::AVHWDeviceContext, &ffi::AVVulkanDeviceContext) {
    // SAFETY: as the caller says.
    unsafe {
        let ctx = (*vulkan.0).data as *mut ffi::AVHWDeviceContext;
        (ctx, &*((*ctx).hwctx as *const ffi::AVVulkanDeviceContext))
    }
}

impl Vk {
    /// FFmpeg's Vulkan device `vulkan`, its calls looked up through its own
    /// loader.
    fn load(vulkan: &Device) -> Result<Self> {
        // SAFETY: FFmpeg's Vulkan device, alive as long as `vulkan`; objects
        // made here are freed with `Shared`.
        unsafe {
            let (_, dev) = hwctx(vulkan);
            let loader = dev
                .get_proc_addr
                .ok_or_else(|| Error::Invalid("no Vulkan loader".into()))?;
            let entry = ash::StaticFn {
                get_instance_proc_addr: std::mem::transmute::<
                    unsafe extern "C" fn(
                        ffi::VkInstance,
                        *const std::ffi::c_char,
                    ) -> ffi::PFN_vkVoidFunction,
                    vk::PFN_vkGetInstanceProcAddr,
                >(loader),
            };
            let instance =
                ash::Instance::load(&entry, vk::Instance::from_raw(dev.inst as usize as u64));
            let device = ash::Device::load(
                instance.fp_v1_0(),
                vk::Device::from_raw(dev.act_dev as usize as u64),
            );
            let physical = vk::PhysicalDevice::from_raw(dev.phys_dev as usize as u64);
            let win32 = ash::khr::external_memory_win32::Device::new(&instance, &device);
            let memory = instance.get_physical_device_memory_properties(physical);
            let queues = &dev.qf[..dev.nb_qf.max(0) as usize];
            let mut families: Vec<u32> = queues.iter().map(|q| q.idx as u32).collect();
            families.sort_unstable();
            families.dedup();
            let graphics = queues
                .iter()
                .find(|q| q.flags & vk::QueueFlags::GRAPHICS.as_raw() != 0)
                .ok_or_else(|| Error::Invalid("no Vulkan graphics queue".into()))?
                .idx as u32;
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(graphics)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| Error::Invalid(format!("making a Vulkan command pool: {e}")))?;
            let queue = device.get_device_queue(graphics, 0);
            Ok(Self {
                shared: Arc::new(Shared {
                    vulkan: vulkan.clone(),
                    device,
                    pool,
                    retired: Mutex::new(Vec::new()),
                }),
                win32,
                memory,
                families,
                queue: (graphics, queue),
            })
        }
    }

    fn device(&self) -> &ash::Device {
        &self.shared.device
    }

    /// Images shared among the device's queues, as FFmpeg's frames are, so
    /// libplacebo may use them on any of its queues.
    fn concurrent(&self) -> bool {
        self.families.len() > 1
    }

    /// The image of the texture whose NT handle is `handle`, `size` big,
    /// made as `image` says, with its memory.
    ///
    /// # Safety
    /// `handle` is a live NT handle of a Direct3D 11 texture on this GPU.
    unsafe fn open(
        &self,
        handle: HANDLE,
        (w, h): (u32, u32),
        (format, usage, flags): (vk::Format, vk::ImageUsageFlags, vk::ImageCreateFlags),
    ) -> Result<Opened> {
        let device = self.device();
        let kind = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(kind);
        let info = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .flags(flags)
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(if self.concurrent() {
                vk::SharingMode::CONCURRENT
            } else {
                vk::SharingMode::EXCLUSIVE
            })
            .queue_family_indices(&self.families)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: as the caller says; the image is freed on failure.
        unsafe {
            let image = device.create_image(&info, None).map_err(|e| {
                Error::Invalid(format!(
                    "making a Vulkan image of a Direct3D 11 texture: {e}"
                ))
            })?;
            let freeing = |e: String| {
                device.destroy_image(image, None);
                Error::Invalid(e)
            };
            let mut props = vk::MemoryWin32HandlePropertiesKHR::default();
            self.win32
                .get_memory_win32_handle_properties(kind, handle.0 as vk::HANDLE, &mut props)
                .map_err(|e| freeing(format!("reading a Direct3D 11 texture's memory: {e}")))?;
            let mut dedicated = vk::MemoryDedicatedRequirements::default();
            let mut needs = vk::MemoryRequirements2::default().push_next(&mut dedicated);
            device.get_image_memory_requirements2(
                &vk::ImageMemoryRequirementsInfo2::default().image(image),
                &mut needs,
            );
            let types = needs.memory_requirements.memory_type_bits & props.memory_type_bits;
            let Some(type_index) =
                (0..self.memory.memory_type_count).find(|&i| types & (1 << i) != 0)
            else {
                return Err(freeing(
                    "no Vulkan memory takes a Direct3D 11 texture".into(),
                ));
            };
            let mut import = vk::ImportMemoryWin32HandleInfoKHR::default()
                .handle_type(kind)
                .handle(handle.0 as vk::HANDLE);
            let mut only = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let alloc = vk::MemoryAllocateInfo::default()
                .push_next(&mut import)
                .push_next(&mut only)
                .allocation_size(needs.memory_requirements.size)
                .memory_type_index(type_index);
            let memory = device
                .allocate_memory(&alloc, None)
                .map_err(|e| freeing(format!("opening a Direct3D 11 texture on Vulkan: {e}")))?;
            if let Err(e) = device.bind_image_memory(image, memory, 0) {
                device.free_memory(memory, None);
                return Err(freeing(format!(
                    "binding a Direct3D 11 texture's memory: {e}"
                )));
            }
            Ok(Opened { image, memory })
        }
    }

    /// A command buffer of the device's pool.
    fn command(&self) -> Result<vk::CommandBuffer> {
        // SAFETY: the device's own pool.
        unsafe {
            self.device().allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.shared.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .map(|c| c[0])
        .map_err(|e| Error::Invalid(format!("making a Vulkan command buffer: {e}")))
    }

    /// Submits `command` on the graphics queue, under FFmpeg's lock (which
    /// libplacebo takes too), after `waits` and signalling `signals`:
    /// timeline semaphores at those values.
    ///
    /// # Safety
    /// `command` is recorded and ended; the semaphores are this device's.
    unsafe fn submit(
        &self,
        command: vk::CommandBuffer,
        waits: &[(vk::Semaphore, u64)],
        signals: &[(vk::Semaphore, u64)],
    ) -> Result<()> {
        let wait_semaphores: Vec<_> = waits.iter().map(|w| w.0).collect();
        let wait_values: Vec<_> = waits.iter().map(|w| w.1).collect();
        let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
        let signal_semaphores: Vec<_> = signals.iter().map(|s| s.0).collect();
        let signal_values: Vec<_> = signals.iter().map(|s| s.1).collect();
        let mut timeline = vk::TimelineSemaphoreSubmitInfo::default()
            .wait_semaphore_values(&wait_values)
            .signal_semaphore_values(&signal_values);
        let commands = [command];
        let submit = vk::SubmitInfo::default()
            .push_next(&mut timeline)
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&commands)
            .signal_semaphores(&signal_semaphores);
        // SAFETY: as the caller says; FFmpeg's queues are used under its
        // lock.
        unsafe {
            let (ctx, dev) = hwctx(&self.shared.vulkan);
            if let Some(lock) = dev.lock_queue {
                lock(ctx, self.queue.0, 0);
            }
            let submitted = self
                .device()
                .queue_submit(self.queue.1, &[submit], vk::Fence::null());
            if let Some(unlock) = dev.unlock_queue {
                unlock(ctx, self.queue.0, 0);
            }
            submitted
        }
        .map_err(|e| Error::Invalid(format!("submitting Vulkan work: {e}")))
    }
}

/// A texture's NT handle, to open it on Vulkan: read only, or read and
/// written.
///
/// # Safety
/// `texture` is a live Direct3D 11 texture made shareable by NT handles.
unsafe fn shared_handle(
    texture: *mut std::ffi::c_void,
    write: bool,
) -> Result<(HANDLE, (u32, u32))> {
    // SAFETY: as the caller says.
    unsafe {
        let texture = ID3D11Texture2D::from_raw_borrowed(&texture)
            .ok_or_else(|| Error::Invalid("a picture without a texture".into()))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let resource: IDXGIResource1 = texture
            .cast()
            .map_err(|e| Error::Invalid(format!("sharing a Direct3D 11 texture: {e}")))?;
        let access = if write {
            DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0
        } else {
            DXGI_SHARED_RESOURCE_READ.0
        };
        let handle = resource
            .CreateSharedHandle(None, access, None)
            .map_err(|e| Error::Invalid(format!("sharing a Direct3D 11 texture: {e}")))?;
        Ok((handle, (desc.Width, desc.Height)))
    }
}

/// The texture of a Direct3D 11 frame, which must be one of its own.
///
/// # Safety
/// `f` is a Direct3D 11 frame.
unsafe fn texture_of(f: &frame::Video) -> Result<*mut std::ffi::c_void> {
    // SAFETY: as the caller says: its texture and index.
    let (texture, index) = unsafe {
        let p = f.as_ptr();
        ((*p).data[0] as *mut std::ffi::c_void, (*p).data[1] as usize)
    };
    if index != 0 {
        return Err(Error::Invalid("a picture in a texture array".into()));
    }
    Ok(texture)
}

/// Opens Direct3D 11's frames on a Vulkan device on the same GPU.
pub(crate) struct Importer {
    vk: Vk,
    /// The frames context the imported frames are of: their layout and
    /// size, which tell libplacebo how to read them.
    pub(crate) frames: *mut ffi::AVBufferRef,
    /// The images' format, usage and flags, as the frames context says.
    image: (vk::Format, vk::ImageUsageFlags, vk::ImageCreateFlags),
    /// Copies the decoder's frames into textures of their own.
    keeper: Option<crate::hw::Keeper>,
}

// SAFETY: the frames context is reference-counted; the Vulkan objects are
// used by one thread at a time, the one feeding libplacebo.
unsafe impl Send for Importer {}

impl Importer {
    /// An importer onto `vulkan`, FFmpeg's Vulkan device on the GPU that
    /// decodes `first` (a Direct3D 11 frame), for frames like it.
    pub(crate) fn new(vulkan: &Device, first: &frame::Video) -> Result<Self> {
        let sw = crate::hw::sw_format(first);
        if !matches!(sw, format::Pixel::NV12 | format::Pixel::P010LE) {
            return Err(Error::Invalid(format!(
                "no opening {sw:?} frames on Vulkan"
            )));
        }
        let vk = Vk::load(vulkan)?;
        // Frames of the picture's layout and (even) size, read only: their
        // images come from Direct3D 11. They lock nothing, as only
        // libplacebo's thread uses them, and frames made by hand have none
        // of FFmpeg's own locks. A pool of FFmpeg's own gives black frames
        // where libplacebo wants a filler (`cuda::Blacks`).
        let size = (
            first.width().next_multiple_of(2),
            first.height().next_multiple_of(2),
        );
        // SAFETY: frames FFmpeg allocates on the device, freed with the
        // importer.
        unsafe {
            let frames = ffi::av_hwframe_ctx_alloc(vulkan.0);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for Vulkan frames".into()));
            }
            let mut importer = Self {
                vk,
                frames,
                image: Default::default(),
                keeper: None,
            };
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN;
            fc.sw_format = sw.into();
            fc.width = size.0 as i32;
            fc.height = size.1 as i32;
            let vk_fc = &mut *(fc.hwctx as *mut ffi::AVVulkanFramesContext);
            vk_fc.usage = (vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST)
                .as_raw() as ffi::VkImageUsageFlagBits;
            vk_fc.img_flags = vk::ImageCreateFlags::MUTABLE_FORMAT.as_raw();
            vk_fc.lock_frame = Some(no_lock);
            vk_fc.unlock_frame = Some(no_lock);
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "setting up Vulkan frames for Direct3D 11's".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let vk_fc = &*(fc.hwctx as *const ffi::AVVulkanFramesContext);
            importer.image = (
                vk::Format::from_raw(vk_fc.format[0]),
                vk::ImageUsageFlags::from_raw(vk_fc.usage as u32),
                vk::ImageCreateFlags::from_raw(vk_fc.img_flags),
            );
            Ok(importer)
        }
    }

    /// `f`, a Direct3D 11 frame, as a Vulkan frame of [`frames`]: its
    /// picture copied into a texture of its own, opened on the device, and
    /// taken over there; libplacebo waits for that on the frame's
    /// semaphore. Its timestamp and colors stay.
    ///
    /// [`frames`]: Self::frames
    pub(crate) fn import(&mut self, f: &frame::Video) -> Result<frame::Video> {
        let kept = crate::hw::Keeper::keep(&mut self.keeper, f)?;
        // SAFETY: the kept frame's texture, alive as long as the frame; the
        // Vulkan objects are made on FFmpeg's device and handed to the
        // frame, which frees them through `Shared`.
        unsafe {
            let (handle, size) = shared_handle(texture_of(&kept)?, false)?;
            let opened = self.vk.open(handle, size, self.image);
            // Vulkan holds the memory once it is imported.
            let _ = CloseHandle(handle);
            let Opened { image, memory } = opened?;
            let (semaphore, command) = match self.take_over(image) {
                Ok(m) => m,
                Err(e) => {
                    self.vk.device().destroy_image(image, None);
                    self.vk.device().free_memory(memory, None);
                    return Err(e);
                }
            };
            let imported = Imported {
                image,
                memory,
                semaphore,
                last: 1,
                command,
                kept,
            };
            self.frame(f, imported)
        }
    }

    /// Takes `image` over from Direct3D 11 with what it holds, on the
    /// device's graphics queue: a timeline semaphore at 1 once done.
    ///
    /// # Safety
    /// `image` is this device's, on memory Direct3D 11 wrote, not yet used.
    unsafe fn take_over(&self, image: vk::Image) -> Result<(vk::Semaphore, vk::CommandBuffer)> {
        let device = self.vk.device();
        // SAFETY: as the caller says; objects made here are freed on
        // failure, or with the frame.
        unsafe {
            let mut timeline = vk::SemaphoreTypeCreateInfo::default()
                .semaphore_type(vk::SemaphoreType::TIMELINE)
                .initial_value(0);
            let semaphore = device
                .create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut timeline),
                    None,
                )
                .map_err(|e| Error::Invalid(format!("making a Vulkan semaphore: {e}")))?;
            let command = match self.vk.command() {
                Ok(c) => c,
                Err(e) => {
                    device.destroy_semaphore(semaphore, None);
                    return Err(e);
                }
            };
            let recorded = (|| -> std::result::Result<(), vk::Result> {
                device.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                // Acquired from Direct3D 11 in `GENERAL`, as other APIs
                // leave what they share: from `UNDEFINED` a driver may drop
                // the picture (gui/acquire.rs).
                let barrier = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .dst_queue_family_index(self.vk.ours())
                    .image(image)
                    .subresource_range(color());
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                device.end_command_buffer(command)
            })();
            let submitted = match recorded {
                Ok(()) => self.vk.submit(command, &[], &[(semaphore, 1)]),
                Err(e) => Err(Error::Invalid(format!(
                    "taking a Direct3D 11 texture over on Vulkan: {e}"
                ))),
            };
            if let Err(e) = submitted {
                device.free_command_buffers(self.vk.shared.pool, &[command]);
                device.destroy_semaphore(semaphore, None);
                return Err(e);
            }
            Ok((semaphore, command))
        }
    }

    /// The Vulkan frame of `imported`, timed and tagged as `f`; freeing it
    /// retires the import.
    ///
    /// # Safety
    /// `imported`'s takeover was submitted, signalling its semaphore at 1.
    unsafe fn frame(&self, f: &frame::Video, imported: Imported) -> Result<frame::Video> {
        // SAFETY: an AVVkFrame of FFmpeg's, filled as hwcontext_vulkan.h
        // says, owned by the frame's buffer, which retires the import.
        unsafe {
            let vkf = ffi::av_vk_frame_alloc();
            if vkf.is_null() {
                self.vk.shared.free(imported);
                return Err(Error::Invalid("no memory for a Vulkan frame".into()));
            }
            let v = &mut *vkf;
            v.img[0] = imported.image.as_raw();
            v.mem[0] = imported.memory.as_raw();
            v.tiling = vk::ImageTiling::OPTIMAL.as_raw();
            v.flags =
                vk::MemoryPropertyFlags::DEVICE_LOCAL.as_raw() as ffi::VkMemoryPropertyFlagBits;
            v.layout[0] = vk::ImageLayout::GENERAL.as_raw();
            v.sem[0] = imported.semaphore.as_raw();
            v.sem_value[0] = 1;
            v.queue_family[0] = self.vk.ours();
            let release = Box::into_raw(Box::new(Release {
                shared: self.vk.shared.clone(),
                imported,
            }));
            let buf = ffi::av_buffer_create(
                vkf.cast(),
                std::mem::size_of::<ffi::AVVkFrame>(),
                Some(retire),
                release.cast(),
                0,
            );
            if buf.is_null() {
                let release = Box::from_raw(release);
                release.shared.free(release.imported);
                ffi::av_free(vkf.cast());
                return Err(Error::Invalid("no memory for a Vulkan frame".into()));
            }
            let mut out = frame::Video::empty();
            let o = out.as_mut_ptr();
            (*o).buf[0] = buf;
            (*o).data[0] = vkf.cast();
            (*o).format = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN as i32;
            (*o).width = f.width() as i32;
            (*o).height = f.height() as i32;
            (*o).hw_frames_ctx = ffi::av_buffer_ref(self.frames);
            if (*o).hw_frames_ctx.is_null() {
                return Err(Error::Invalid("no memory for a Vulkan frame".into()));
            }
            ffi::av_frame_copy_props(o, f.as_ptr());
            Ok(out)
        }
    }

    /// Frees the imports libplacebo let go of. Call once the GPU finished a
    /// picture made after them.
    pub(crate) fn sweep(&self) {
        let retired = std::mem::take(
            &mut *self
                .vk
                .shared
                .retired
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for imported in retired {
            // SAFETY: as this says: nothing submits work on them any more.
            unsafe { self.vk.shared.free(imported) };
        }
    }
}

impl Drop for Importer {
    fn drop(&mut self) {
        // SAFETY: its own reference. The imports go with `Shared`, once the
        // last frame does.
        unsafe { ffi::av_buffer_unref(&mut self.frames) };
    }
}

impl Vk {
    /// The queue family the images belong to once taken over: none in
    /// particular where they are shared among the device's queues.
    fn ours(&self) -> u32 {
        if self.concurrent() {
            vk::QUEUE_FAMILY_IGNORED
        } else {
            self.queue.0
        }
    }
}

/// The whole of a one-level, one-layer image.
fn color() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Puts libplacebo's pictures into an encoder's own Direct3D 11 frames on
/// the same GPU (AMF takes those alone): each texture of their pool opened
/// on the Vulkan device once, the picture copied into it there and handed
/// back to Direct3D 11, done before the encoder gets it.
pub(crate) struct Exporter {
    vk: Vk,
    /// The encoder's frames: shareable textures (`d3d11::encoder_frames`).
    into: crate::hw::Frames,
    /// Their textures opened so far, by texture.
    opened: HashMap<usize, Opened>,
    /// Their images' format.
    format: vk::Format,
    command: vk::CommandBuffer,
}

// SAFETY: as `Importer`.
unsafe impl Send for Exporter {}

impl Exporter {
    /// An exporter from `vulkan`, FFmpeg's Vulkan device on the GPU, into
    /// `into`, an encoder's Direct3D 11 frames there (NV12 or P010).
    pub(crate) fn new(vulkan: &Device, into: &crate::hw::Frames) -> Result<Self> {
        // SAFETY: a live frames context's layout.
        let sw = format::Pixel::from(unsafe {
            (*((*into.as_ptr()).data as *const ffi::AVHWFramesContext)).sw_format
        });
        let format = match sw {
            format::Pixel::NV12 => vk::Format::G8_B8R8_2PLANE_420_UNORM,
            format::Pixel::P010LE => vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
            other => {
                return Err(Error::Invalid(format!(
                    "no copying Vulkan pictures into {other:?} frames"
                )));
            }
        };
        let vk = Vk::load(vulkan)?;
        let command = vk.command()?;
        // SAFETY: another reference to the encoder's live frames.
        let into = unsafe { crate::hw::Frames::of(into.as_ptr())? };
        Ok(Self {
            vk,
            into,
            opened: HashMap::new(),
            format,
            command,
        })
    }

    /// `picture`, a Vulkan frame libplacebo made on the device, copied into
    /// a frame of the encoder's, with its timestamp and colors; the copy is
    /// done when this returns.
    pub(crate) fn export(&mut self, picture: &frame::Video) -> Result<frame::Video> {
        // SAFETY: a frame of the encoder's pool, its texture opened once;
        // the picture's AVVkFrame used under its frames context's lock,
        // waited on and signalled as hwcontext_vulkan.h says.
        unsafe {
            let mut out = frame::Video::empty();
            let ret = ffi::av_hwframe_get_buffer(self.into.as_ptr(), out.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "getting an encoder's Direct3D 11 frame".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let texture = texture_of(&out)?;
            let image = match self.opened.get(&(texture as usize)) {
                Some(o) => o.image,
                None => {
                    let (handle, size) = shared_handle(texture, true)?;
                    let opened = self.vk.open(
                        handle,
                        size,
                        (
                            self.format,
                            vk::ImageUsageFlags::TRANSFER_DST,
                            vk::ImageCreateFlags::empty(),
                        ),
                    );
                    let _ = CloseHandle(handle);
                    let opened = opened?;
                    let image = opened.image;
                    self.opened.insert(texture as usize, opened);
                    image
                }
            };
            let fc = (*(*picture.as_ptr()).hw_frames_ctx).data as *mut ffi::AVHWFramesContext;
            let vk_fc = &*((*fc).hwctx as *const ffi::AVVulkanFramesContext);
            let vkf = (*picture.as_ptr()).data[0] as *mut ffi::AVVkFrame;
            if let Some(lock) = vk_fc.lock_frame {
                lock(fc, vkf);
            }
            let copied = self.copy(&mut *vkf, image, (picture.width(), picture.height()));
            if let Some(unlock) = vk_fc.unlock_frame {
                unlock(fc, vkf);
            }
            let done = copied?;
            self.vk.shared.wait(done.0, done.1)?;
            ffi::av_frame_copy_props(out.as_mut_ptr(), picture.as_ptr());
            (*out.as_mut_ptr()).width = picture.width() as i32;
            (*out.as_mut_ptr()).height = picture.height() as i32;
            Ok(out)
        }
    }

    /// Records and submits the copy of `vkf`'s picture, `size` big, into
    /// `image`, handing that back to Direct3D 11: after the picture's own
    /// semaphore, which it signals one step on. Returns that semaphore and
    /// value.
    ///
    /// # Safety
    /// `vkf` is a locked frame of this device; `image` an opened texture.
    unsafe fn copy(
        &self,
        vkf: &mut ffi::AVVkFrame,
        image: vk::Image,
        (w, h): (u32, u32),
    ) -> Result<(vk::Semaphore, u64)> {
        let device = self.vk.device();
        let planes = if vkf.img[1] == 0 { 1 } else { 2 };
        let sources: Vec<vk::Image> = (0..planes)
            .map(|i| vk::Image::from_raw(vkf.img[i]))
            .collect();
        // SAFETY: as the caller says; the command buffer is this
        // exporter's, its last use done (waited on).
        unsafe {
            let recorded = (|| -> std::result::Result<(), vk::Result> {
                device.reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())?;
                device.begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let mut before: Vec<vk::ImageMemoryBarrier> = sources
                    .iter()
                    .enumerate()
                    .map(|(i, &src)| {
                        vk::ImageMemoryBarrier::default()
                            .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                            .old_layout(vk::ImageLayout::from_raw(vkf.layout[i]))
                            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .image(src)
                            .subresource_range(color())
                    })
                    .collect();
                // Its old picture goes: written whole.
                before.push(
                    vk::ImageMemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::empty())
                        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                        .dst_queue_family_index(self.vk.ours())
                        .image(image)
                        .subresource_range(color()),
                );
                device.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &before,
                );
                // Luma whole, chroma at half each way (4:2:0).
                for (plane, (aspect, size)) in [
                    (vk::ImageAspectFlags::PLANE_0, (w, h)),
                    (
                        vk::ImageAspectFlags::PLANE_1,
                        (w.div_ceil(2), h.div_ceil(2)),
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let (src, src_aspect) = if planes == 1 {
                        (sources[0], aspect)
                    } else {
                        (sources[plane], vk::ImageAspectFlags::COLOR)
                    };
                    let layers = |aspect_mask| vk::ImageSubresourceLayers {
                        aspect_mask,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    };
                    let region = vk::ImageCopy {
                        src_subresource: layers(src_aspect),
                        src_offset: vk::Offset3D::default(),
                        dst_subresource: layers(aspect),
                        dst_offset: vk::Offset3D::default(),
                        extent: vk::Extent3D {
                            width: size.0,
                            height: size.1,
                            depth: 1,
                        },
                    };
                    device.cmd_copy_image(
                        self.command,
                        src,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
                    );
                }
                // Handed back to Direct3D 11 in `GENERAL`.
                let after = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(self.vk.ours())
                    .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .image(image)
                    .subresource_range(color());
                device.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[after],
                );
                device.end_command_buffer(self.command)
            })();
            recorded.map_err(|e| {
                Error::Invalid(format!("copying a picture into an encoder's frame: {e}"))
            })?;
            let waits: Vec<(vk::Semaphore, u64)> = (0..planes)
                .map(|i| (vk::Semaphore::from_raw(vkf.sem[i]), vkf.sem_value[i]))
                .collect();
            let signals: Vec<(vk::Semaphore, u64)> =
                waits.iter().map(|&(s, v)| (s, v + 1)).collect();
            self.vk.submit(self.command, &waits, &signals)?;
            for i in 0..planes {
                vkf.sem_value[i] += 1;
                vkf.layout[i] = vk::ImageLayout::TRANSFER_SRC_OPTIMAL.as_raw();
                vkf.access[i] = vk::AccessFlags2::TRANSFER_READ.as_raw();
            }
            Ok(signals[0])
        }
    }
}

impl Drop for Exporter {
    fn drop(&mut self) {
        // SAFETY: every copy was waited on; the textures stay the
        // encoder's.
        unsafe {
            let device = self.vk.device();
            device.free_command_buffers(self.vk.shared.pool, &[self.command]);
            for (_, opened) in self.opened.drain() {
                device.destroy_image(opened.image, None);
                device.free_memory(opened.memory, None);
            }
        }
    }
}

/// What a frame's buffer frees: an import, retired to `shared`.
struct Release {
    shared: Arc<Shared>,
    imported: Imported,
}

/// Retires a frame's import when FFmpeg lets go of the frame: the GPU may
/// still be reading it, so it waits for [`Importer::sweep`]. The AVVkFrame
/// is freed here.
unsafe extern "C" fn retire(opaque: *mut std::ffi::c_void, data: *mut u8) {
    // SAFETY: the Release `Importer::frame` boxed, and the AVVkFrame FFmpeg
    // allocated.
    unsafe {
        let release = Box::from_raw(opaque.cast::<Release>());
        let Release {
            shared,
            mut imported,
        } = *release;
        // What its users signalled last: the GPU is done with it there.
        imported.last = (*data.cast::<ffi::AVVkFrame>()).sem_value[0];
        ffi::av_free(data.cast());
        shared
            .retired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(imported);
    }
}

/// A frame lock that locks nothing: these frames are made by hand, without
/// FFmpeg's own locks, and only libplacebo's thread uses them.
unsafe extern "C" fn no_lock(_: *mut ffi::AVHWFramesContext, _: *mut ffi::AVVkFrame) {}
