# Remove the libplacebo workaround

**When:** every FFmpeg build vidcut supports comes with a libplacebo that has commit 2ba21c07 ("vulkan/gpu_tex: order the hold signal after reads on other queues"). No libplacebo release has it yet.

**What:** the workaround for libplacebo marking frames done too early (gotchas.md). Remove `cuda::wait_done` and its two calls in `cuda::Placebo::push` and `pull`, and the gotcha. Then run the check below on Linux and on Windows with an NVIDIA GPU: three NVENC exports at once must not hang or show the previous frame.

**The check:** a project `check.vidcut` of four barcode fixtures one after another (tools.md): a turned clip, an SD clip (untagged, so BT.601), a PQ clip and a 10-bit clip of another aspect.

```bash
make_fixture h264 mp4 1920x1080 30 300 rot=90
make_fixture h264 mp4 720x576 30 300
make_fixture hevc10 mkv 1920x1080 30 300 hdr=pq
make_fixture hevc10 mkv 1440x1080 30 300
```

Export it once with `--encoder cpu` to `cpu.mp4`. Then run three NVENC exports at once, 60 times (about 8.5 s a round on an RTX 4080 SUPER):

```bash
for i in $(seq 1 60); do for j in 1 2 3; do (VIDCUT_AV_LOG=error timeout 180 vidcut -p check.vidcut export start out-$i-$j.mp4 --encoder nvenc > run-$i-$j.log 2>&1; echo "run $i.$j exit $?") & done; wait; done
```

- Exit 124 is a hang. Before the timeout kills it, take every thread's stack with `gdb -p PID -batch -ex 'thread apply all bt'`. Where Yama allows gdb only on its own children (`ptrace_scope` 1), start the exports with a preloaded library that calls `prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY)`.
- A wrong picture: `read_barcode --all` prints other barcodes for an `out-*.mp4` than for `cpu.mp4`.
- A lost device: `VK_ERROR_DEVICE_LOST` or `VkResult -4` in the logs.
- The turned clip logs libplacebo's `Validation failed: dst->params.blit_dst` about four times a frame on Linux; its pictures are right.
- Note the driver, FFmpeg and libplacebo versions (`nvidia-smi`, `vidcut doctor`).
