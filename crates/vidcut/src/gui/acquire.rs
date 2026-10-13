//! Images another API wrote, taken over by wgpu's queue with what they
//! hold: VA-API's surfaces imported as DMA-BUFs (Linux), Direct3D 11's
//! textures opened by their NT handles (Windows).

use ash::vk;

/// Takes images another API wrote over to wgpu's queue with their
/// contents: an acquire from the external queue family, which wgpu can't
/// record, submitted on its queue ahead of the work that reads them. An
/// imported image starts `UNDEFINED`, and wgpu's first barrier from there
/// lets the driver drop what the memory holds: Mesa's drivers then set up
/// compression state afresh (RADV its metadata; ANV its fast-clear state
/// and aux surface), and a compressed picture is lost. Acquired from the
/// external queue in `GENERAL`, as other APIs leave what they share, the
/// driver keeps it: RADV does nothing, ANV keeps the aux surface the
/// modifier shares.
pub(super) struct Acquirer {
    device: ash::Device,
    queue: vk::Queue,
    /// wgpu's queue's family, and the one the images come from.
    family: u32,
    foreign: u32,
    pool: vk::CommandPool,
    /// Command buffers, each with the fence that says it is done.
    slots: Vec<(vk::CommandBuffer, vk::Fence)>,
}

/// Command buffers in flight at most; past them the next waits.
const SLOTS: usize = 8;

impl Acquirer {
    /// One on wgpu's Vulkan device and queue.
    ///
    /// # Safety
    /// `hal` is the device wgpu draws with.
    pub(super) unsafe fn new(hal: &wgpu::hal::vulkan::Device) -> Result<Self, String> {
        let device = hal.raw_device().clone();
        let family = hal.queue_family_index();
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
        let info = vk::CommandPoolCreateInfo::default()
            .flags(
                vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER
                    | vk::CommandPoolCreateFlags::TRANSIENT,
            )
            .queue_family_index(family);
        // SAFETY: a pool for the device's own queue family.
        let pool = unsafe { device.create_command_pool(&info, None) }
            .map_err(|e| format!("a command pool: {e}"))?;
        Ok(Self {
            device,
            queue: hal.raw_queue(),
            family,
            foreign,
            pool,
            slots: Vec::new(),
        })
    }

    /// Submits the acquire of `images` (color images of one plane, one
    /// level) into `layout`, for `access` at `stage`. What is submitted to
    /// wgpu's queue after this finds them so.
    ///
    /// # Safety
    /// The images are this device's, on memory another API wrote, not yet
    /// used by wgpu; nothing else submits to wgpu's queue meanwhile (the
    /// viewer's thread does all of it).
    pub(super) unsafe fn acquire(
        &mut self,
        images: &[vk::Image],
        layout: vk::ImageLayout,
        stage: vk::PipelineStageFlags,
        access: vk::AccessFlags,
    ) -> Result<(), String> {
        let (cmd, fence) = self.slot()?;
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
        let d = &self.device;
        let failed = |what: &str| {
            let what = what.to_string();
            move |e: vk::Result| format!("{what}: {e}")
        };
        // SAFETY: a command buffer of its own pool, done with and reset,
        // recorded and submitted on the device's queue with its fence.
        unsafe {
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            d.begin_command_buffer(cmd, &begin)
                .map_err(failed("recording an acquire"))?;
            d.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
            d.end_command_buffer(cmd)
                .map_err(failed("recording an acquire"))?;
            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            d.queue_submit(self.queue, &[submit], fence)
                .map_err(failed("submitting an acquire"))?;
        }
        Ok(())
    }

    /// A command buffer to record into, with its fence, both reset.
    fn slot(&mut self) -> Result<(vk::CommandBuffer, vk::Fence), String> {
        let d = &self.device;
        // SAFETY: the slots' fences are this device's.
        let done = |f: vk::Fence| unsafe { d.get_fence_status(f) } == Ok(true);
        let mut free = self.slots.iter().position(|&(_, f)| done(f));
        if free.is_none() && self.slots.len() < SLOTS {
            let info = vk::CommandBufferAllocateInfo::default()
                .command_pool(self.pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            // SAFETY: from its own pool; a fence of its own, signalled so
            // it reads as free.
            let made = unsafe {
                let cmd = d
                    .allocate_command_buffers(&info)
                    .map_err(|e| format!("a command buffer: {e}"))?[0];
                let signalled =
                    vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
                let fence = d
                    .create_fence(&signalled, None)
                    .map_err(|e| format!("a fence: {e}"))?;
                (cmd, fence)
            };
            self.slots.push(made);
            free = Some(self.slots.len() - 1);
        }
        let i = match free {
            Some(i) => i,
            // Every one in flight: wait for one, a second at most.
            None => {
                let fences: Vec<_> = self.slots.iter().map(|&(_, f)| f).collect();
                // SAFETY: the device's fences.
                unsafe { d.wait_for_fences(&fences, false, 1_000_000_000) }
                    .map_err(|e| format!("waiting for an acquire: {e}"))?;
                self.slots
                    .iter()
                    .position(|&(_, f)| done(f))
                    .ok_or("no acquire done")?
            }
        };
        let (cmd, fence) = self.slots[i];
        // SAFETY: done with (its fence signalled), so both may be reset.
        unsafe {
            d.reset_fences(&[fence])
                .map_err(|e| format!("a fence: {e}"))?;
            d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("a command buffer: {e}"))?;
        }
        Ok((cmd, fence))
    }
}

impl Drop for Acquirer {
    fn drop(&mut self) {
        let d = &self.device;
        let fences: Vec<_> = self.slots.iter().map(|&(_, f)| f).collect();
        // SAFETY: its own objects, destroyed once their work is done (the
        // pool frees its command buffers).
        unsafe {
            if !fences.is_empty() {
                let _ = d.wait_for_fences(&fences, true, 1_000_000_000);
            }
            for f in fences {
                d.destroy_fence(f, None);
            }
            d.destroy_command_pool(self.pool, None);
        }
    }
}
