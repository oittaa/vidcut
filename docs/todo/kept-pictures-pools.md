# Kept pictures and their decoders' pools

A picture the player keeps as decoded holds its decoder's whole frame pool, which never shrinks while any of its frames lives (gotchas.md). NVDEC's are copied into memory of their own (`cuda::keep`), Direct3D 11's into textures of their own (`hw::Keeper`), VA-API's by `vaCopy` into surfaces of their own where the driver has it (Intel's), and pictures in memory are in buffers of vidcut's own (`buffers.rs`). Mesa's VA-API has no exact copy (gotchas.md): its pictures stay in a pool the decoders share, which the store lets go whole (player.md §3). Still open:

- **Vulkan** (ProRes on NVIDIA's GPU, FFmpeg 8.1 and later): a kept frame is the decoder's own image, from a pool that grows. A copy into images of vidcut's own needs a submission on FFmpeg's queues (`lock_queue`, deprecated), preview.md §3. Ubuntu's FFmpeg 8.0 has no Vulkan ProRes decoder, so it needs a newer build to test.
