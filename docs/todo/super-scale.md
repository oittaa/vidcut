# Super Scale on more GPUs

**When:** next: AMD.

Super Scale enlarges pictures with a better upscaler ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). Exports use libplacebo for NVENC and swscale on the CPU. These parts are still to do:

- **AMD on Windows:** FFmpeg's `sr_amf` (AMD's video upscaler, since FFmpeg 8.0).
  - It takes Direct3D 11 frames, which the AMD path decodes. Its output goes to AMF.
  - Its `sharpness` option takes the Sharpness of the media.
- **AI Fast and AI Best on macOS:** they are off. On the owner's M4, AI Fast took 11.5 s for each frame (480p to 1080p).
  - Find where the time goes on Metal. Turn them on again when they are fast enough.
- **The other GPUs** (VA-API, Direct3D 11 on Intel, Vulkan): they fit pictures as before. A picture with Super Scale could come down to the CPU, or use libplacebo where Vulkan works.
- **The export's report:** it does not yet say which upscaler each file used.
- **Placed pictures on NVENC:** they are made on the CPU. libplacebo could make them on the GPU.
- **Noise Reduction and the Enhanced modes** of Resolve (p. 433).
- **Measure** (with a plan that the owner accepts first):
  - The time of an export with a zoomed clip, made on the CPU and on the GPU.
  - Dropped frames in the viewer. It draws zoomed clips on the GPU, and mixes wipes on the CPU.
- **AI models:** [ai-upscaling.md](ai-upscaling.md).
