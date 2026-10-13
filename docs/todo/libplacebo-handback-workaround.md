# Drop the cover for libplacebo's early hand-backs

**When:** every FFmpeg build vidcut supports (AGENTS.md) has libplacebo with its commit 2ba21c07, "vulkan/gpu_tex: order the hold signal after reads on other queues"; no libplacebo release has it yet. FFmpeg builds that link libplacebo statically (BtbN's) carry the libplacebo of their build date.

libplacebo without it gives a frame back before reading it (gotchas.md). `cuda::wait_done` covers for it: in `cuda::Placebo::push` a frame goes to libplacebo only once its copy from CUDA is done, and in `pull` the next copy waits until libplacebo's picture is done. Each is the CPU waiting for the GPU once a frame.

Then remove `wait_done`'s two calls, `wait_done` and the gotcha, and run the loop of the [Linux check](nvidia-linux-converter-check.md) (on Windows too): three NVENC exports at once must neither hang nor show the frame before.
