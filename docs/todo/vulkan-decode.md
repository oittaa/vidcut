# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**The goal:** ProRes decoded on NVIDIA's GPU reaches the viewer and the encoders without coming down to memory. It decodes there through FFmpeg's Vulkan decoder already, its frames coming down whole (preview.md §3), and proxies are made from NVDEC's frames on the GPU (preview.md §6).

## Design

FFmpeg's sources cited here are 9.0.1's. The checks were run with FFmpeg 9.0.1's command line on the RTX 4080 SUPER.

**The device.** The decoder decodes on the Vulkan device made from NVIDIA's CUDA device (`CudaDevice::vulkan`), one for each GPU in the process, which libplacebo and `prores_ks_vulkan` use (`Device::vulkan_on`). So decoded frames can go to them without a copy, and FFmpeg copies pictures to and from CUDA on the GPU (`av_hwframe_transfer_data`: CUDA imports each image's memory once and waits on its semaphore, hwcontext_vulkan.c `vulkan_transfer_data_to_cuda` and `_from_cuda`). A reader asks for the device when it opens, so a device dropped after a converter failed (`forget_vulkan`) isn't used again. Readers already open keep theirs, so libplacebo must check that a frame is on its own device.

**The frames** (proresdec.c, vulkan_decode.c `ff_vk_frame_params`, hwcontext_vulkan.c):
- ProRes 422 (Proxy, LT, Standard, HQ) is `yuv422p10le`: 10 bits in the low bits of 16, as the CPU's decoder gives it. 4444 and 4444 XQ are `yuv444p12le`, or `yuva444p12le` with alpha: the decoder takes 12 bits for those profiles.
- Each plane is an image of its own in `R16_UNORM`, whatever the device's multiplane setting, because the decoder clears them first and a YUV format can't be cleared. Optimal tiling. Usage transfer, storage and sampled. Created `MUTABLE_FORMAT` and `ALIAS`, shared (`CONCURRENT`) by the device's queue families.
- Each image has memory of its own, dedicated where the driver prefers, exportable as an opaque Win32 handle or file descriptor where the driver allows (`try_export_flags`). Each has a timeline semaphore, exportable the same way.
- The decode leaves the images in `GENERAL` and signals the semaphores when it is done.
- The frames come from an `AVBufferPool`, which makes another frame when none is free (`vulkan_frames_init`), so frames held for the viewer don't run the decoder out. A frame let go goes back to the pool, which keeps it until the reader closes.
- A frame is the picture's size: ProRes's coded size is its picture size.

**The encoders.** libplacebo on the device is the Vulkan backend's `fit` (`cuda::CudaPictures`). It makes NVENC's pictures and `prores_ks_vulkan`'s from NVDEC's frames, each frame copied from CUDA into Vulkan first.
- **libplacebo's levels:** its `yuv422p10le` pictures, from H.264 at 1280×720 shrunk to 640×360, are within a level of the CPU's (`scale` with `accurate_rnd`) wherever the CPU's picture is flat, in every plane. `cuda::as_read` found libplacebo missing P010's shift in planes that are images of their own. `yuv422p10le` has no shift.
- **`prores_ks_vulkan` from Vulkan's frames** (ProRes sources): libplacebo takes the decoded frame as it is, without the copy from CUDA. Nothing is copied.
- **NVENC from Vulkan's frames:** libplacebo makes the NV12 or P016 picture, and FFmpeg copies it into NVENC's CUDA frames: one copy on the GPU, of the small picture. NVENC reads CUDA memory, and CUDA maps an optimally tiled Vulkan image as an array, so a copy is needed. The other way, a copy into CUDA `yuv422p10le` frames and then `scale_cuda` (which takes `yuv422p10`), copies the large picture, still needs libplacebo for turned pictures and HDR, and its 4:2:2 to 4:2:0 is untested here.
- **One converter at a time** (`cuda::converting`): jobs converting at once take turns there. The PRs measure it.

