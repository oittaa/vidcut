# Dissolves mixed on Direct3D 11, VA-API, VideoToolbox and Vulkan GPUs

**When:** not decided.

These GPUs' frames come down for a dissolve to be mixed on the CPU, and the mix goes back up to the encoder (spec D50, `hw::can_mix`); NVIDIA's are mixed on the GPU (`cuda/mix.rs`). Each needs a mix with `export::mix`'s sums, checked against the CPU's pictures (`tests/dissolves.rs`):

- **VideoToolbox** (the Mac comes after NVIDIA): a Metal compute pass on the frames' IOSurfaces.
- **VA-API:** FFmpeg's `overlay_vaapi` takes a global `alpha`.
- **Direct3D 11:** the video processor blends a stream at an alpha (`VideoProcessorSetStreamAlpha`) over a background color.
- **Vulkan** (Vulkan's ProRes encoder): FFmpeg 9.0.1 has `xfade_vulkan` and `blend_vulkan`; `blend_vulkan` takes its opacity only when it is set up, not per frame.
