# Remove the GPU deinterlacers' check

**When:** every FFmpeg build vidcut supports (AGENTS.md rule 7) has the fixes for `bwdif_cuda` and `bwdif_vulkan` (code.ffmpeg.org issues 25105 and 25106, filed with patches; in no release yet).

**What:** remove `hw::deinterlace`, which runs three test frames through a GPU deinterlacer and through `bwdif` before trusting it, its call in `Deinterlacer::takes`, and the gotcha. NVDEC's and Vulkan's interlaced frames then deinterlace on the GPU. `tests/interlaced_gpu.rs` and the interlaced ProRes test in `tests/vulkan_prores.rs` must still find every row within a level of the CPU's, with the frames in GPU memory.

`hw::GpuGraph` stays: FFmpeg's Vulkan filters keeping frames without their frames context isn't reported upstream.
