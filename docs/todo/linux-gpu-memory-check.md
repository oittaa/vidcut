# GPU memory on Linux

**When:** next time on Ubuntu 26.04 with an NVIDIA GPU. WSL can't: its render node has no PCI address, and it has no NVIDIA Vulkan driver.

On Linux a GPU's memory (`Gpu::memory_mb`) is the largest device-local heap of the Vulkan device on its PCI bus (`vulkan_memory`, [design/gpu.md](../design/gpu.md#choosing-the-gpu-and-the-decoder)), and the player's automatic video budget is half of it. That code has not run on Linux yet.

1. `cargo build --release -p vidcut`
2. `target/release/vidcut preferences get --json | grep -oE '"(name|memory_mb)":[^,]*'`
3. `nvidia-smi --query-gpu=name,memory.total --format=csv`

Expected:
- The NVIDIA GPU has a `memory_mb`, close to nvidia-smi's total.
- With a GUI decoding a clip, `vidcut player stats` shows half of it as the Kept group's video memory budget.

Also open: on an AMD or Intel integrated GPU, whether the figure is shared system memory. AMD's Windows driver gives the Radeon iGPU a 32364 MB device-local heap where DXGI reports 2021 MB dedicated.
