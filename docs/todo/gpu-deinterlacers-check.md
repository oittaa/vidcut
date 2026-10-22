# Remove the GPU deinterlacers' check

**When:** every FFmpeg build vidcut supports (AGENTS.md rule 7) has the fixes for `bwdif_cuda` and `bwdif_vulkan` (code.ffmpeg.org issues 25105 and 25106, filed with patches; in no release yet).

**What:**

1. Remove `hw::deinterlace`. Before vidcut trusts a GPU deinterlacer, this function runs three test frames through it and through `bwdif`.
2. Remove its call in `Deinterlacer::takes`.
3. Remove the gotcha.

Then NVDEC's and Vulkan's interlaced frames deinterlace on the GPU. With the frames in GPU memory, these tests must still find each row within a level of the CPU's row:

- `tests/interlaced_gpu.rs`.
- The interlaced ProRes test in `tests/vulkan_prores.rs`.

`hw::GpuGraph` stays. It works around a different FFmpeg bug, which is not reported upstream: FFmpeg's Vulkan filters keep frames without their frames context.
