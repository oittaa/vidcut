# Remove the libplacebo workaround

**When:** every FFmpeg build vidcut supports comes with a libplacebo that has commit 2ba21c07 ("vulkan/gpu_tex: order the hold signal after reads on other queues"). No libplacebo release has it yet.

**What:** the workaround for libplacebo marking frames done too early (gotchas.md). Remove `cuda::wait_done` and its two calls in `cuda::Placebo::push` and `pull`, and the gotcha. Then run the loop in [the Linux check](nvidia-linux-converter-check.md), on Windows too: three NVENC exports at once must not hang or show the previous frame.
