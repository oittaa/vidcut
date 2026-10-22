# An animated WebP with FFmpeg 8

**When:** not decided.

- FFmpeg has a demuxer and a decoder for animated WebP from 9.0.
- FFmpeg 8 reads an animated WebP with `webp_pipe`, as one image that it cannot decode. An animated WebP of one frame gives the same result.
- Thus vidcut takes the file as a still of 2×2 pixels, and its picture never renders. A render says that the still "ends before" the frame.
- Ubuntu 26.04's FFmpeg 8.0.1 shows this behavior.

**To do:**

- With FFmpeg 8, refuse an animated WebP on import, and tell the user that it needs FFmpeg 9.0. This is better than a still that shows nothing.
- The animation flag in the VP8X chunk identifies an animated WebP.
