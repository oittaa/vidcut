//! Direct3D 11's frames on a Vulkan device (Windows), for libplacebo there.
//! FFmpeg maps no Direct3D 11 frames into Vulkan, so each frame's picture
//! is copied into a texture of its own (`hw::Keeper`), opened on the Vulkan
//! device by its NT handle, and taken over from Direct3D 11 there with what
//! it holds, as the viewer's are on wgpu's (gui/acquire.rs): an acquire
//! from the external queue family in `GENERAL`, which keeps the picture.

use std::sync::{Arc, Mutex};

use ::windows::Win32::Foundation::{CloseHandle, HANDLE};
use ::windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use ::windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, IDXGIResource1};
use ::windows::core::Interface;
use ash::vk;
use ash::vk::Handle;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};
use crate::hw::Device;

/// Opens Direct3D 11's frames on a Vulkan device on the same GPU.
pub(crate) struct Importer {
    shared: Arc<Shared>,
    win32: ash::khr::external_memory_win32::Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    /// The device's queue families, which the images are shared among, and
    /// the queue the takeovers are submitted on.
    families: Vec<u32>,
    queue: (u32, vk::Queue),
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

/// What the imported frames' release needs: the device, and the imports
/// libplacebo let go of, freed once the GPU is done with them
/// ([`Importer::sweep`]).
struct Shared {
    vulkan: Device,
    device: ash::Device,
    /// Where the takeovers' command buffers come from.
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

/// How long freeing an import waits for the GPU to be done with it.
const DONE: std::time::Duration = std::time::Duration::from_secs(5);

// SAFETY: Vulkan handles and a frame, freed on whichever thread sweeps.
unsafe impl Send for Imported {}

impl Shared {
    /// Frees `imported`'s Vulkan objects once its semaphore says the GPU is
    /// done with it ([`DONE`] at most); the texture goes with it.
    ///
    /// # Safety
    /// Nothing submits work on it any more.
    unsafe fn free(&self, imported: Imported) {
        // SAFETY: as the caller says; the objects are this device's.
        unsafe {
            let semaphores = [imported.semaphore];
            let values = [imported.last];
            let waited = self.device.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&semaphores)
                    .values(&values),
                DONE.as_nanos() as u64,
            );
            if waited.is_err() {
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
        // SAFETY: FFmpeg's Vulkan device, its calls looked up through its
        // own loader; objects made here are freed on failure or with it.
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
            let mut families: Vec<u32> = dev.qf[..dev.nb_qf.max(0) as usize]
                .iter()
                .map(|q| q.idx as u32)
                .collect();
            families.sort_unstable();
            families.dedup();
            // The takeover goes on a graphics queue, as libplacebo renders.
            let graphics = dev.qf[..dev.nb_qf.max(0) as usize]
                .iter()
                .find(|q| q.flags & vk::QueueFlags::GRAPHICS.as_raw() != 0)
                .ok_or_else(|| Error::Invalid("no Vulkan graphics queue".into()))?
                .idx as u32;
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(graphics)
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                    None,
                )
                .map_err(|e| Error::Invalid(format!("making a Vulkan command pool: {e}")))?;
            let queue = device.get_device_queue(graphics, 0);
            let shared = Arc::new(Shared {
                vulkan: vulkan.clone(),
                device,
                pool,
                retired: Mutex::new(Vec::new()),
            });
            let mut importer = Self {
                shared,
                win32,
                memory,
                families,
                queue: (graphics, queue),
                frames: std::ptr::null_mut(),
                image: Default::default(),
                keeper: None,
            };
            // Frames of the picture's layout and (even) size, read only:
            // their images come from Direct3D 11. They lock nothing, as
            // only libplacebo's thread uses them, and frames made by hand
            // have none of FFmpeg's own locks.
            let size = (
                first.width().next_multiple_of(2),
                first.height().next_multiple_of(2),
            );
            let frames = ffi::av_hwframe_ctx_alloc(vulkan.0);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for Vulkan frames".into()));
            }
            importer.frames = frames;
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
            let (texture, index) = {
                let k = kept.as_ptr();
                ((*k).data[0] as *mut std::ffi::c_void, (*k).data[1] as usize)
            };
            if index != 0 {
                return Err(Error::Invalid("a kept picture in a texture array".into()));
            }
            let texture = ID3D11Texture2D::from_raw_borrowed(&texture)
                .ok_or_else(|| Error::Invalid("a picture without a texture".into()))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let resource: IDXGIResource1 = texture
                .cast()
                .map_err(|e| Error::Invalid(format!("sharing a Direct3D 11 texture: {e}")))?;
            let handle = resource
                .CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0, None)
                .map_err(|e| Error::Invalid(format!("sharing a Direct3D 11 texture: {e}")))?;
            let opened = self.open(handle, (desc.Width, desc.Height));
            // Vulkan holds the memory once it is imported.
            let _ = CloseHandle(handle);
            let (image, memory) = opened?;
            let device = &self.shared.device;
            let made = self.take_over(image);
            let (semaphore, command) = match made {
                Ok(m) => m,
                Err(e) => {
                    device.destroy_image(image, None);
                    device.free_memory(memory, None);
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

    /// The image of the texture whose NT handle is `handle`, `size` big,
    /// with its memory.
    ///
    /// # Safety
    /// `handle` is a live NT handle of a Direct3D 11 texture on this GPU.
    unsafe fn open(
        &self,
        handle: HANDLE,
        (w, h): (u32, u32),
    ) -> Result<(vk::Image, vk::DeviceMemory)> {
        let device = &self.shared.device;
        let kind = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
        let (format, usage, flags) = self.image;
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
            .sharing_mode(if self.families.len() > 1 {
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
            Ok((image, memory))
        }
    }

    /// Takes `image` over from Direct3D 11 with what it holds, on the
    /// device's graphics queue: a timeline semaphore at 1 once done.
    ///
    /// # Safety
    /// `image` is this device's, on memory Direct3D 11 wrote, not yet used.
    unsafe fn take_over(&self, image: vk::Image) -> Result<(vk::Semaphore, vk::CommandBuffer)> {
        let device = &self.shared.device;
        let failed =
            |what: &'static str| move |e: vk::Result| Error::Invalid(format!("{what}: {e}"));
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
                .map_err(failed("making a Vulkan semaphore"))?;
            let command = match device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.shared.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            ) {
                Ok(c) => c[0],
                Err(e) => {
                    device.destroy_semaphore(semaphore, None);
                    return Err(failed("making a Vulkan command buffer")(e));
                }
            };
            let recorded = (|| {
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
                    .dst_queue_family_index(if self.families.len() > 1 {
                        vk::QUEUE_FAMILY_IGNORED
                    } else {
                        self.queue.0
                    })
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    });
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                device.end_command_buffer(command)?;
                let values = [1u64];
                let mut signal =
                    vk::TimelineSemaphoreSubmitInfo::default().signal_semaphore_values(&values);
                let commands = [command];
                let semaphores = [semaphore];
                let submit = vk::SubmitInfo::default()
                    .push_next(&mut signal)
                    .command_buffers(&commands)
                    .signal_semaphores(&semaphores);
                // FFmpeg's queues are used under its lock, libplacebo's too.
                let (ctx, dev) = hwctx(&self.shared.vulkan);
                if let Some(lock) = dev.lock_queue {
                    lock(ctx, self.queue.0, 0);
                }
                let submitted = device.queue_submit(self.queue.1, &[submit], vk::Fence::null());
                if let Some(unlock) = dev.unlock_queue {
                    unlock(ctx, self.queue.0, 0);
                }
                submitted
            })();
            match recorded {
                Ok(()) => Ok((semaphore, command)),
                Err(e) => {
                    device.free_command_buffers(self.shared.pool, &[command]);
                    device.destroy_semaphore(semaphore, None);
                    Err(failed("taking a Direct3D 11 texture over on Vulkan")(e))
                }
            }
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
                self.shared.free(imported);
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
            v.queue_family[0] = if self.families.len() > 1 {
                vk::QUEUE_FAMILY_IGNORED
            } else {
                self.queue.0
            };
            let release = Box::into_raw(Box::new(Release {
                shared: self.shared.clone(),
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
                .shared
                .retired
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for imported in retired {
            // SAFETY: as this says: the GPU is done with them.
            unsafe { self.shared.free(imported) };
        }
    }
}

impl Drop for Importer {
    fn drop(&mut self) {
        // SAFETY: its own reference. The imports go with `Shared`, once the
        // last frame does and the device is idle.
        unsafe { ffi::av_buffer_unref(&mut self.frames) };
    }
}

/// What a frame's buffer frees: an import, retired to `shared`.
struct Release {
    shared: Arc<Shared>,
    imported: Imported,
}

/// Retires a frame's import when FFmpeg lets go of the frame: the GPU may
/// still be reading it, so it waits for [`Importer::sweep`] (or the device
/// going idle). The AVVkFrame is freed here.
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
