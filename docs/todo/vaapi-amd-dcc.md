# VA-API pictures with compression on AMD

**When:** on an AMD GPU whose VA-API surfaces carry DCC. The Radeon of a Ryzen 7 9800X3D (Mesa 26.0) gives them tiled without it, one plane per layer.

**Why:**

- The viewer shows VA-API pictures from video memory. It imports their DMA-BUFs ([design/gpu.md](../design/gpu.md#each-decoders-frames)).
- `tests/picture.rs` checks this on the GPU that VA-API decodes on.
- A surface with compression metadata (AMD's DCC modifiers) has more than one plane for each layer.
- `DrmFrame::planes` (`crates/vidcut-av/src/hw/vaapi.rs`) takes only the first plane of each layer.
- wgpu's `texture_from_dmabuf_fd` takes one plane layout.

**To do:**

1. Import every plane of a layer into a Vulkan image of vidcut's own.
2. Check that the pictures show correctly and pass the barcode fixtures.
