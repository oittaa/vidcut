# Export: decode the next frame during the draw

**When:** an export where the encoder is not the slowest step.

- The export thread decodes and draws one frame at a time. `vidcut-encode` encodes the drawn frames ([design/export.md](../design/export.md#encoding-one-output)).
- A draw thread can let the export thread decode the next frame while the current frame draws.
- With NVENC, this cannot make an export faster. NVENC is the slowest step, and decoding and the draw already run ahead of it.
  - Measured: NVENC takes 3.1 ms for each 1080p H.264 frame and 7.9 ms for each 4K 10-bit HEVC frame.
- `VIDCUT_DEBUG=1` prints where the time of an export goes ([tools.md](../tools.md)).
