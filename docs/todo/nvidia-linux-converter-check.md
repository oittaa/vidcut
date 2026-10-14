# The GPU converter on Linux with an NVIDIA GPU

**When:** next time on a Linux desktop with an NVIDIA GPU. High priority.

NVENC exports convert turned, BT.601, HDR and 10-bit placed pieces with libplacebo on a Vulkan device derived from CUDA (export.md §5). A libplacebo bug makes exports hang or show the previous frame, and vidcut works around it (gotchas.md). Check the workaround on Linux with an NVIDIA GPU.

On the desktop, make a project `check.vidcut` from the barcode fixtures in `$VIDCUT_TEST_DATA`: a turned clip, an SD clip, an HDR clip and a 10-bit clip. Export it with `--encoder cpu` to `cpu.mp4` once. Then run three NVENC exports at once, 60 times:

```bash
for i in $(seq 1 60); do for j in 1 2 3; do (VIDCUT_AV_LOG=error timeout 180 vidcut -p check.vidcut export start out-$i-$j.mp4 --encoder nvenc > run-$i-$j.log 2>&1; echo "run $i.$j exit $?") & done; wait; done
```

- Exit 124 is a hang. Before the timeout kills it, take every thread's stack:

  ```bash
  gdb -p "$(pgrep -n vidcut)" -batch -ex 'thread apply all bt' > stacks.txt
  ```

- A wrong picture: `cargo run -p vidcut-av --example read_barcode` prints other barcodes for an `out-*.mp4` than for `cpu.mp4`.
- A lost device: `VK_ERROR_DEVICE_LOST` or `VkResult -4` in the logs.
- Note the driver and FFmpeg versions (`nvidia-smi`, `vidcut doctor`).

The same desktop has the [ALSA check](alsa-audio-devices.md) to do.
