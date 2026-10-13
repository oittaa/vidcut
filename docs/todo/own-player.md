# Player: what is left

**When:** next. It opens the way for [macOS](macos.md).

- **Vulkan Video decoding** (H.264, HEVC, AV1 on NVIDIA, AMD and Intel): its frames are Vulkan images already, so nothing is copied out of the decoder or shared between APIs (preview.md §3).
- **VA-API on AMD, shown from video memory:** untested. A surface with compression metadata (AMD's DCC modifiers) needs more than one plane per layer, which the import doesn't take, so such pictures are copied down instead.
- **macOS:** VideoToolbox, into Metal textures through IOSurface.
- **Encoding from GPU memory** on QSV ([gpu-decoding.md](gpu-decoding.md)) and VideoToolbox.
- **Backward play right after a seek** waits once for a GOP of decoding (the first window behind the playhead).

Every decoding path must pass the barcode fixtures, as the software decoder does. The CPU path stays for what GPUs don't decode: VP8, ProRes outside Apple, image sequences.
