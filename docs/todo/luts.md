# LUTs

**When:** not decided.

vidcut has an input LUT and an output LUT, from a `.cube` file ([design](../design/README.md#picture-order)). It has no display LUT.

**Why:**

- A display LUT corrects the viewer for one monitor. The export does not get it.
- With a LUT, a source's light is not the light of the frames. An export then has only the HDR metadata that the user gives.
- Each fact below decides the picture, and no measurement gives it yet.

**In the code:**

- `fs_lut` runs the output LUT on the canvas. The display LUT is a separate branch after that ([§13](../spec.md#13-order-of-operations-pp-287296-3407)).
- `choose_through_lut` gives the HDR metadata of an export with a LUT.
- `Lights::analysis` measures the source files only.

**To do:**

- The display LUT, as the viewer's own branch.
- With a LUT, the export measures light on the resulting frames.
- AI Slow in an HDR timeline, with a Retime Process, gives `T(C)` on a Premultiplied media item with an input LUT.
  - Why: the LUT runs on the enlarged frames, and they have no alpha ([upscale.md](upscale.md)).
- The Input Lookup Table of the project (manual p. 152). vidcut has an input LUT on a media item only.
  - The measured facts are in [spec §13](../spec.md#the-lut-tables).
- The source viewer shows the input LUT of the media item, as the viewer of the Media page does.
- Measure in Resolve. No script reaches these, so each needs the window of Resolve:
  - The thumbnails of the Media Pool: do they show the input LUT of a clip, and the Input LUT of the project?
  - The Input LUT of the project in the viewer of the Media page.
  - The Input LUT of the project on a generator, on a Text title, on a Text+ title and on a compound clip.
  - A LUT on the node of a Text+ title. Generators and Text titles take no grade ([spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)).
  - A gap through the Output LUT of the project, in a render with alpha. vidcut keeps the gap clear.

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
