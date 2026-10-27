# Dissolves mixed on Direct3D 11, VA-API and Vulkan GPUs

**When:** not decided.

- On these GPUs, the frames of a dissolve come down, and the CPU mixes them. The mix goes back up to the encoder (spec D50, `hw::can_mix`).
- vidcut mixes NVIDIA's frames on the GPU (`cuda/mix.rs`).
- vidcut mixes VideoToolbox's frames on Metal (`mixing/macos.rs`).
- The CPU draws a wipe that skips the GPU mix.
- Each GPU below needs a mix with the sums of `export::mix`.
- Compare each mix with the CPU's pictures.

The GPUs:

- **VA-API:** FFmpeg's `overlay_vaapi` takes a global `alpha`.
- **Direct3D 11:** the video processor blends a stream at an alpha (`VideoProcessorSetStreamAlpha`) over a background color.
- **Vulkan** (Vulkan's ProRes encoder): FFmpeg 9.0.1 has `xfade_vulkan` and `blend_vulkan`. `blend_vulkan` takes its opacity only at setup, not for each frame.
