# LUTs

**When:** not decided.

vidcut has an input LUT of a media item and of the project, and an output LUT, from a `.cube` file ([design](../design/README.md#lut-files)). It has no display LUT.

**Why:**

- A display LUT corrects the viewer for one monitor. The export does not get it.
- Each fact below decides the picture, and no measurement gives it yet.

**In the code:**

- `fs_lut` runs the output LUT on the canvas. The display LUT is a separate branch after that ([§13](../spec.md#13-order-of-operations-pp-287296-3407)).

**To do:**

- The display LUT, as the viewer's own branch.
- AI Slow in an HDR timeline, with a Retime Process, gives `T(C)` on a Premultiplied media item with an input LUT.
  - Why: the LUT runs on the enlarged frames, and they have no alpha ([upscale.md](upscale.md)).
- The source viewer shows the input LUT of the media item, as the viewer of the Media page does.
- Measure in Resolve. No script reaches these, so each needs the window of Resolve:
  - The thumbnails of the Media Pool: do they show the input LUT of a clip, and the Input LUT of the project?
  - The Input LUT of the project in the viewer of the Media page.
  - A LUT on the node of a Text+ title. Generators and Text titles take no grade ([spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)).
  - A gap through the Output LUT of the project, in a render with alpha. vidcut keeps the gap clear.

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
