# More GPU decoding

**When:** later.

Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (preview.md §6, export.md §5). The viewer's engine decodes HEVC, AV1 and VP9 on NVDEC and copies the frames down (preview.md §3). Still on the CPU:
- **H.264 in the viewer:** 16 CPU threads decode it four times faster than FFmpeg's NVDEC path, which waits for each frame; worth NVDEC on CPUs with few cores, or with a deeper decode queue.
- **Timeline tiles and the two-up** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that for HEVC and AV1, the frames then copied down once at their small size.
- **VAAPI** (AMD, Intel): the same paths with VAAPI decoding, `scale_vaapi` and VAAPI encoders.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
