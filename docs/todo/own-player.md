# Our own player: what is left

**When:** next. It opens the way for [macOS and Windows](macos-windows.md).

## Where it stands

- **The GUI plays through vidcut's own engine** (`vidcut_av::player`, [decisions/player.md](../decisions/player.md)) and draws with eframe on wgpu: Vulkan on Linux, Metal and DX12 compiled in. See [decisions/preview.md](../decisions/preview.md). One decoder for playing, paused frames, exports and thumbnails.
- **Pictures** are YUV, converted to BT.709 RGB by the viewer's own WGSL shader with the export's math, HDR tone-mapped as the export does.
- **GPU decoding:** H.264, HEVC, AV1 and VP9 decode on NVDEC and are copied down to memory at their own size, then uploaded as textures (preview.md §3). Exports and proxies made with NVENC decode on NVDEC and scale on the GPU (export.md §5, preview.md §6). The rest is in [gpu-decoding.md](gpu-decoding.md).

## How the rest goes

- **Pictures left on the GPU** (zero-copy): NVDEC's CUDA frames, or Vulkan Video's images, imported into wgpu's device as textures without the copy down and up, and kept pictures in video memory (2 s of 1080p60 in NV12 is about 370 MB). Needs external-memory interop between FFmpeg's CUDA or Vulkan device and wgpu's (wgpu-hal's Vulkan device from raw handles, or FFmpeg's Vulkan device made on wgpu's `VkDevice`). Today the copy costs 0.25–0.3 cores at 4K60 (preview.md §8).
- **More decoders into the engine's pictures**, through FFmpeg's hardware contexts:
  - Linux: VAAPI (AMD, Intel) by the NVDEC path (`VideoReader::open_nvdec_or_cpu`), or Vulkan Video (H.264, HEVC, AV1 on NVIDIA, AMD and Intel).
  - Windows: Vulkan Video, or D3D11/D3D12.
  - macOS: VideoToolbox, into Metal textures through IOSurface.
- **Faster NVDEC seeks:** FFmpeg's NVDEC path decodes a frame at a time and waits for each (4K H.264 at 253 frames a second here, the CPU at 1,014), so seeks and backward windows take three to four times as long as on a CPU with many cores; a deeper decode queue would help.
- **Encoding from GPU memory:** NVENC takes CUDA frames, VAAPI its surfaces, VideoToolbox its pixel buffers; scaling stays on the GPU (FFmpeg's CUDA, VAAPI and Vulkan scalers).
- **Still open in the engine:** choosing the audio device.
- **Backward 4K play in long GOPs** is short of real time: 3 s backward of 2160p60 HEVC shows 84 of 181 frames on NVDEC and 23 on the CPU.
- **The first step back** from the middle of a GOP takes 150–500 ms (a window decodes from the keyframe); the steps after it take 17–35 ms.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
