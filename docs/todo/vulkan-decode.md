# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**Where it is:** ProRes decodes on NVIDIA's GPU through FFmpeg's Vulkan decoder, the viewer shows it from there (preview.md §3), exports and proxies take its frames on the GPU into NVENC and Vulkan's ProRes encoder (export.md §5, preview.md §6), and automatic proxies are ProRes Proxy there (spec D41). What is open is below.

## Open

- The encoder on a thread of its own. An export or a proxy decodes, makes each picture and encodes it on one thread, and the encoder holds that thread while the GPU encodes. A ProRes proxy of 10 s of 4K60 ProRes HQ takes 8.1 s on the RTX 4080 SUPER. `prores_ks_vulkan`'s `send_frame` takes 5.9 s of that, decoding 1.4 s, and libplacebo 0.8 s, of which its waits (`cuda::wait_done`) are 0.1 s. NVENC's `send_frame` takes 5.3 s of the clip's 9 s NVENC export at 4K. With the proxy's encoder on a thread of its own, fed through a channel of three pictures, the proxy took 6.8 s.
- A 4:4:4 12-bit viewer layout for ProRes 4444 (below).
- FFv1 on Vulkan, once FFmpeg's decoder is faster. It decodes right there (barcodes, timestamps, seeks: `VideoCodec::Ffv1`), but 1080p with 24 slices runs at 17.7 fps on the RTX 4080 SUPER with one thread and 29 with eight frame threads, against the CPU's 484, and seeks take 340 ms against 25. So it stays on the CPU. FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.

## A 4:4:4 12-bit viewer layout

ProRes 4444 and XQ pictures come down and `Relayout` resamples them to P010, also where the GPU decodes them.

- **What the decoders give.** ProRes 4444 and XQ are 12 bits (proresdec.c, `ap4h` and `ap4x`). FFmpeg's CPU decoder and its Vulkan decoder give `yuv444p12le`, or `yuva444p12le` with alpha: a plane each, 12 bits in the low bits of 16, chroma at full size. On Vulkan each plane is an R16 image of its own, four with alpha (vulkan_decode.c, `ff_vk_frame_params`). VideoToolbox gives P416 (videotoolbox.c, `videotoolbox_best_pixel_format`), which hwcontext_videotoolbox.c maps to `kCVPixelFormatType_444YpCbCr16BiPlanarVideoRange`: Y, then U and V interleaved, 16 bits, at full size. With alpha it gives AYUV64 (`kCVPixelFormatType_4444AYpCbCr16`): A, Y, U and V packed in one plane.
- **Two layouts.** `Layout::Yuv444p12` for the planar ones and `Layout::P416` for VideoToolbox's. `displayable` takes `yuv444p12le`, `yuva444p12le` and `p416le`, so the player keeps them in GPU memory for the viewer (`route::plan`), and `Relayout` passes them as they are.
- **The shader.** Its parameters say how many luma samples a chroma sample covers across and down: 2 and 2 for 4:2:0, 2 and 1 for 4:2:2, 1 and 1 for 4:4:4. A 12-bit sample is a code out of 4095 in the low bits of 16. P416's are 16-bit codes. Without 16-bit textures, the top 8 bits go up.
- **Alpha.** The viewer leaves alpha out, as the export does. `Normalizer` gives the same RGB and the same YUV at any alpha: an orange at alpha 100%, 50% and 0% gave 173, 84, 18 each time. A planar alpha is a fourth plane the viewer doesn't read. On Vulkan it is an image of its own, which the viewer imports and acquires with the others, since FFmpeg's frame records one layout for all of its images. VideoToolbox's AYUV64 comes down, as now: a packed layout is a shader mode of its own, and only a Mac makes one.
- **Imports.** Vulkan's import takes three or four planes. VideoToolbox's takes P416 as it takes P010: R16 and RG16, the sizes from the IOSurface.
- **Tests.** Unit tests for `Layout::of` and `displayable`. Fixtures `VideoCodec::ProRes4444` and `ProRes4444Alpha`, the latter half transparent. The picture test draws each within a level of `Picture::to_rgba` where flat, decoded on the CPU, and from GPU memory where the GPU decodes ProRes on Vulkan. A script for the Mac checks P416 there.
