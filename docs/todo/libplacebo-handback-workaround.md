# Remove the libplacebo workaround

**When:** every FFmpeg build vidcut supports comes with a libplacebo that has commit 2ba21c07 ("vulkan/gpu_tex: order the hold signal after reads on other queues"). No libplacebo release has it yet.

**What:** remove the workaround for libplacebo, which marks frames done too early (gotchas.md).

1. Remove `cuda::wait_done`.
2. Remove its three calls in `cuda.rs`:
   - Two in `Placebo::feed_frame`: after the copy into Vulkan, and on a frame decoded on Vulkan.
   - One in `Placebo::converted`, on libplacebo's output.
3. Remove the gotcha.
4. On Linux, and on Windows with an NVIDIA GPU, run three NVENC exports at once, sixty times. Use turned, SD, HDR and 10-bit clips.
5. An export must not hang or show the previous frame.
