# Our own player: what is left

**When:** next. It opens the way for [macOS](macos.md).

The GUI already plays through the engine ([player.md](../decisions/player.md), [preview.md](../decisions/preview.md)). Still open:

- **Other GPUs' pictures shown from video memory.** VA-API (Linux) and Direct3D 11 (Windows) decode into the engine's pictures, copied down as they are decoded (preview.md §3). Shown from there, as NVDEC's are: VA-API's surfaces exported as DMA-BUFs into Vulkan, Direct3D 11's textures shared with Vulkan, or Vulkan Video decoding (H.264, HEVC, AV1 on NVIDIA, AMD and Intel), whose frames are Vulkan images already.
- **macOS:** VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers).
- **Still open in the engine:** choosing the audio device.
- **Backward play right after a seek** waits once for the first window behind it, a GOP of decoding: with 2160p60 0.27 s on the CPU and 0.47 s for H.264 on NVDEC (its decoder is made anew at every seek, gotchas.md). After that, 20 s backward across keyframes shows 96–100% of the frames at the automatic budget.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
