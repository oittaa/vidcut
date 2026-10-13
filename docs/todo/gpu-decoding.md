# More GPU decoding

**When:** later.

What decodes on the GPU now: preview.md §3 and §6, export.md §5, gui.md §11. Left:
- **The CLI's pictures** (`frame render`, `storyboard render`) decode on the CPU.
- **QSV exports keeping frames on the GPU:** QSV takes only QSV frames, which could be mapped from Direct3D 11's or VA-API's (FFmpeg's derived frames contexts). VAAPI and AMF already take the decoder's frames (export.md §5).
- **Color conversion on AMD and Intel:** pieces in other colors than the output's (BT.601, full range, HDR into SDR) come down to the CPU on every GPU but NVIDIA's, where libplacebo converts them. libplacebo could read VA-API's surfaces imported into Vulkan, and Direct3D 11's shared with it as the viewer shares them.
- **Choosing where to decode:** a small GPU that also encodes can decode slower than the CPU, for exports and for the timeline's tiles. `gpu.decoding` off is the manual fix; vidcut could measure each machine's GPU decoding against its CPU and choose by itself.
- **Hardware AV1 proxies on AMF and QSV** (AMD RDNA 3 and later, Intel Arc and later): proxies use only NVENC's and VAAPI's AV1. Needs a machine with AV1 on AMF or QSV to check keyframes and frame order against the barcodes (`hardware_av1_proxies_are_exact_too`).

Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures. Image sequences stay on the CPU: GPUs don't decode PNG.
