# GPU frames: one pipeline

**When:** before another GPU path is added: QSV encoding from GPU memory, or Vulkan Video decoding ([vulkan-video.md](vulkan-video.md)).

**The rule:** a frame stays in GPU memory from the decoder to the encoder or the display, where this is possible (AGENTS.md rule 9). This is also true where the CPU is a little faster.

- [design/gpu.md](../design/gpu.md) tells how frames go.

## Open

1. **Frames to the encoders of AMD's and Intel's GPUs.**
   - The viewer, exports, proxies and thumbnails draw on the GPU. NVENC and VideoToolbox take the drawn planes in GPU memory ([design/export.md](../design/export.md#timeline-colors)).
   - VA-API, AMF, QSV and the Vulkan ProRes encoder take them from memory. Then VA-API and AMF frames go up again (`Upload`).
   - VA-API: export the planes from wgpu's Vulkan device as DMA-BUFs, and map them as VA surfaces.
   - AMF: share the planes with Direct3D 11 as textures, as the viewer imports the decoder's textures (`d3d11::Shared`).
   - QSV: take VA-API surfaces on Linux and Direct3D 11 textures on Windows.
   - Vulkan's ProRes: FFmpeg's Vulkan device is not wgpu's. The planes need memory that both devices import.
   - Each needs AMD or Intel hardware to check. Compare each with the frames that come through memory.
2. **One backend for each place** (memory, CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan).
   - Each backend has `down`, `up`, `keep` and `encoder_frames`.
   - Thus no code outside a backend asks on which kind of GPU a frame is.
   - The backends replace the split by mechanism: `hw::Keeper`, `Upload`, and `encoder_frames` for Direct3D 11 only.
3. **Capabilities tested, not written as rules.**
   - The code has rules for which encoders take the frames of which places (`encode::shares`).
   - Test these capabilities once for each GPU and FFmpeg, as vidcut tests the encoders. Keep the results with `encoders.json`.
4. **The HDR10+ light meter on the GPU.**
   - The meter of an HDR export brings each frame down to measure it (`light::Meter`).
   - A reduction on the GPU removes the last download of a whole frame in an HDR export.
   - Check its values against the values that the CPU measures.
