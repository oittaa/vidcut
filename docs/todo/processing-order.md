# Processing order

**When:** next.

The [spec](../cut-page-spec.md#13-order-of-operations-pp-287296-3407) gives the order in which Resolve processes a picture. The [design](../design/README.md#picture-order) gives the order of vidcut. These are the places where they are different.

**Why:** the spec is the behavior of Resolve (AGENTS.md rule 1). The order of two stages can change the picture.

- **Retime before Edit Sizing.**
  - The spec: Edit Sizing (Transform, Cropping, Dynamic Zoom) comes before Speed Effects.
  - vidcut: Retime Process makes the frame from two source frames, upright, at the size that the picture shows at (`VideoFrames::retimed_frame`).
  - Then the draw places the made frame (`place::Placing`). Thus the rotation, the position and the crop come after the retime.
- **Retime before Classic Super Scale.**
  - The spec: Super Scale comes before Speed Effects.
  - vidcut: the frame of Retime Process is at most the size of its source. Classic Super Scale enlarges it later, in the draw (`screen::kernel`).
  - An AI upscaler already comes before the retime (`scaled_pair`).
- **The timeline color space last.**
  - The spec: the input goes into the timeline color space before Deinterlace and Super Scale.
  - vidcut: the draw converts each picture into the timeline color space (`color::shader_colors`). This is after the deinterlacer, the AI upscalers and Retime Process.
  - The AI upscalers refuse HDR media. Retime Process makes no HDR frames ([export](../design/export.md#which-source-frame)).

**Open:** which clips get a different picture from each difference. Measure this in Resolve and in vidcut before the fix.
