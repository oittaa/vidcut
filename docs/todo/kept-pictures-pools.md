# Kept pictures and their decoders' pools

A picture the player keeps as decoded holds its decoder's whole frame pool, which never shrinks while any of its frames lives (gotchas.md). NVDEC's are copied into memory of their own (`cuda::keep`), and the CPU's decoders decode into buffers of vidcut's own (`buffers.rs`). The others still hold their pools:

- **VA-API**: a kept frame is the decoder's surface. Copying it out needs the video processor (VPP) or `vaCopy`, on the decoder's display.
- **Vulkan** (ProRes on NVIDIA's GPU): a copy into images of vidcut's own needs a submission on FFmpeg's queues (`lock_queue`, deprecated), preview.md §3.
- **Direct3D 11**: `hw::Keeper` copies into textures of its own, but from a pool of its own that grows, made again for each decoder's frames. Textures made and released one by one would free with the picture.
- **libdav1d** (AV1 on the CPU): its frames come from a pool of its own, and it takes no `get_buffer2` (no `AV_CODEC_CAP_DR1`). Only a copy frees them.
- **The deinterlacer** (interlaced video in memory): its frames are buffers of the filter graph's pools (libavfilter's, one for each link), freed after the graph and its last frame.

How much it costs shows where a decoder closes while its pictures are kept: proxies switched on and off, a cut to another file and back, each prepared picture's reader.
