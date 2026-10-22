# Placed pictures on the GPU

**When:** not decided.

An NVENC export places a moved, zoomed or cropped picture on the CPU ([design/export.md](../design/export.md#placed-pictures)):

1. The picture's frames come down. The GPU first shrinks them to the size at which they show.
2. The CPU fits them and lays them out with their coverage (`export::place::overlay`).
3. `cuda::burn` mixes the result into the frame below. That frame stays on the GPU.

Thus a full-frame punch-in on 4K brings every frame down whole.

Open:

- **Fit it on the GPU** at the size it shows.
  - vidcut makes the CUDA scaler (`CudaPictures`) once for each source, at the output's size.
  - A placed picture needs a scaler at its own size.
- **Place it with a CUDA kernel:**
  - Map each pixel back through its `place::Placing` levels.
  - Cover and tap it as `export::place` does.
  - Mix it into the frame below.
  - Compare the frames with the CPU's frames.
- **Vulkan,** possibly: the same placement as a compute pass on Vulkan frames.
