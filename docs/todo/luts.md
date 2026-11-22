# LUTs

**When:** not decided.

vidcut has an input LUT of a media item and of the project, and an output LUT, from a `.cube` file ([design](../design/README.md#lut-files)). It has no display LUT.

**Why:**

- A display LUT corrects the viewer for one monitor. The export does not get it.
- The measured facts are in [spec §13](../spec.md#the-lut-tables).

**In the code:**

- `fs_lut` runs the output LUT on the canvas. The display LUT is a separate branch after that ([§13](../spec.md#13-order-of-operations-pp-287296-3407)).

**To do:**

- The display LUT, as the viewer's own branch.
- AI Slow in an HDR timeline, with a Retime Process, gives `T(C)` on a Premultiplied media item with an input LUT.
  - Why: the LUT runs on the enlarged frames, and they have no alpha ([upscale.md](upscale.md)).
- The source viewer shows the input LUT of the media item, or of the project, as the viewer of the Media page does.
- The thumbnails of the pool show the Input LUT of the project.
- A LUT on the node of a title, when titles take a grade.

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
