# Untangle video decoding

**When:** next, before VideoToolbox decoding ([macos.md](macos.md)).

Each kind of GPU decoder is handled in many files. Adding one, such as VideoToolbox or Vulkan Video, touches most of them.

- **One module per kind** (NVDEC, VA-API, Direct3D 11), each with the same few functions: open, what it decodes, keep a frame, copy down, shrink, and fit for an encoder. Nothing outside matches on GPU frame formats. The viewer's import of GPU pictures (`gui/picture.rs`) goes the same way.
- **Split `VideoFrames`** (`export/video.rs`). Picking frames is one part. Making the output is the other, with one setting for it: as decoded, fitted, or fitted on the GPU for the encoder. That replaces `decoded`, `decode_on`, `keep_on_gpu`, `fit_on_gpu`, `fit_on_gpu_into` and `with_gpu`.
- **One GPU path for exports and proxies.** NVENC's own NVDEC path (`GpuSide`, `gpu_reader`, `proxy::encode_on_gpu`) becomes the ordinary NVDEC one, and `hw::Decoding::Nvdec` goes.

Frames stay exact: each decoder's pictures match the CPU's by barcode.
