# GPU decoding for exports and pictures

**When:** later.

Proxies made with NVENC decode on NVDEC and scale on the GPU (preview.md §12): 4K60 AV1 to a proxy went from 107 s and 381 CPU-seconds to 37 s and 2.6. Everything else vidcut decodes itself still runs on the CPU:
- **Exports:** the same NVDEC → CUDA → NVENC path, where the plan allows it. Export pieces cut, mix sources and fit frames, so the CUDA graph needs padding, frame-rate conversion and more than one source at a time; color and HDR conversions still need the CPU (or libplacebo on Vulkan).
- **Timeline tiles** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that, the frames then copied down once at their small size.
- **Playback** (`play.rs`): the viewer's picture, playing and paused, is still a software frame.
- **VAAPI** (AMD, Intel): the same path with VAAPI decoding, `scale_vaapi` and VAAPI encoders, for proxies first.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
