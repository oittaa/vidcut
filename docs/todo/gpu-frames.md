# GPU frames: one pipeline

**When:** before another GPU path is added: QSV encoding from GPU memory, or Vulkan Video decoding ([vulkan-video.md](vulkan-video.md)).

**The rule:** a frame stays in GPU memory from the decoder to the encoder or the display, where this is possible (AGENTS.md rule 9). This is also true where the CPU is a little faster.

- For light measurements and analyses, frames come down whole on purpose: their values must be the CPU's values.
- [design/gpu.md](../design/gpu.md) tells how frames go.

## Open

1. **Colors converted on AMD's, Intel's and Windows' GPUs.**
   - VA-API and Direct3D 11 fit a frame only where its colors do not change (`vaapi::can_fit`, `d3d11::can_fit`).
   - Thus a change of matrix or range, or HDR to SDR, brings every frame down.
   - VA-API's frames can get to libplacebo on Vulkan through FFmpeg. FFmpeg makes a Vulkan device from a VA-API or DRM device (`hw::Device::vulkan_on`).
   - Direct3D 11's frames need an import of vidcut's own, as the viewer has (`gui/picture.rs`).
   - Check each against the CPU's pictures on the barcode fixtures.
2. **One backend for each place** (memory, CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan).
   - Each backend has `fit`, `down`, `up`, `keep` and `encoder_frames`.
   - Thus no code outside a backend asks on which kind of GPU a frame is.
   - The backends replace the split by mechanism: `cuda::CudaPictures`, `hw::Fitter`, `hw::Shrinker`, `hw::Keeper`, and `encoder_frames` for Direct3D 11 only.
3. **Deinterlacing and the size to scale to in `route::Ops`.**
   - The reader deinterlaces before the planner sees a frame.
   - The planner knows only if the output is smaller (`Facts::shrinks`).
4. **Capabilities tested, not written as rules.**
   - The code has rules for what each GPU fits (`can_fit`). It also has rules for which encoders take the frames of which places (`encode::shares`, `hw::reaches`).
   - Test these capabilities once for each GPU and FFmpeg, as vidcut tests the encoders. Keep the results with `encoders.json`.
5. **The media pool's thumbnails through the `Maker`.**
   - `thumbs.rs` shrinks and converts with its own `hw::Shrinker` and `Normalizer`.
   - Exports, proxies, the timeline's tiles, storyboards and `frame render` go through `export::make::Maker`.
6. **The route in an export's result.**
   - The result of a proxy tells how its frames went (`made_with`: `frames came down at 960×540`).
   - The result of an export gives only its `copies`.
