# Our own player: what is left

**When:** next. It opens the way for [macOS](macos.md).

The GUI already plays through the engine ([player.md](../decisions/player.md), [preview.md](../decisions/preview.md)). Still open:

- **Vulkan Video decoding** (H.264, HEVC, AV1 on NVIDIA, AMD and Intel), whose frames are Vulkan images already: none copied out of the decoder, nothing shared between APIs (preview.md §3 copies and shares).
- **VA-API's on AMD:** shown from video memory as Intel's are, untested: a surface with compression metadata (AMD's DCC modifiers) needs more than one plane per layer, which the import doesn't take, and such pictures are copied down instead.
- **Encoding from GPU memory:** NVENC, VAAPI and AMF take their decoder's frames on the GPU (export.md §5); QSV is left (todo/gpu-decoding.md). VideoToolbox's gained nothing on a Mac (export.md §5).
- **Backward play right after a seek** waits once for the first window behind it, a GOP of decoding: with 2160p60 0.27 s on the CPU and 0.47 s for H.264 on NVDEC. FFmpeg 8.0 also makes a GPU's H.264 decoder anew at every seek (gotchas.md). After that, 20 s backward across keyframes shows 96–100% of the frames at the automatic budget.
- **CPU path stays** for what GPUs don't decode: VP8, ProRes outside Apple, PNG and other image sequences.
- **Frame-exact:** every decoding path passes the barcode fixtures, as the software decoder does.
