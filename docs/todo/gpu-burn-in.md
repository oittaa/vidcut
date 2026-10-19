# Subtitles and titles drawn on Direct3D 11, VA-API and VideoToolbox GPUs

**When:** not decided.

Burning subtitles in, and drawing titles, mixes them into CUDA frames on the GPU (`cuda/burn.rs`); other GPUs' frames come down to memory while a cue or a title shows and go back up to the encoder ([design/export.md](../design/export.md#subtitles), [titles](../design/export.md#titles)). Each needs a mix of a picture with coverage over its frames, checked against `burn::blend`'s pictures, as [gpu-dissolves.md](gpu-dissolves.md) needs a mix at one weight:

- **VA-API:** FFmpeg's `overlay_vaapi` takes a second surface with alpha.
- **Direct3D 11:** the video processor blends a second stream with alpha.
- **VideoToolbox:** a Metal compute pass on the frames' IOSurfaces.
