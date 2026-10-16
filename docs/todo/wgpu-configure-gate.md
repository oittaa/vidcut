# wgpu 31: drop the submit gate

**When:** once eframe moves to a wgpu release with gfx-rs/wgpu#10296 (merged into trunk for 31; not in 30.0.1).

wgpu 30's `Surface::configure` fails when another thread submits while it waits for the device to go idle (gotchas.md), so the viewer thread's submissions wait for the UI's `logic` and `ui` (`gui/gate.rs`, preview.md §2). #10296 no longer fails a configure for that: it keeps the surface's presentation locked through it instead. With it, remove the gate, `Gpu::behind`, and the gotcha.
