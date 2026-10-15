# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)). **In progress:** this design, then the PRs under Steps.

**The goal:** ProRes proxies are made on NVIDIA's GPU and ProRes decodes there, and the frames reach the viewer and the encoders without coming down to memory.
- **Proxies today:** `prores_ks_vulkan` takes its pictures from memory. NVDEC's frames are shrunk by `scale_cuda`, come down, are converted on the CPU and go up again (`frames came down at 960×540`): 600 frames each way for 10 s of 4K60.
- **Decoding today:** NVDEC decodes no ProRes, so the CPU does. `decode::gpu_decoder` finds no NVDEC decoder for it, and `VideoReader::open_gpu_or_cpu` falls back. FFmpeg 8.1 and later decode ProRes on Vulkan with compute shaders. With FFmpeg 9.0.2's command line on the RTX 4080 SUPER, 4K ProRes HQ decodes at 512 frames a second and 0.6 ms of CPU a frame, against 31 frames a second on one CPU thread.

## Design

FFmpeg's sources cited here are 9.0.1's. The checks were run with FFmpeg 9.0.1's command line on the RTX 4080 SUPER.

**The device.** Everything on Vulkan uses the device made from NVIDIA's CUDA device (`CudaDevice::vulkan`), one for each GPU in the process: the decoder, libplacebo and `prores_ks_vulkan` (`Device::vulkan_on`). So frames pass between them without a copy, and FFmpeg copies pictures to and from CUDA on the GPU (`av_hwframe_transfer_data`: CUDA imports each image's memory once and waits on its semaphore, hwcontext_vulkan.c `vulkan_transfer_data_to_cuda` and `_from_cuda`).
- FFmpeg makes it as it makes its own devices, with one image per plane (`vulkan_device_derive`). Its ProRes decoder gives the CPU decoder's frames to the bit there: the 1280×720 ProRes fixture's 45 frames have the same MD5.
- A reader or an encoder asks for the device when it opens, so a device dropped after a converter failed (`forget_vulkan`) isn't used again. Those already open keep theirs, so libplacebo checks that a frame is on its own device.

**Which decoder.** A GPU decoder decodes a stream with its own kind where FFmpeg has a decoder for the codec there, else on Vulkan where the GPU allows it (below).
- `hw::Kind::Vulkan` is new: frames `AV_PIX_FMT_VULKAN` at `Place::Vulkan`, named `vulkan`, labeled `Vulkan`.
- The reader's `gpu()` is the kind that decodes its stream, and `Decoding` says `Vulkan on NVIDIA GeForce RTX 4080 SUPER`. `reader.cuda()` is NVDEC's only.
- FFmpeg's version isn't checked. FFmpeg 8.0, or a build without Vulkan, has no `prores` decoder with a Vulkan configuration (`gpu_decoder`), so ProRes stays on the CPU there.
- `VIDCUT_HWDEC=nvdec` decodes ProRes on Vulkan too, and `none` on the CPU. `vulkan` isn't a choice there.

**Codecs.** FFmpeg 9.0.1 decodes AV1, H.264, HEVC and VP9 on Vulkan with the GPU's video engine (Vulkan Video), and ProRes, ProRes RAW, FFv1, DPX and APV with compute shaders (`ffmpeg -h decoder=…`). Only ProRes is enabled.
- AV1, H.264, HEVC and VP9 stay on NVDEC, whose CUDA frames NVENC and the CUDA filters take. Vulkan Video on AMD and Intel is [own-player.md](own-player.md)'s item.
- FFv1 has a barcode fixture (`VideoCodec::Ffv1`) and can follow once its test passes. APV has no fixture.
- DPX is an image format, and stills and image sequences decode on the CPU. ProRes RAW decodes to a Bayer pattern, which vidcut doesn't develop.

**The frames** (proresdec.c, vulkan_decode.c `ff_vk_frame_params`, hwcontext_vulkan.c):
- ProRes 422 (Proxy, LT, Standard, HQ) is `yuv422p10le`: 10 bits in the low bits of 16, as the CPU's decoder gives it. 4444 and 4444 XQ are `yuv444p12le`, or `yuva444p12le` with alpha: the decoder takes 12 bits for those profiles.
- Each plane is an image of its own in `R16_UNORM`, whatever the device's multiplane setting, because the decoder clears them first and a YUV format can't be cleared. Optimal tiling. Usage transfer, storage and sampled. Created `MUTABLE_FORMAT` and `ALIAS`, shared (`CONCURRENT`) by the device's queue families.
- Each image has memory of its own, dedicated where the driver prefers, exportable as an opaque Win32 handle or file descriptor where the driver allows (`try_export_flags`). Each has a timeline semaphore, exportable the same way.
- The decode leaves the images in `GENERAL` and signals the semaphores when it is done.
- The frames come from an `AVBufferPool`, which makes another frame when none is free (`vulkan_frames_init`), so frames held for the viewer don't run the decoder out. A frame let go goes back to the pool, which keeps it until the reader closes.
- A frame is the picture's size: ProRes's coded size is its picture size.

