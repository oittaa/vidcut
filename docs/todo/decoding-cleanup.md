# Untangle video decoding

**When:** next, before VideoToolbox decoding.

Adding a GPU decoder kind still touches the export and proxy code.

- NVENC's own NVDEC path (`GpuSide`, `gpu_reader`, `proxy::encode_on_gpu`) becomes the ordinary one, and `hw::Decoding::Nvdec` goes. The encoders' frames (`export/encode.rs`, `proxy.rs`) stop matching GPU formats.
