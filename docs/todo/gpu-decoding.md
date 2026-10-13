# More GPU decoding

**When:** later.

The viewer's engine decodes on the GPU (NVDEC, VA-API, Direct3D 11) and shows the pictures from video memory (preview.md §3). Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (export.md §5); other proxies decode on any of these GPUs, AMD's and Intel's shrinking the frames before they come down (preview.md §6). Still on the CPU:
- **Timeline tiles and the two-up** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that for HEVC and AV1, the frames then copied down once at their small size.
- **Exports on AMD and Intel GPUs:** their frames decode on the CPU. The same path as NVDEC's, with VA-API or Direct3D 11 decoding and scaling on the GPU (`hw::Shrinker`'s scalers) into the GPU's encoder; their encoders (VAAPI, AMF, QSV) take frames from memory meanwhile (export.md §5). Proxies made with those encoders decode on the GPU but come down for the CPU to turn, convert and hand to the encoder, which takes them back up.
- **Hardware AV1 proxies on AMF and QSV** (AMD's RDNA 3 and later, Intel's Arc and later): proxies take NVENC's and VAAPI's AV1 only, as neither test machine has AV1 on AMF or QSV to check their keyframes and order against the barcodes (`hardware_av1_proxies_are_exact_too`).

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
