# Our own player: what is left

**When:** next. It opens the way for [macOS and Windows](macos-windows.md).

The GUI already plays through the engine ([player.md](../decisions/player.md), [preview.md](../decisions/preview.md)). Still open:

- **More decoders into the engine's pictures**, through FFmpeg's hardware contexts:
  - Linux: VAAPI (AMD, Intel) by the NVDEC path (`VideoReader::open_nvdec_or_cpu`), or Vulkan Video (H.264, HEVC, AV1 on NVIDIA, AMD and Intel).
  - Windows: Vulkan Video, or D3D11/D3D12.
  - macOS: VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers).
- **Still open in the engine:** choosing the audio device.
- **Backward play right after a seek** waits once for the first window behind it, a GOP of decoding: with 2160p60 0.27 s on the CPU and 0.47 s for H.264 on NVDEC (its decoder is made anew at every seek, gotchas.md). After that, 20 s backward across keyframes shows 96–100% of the frames at the automatic budget.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
