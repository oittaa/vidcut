# Remove the libplacebo workaround

**When:** every FFmpeg build vidcut supports comes with a libplacebo that has commit 2ba21c07 ("vulkan/gpu_tex: order the hold signal after reads on other queues"). No libplacebo release has it yet.

**What:** the workaround for libplacebo marking frames done too early (gotchas.md). Remove `cuda::wait_done`, its three calls in `cuda.rs` (two in `Placebo::feed_frame`, after the copy into Vulkan and on a frame decoded on Vulkan, and one in `Placebo::converted` on libplacebo's output), and the gotcha. Then, on Linux and on Windows with an NVIDIA GPU, run three NVENC exports of turned, SD, HDR and 10-bit clips at once, sixty times: none may hang or show the previous frame.
