# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**Where it is:** ProRes decodes on NVIDIA's GPU through FFmpeg's Vulkan decoder, the viewer shows it from there (preview.md §3), exports and proxies take its frames on the GPU into NVENC and Vulkan's ProRes encoder (export.md §5, preview.md §6), and automatic proxies are ProRes Proxy there (spec D41). What is open is below.

## Open

- The encoder on a thread of its own. An export or a proxy decodes, makes each picture and encodes it on one thread, and the encoder holds that thread while the GPU encodes. A ProRes proxy of 10 s of 4K60 ProRes HQ takes 8.1 s on the RTX 4080 SUPER. `prores_ks_vulkan`'s `send_frame` takes 5.9 s of that, decoding 1.4 s, and libplacebo 0.8 s, of which its waits (`cuda::wait_done`) are 0.1 s. NVENC's `send_frame` takes 5.3 s of the clip's 9 s NVENC export at 4K. With the proxy's encoder on a thread of its own, fed through a channel of three pictures, the proxy took 6.8 s.
- Keeping the viewer's imports of Vulkan's frames across pictures (below).
- Shrinking on Vulkan (`scale_vulkan`), and `bwdif_vulkan` for interlaced ProRes, which comes down before `bwdif` now.
- A 4:4:4 12-bit viewer layout (ProRes 4444).
- FFv1 on Vulkan: it has a barcode fixture (`VideoCodec::Ffv1`). FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.

## Keeping the viewer's imports

Each picture drawn is imported anew. That takes 2.4 ms of the viewer thread for a 4K picture on the RTX 4080 SUPER. The three planes' memory takes 1.3 ms of it, and the acquire's own submission 1 ms. Uploading a picture takes 2 ms.

- **FFmpeg's pool.** The Vulkan decoder takes its frames from a pool that grows and never shrinks (hwcontext_vulkan.c, `av_buffer_pool_init2`, no `initial_pool_size`). A frame given back is the next one decoded into, with the same images and memory. FFmpeg frees them all when it frees the frames context. Then it calls the context's `free`, which closes vidcut's exported handles (`hw::vulkan`).
- **The key.** A screen keeps the import of each frame it draws: the planes' images on wgpu's device, bound to FFmpeg's memory, and their views. It finds one by the frame's frames context and its planes' `VkDeviceMemory` handles (`hw::vulkan::Identity`). In a live frames context, those name the same images every time.
- **One frames context at a time.** The imports kept are those of the frames context of the picture drawn last. That picture holds the context, so no handle is looked up after FFmpeg freed it. A picture of another context, black, or a picture in memory lets them all go. So a kept import never holds memory FFmpeg freed. Imports of several contexts, let go from FFmpeg's `free`, would hold freed memory until the viewer draws again, and it draws nothing while paused.
- **Bounded by the pool:** one import for each frame of the context, as many as FFmpeg made.
- **Each draw** acquires the images from the layout FFmpeg's frame records, as now, since FFmpeg decoded into them again. The acquire's command buffer goes into the draw's submission, ahead of the draw. That saves a submission of its own.
- **Checked:** the viewer thread's time per picture before and after, 10 s of 4K60 ProRes HQ played in real time (release build). The picture tests, and a new one: a picture drawn twice is imported once (`Screen::imports`). The re-import test draws the picture on a second screen, which imports it again.