**Into memory (the `Whole` route).** `hw::download` brings a frame down with `av_hwframe_transfer_data`, as it does the other GPUs' frames, and `copies` counts it. Interlaced ProRes comes down before `bwdif`, as VA-API's does. `bwdif_vulkan` could deinterlace it on the GPU later.

**The encoders.** libplacebo on the device is the Vulkan backend's `fit`: it turns, converts, scales and places a frame, as `cuda::Placebo`'s `Layout::Placed` does for NVENC today.
- **`prores_ks_vulkan` from NVDEC's frames:** a frame is copied from CUDA into Vulkan, as `Placebo` copies it today, and libplacebo makes the `yuv422p10le` picture the encoder takes, on the same device. Nothing comes down or goes up. The encoder takes `AV_PIX_FMT_VULKAN` frames and reads each through the frame's own frames context (proresenc_kostya_vulkan.c, vulkan.c `ff_vk_create_imageviews`), so libplacebo's frames do. FFmpeg checks the encoder's frames context only when it opens (encode.c).
- **libplacebo's levels:** its `yuv422p10le` pictures, from H.264 at 1280×720 shrunk to 640×360, are within a level of the CPU's (`scale` with `accurate_rnd`) wherever the CPU's picture is flat, in every plane. `cuda::as_read` found libplacebo missing P010's shift in planes that are images of their own. `yuv422p10le` has no shift.
- **The whole chain:** NVDEC, a copy into Vulkan, libplacebo and `prores_ks_vulkan` made a 640×360 ProRes Proxy, 10-bit 4:2:2, within 3 to 5 levels of 1023 of the CPU's picture where it is flat. The CPU's `prores_ks` from the CPU's picture is within 2 to 3.
- **`prores_ks_vulkan` from Vulkan's frames** (ProRes sources): libplacebo takes the decoded frame as it is. Nothing is copied.
- **NVENC from Vulkan's frames:** libplacebo makes the NV12 or P016 picture, and FFmpeg copies it into NVENC's CUDA frames: one copy on the GPU, of the small picture. NVENC reads CUDA memory, and CUDA maps an optimally tiled Vulkan image as an array, so a copy is needed. The other way, a copy into CUDA `yuv422p10le` frames and then `scale_cuda` (which takes `yuv422p10`), copies the large picture, still needs libplacebo for turned pictures and HDR, and its 4:2:2 to 4:2:0 is untested here.
- **One converter at a time** (`cuda::converting`): jobs converting at once take turns there. The PRs measure it.

