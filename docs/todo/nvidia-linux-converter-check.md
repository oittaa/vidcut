# The GPU converter on Linux with an NVIDIA GPU

**When:** next time on a Linux desktop with an NVIDIA GPU. High priority.

NVENC exports convert turned, BT.601, HDR and 10-bit placed pieces with libplacebo on a Vulkan device derived from CUDA (export.md §5). On Windows (RTX 4080 SUPER) three exports at once hung at a converter's teardown or showed the frame before: libplacebo gives frames back before reading them (gotchas.md), and `cuda::wait_done` covers for it. None of the Linux machines here runs this path: the N150 is Intel, WSL has no NVIDIA Vulkan driver, and CI has no GPU. So whether the cover holds on Linux is unchecked.

On the desktop:

```bash
for i in $(seq 1 60); do timeout 180 cargo test -p vidcut-av --features gpu-tests --test export -- nvenc_exports --nocapture > run-$i.log 2>&1; echo "run $i exit $?"; done
```

- Exit 124 is a hang. Before the timeout kills it, take every thread's stack:

  ```bash
  gdb -p "$(pgrep -n export-)" -batch -ex 'thread apply all bt' > stacks.txt
  ```

- In the logs, look for:
  - a barcode assertion (`frame N: Some(a)` against `Some(b)`): a wrong picture;
  - `VK_ERROR_DEVICE_LOST` or `VkResult -4`: run with `VIDCUT_AV_LOG=error` to see it.
- Note the driver and FFmpeg versions (`nvidia-smi`, `vidcut doctor`).

The same desktop has the [ALSA check](alsa-audio-devices.md) to do.