**The viewer (the `Kept` route).** wgpu's Vulkan device imports each plane's memory (the opaque handle FFmpeg's device exports with `vkGetMemoryWin32HandleKHR` or `vkGetMemoryFdKHR`) into an image made with the same create info, and the shader draws from it in place, as it draws Direct3D 11's textures. Nothing is copied. The viewer takes planar 4:2:2 already (`Layout::Yuv422p10`, three `R16Unorm` textures), so the shader stays. `Screen::views` takes a V plane too.
- **Released and done:** after the decode, the keeper releases the images to the external queue family on FFmpeg's device, as FFmpeg does before CUDA reads a frame (`PREP_MODE_EXTERNAL_EXPORT`, internal to hwcontext_vulkan.c). It waits on the CPU until that is done, by the frame's timeline semaphores as `cuda::wait_signalled` waits, before the player keeps the frame. wgpu-hal can make its next submission wait on a timeline semaphore (`Queue::add_wait_semaphore`), but that may be anyone's next submission, the UI's too.
- **Acquired:** wgpu acquires the images from the external queue family in `GENERAL` (`gui::acquire`). For a `CONCURRENT` image the acquire's other family is `IGNORED` (VUID-VkImageMemoryBarrier-image-04071).
- **Held:** the picture is held until wgpu's drawing is done (`imported`), so the decoder doesn't reuse an image under it.
- **Imported for each picture drawn**, as Direct3D 11's are opened. Keeping imports across pictures needs a way to tell a reused image from a freed one, so it waits for a measurement.
- **The same GPU:** the display's device must be the Vulkan device's GPU, by their UUIDs, as `shared::cuda_on` checks CUDA's. Elsewhere the frames come down, as NVDEC's do.
- **Not shown from there:** 4:4:4 at 12 bits (ProRes 4444) isn't a viewer layout yet. Those frames come down whole and `Relayout` resamples them, as it resamples the CPU's 4444 today.
- **Rejected:** a copy through CUDA into the buffer NVDEC's pictures use (three copies on the GPU), and decoding on wgpu's own device (no import, but wgpu's device would need FFmpeg's extensions, features and queues, and headless jobs would decode on another device).

**The player's budget.** A Vulkan picture counts in the video memory budget (`store::Kind::of`: every GPU picture does), at its layout's bytes as every picture does (`Picture::bytes`): 4 bytes a pixel for `yuv422p10le`, 33 MB for a 4K frame. On NVIDIA the budget is half the GPU's memory, which CUDA tells (`gpu_found`), and NVDEC's and Vulkan's pictures share it, as they share the GPU's memory.

**The planner and the backends.** Vulkan's frames fit as libplacebo fits them (`hw::can_fit`), and CUDA's reach Vulkan's encoder (`hw::reaches`). To come:
- Vulkan's frames reach NVENC's CUDA frames, libplacebo's picture copied out by FFmpeg (`hw::reaches`).
- **Shrinker:** none at first, so Vulkan's frames come down at their own size (`Scaling::new` has no arm for them). Then `scale_vulkan`, which keeps the layout and samples bilinearly, in halvings as `scale_cuda` does, checked against the CPU's levels.
- **Keeper:** the frame itself (`frame_ref`), once the release and the wait above are done.
- **Fitter:** none. libplacebo is the fit.

## Steps

Each PR lands something to see on the RTX 4080 SUPER, and is measured against the one before there: frames copied each way (`copies`), then CPU time, then time.
1. **The viewer (`Kept`):** the keeper's release and wait, the import, `gpu_pictures` with `Vulkan`. Seen: ProRes and its proxies shown from GPU memory (`viewer.shown`), nothing copied. Test: a ProRes picture decoded on Vulkan and kept there draws from GPU memory within a level of `Picture::to_rgba` (`crates/vidcut/tests/picture.rs`).
2. **Vulkan's frames into the encoders:** libplacebo takes the decoded frames as they are, for `prores_ks_vulkan` and for NVENC. Seen: exports and proxies of ProRes sources say `frames stayed there`. Test: a ProRes source's export into NVENC and its proxies into NVENC and `prores_ks_vulkan` keep its barcodes and timestamps, nothing copied down or up, every frame made on the GPU. libplacebo's pass and the encoder now run one at a time (`cuda::converting`, `wait_done`), and a ProRes proxy of NVDEC's frames takes 8 to 17 % longer than when it came down: see whether they can overlap.
3. **Automatic prefers ProRes Proxy** (the user's decision): `AUTOMATIC` in proxy.rs becomes DNxHR LB, ProRes Proxy, AV1, HEVC, H.264. Where `prores_ks_vulkan` takes the decoder's frames on the GPU (on NVIDIA), it counts as made on the GPU (`round`: 0 where the GPU plays ProRes). Where Vulkan's ProRes isn't allowed it stays played only, as now. The docs' reason for AV1 and HEVC first, that ProRes's 4:2:2 frames come down to be shown, no longer holds on the Mac (P210) or, after step 1, on NVIDIA: D41 (cut-page-spec.md), preview.md §6, the tooltip in gui/preferences.rs and cli.md change with it. Tests in proxy.rs: `automatic_takes_apples_hevc_hardware` becomes ProRes on VideoToolbox. A new one: NVIDIA with `prores_ks_vulkan`, playing ProRes, takes `prores_ks_vulkan`. AMD's and the GPU-less cases stay.

**Later:**
- Shrinking on Vulkan (`scale_vulkan`), and `bwdif_vulkan` for interlaced ProRes, which comes down before `bwdif` now.
- A 4:4:4 12-bit viewer layout (ProRes 4444).
- FFv1 on Vulkan: it has a barcode fixture (`VideoCodec::Ffv1`). FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.
