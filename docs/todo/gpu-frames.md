# GPU frames: what is left

Frames stay in GPU memory from the decoder to the encoder wherever the GPU does the work right (AGENTS.md rule 9). `route::plan` picks each source's route, `export::video::Maker` makes its pictures, and `copies` counts what still crosses (export.md §5, preview.md §6). Open:

- **VideoToolbox on a Mac:** `proxy_check` on the barcode fixtures and the measurement on the M4 (tools.md). Whether VideoToolbox's H.264 encoder takes 10-bit frames, so 10-bit sources needn't come down for 8-bit proxies. Its colors and HDR: `scale_vt` converts matrix, primaries and transfer, untested against the CPU's tone curve.
- **VA-API's colors and HDR through libplacebo:** FFmpeg maps VA-API frames into Vulkan and back (`vulkan_map_from_vaapi`, `vulkan_map_to_vaapi`), so the pieces VA-API's own filters can't convert could stay on the GPU as Direct3D 11's do. It needs a Linux machine with a VA-API GPU to test.
- **QSV from GPU memory:** QSV takes frames from memory.
- **Capabilities kept for each GPU and FFmpeg**, with `encoders.json`: what each backend fits, tested once rather than as each scaler is made (the Direct3D 11 scaler's levels check), so the planner knows before a job starts.
