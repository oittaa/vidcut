# Our own player instead of mpv

**When:** later, after [GPU decoding](gpu-decoding.md); it opens the way for [macOS and Windows](macos-windows.md).

The preview plays through libmpv. libmpv embeds only through OpenGL or software rendering, so egui stays on OpenGL (glow): no Vulkan, no Metal, and OpenGL is deprecated on macOS. mpv is also a second decoder next to vidcut-av's, and most of the preview gotchas in AGENTS.md come from that seam: leading pictures dropped at EDL segment ends (preview.md §9), the MKV remux for index-less containers, the render-thread rules, the 200 ms renderer timeout, paused frames drawn by another renderer (preview.md §13).

mpv's hardware decoding is not the problem: NVDEC frames already stay in GPU memory up to the screen. The aim is one decoder for playing, paused frames, exports and thumbnails, decoding and encoding on the GPU wherever that is faster.

## How

- **Display:** eframe on wgpu (Vulkan on Linux and Windows, Metal on macOS, DX12 possible). wgpu takes a decoded GPU image as a texture without a copy (a raw `VkImage` or `MTLTexture`). YUV to RGB, ranges, and HDR tone mapping become our shaders, matching `vidcut_av::color`.
- **Decoding into GPU memory**, through FFmpeg's hardware contexts:
  - Linux: Vulkan Video (H.264, HEVC, AV1 on NVIDIA, AMD and Intel), or NVDEC and VAAPI shared into Vulkan.
  - Windows: Vulkan Video, or D3D11/D3D12.
  - macOS: VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers). A plain cut then decodes, scales and encodes without frames reaching the CPU.
- **What mpv does now that we build:** audio output (cpal), the audio clock, A/V sync and frame pacing, real-time playback across cuts (the export reader already opens the next piece ahead, `export/ahead.rs`), J/K/L speeds and reverse play, live fader changes (the export mix is already ours).
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
- **Memory:** kept frames move to VRAM (2 s of 1080p60 in NV12 is about 370 MB).

## Order

1. GPU decoding in vidcut-av ([gpu-decoding.md](gpu-decoding.md)): paused frames, proxies and exports get faster; mpv stays.
2. A GPU-only export path: hardware decoding, GPU scaling, hardware encoding.
3. The player engine (audio, clock, playback across cuts), tested headless against `frame render`.
4. eframe on wgpu and mpv removed, in one step: mpv needs OpenGL, so the two can't run side by side.
