# Kept pictures and their decoders' pools

A picture the player keeps as decoded holds its decoder's whole frame pool, which never shrinks while any of its frames lives (gotchas.md). NVDEC's are copied into memory of their own (`cuda::keep`), Direct3D 11's into textures of their own (`hw::Keeper`). The others still hold their pools:

- **VA-API**: a kept frame is the decoder's surface. Copying it out needs the video processor (VPP) or `vaCopy`, on the decoder's display.
- **Vulkan** (ProRes on NVIDIA's GPU): a copy into images of vidcut's own needs a submission on FFmpeg's queues (`lock_queue`, deprecated), preview.md §3.
- **The CPU**: pictures kept as decoded (layouts the display takes: yuv420p, NV12, P010…) are buffers of the decoder's pools. A `get_buffer2` of vidcut's own that allocates each frame on its own frees them with the picture, without a copy. Converted pictures (RGB images) are already their own.

How much it costs shows where a decoder closes while its pictures are kept: proxies switched on and off, a cut to another file and back, each prepared picture's reader.
