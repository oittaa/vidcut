# GPU frames: one pipeline

**When:** before another GPU path is added: QSV encoding from GPU memory, or Vulkan Video decoding ([own-player.md](own-player.md)).

**The rule:** a frame stays in GPU memory from the decoder to the encoder or the display wherever that is possible, even where the CPU would be a little quicker (AGENTS.md rule 9). Light measurements and analyses come down whole on purpose: their values must be the CPU's (export.md §4).

## Open

1. **Colors converted on AMD's, Intel's and Windows' GPUs.** VA-API and Direct3D 11 fit a frame only where its colors stay (`vaapi::can_fit`, `d3d11::can_fit`), so a change of matrix or range, or HDR to SDR, brings every frame down to the CPU. NVIDIA's frames are converted by libplacebo on Vulkan (`cuda::CudaPictures`). VA-API's can go there through FFmpeg, which makes a Vulkan device from a VA-API or DRM one (`hw::Device::vulkan_on`). Direct3D 11's need an import of vidcut's own, as the viewer has (gui/picture.rs). VideoToolbox's `scale_vt` converts colors, but no test compares it with the CPU's math yet (`vt::can_fit`). Each is checked against the CPU's pictures on the barcode fixtures.
2. **One backend for each place** (memory, CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan), each with `fit`, `down`, `up`, `keep` and `encoder_frames`, so nothing outside a backend asks what kind of GPU a frame is on. Today the work is split by mechanism instead: `cuda::CudaPictures`, `hw::Fitter`, `hw::Shrinker`, `hw::Keeper`, and `encoder_frames` for Direct3D 11 alone.
3. **Deinterlacing and the size to scale to in `route::Ops`.** The reader deinterlaces before the planner sees a frame, and the planner knows only whether the output is smaller (`Facts::shrinks`).
4. **Capabilities tested, not written as rules.** What each GPU fits (`can_fit`) and which encoders take which places' frames (`encode::shares`, `hw::reaches`) are written in code. They are to be tested once for each GPU and FFmpeg and kept with `encoders.json`, as the encoders are.
5. **The media pool's thumbnails through the `Maker`.** `thumbs.rs` shrinks and converts with its own `hw::Shrinker` and `Normalizer`. Exports, proxies, the timeline's tiles, storyboards and `frame render` go through `export::make::Maker`.
6. **The route in an export's result.** A proxy's says how its frames went (`made_with`: `frames came down at 960×540`), an export's only its `copies`.
