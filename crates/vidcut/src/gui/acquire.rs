//! Images another API or another Vulkan device wrote, taken over by wgpu's
//! queue with what they hold: VA-API's surfaces imported as DMA-BUFs
//! (Linux), Direct3D 11's textures opened by their NT handles (Windows),
//! the planes FFmpeg's Vulkan device decoded ProRes into.

use ash::vk;

/// Who wrote the images, which says the queue families of their acquire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// Another API: from the foreign queue family, to wgpu's queue's.
    Api,
    /// Another Vulkan device on this GPU (FFmpeg's), which left the images
    /// in `layout`: from the external queue family; to wgpu's queue's, or
    /// to none in particular (`IGNORED`) for images its queue families
    /// share (`CONCURRENT`), as Vulkan has it for those.
    Vulkan {
        concurrent: bool,
        layout: vk::ImageLayout,
    },
}

/// Takes images another API or device wrote over to wgpu's queue with their
/// contents: an acquire from the foreign or external queue family, which
/// wgpu can't record, recorded raw into a command buffer of wgpu's own and
/// submitted through wgpu's queue ahead of the work that reads them. A
/// submission of our own on wgpu's queue could meet the UI thread's at the
/// same time, which Vulkan forbids; wgpu keeps them apart. An imported image
/// starts `UNDEFINED`, and wgpu's first barrier from there lets the driver
/// drop what the memory holds: Mesa's drivers then set up compression state
/// afresh (RADV its metadata; ANV its fast-clear state and aux surface), and
/// a compressed picture is lost. Acquired from outside in the layout it was
/// left in (`GENERAL` for other APIs' images; FFmpeg's frames say theirs),
/// the driver keeps it: RADV does nothing, ANV keeps the aux surface the
/// modifier shares.
pub(super) struct Acquirer {
    device: ash::Device,
    /// wgpu's queue's family, and the one other APIs' images come from.
    family: u32,
    foreign: u32,
}

impl Acquirer {
    /// One for wgpu's Vulkan device.
    pub(super) fn new(hal: &wgpu::hal::vulkan::Device) -> Self {
        // `VK_QUEUE_FAMILY_FOREIGN_EXT` is for an owner outside Vulkan, but
        // takes an extension wgpu doesn't turn on; Mesa takes the external
        // family the same way.
        let foreign = if hal
            .enabled_device_extensions()
            .contains(&ash::ext::queue_family_foreign::NAME)
        {
            vk::QUEUE_FAMILY_FOREIGN_EXT
        } else {
            vk::QUEUE_FAMILY_EXTERNAL
        };
        Self {
            device: hal.raw_device().clone(),
            family: hal.queue_family_index(),
            foreign,
        }
    }

    /// Records into `cmd` the acquire of `images` (color images of one
    /// plane, one level), which `source` wrote, into `layout`, for `access`
    /// at `stage`. Work submitted after it on the queue finds them so: a
    /// barrier waits for nothing before it and holds up everything after it
    /// in submission order, other submissions too.
    ///
    /// # Safety
    /// `cmd` is a command buffer of this device being recorded for wgpu's
    /// queue; the images are this device's, on memory `source` wrote, not
    /// yet used by wgpu.
    pub(super) unsafe fn record(
        &self,
        cmd: vk::CommandBuffer,
        images: &[vk::Image],
        source: Source,
        layout: vk::ImageLayout,
        stage: vk::PipelineStageFlags,
        access: vk::AccessFlags,
    ) {
        let (from, to, old) = match source {
            Source::Api => (self.foreign, self.family, vk::ImageLayout::GENERAL),
            Source::Vulkan { concurrent, layout } => {
                let to = if concurrent {
                    vk::QUEUE_FAMILY_IGNORED
                } else {
                    self.family
                };
                (vk::QUEUE_FAMILY_EXTERNAL, to, layout)
            }
        };
        let barriers: Vec<_> = images
            .iter()
            .map(|&image| {
                vk::ImageMemoryBarrier::default()
                    .dst_access_mask(access)
                    .old_layout(old)
                    .new_layout(layout)
                    .src_queue_family_index(from)
                    .dst_queue_family_index(to)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
            })
            .collect();
        // SAFETY: as the caller says.
        unsafe {
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
        }
    }
}
