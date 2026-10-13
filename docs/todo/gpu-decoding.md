# More GPU decoding

**When:** later.

The viewer's engine decodes on the GPU (NVDEC, VA-API, Direct3D 11) and shows NVDEC's pictures from video memory (preview.md §3). Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (export.md §5). Still on the CPU:
- **Timeline tiles and the two-up** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that for HEVC and AV1, the frames then copied down once at their small size.
- **Exports and proxies on AMD and Intel GPUs:** their frames decode on the CPU. The same paths as NVDEC's with VA-API or Direct3D 11 decoding, scaling on the GPU (`scale_vaapi`, `scale_d3d11`) into the GPU's encoder.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
