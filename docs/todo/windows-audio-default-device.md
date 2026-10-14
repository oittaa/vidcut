# Sound follows Windows's default output

**When:** later.

On macOS and with PipeWire the default output's stream moves to a new default device by itself. WASAPI's doesn't: cpal reports `StreamInvalidated`, and the viewer shows it as a device failure while the sound stays on the old device or stops. Open the stream again on the new default (`player/output.rs`).
