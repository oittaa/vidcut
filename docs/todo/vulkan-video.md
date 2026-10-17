# Decoding on Vulkan: Vulkan Video, FFv1, other vendors

**When:** after the interchange work ([roadmap](../roadmap.md)), NVIDIA first.

ProRes alone decodes on Vulkan now (`ON_VULKAN`), on NVIDIA's GPUs ([design/gpu.md](../design/gpu.md)).

- **Vulkan Video decoding:** H.264, HEVC, AV1 and VP9 on the GPU's video engine, which FFmpeg 9.0.1 has, on NVIDIA, AMD and Intel. Its frames are Vulkan images already, so nothing is copied out of the decoder or shared between APIs. Every decoding path must pass the barcode fixtures, as the CPU's does.
- **FFv1 on Vulkan,** once FFmpeg's decoder is faster. It decodes right (barcodes, timestamps, seeks), but 1080p with 24 slices runs at 29 fps on an RTX 4080 SUPER with eight frame threads, against the CPU's 484, and a seek takes 340 ms against 25. FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders.
- **Other vendors:** Vulkan decoding is trusted on NVIDIA only (`GpuDecoder::why_not`; FFmpeg's ProRes decoder crashes on AMD's Windows driver, [gotchas](../gotchas.md)). A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a crashed process keeps Vulkan decoding off there.
