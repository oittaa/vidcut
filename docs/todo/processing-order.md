# Processing order

**When:** next.

The [spec](../cut-page-spec.md#13-order-of-operations-pp-287296-3407) gives the order in which Resolve processes a picture. The [design](../design/README.md#picture-order) gives the order of vidcut. This is the place where they are still different.

**Why:** the spec is the behavior of Resolve (AGENTS.md rule 1). The order of two stages can change the picture.

**Deinterlace after the conversion:**

- The spec: the input goes into the timeline color space before Deinterlace.
- vidcut: the reader deinterlaces the frames of the decoder, in their own color space (`Deinterlacer`). The FFmpeg filters `bwdif_cuda`, `bwdif_vulkan` and `bwdif` take only such frames.
- To do: a deinterlacer of its own in the draw, after `fs_convert`. It reads the converted pictures of the frames before, at and after the frame.
