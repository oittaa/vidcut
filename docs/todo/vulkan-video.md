# Decoding on Vulkan: Vulkan Video, FFv1, other vendors

**When:** after the interchange work ([roadmap](../roadmap.md)), NVIDIA first.

Only ProRes decodes on Vulkan (`ON_VULKAN`), and only on NVIDIA's GPUs ([design/gpu.md](../design/gpu.md)).

- **Vulkan Video decoding:**
  - H.264, HEVC, AV1 and VP9 on the GPU's video engine, on NVIDIA, AMD and Intel. FFmpeg 9.0.1 has this decoding.
  - Its frames are already Vulkan images. Thus vidcut copies nothing out of the decoder and shares nothing between APIs.
  - Every decoding path must pass the barcode fixtures, as the CPU's path does.
- **FFv1 on Vulkan,** when FFmpeg's decoder is faster.
  - It decodes correctly (barcodes, timestamps, seeks).
  - But 1080p with 24 slices runs at 29 fps on an RTX 4080 SUPER with eight frame threads. The CPU decodes at 484 fps.
  - A seek takes 340 ms, against 25 ms on the CPU.
  - FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders.
- **Other vendors:**
  - vidcut trusts Vulkan decoding on NVIDIA only (`GpuDecoder::why_not`).
  - FFmpeg's ProRes decoder crashes on AMD's Windows driver ([gotchas](../gotchas.md)).
  - vidcut cannot catch a crash. Thus the first test decode on a vendor's GPU must run under a marker, as the encoders' tests do (`encoders.testing`).
  - A marker that a crashed process leaves keeps Vulkan decoding off for that vendor.
