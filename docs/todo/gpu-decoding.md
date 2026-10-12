# More GPU decoding

**When:** later.

Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (preview.md §12, export.md §5). Still on the CPU:
- **Timeline tiles and the paused viewer** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that, the frames then copied down once at their small size.
- **VAAPI** (AMD, Intel): the same paths with VAAPI decoding, `scale_vaapi` and VAAPI encoders.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
