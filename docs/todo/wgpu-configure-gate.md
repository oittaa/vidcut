# wgpu 31: drop the submit gate

**When:** once eframe moves to a wgpu release with gfx-rs/wgpu#10296 (merged for 31; not in 30.0.1).

wgpu 30's `Surface::configure` fails when another thread submits while it waits for the device to go idle ([gotchas](../gotchas.md)), so the viewer thread submits through `Gate::pass` (`gui/gate.rs`, [design/playback.md](../design/playback.md#the-viewer-thread)). #10296 keeps the surface's presentation locked through a configure instead of failing it. With it, remove the gate, `Gpu::behind`, and the gotcha.
