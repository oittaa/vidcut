# wgpu 31: drop the submit gate

**When:** once eframe moves to a wgpu release with gfx-rs/wgpu#10296 (merged for 31; not in 30.0.1).

**Why:**

- wgpu 30's `Surface::configure` waits for the device to go idle. It fails when another thread submits during this wait ([gotchas](../gotchas.md)).
- Thus the viewer thread submits through `Gate::pass` (`gui/gate.rs`, [design/playback.md](../design/playback.md#the-viewer-thread)).
- #10296 keeps the surface's presentation locked through a configure, and the configure does not fail.

**To do:** with #10296, remove the gate, `Gpu::behind` and the gotcha.
