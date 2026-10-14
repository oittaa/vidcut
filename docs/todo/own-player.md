# Our own player: what is left

**When:** next. It opens the way for [macOS](macos.md).

- **Vulkan Video decoding** (H.264, HEVC, AV1 on NVIDIA, AMD and Intel): its frames are Vulkan images already, so nothing is copied out of the decoder or shared between APIs (preview.md §3).
- **VA-API on AMD:** pictures shown from video memory are untested there. A surface with compression metadata (AMD's DCC modifiers) needs more than one plane per layer, which the import doesn't take, so such pictures are copied down.
- **Encoding from GPU memory** on QSV, and VideoToolbox's pixel buffers on macOS: [gpu-frames.md](gpu-frames.md).
- **Backward play right after a seek** waits once for the first window behind it, a GOP of decoding.

Every decoding path must pass the barcode fixtures, as the software decoder does.
