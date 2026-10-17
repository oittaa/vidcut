# VA-API pictures with compression on AMD

**When:** with an AMD GPU at hand to test on.

The viewer shows VA-API pictures from video memory by importing their DMA-BUFs ([design/gpu.md](../design/gpu.md#each-decoders-frames)). That is untested on AMD. A surface with compression metadata (AMD's DCC modifiers) has more than one plane per layer, and `DrmFrame::planes` (`crates/vidcut-av/src/hw/vaapi.rs`) takes only each layer's first. Import every plane of a layer, then check on an AMD GPU that pictures show right and pass the barcode fixtures.
