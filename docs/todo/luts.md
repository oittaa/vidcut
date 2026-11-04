# LUTs

**When:** not decided.

vidcut has an input LUT and an output LUT, from a `.cube` file ([design](../design/README.md#picture-order)). It has no display LUT.

**Why:**

- A display LUT corrects the viewer for one monitor. The export does not get it.
- With a LUT, a source's light is not the light of the frames. An export then has only the HDR metadata that the user gives.
- Each fact below decides the picture, and no measurement gives it yet.

**In the code:**

- `fs_lut` runs the output LUT on the canvas. The display LUT is a separate branch after that ([§13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
- `choose_through_lut` gives the HDR metadata of an export with a LUT.
- `Lights::analysis` measures the source files only.

**To do:**

- The display LUT, as the viewer's own branch.
- With a LUT, the export measures light on the resulting frames.
- Measure in Resolve:
  - The Output LUT of the project: does it use the math of the node LUT?
  - A picture with an alpha channel through a LUT.
  - The right-click menu of the Media Pool on a compound clip: does it give an input LUT? A script cannot set one.
  - A gap through the Output LUT.
  - A table range with equal low and high values.
  - The messages of the GUI for a broken file.
  - The source viewer: does it show the input LUT?

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
