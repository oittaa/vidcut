# Our own player instead of mpv

**When:** the engine exists; GPU frames and the display are next. It opens the way for [macOS and Windows](macos-windows.md).

The preview plays through libmpv. libmpv embeds only through OpenGL or software rendering, so egui stays on OpenGL (glow): no Vulkan, no Metal, and OpenGL is deprecated on macOS. mpv is also a second decoder next to vidcut-av's, and most of the preview gotchas in AGENTS.md come from that seam: leading pictures dropped at EDL segment ends (preview.md §9), the MKV remux for index-less containers, the render-thread rules, the 200 ms renderer timeout, paused frames drawn by another renderer (preview.md §13).

mpv's hardware decoding is not the problem: NVDEC frames already stay in GPU memory up to the screen. The aim is one decoder for playing, paused frames, exports and thumbnails, decoding and encoding on the GPU wherever that is faster.

## Where it stands

- **GPU decoding:** exports and proxies made with NVENC decode on NVDEC and scale on the GPU (export.md §5, preview.md §12). The rest is in [gpu-decoding.md](gpu-decoding.md).
- **The engine** (`vidcut_av::player`, [decisions/player.md](../decisions/player.md)): plays a render plan headless with the export's readers, so every picture is `frame render`'s. A decode thread keeps pictures ahead of the clock and windows for going back; requests return at once and merge; the audio mix goes to a device through cpal, and the device's callback is the clock, its latency made up for. J/K/L speeds, backward play, live faders, edits that keep open files and kept pictures. Pictures are CPU frames. Nothing in the GUI uses it yet.

## How the rest goes

- **Display:** eframe on wgpu (Vulkan on Linux and Windows, Metal on macOS, DX12 possible). wgpu takes a decoded GPU image as a texture without a copy (a raw `VkImage` or `MTLTexture`). YUV to RGB, ranges, and HDR tone mapping become our shaders, matching `vidcut_av::color`.
- **Decoding into GPU memory**, through FFmpeg's hardware contexts, into the engine's pictures (`Picture::memory`):
  - Linux: Vulkan Video (H.264, HEVC, AV1 on NVIDIA, AMD and Intel), or NVDEC and VAAPI shared into Vulkan.
  - Windows: Vulkan Video, or D3D11/D3D12.
  - macOS: VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers).
- **The GUI on the engine:** the live session seeks and presents through it (a seek is shown once presented), `player.state` reads its status, `mixer.preview` sets a lane gain instead of going through ZeroMQ, `player.mute` mutes it. The tail patches, the MKV remuxes and the image lists of the mpv preview go.
- **Still open in the engine:** reversed sound playing backward, sound while scrubbing, choosing the audio device.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
- **Memory:** kept frames move to VRAM (2 s of 1080p60 in NV12 is about 370 MB).

## Order

1. GPU frames in the engine: hardware decoding into its pictures, scaled on the GPU, kept in video memory; tested headless against `frame render` as the CPU frames are.
2. eframe on wgpu, the GUI on the engine, and mpv removed, in one step: mpv needs OpenGL, so the two can't run side by side.
