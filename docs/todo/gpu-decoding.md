# More GPU decoding

**When:** later.

The viewer's engine decodes on the GPU (NVDEC, VA-API, Direct3D 11) and shows the pictures from video memory (preview.md §3). Proxies and exports made with NVENC decode on NVDEC and scale on the GPU (export.md §5); other proxies and exports, the timeline's tiles, the two-up, `frame render` and storyboards decode on any of these GPUs, which shrink the frames before they come down (preview.md §3, §6 and §9, export.md §5, gui.md §11). Still to do:
- **Exports on AMD and Intel GPUs, frames staying there:** VAAPI and AMF take the decoder's frames on its device (export.md §5). QSV still takes frames from memory: it takes only QSV frames, which would be mapped from Direct3D 11's or VA-API's (FFmpeg's derived frames contexts). Pieces in other colors than the output's (BT.601, full range, HDR into SDR) come down on every GPU but NVIDIA's, where libplacebo converts them; libplacebo would read VA-API's surfaces imported into Vulkan, and Direct3D 11's shared with it as the viewer shares them.
- **Where exports and tiles decode, by itself:** a GPU that encodes too can be slower decoding as well than the CPU decoding for it, where the frames come down and go back up: the N150's QSV on footage easy to decode, and the 9800X3D's small Radeon with AMF on any, and for the timeline's tiles (export.md §5, gui.md §11). `gpu.decoding` off decodes on the CPU there; vidcut could choose so itself, measuring each machine's decoding against its CPU's.
- **Hardware AV1 proxies on AMF and QSV** (AMD's RDNA 3 and later, Intel's Arc and later): proxies take NVENC's and VAAPI's AV1 only, as neither test machine has AV1 on AMF or QSV to check their keyframes and order against the barcodes.

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. It doesn't help image sequences: GPUs don't decode PNG.
