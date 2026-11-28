# Float pictures

- The AI upscalers, the macOS scaler and the AI frames of Retime Process and Smooth Cut cut float pictures to 0..1.
- ProRes 4444, ProRes 4444 XQ and DNxHR 444 cache files: keep the values outside 0..1 with the curve of [X177](../deviations.md#x177-prores-4444-and-dnxhr-444-render-cache-files-hold-only-01). This needs 12-bit input in FFmpeg's ProRes and DNxHR encoders. First measure the ends of that curve and color patches.
