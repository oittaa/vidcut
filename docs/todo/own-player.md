# GPU presentation for the player

**When:** after more of [GPU decoding](gpu-decoding.md). It opens the way for [macOS and Windows](macos-windows.md).

Playback already decodes and mixes with the FFmpeg libraries in-process (`vidcut_av::play`) and writes the mix to PulseAudio on Linux. It does not load libmpv. Seek, step, forward and reverse play, and live faders run that way. Reverse is stepped seeks. The mix is the clock. Preview audio is PulseAudio (`libpulse-simple`), and macOS and Windows have no playback device yet.

What is left is drawing a GPU frame. The GUI is still egui on OpenGL (glow). libmpv remains only for the preview spikes, which embed through OpenGL. A GPU viewer would use wgpu (Vulkan on Linux and Windows, Metal on macOS) so the GUI can leave OpenGL. OpenGL is deprecated on macOS.

The aim is one decoder for playing, paused frames, exports and thumbnails, decoding on the GPU wherever that is faster. Proxies already decode on NVDEC when the source allows it.

## How

- **Display:** eframe on wgpu (Vulkan on Linux and Windows, Metal on macOS, DX12 possible). wgpu takes a decoded GPU image as a texture without a copy (a raw `VkImage` or `MTLTexture`). YUV to RGB, ranges, and HDR tone mapping become our shaders, matching `vidcut_av::color`.
- **Decoding into GPU memory**, through FFmpeg's hardware contexts:
  - Linux: Vulkan Video (H.264, HEVC, AV1 on NVIDIA, AMD and Intel), or NVDEC and VAAPI shared into Vulkan.
  - Windows: Vulkan Video, or D3D11/D3D12.
  - macOS: VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers). A plain cut then decodes, scales and encodes without frames reaching the CPU.
- **Already built, in software:** the audio clock, playback across cuts at the project's sample rate and channel count, J/K/L speeds, reverse play, and live fader changes. Do not replace PulseAudio with another output library for Linux.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
- **Memory:** kept frames move to VRAM (2 s of 1080p60 in NV12 is about 370 MB).

## Order

1. GPU decoding in vidcut-av for what is still on the CPU: exports, timeline tiles, then the playing viewer ([gpu-decoding.md](gpu-decoding.md)). Proxies already decode on NVDEC.
2. A GPU-only export path: hardware decoding, GPU scaling, hardware encoding.
3. Upload the session player's frame through wgpu, and drop the OpenGL viewer path. The libmpv spikes can stay until nothing calls them.
