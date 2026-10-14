# Untangle video decoding

**When:** next, before VideoToolbox decoding.

Adding a GPU decoder kind still touches the export and proxy code.

- `VideoFrames` (`export/video.rs`): one output setting (as decoded, fitted, or fitted on the GPU) instead of six builder options.
- NVENC's own NVDEC path (`GpuSide`, `gpu_reader`, `proxy::encode_on_gpu`) becomes the ordinary one, and `hw::Decoding::Nvdec` goes. The encoders' frames (`export/encode.rs`, `proxy.rs`) stop matching GPU formats.
