# VA-API pictures with compression on AMD

**When:** on an AMD GPU whose VA-API surfaces carry DCC. The Radeon of a Ryzen 7 9800X3D (Mesa 26.0) gives them tiled without it, one plane per layer.

The viewer shows VA-API pictures from video memory by importing their DMA-BUFs ([design/gpu.md](../design/gpu.md#each-decoders-frames)); `tests/picture.rs` checks that on the GPU VA-API decodes on. A surface with compression metadata (AMD's DCC modifiers) has more than one plane per layer, but `DrmFrame::planes` (`crates/vidcut-av/src/hw/vaapi.rs`) takes only each layer's first, and wgpu's `texture_from_dmabuf_fd` takes one plane layout. Import every plane of a layer into a Vulkan image of vidcut's own, then check that pictures show right and pass the barcode fixtures.
