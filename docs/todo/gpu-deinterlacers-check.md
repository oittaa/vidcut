# Remove the GPU deinterlacers' check

**When:** every FFmpeg build vidcut supports has the fixes for `bwdif_cuda` and `bwdif_vulkan` (code.ffmpeg.org issues 25105 and 25106, filed with patches; 9.0.2 and master 5fb8bfe275 have neither). AGENTS.md rule 7 says which builds those are.

**What:** `hw::deinterlace`, the check that runs three test frames through a GPU deinterlacer and through `bwdif` before trusting it, and `Deinterlacer::takes`'s call to it (decode.rs): NVDEC's and Vulkan's interlaced frames then deinterlace on the GPU without it. The gotchas.md entry on the GPU deinterlacers goes too. Then `tests/interlaced_gpu.rs` and the interlaced ProRes test in `tests/vulkan_prores.rs` must still find every row within a level of the CPU's, with the frames in GPU memory.

`hw::GpuGraph` (frees a graph's filters downstream first, gotchas.md) stays: FFmpeg's Vulkan filters keeping frames without their frames context isn't reported upstream.
