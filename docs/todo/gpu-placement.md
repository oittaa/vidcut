# Placed pictures on the GPU

**When:** not decided.

For NVENC and VideoToolbox, a GPU kernel places a picture when the GPU fits it at the size of the frame (`cuda::place`, `Mixer::place`, [design/export.md](../design/export.md#placed-pictures)). These parts are open:

- **Placed pictures at another size:** a punch-in on a source with more pixels than the frame, or a picture with Super Scale.
  - vidcut makes the CUDA scaler (`CudaPictures`) once for each source, at the output's size.
  - A placed picture at another size needs a scaler at its own size. Until then, its frames come down.
- **Composite modes and alpha:** a frame with a mode, or a picture with alpha, mixes on the CPU.
  - A kernel needs the sums of `blend::mixed`, and floats between the layers (spec D75).
- **Titles in a placed compound clip:** the CPU maps them (`export::place::title`). Then `cuda::burn` mixes them in. `cuda::place` can map them.
- **Vulkan,** possibly: the same placement as a compute pass on Vulkan frames.
- **Compare** the frames of `Mixer::place` with the CPU's frames, on a Mac (`scripts/gpu-check.sh`).
