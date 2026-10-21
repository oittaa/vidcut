# Placed pictures on the GPU

**When:** not decided.

An NVENC export places a moved, zoomed or cropped picture on the CPU ([design/export.md](../design/export.md#placed-pictures)): its frames come down, shrunk on the GPU to the size they show, the CPU fits them and lays them out with their coverage (`export::place::overlay`), and `cuda::burn` mixes that into the frame below, which stays on the GPU. A full-frame punch-in on 4K brings every frame down whole.

Open:

- **Fit it on the GPU** at the size it shows: the CUDA scaler (`CudaPictures`) is made once per source at the output's size, and a placed picture needs one at its own.
- **Place it with a CUDA kernel:** each pixel mapped back through its `place::Placing` levels, covered and tapped as `export::place` does, mixed into the frame below; checked against the CPU's frames.
- **Vulkan,** possibly: the same placement as a compute pass on Vulkan frames.
