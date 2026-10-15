# Dissolves mixed on Direct3D 11, VA-API and VideoToolbox GPUs

The frames a dissolve mixes come down from these GPUs to be mixed on the CPU, and the mix goes back up to the encoder (spec D50, `hw::can_mix`). NVIDIA's GPU mixes them for NVENC (`cuda/mix.rs`). Each needs a mix with `export::mix`'s sums, checked against the CPU's pictures (`tests/dissolves.rs`):

- **VideoToolbox** (the Mac comes second, after NVIDIA): a Metal compute pass on the frames' IOSurfaces.
- **VA-API:** FFmpeg's `overlay_vaapi` takes a global `alpha`.
- **Direct3D 11:** the video processor blends a stream at an alpha (`VideoProcessorSetStreamAlpha`) over a background color.
