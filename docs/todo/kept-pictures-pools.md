# Kept pictures and their decoders' pools

A picture the player keeps as decoded holds its decoder's whole frame pool, which never shrinks while any of its frames lives (gotchas.md). NVDEC's are copied into memory of their own (`cuda::keep`), Direct3D 11's into textures of their own (`hw::Keeper`), and the CPU's decoders decode into buffers of vidcut's own (`buffers.rs`). The others still hold their pools:

- **VA-API**: a kept frame is the decoder's surface. Decoders on a device share one pool for each codec, layout and size (`hw::vaapi::share_frames`), so a decoder made anew doesn't start a pool beside the one kept pictures hold; but a pool keeps its peak while any of its frames is kept, so pictures of other sizes or codecs can still go past the budget. A surface of its own for each kept picture needs an exact copy: Intel's driver (iHD 26.1) has `vaCopy`, exact for P010 and NV12, 1.0–1.4 ms for a 960×540 frame with the wait; Mesa's has neither it nor an exact video processor (gotchas.md). The budget also counts a surface by its samples, where AMD's take more (960×540 P010: 337 pictures counted as 499 MB took about 700 MB): DRM PRIME's export gives each surface's objects' sizes.
- **Vulkan** (ProRes on NVIDIA's GPU): a copy into images of vidcut's own needs a submission on FFmpeg's queues (`lock_queue`, deprecated), preview.md §3.
- **libdav1d** (AV1 on the CPU): its frames come from a pool of its own, and it takes no `get_buffer2` (no `AV_CODEC_CAP_DR1`). Only a copy frees them.
- **The deinterlacer** (interlaced video in memory): its frames are buffers of the filter graph's pools (libavfilter's, one for each link), freed after the graph and its last frame.

How much it costs shows where a decoder closes while its pictures are kept: proxies switched on and off, a cut to another file and back, each prepared picture's reader.
