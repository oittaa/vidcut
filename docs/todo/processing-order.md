# Processing order

**When:** next, after a decision on the open points.

The [spec](../cut-page-spec.md#13-order-of-operations-pp-287296-3407) gives the order in which Resolve processes a picture. The [design](../design/README.md#picture-order) gives the order of vidcut. This is the place where they are still different.

**Why:** the spec is the behavior of Resolve (AGENTS.md rule 1). The order of two stages can change the picture.

**The timeline color space last:**

- The spec: the input goes into the timeline color space before Deinterlace and Super Scale.
- vidcut: the draw converts each picture into the timeline color space (`color::shader_colors`). This is after the deinterlacer, the AI upscalers and Retime Process.
- In an SDR timeline, an SDR picture gets no other conversion than its matrix (D35, D50).

**Open:**

- The AI upscalers refuse HDR media. Retime Process makes no HDR frames ([export](../design/export.md#which-source-frame)).
  - In an HDR timeline, a conversion first gives them an HDR picture, also from SDR media.
  - In an SDR timeline, a conversion first gives them an SDR picture, also from HDR media.
- The GPU deinterlacers take the frames of the decoder (`route::Facts::deinterlacing`). A conversion first needs a deinterlacer for converted pictures.
