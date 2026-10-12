# More GPU decoding

**When:** later.

Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (preview.md §12, export.md §5). Still on the CPU:
- **Turned pictures:** libplacebo (already in the pipeline for BT.601, export.md §5) has `rotate` for 90° steps; phone footage would stay on the GPU.
- **Padding in 10-bit output:** `pad_cuda` takes 8 bits only, and libplacebo can't clear borders in FFmpeg's Vulkan frames. libplacebo with two inputs could draw the picture over a black frame of the output's size, which needs no clearing.
- **HDR:** tone mapping into SDR, and SDR into HDR output. libplacebo does both; it must give the CPU's curve (`color.rs`).
- **Interlaced sources** in exports (deinterlacing before frames are picked), and interlaced BT.601 anywhere (`bwdif_cuda` would go before libplacebo).
- **Timeline tiles** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that, the frames then copied down once at their small size.
- **Playback** (`play.rs`): the viewer's picture, playing and paused, is still a software frame. The GPU path writes NV12 or P010 for NVENC and does not hand back a picture the viewer can show; libplacebo also holds one frame back, which fights a one-frame scrub.
- **VAAPI** (AMD, Intel): the same paths with VAAPI decoding, `scale_vaapi` and VAAPI encoders.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
