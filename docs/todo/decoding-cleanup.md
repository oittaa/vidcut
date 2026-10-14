# Untangle video decoding

**When:** next, before VideoToolbox decoding.

Each GPU decoder kind is handled in many files, so adding one touches most of them.

- One module per kind (NVDEC, VA-API, Direct3D 11) with the same functions: open, what it decodes, keep a frame, copy down, shrink, fit for an encoder. The viewer's import (`gui/picture.rs`) the same way.
- `VideoFrames` (`export/video.rs`): one output setting (as decoded, fitted, or fitted on the GPU) instead of six builder options.
- NVENC's own NVDEC path (`GpuSide`, `gpu_reader`, `proxy::encode_on_gpu`) becomes the ordinary one, and `hw::Decoding::Nvdec` goes.