**The viewer (the `Kept` route).** wgpu's Vulkan device imports each plane's memory (the opaque handle FFmpeg's device exports with `vkGetMemoryWin32HandleKHR` or `vkGetMemoryFdKHR`) into an image made with the same create info, and the shader draws from it in place, as it draws Direct3D 11's textures. Nothing is copied. The viewer takes planar 4:2:2 already (`Layout::Yuv422p10`, three `R16Unorm` textures), so the shader stays. `Screen::views` takes a V plane too.
- **Released and done:** after the decode, the keeper releases the images to the external queue family on FFmpeg's device, as FFmpeg does before CUDA reads a frame (`PREP_MODE_EXTERNAL_EXPORT`, internal to hwcontext_vulkan.c). It waits on the CPU until that is done, by the frame's timeline semaphores as `cuda::wait_signalled` waits, before the player keeps the frame. wgpu-hal can make its next submission wait on a timeline semaphore (`Queue::add_wait_semaphore`), but that may be anyone's next submission, the UI's too.
- **Acquired:** wgpu acquires the images from the external queue family in `GENERAL` (`gui::acquire`). For a `CONCURRENT` image the acquire's other family is `IGNORED` (VUID-VkImageMemoryBarrier-image-04071).
- **Held:** the picture is held until wgpu's drawing is done (`imported`), so the decoder doesn't reuse an image under it.
- **Imported for each picture drawn**, as Direct3D 11's are opened. Keeping imports across pictures needs a way to tell a reused image from a freed one, so it waits for a measurement.
- **The same GPU:** the display's device must be the Vulkan device's GPU, by their UUIDs, as `shared::cuda_on` checks CUDA's. Elsewhere the frames come down, as NVDEC's do.
- **Kept by kind:** one GPU decoder now gives NVDEC's frames for H.264 and Vulkan's for ProRes. The player keeps each kind where the display shows that kind (`Output::Decoded` names the kinds, and the Maker plans `Dest::Display` for each frame).
- **Not shown from there:** 4:4:4 at 12 bits (ProRes 4444) isn't a viewer layout yet. Those frames come down whole and `Relayout` resamples them, as it resamples the CPU's 4444 today.
- **Rejected:** a copy through CUDA into the buffer NVDEC's pictures use (three copies on the GPU), and decoding on wgpu's own device (no import, but wgpu's device would need FFmpeg's extensions, features and queues, and headless jobs would decode on another device).

**The player's budget.** A Vulkan picture counts in the video memory budget (`store::Kind::of`: every GPU picture does), at its layout's bytes as every picture does (`Picture::bytes`): 4 bytes a pixel for `yuv422p10le`, 33 MB for a 4K frame. On NVIDIA the budget is half the GPU's memory, which CUDA tells (`gpu_found`), and NVDEC's and Vulkan's pictures share it, as they share the GPU's memory.

**The planner and the backends.** `route::Place::Vulkan` is there: `Place::of` gives it for Vulkan frames. `EncoderChoice::Vulkan` names `prores_ks_vulkan`, but the Maker never makes pictures for it on the GPU: the proxy opens it on frames of its own (`Upload::vulkan`), `Output::for_encoder` gives `Output::Fitted`, and the planner sees `Dest::Memory`. What the planner lacks:
- An output for Vulkan pictures, whose destination is `Dest::Encoder { place: Vulkan }`.
- `hw::can_fit(Place::Vulkan, ops)`, false today. It becomes libplacebo's, as `cuda::can_fit` is: YUV pictures turned by quarter turns.
- A way from one place to another. `plan` gives `Gpu` only when the encoder's place is the frame's: a CUDA frame for a Vulkan encoder is `Shrunk` (`an_encoder_on_another_place_never_takes_a_frame_as_it_is`). `Facts` gains `reaches`: the frame's GPU hands a fitted picture to the encoder's place. On NVIDIA CUDA reaches Vulkan (copied in before libplacebo) and Vulkan reaches CUDA (libplacebo's picture copied out), where the Vulkan device is the CUDA device's. `plan` gives `Gpu` where the encoder's place is the frame's or one it reaches, the frame fits, and fitting didn't fail before.
- **Shrinker:** none at first, so Vulkan's frames come down at their own size (`Scaling::new` has no arm for them). Then `scale_vulkan`, which keeps the layout and samples bilinearly, in halvings as `scale_cuda` does, checked against the CPU's levels.
- **Keeper:** the frame itself (`frame_ref`), once the release and the wait above are done.
- **Fitter:** none. libplacebo is the fit.

**Which GPUs** (`GpuDecoder::why_not`):
- Vulkan decodes ProRes on NVIDIA's GPUs only, by the GPU's PCI vendor. FFmpeg 9.0.2's decoder segfaults on AMD's Windows driver (gotchas.md), and Intel's and Mesa's drivers are untested. Elsewhere the CPU decodes ProRes, as now.
- `hw::plays` and `vidcut decoders list` decode their ProRes clip through the GPU decoder, which is Vulkan on NVIDIA now, and name the decoder that ran. Automatic proxies then rank ProRes's encoders with the codecs the GPU plays. On NVIDIA the first choice stays NVENC's (AV1 here).
- `seek_check` runs by hand on each GPU before it is allowed, as for every other decoder (preview.md §3).
- A crash can't be caught. Allowing another vendor also needs its first test decode to run under a marker, as the encoders' tests do (`encoders.testing`): a marker left by a process that crashed keeps Vulkan's decoding off there.

## Steps

Each PR lands something to see on the RTX 4080 SUPER, and is measured against the one before there: frames copied each way (`copies`), then CPU time, then time.
1. **ProRes proxies made on the GPU:** NVDEC's frames to `prores_ks_vulkan` through libplacebo, the output for Vulkan pictures, `can_fit` for Vulkan, `reaches`. Seen: a ProRes proxy of H.264, HEVC or AV1 says `frames stayed there`, and nothing comes down or goes up. Test (`tests/proxy.rs`, on NVIDIA): such a proxy is 10-bit, keeps the source's barcodes and timestamps, and its `copies` are 0.
2. **The decoder (`Whole`):** `Kind::Vulkan`, ProRes on NVIDIA's Vulkan device, frames that come down whole, frames kept by kind (so the viewer keeps showing NVDEC's from GPU memory). Seen: ProRes, a ProRes proxy too, decodes on the GPU (`player.state`'s `hwdec`: `vulkan`) with less CPU. Test (`crates/vidcut-av/tests/`): the ProRes 422 HQ fixture decoded on the GPU in use gives Vulkan frames whose barcodes and timestamps are the CPU decoder's, read through and after seeks, one copy down each. It skips where no GPU decodes ProRes on Vulkan. `seek_check` by hand. Measured with `decode_speed` on a 4K ProRes HQ clip.
3. **The viewer (`Kept`):** the keeper's release and wait, the import, `gpu_pictures` with `Vulkan`. Seen: ProRes and its proxies shown from GPU memory (`viewer.shown`), nothing copied. Test: a ProRes picture decoded on Vulkan and kept there draws from GPU memory within a level of `Picture::to_rgba` (`crates/vidcut/tests/picture.rs`).
4. **Vulkan's frames into the encoders:** libplacebo takes the decoded frames as they are, for `prores_ks_vulkan` and for NVENC. Seen: exports and proxies of ProRes sources say `frames stayed there`. Test: a ProRes source's export into NVENC and its proxies into NVENC and `prores_ks_vulkan` keep its barcodes and timestamps, nothing copied down or up, every frame made on the GPU.

**Later:** shrinking on Vulkan (`scale_vulkan`), `bwdif_vulkan`, a 4:4:4 12-bit viewer layout, FFv1, other vendors.
