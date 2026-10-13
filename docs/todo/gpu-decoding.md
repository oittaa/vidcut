# More GPU decoding

**When:** later.

The viewer's engine decodes on the GPU (NVDEC, VA-API, Direct3D 11) and shows NVDEC's pictures from video memory (preview.md §3). Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (export.md §5). Still on the CPU:
- **Timeline tiles and the two-up** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that for HEVC and AV1, the frames then copied down once at their small size.
- **Exports and proxies on AMD and Intel GPUs:** their frames decode on the CPU. The same paths as NVDEC's with VA-API or Direct3D 11 decoding, scaling on the GPU (`scale_vaapi`, `scale_d3d11`) into the GPU's encoder. Their encoders (VAAPI, AMF, QSV) take frames from memory meanwhile (export.md §5).
- **Hardware AV1 proxies on AMF and QSV** (AMD's RDNA 3 and later, Intel's Arc and later): proxies take NVENC's and VAAPI's AV1 only, as neither test machine has AV1 on AMF or QSV to check their keyframes and order against the barcodes (`hardware_av1_proxies_are_exact_too`).

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
