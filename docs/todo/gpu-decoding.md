# More GPU decoding

**When:** later.

The viewer's engine decodes on the GPU (NVDEC, VA-API, Direct3D 11) and shows the pictures from video memory (preview.md §3). Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (export.md §5); other proxies and exports decode on any of these GPUs, which shrink the frames before they come down (preview.md §6, export.md §5). Still to do:
- **Timeline tiles and the two-up** (`export/renderer.rs`): far-apart tiles cost 40 ms on 1080p H.264 and 105–170 ms on 4K60 on the CPU; NVDEC would cut that for HEVC and AV1, the frames then copied down once at their small size.
- **Exports on AMD and Intel GPUs, frames staying there:** decoded on VA-API or Direct3D 11, their frames come down for the CPU to turn and convert, and VAAPI, AMF and QSV take them back up (export.md §5). NVENC's path keeps them on the GPU throughout, libplacebo turning and converting; the same for these GPUs would save both copies.
- **Where exports decode:** a GPU that encodes too can be slower decoding as well than the CPU decoding for it: the N150's VAAPI and QSV on footage easy to decode, the 9800X3D's small Radeon with AMF on any (export.md §5). Decoding on the CPU while the GPU encodes would be faster there, at the cost of the CPU: a preference, or a choice measured per machine and footage.
- **Hardware AV1 proxies on AMF and QSV** (AMD's RDNA 3 and later, Intel's Arc and later): proxies take NVENC's and VAAPI's AV1 only, as neither test machine has AV1 on AMF or QSV to check their keyframes and order against the barcodes (`hardware_av1_proxies_are_exact_too`).

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
