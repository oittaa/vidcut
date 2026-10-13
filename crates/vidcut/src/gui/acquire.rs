//! Images another API wrote, taken over by wgpu's queue with what they
//! hold: VA-API's surfaces imported as DMA-BUFs (Linux), Direct3D 11's
//! textures opened by their NT handles (Windows).

use ash::vk;

/// Takes images another API wrote over to wgpu's queue with their
/// contents: an acquire from the external queue family, which wgpu can't
/// record, recorded raw into a command buffer of wgpu's own and submitted
/// through wgpu's queue ahead of the work that reads them. A submission of
/// our own on wgpu's queue could meet the UI thread's at the same time,
/// which Vulkan forbids; wgpu keeps them apart. An imported image starts
/// `UNDEFINED`, and wgpu's first barrier from there lets the driver drop
/// what the memory holds: Mesa's drivers then set up compression state
/// afresh (RADV its metadata; ANV its fast-clear state and aux surface),
/// and a compressed picture is lost. Acquired from the external queue in
/// `GENERAL`, as other APIs leave what they share, the driver keeps it:
/// RADV does nothing, ANV keeps the aux surface the modifier shares.
pub(super) struct Acquirer {
    device: ash::Device,
    /// wgpu's queue's family, and the one the images come from.
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
    /// plane, one level) into `layout`, for `access` at `stage`. Work
    /// submitted after it on the queue finds them so: a barrier waits for
    /// nothing before it and holds up everything after it in submission
    /// order, other submissions too.
    ///
    /// # Safety
    /// `cmd` is a command buffer of this device being recorded for wgpu's
    /// queue; the images are this device's, on memory another API wrote,
    /// not yet used by wgpu.
    pub(super) unsafe fn record(
        &self,
        cmd: vk::CommandBuffer,
        images: &[vk::Image],
        layout: vk::ImageLayout,
        stage: vk::PipelineStageFlags,
        access: vk::AccessFlags,
    ) {
        let barriers: Vec<_> = images
            .iter()
            .map(|&image| {
                vk::ImageMemoryBarrier::default()
                    .dst_access_mask(access)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(layout)
                    .src_queue_family_index(self.foreign)
                    .dst_queue_family_index(self.family)
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
