# Subtitles, titles and placed pictures drawn on Direct3D 11 and VA-API GPUs

**When:** not decided.

- On CUDA and VideoToolbox, vidcut mixes burned-in subtitles, titles and placed pictures into the frames on the GPU (`cuda/burn.rs`, `mixing/macos.rs`).
- On other GPUs, the frames come down to memory while a cue, a title or a placed picture shows. Then they go back up to the encoder.
- The export design tells how: [subtitles](../design/export.md#subtitles), [titles](../design/export.md#titles), [placed pictures](../design/export.md#placed-pictures).
- [gpu-placement.md](gpu-placement.md) is the item for placing the picture itself on the GPU.
- Each GPU below needs a mix of a picture with coverage over its frames. In the same way, [gpu-dissolves.md](gpu-dissolves.md) needs a mix at one weight.
- Compare each mix with the pictures of `burn::blend`.

The GPUs:

- **VA-API:** FFmpeg's `overlay_vaapi` takes a second surface with alpha.
- **Direct3D 11:** the video processor blends a second stream with alpha.
