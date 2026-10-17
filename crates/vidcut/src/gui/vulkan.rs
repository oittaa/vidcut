use std::collections::HashMap;

use ash::vk;
use vidcut_av::hw::vulkan::{HANDLE, Identity, Plane};

pub(super) struct Import {
    pub(super) images: Vec<vk::Image>,
    pub(super) views: [wgpu::TextureView; 3],
    pub(super) size: (u32, u32),
    pub(super) concurrent: bool,
    pub(super) _textures: Vec<wgpu::Texture>,
}

#[derive(Default)]
pub(super) struct Imports {
    frames: Option<usize>,
    kept: HashMap<Identity, Import>,
}

impl Imports {
    pub(super) fn get_or_import(
        &mut self,
        identity: Identity,
        import: impl FnOnce() -> Result<Import, String>,
    ) -> Result<(&Import, bool), String> {
        if self.frames != Some(identity.frames) {
            self.clear();
            self.frames = Some(identity.frames);
        }
        let new = !self.kept.contains_key(&identity);
        if new {
            self.kept.insert(identity.clone(), import()?);
        }
        Ok((&self.kept[&identity], new))
    }

    pub(super) fn clear(&mut self) {
        self.frames = None;
        let old = std::mem::take(&mut self.kept);
        if old.is_empty() {
            return;
        }
        let _ = crate::threads::closing(
            "vidcut-frees-imports",
            crate::threads::Work::Interactive,
            move || drop(old),
        );
    }
}

pub(super) unsafe fn uuids(hal: &wgpu::hal::vulkan::Device) -> ([u8; 16], [u8; 16]) {
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    // SAFETY: as the caller says; reads the physical device's properties.
    unsafe {
        hal.shared_instance()
            .raw_instance()
            .get_physical_device_properties2(hal.raw_physical_device(), &mut props);
    }
    (id.device_uuid, id.driver_uuid)
}

pub(super) struct Imported<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    format: wgpu::TextureFormat,
    size: wgpu::Extent3d,
}

impl Imported<'_> {
    pub(super) unsafe fn into_hal(
        self,
        hal: &wgpu::hal::vulkan::Device,
        label: wgpu::Label,
    ) -> (
        wgpu::hal::vulkan::Texture,
        wgpu::Extent3d,
        wgpu::TextureFormat,
    ) {
        let this = std::mem::ManuallyDrop::new(self);
        let desc = wgpu::hal::TextureDescriptor {
            label,
            size: this.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: this.format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        // SAFETY: made on this device as described, its memory its own.
        let texture = unsafe {
            hal.texture_from_raw(
                this.image,
                &desc,
                None,
                wgpu::hal::vulkan::TextureMemory::Dedicated(this.memory),
            )
        };
        (texture, this.size, this.format)
    }
}

impl Drop for Imported<'_> {
    fn drop(&mut self) {
        // SAFETY: made on this device; no work uses them.
        unsafe {
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

pub(super) unsafe fn import(
    hal: &wgpu::hal::vulkan::Device,
    plane: Plane,
) -> Result<Imported<'_>, String> {
    let format = match plane.format {
        vk::Format::R16_UNORM => wgpu::TextureFormat::R16Unorm,
        vk::Format::R8_UNORM => wgpu::TextureFormat::R8Unorm,
        other => return Err(format!("a plane of {other:?}")),
    };
    if plane.layers != 1 {
        return Err(format!("a plane of {} layers", plane.layers));
    }
    let raw = hal.raw_device();
    let (width, height) = plane.extent;
    let mut external =
        vk::ExternalMemoryImageCreateInfo::default().handle_types(plane.handle_types);
    let sharing = if plane.families.len() > 1 {
        vk::SharingMode::CONCURRENT
    } else {
        vk::SharingMode::EXCLUSIVE
    };
    let info = vk::ImageCreateInfo::default()
        .flags(plane.flags)
        .image_type(vk::ImageType::TYPE_2D)
        .format(plane.format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(plane.tiling)
        .usage(plane.usage)
        .sharing_mode(sharing)
        .queue_family_indices(&plane.families)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external);
    // SAFETY: as the caller says; the image is freed on each failure below.
    let image = unsafe { raw.create_image(&info, None) }
        .map_err(|e| format!("making an image for a plane: {e}"))?;
    let fail = |e: String| {
        // SAFETY: made just above, used by nothing.
        unsafe { raw.destroy_image(image, None) };
        e
    };
    // SAFETY: the image is this device's.
    let needs = unsafe { raw.get_image_memory_requirements(image) };
    let allowed = 1u32.checked_shl(plane.memory_type).unwrap_or(0);
    if needs.size > plane.size || needs.memory_type_bits & allowed == 0 {
        return Err(fail(format!(
            "the display's image of a plane isn't FFmpeg's: it takes {} bytes in memory types \
             {:#b}, FFmpeg's has {} bytes in type {}",
            needs.size, needs.memory_type_bits, plane.size, plane.memory_type
        )));
    }
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    #[cfg(windows)]
    let mut import = {
        use std::os::windows::io::AsRawHandle;
        vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(HANDLE)
            .handle(plane.memory.as_raw_handle() as vk::HANDLE)
    };
    #[cfg(not(windows))]
    let fd = std::os::fd::IntoRawFd::into_raw_fd(plane.memory);
    #[cfg(not(windows))]
    let mut import = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(HANDLE)
        .fd(fd);
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(plane.size)
        .memory_type_index(plane.memory_type)
        .push_next(&mut import);
    let alloc = if plane.dedicated {
        alloc.push_next(&mut dedicated)
    } else {
        alloc
    };
    // SAFETY: `alloc` is FFmpeg's plane memory as allocated; on success Vulkan owns `fd`.
    let memory = match unsafe { raw.allocate_memory(&alloc, None) } {
        Ok(memory) => memory,
        Err(e) => {
            // SAFETY: Vulkan didn't take the descriptor.
            #[cfg(not(windows))]
            drop(unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) });
            return Err(fail(format!("importing a plane's memory: {e}")));
        }
    };
    // SAFETY: the memory was imported for this image.
    if let Err(e) = unsafe { raw.bind_image_memory(image, memory, 0) } {
        // SAFETY: imported just above, used by nothing.
        unsafe { raw.free_memory(memory, None) };
        return Err(fail(format!("binding a plane's memory: {e}")));
    }
    Ok(Imported {
        device: raw,
        image,
        memory,
        format,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    })
}
