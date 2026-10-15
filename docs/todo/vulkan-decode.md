# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**Where it is:** ProRes decodes on NVIDIA's GPU through FFmpeg's Vulkan decoder, the viewer shows it from there (preview.md §3), exports and proxies take its frames on the GPU into NVENC and Vulkan's ProRes encoder (export.md §5, preview.md §6), and automatic proxies are ProRes Proxy there (spec D41). What is open is below.

## Open

- FFv1 on Vulkan, once FFmpeg's decoder is faster. It decodes right there (barcodes, timestamps, seeks: `VideoCodec::Ffv1`), but 1080p with 24 slices runs at 17.7 fps on the RTX 4080 SUPER with one thread and 29 with eight frame threads, against the CPU's 484, and seeks take 340 ms against 25. So it stays on the CPU. FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.
