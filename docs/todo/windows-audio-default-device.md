# Sound follows Windows's default output

**When:** later.

On macOS and with PipeWire, a stream on the default output moves to a new default device by itself. WASAPI's doesn't: cpal reports `StreamInvalidated`, which the viewer shows as a device failure while the sound stays on the old device or stops. Open the stream again on the new default (`player/output.rs`).
