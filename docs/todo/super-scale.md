# Super Scale on more GPUs

**When:** next, in this order: AMD, then the Mac.

Super Scale enlarges pictures with a better upscaler ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). Exports use libplacebo for NVENC and swscale on the CPU. These parts are still to do:

- **AMD on Windows:** FFmpeg's `sr_amf` (AMD's video upscaler, since FFmpeg 8.0).
  - It takes Direct3D 11 frames, which the AMD path decodes. Its output goes to AMF.
  - Its `sharpness` option takes the Sharpness of the media.
- **The Mac:** VideoToolbox's super-resolution scaler (`VTSuperResolutionScalerConfiguration`, macOS 26 and later).
  - It is not in FFmpeg. vidcut needs its own binding, for example through `objc2-video-toolbox`.
  - It can download its model at the first use.
  - Reports say that it does 2x and 4x only. Then 3x takes 4x and fits the result. The Inspector and the export's report must say so (AGENTS.md rule 10).
  - The owner has an M4 MacBook Air for tests.
- **The other GPUs** (VA-API, Direct3D 11 on Intel, Vulkan): they fit pictures as before. A picture with Super Scale could come down to the CPU, or use libplacebo where Vulkan works.
- **The export's report:** it does not yet say which upscaler each file used.
- **Placed pictures on NVENC:** they are made on the CPU. libplacebo could make them on the GPU.
- **The viewer:** it shows pictures without Super Scale. A paused frame could show it.
- **Noise Reduction and the Enhanced modes** of Resolve (p. 433).
