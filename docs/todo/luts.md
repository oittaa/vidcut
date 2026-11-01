# LUTs

**When:** not decided.

vidcut applies no LUT. The spec has an input LUT, an output LUT and a display LUT ([§13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).

**Why:**

- A camera LUT makes a log picture look right. The primary knobs cannot replace that table.
- An output LUT is a look for the whole timeline, shared as a file.
- A `.cube` file is the table those looks use.

**In the code:**

- The design lists the LUTs among the stages vidcut does not have.
- `fs_convert` puts the picture into the timeline color space, before the deinterlace and Super Scale.
- The spec puts the input LUT at that point, on each clip.
- The spec puts the output LUT after subtitles, and before the output color space.
- The display LUT is a viewer branch. The export does not get it.
- The primary grade is a separate item ([primary-grade.md](primary-grade.md)).

**To do:**

- Measure a `.cube` in Resolve: its 1D table, its 3D table, its domain and its interpolation.
- An input LUT on a clip, just after `fs_convert`, in the viewer and in the export.
- An output LUT on the timeline, after subtitles, in the viewer and in the export.
- This item's LUTs are in the viewer and in the export.
- The display LUT is the viewer's own branch in §13, and it is separate work.
- The project stores the path as it stores a media path.
- The LUT runs in the timeline color space.
- A conversion for the LUT is shown where the user sets it, and in the export report.
- With no LUT, the picture and the HDR metadata stay as they are ([X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- With a LUT, the export measures light on the resulting frames.
- The CLI, IPC and MCP perform the same action as the GUI.

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
