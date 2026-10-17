use ash::vk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Api,
    Vulkan {
        concurrent: bool,
        layout: vk::ImageLayout,
    },
}

pub(super) struct Acquirer {
    device: ash::Device,
    family: u32,
    foreign: u32,
}

impl Acquirer {
    pub(super) fn new(hal: &wgpu::hal::vulkan::Device) -> Self {
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
