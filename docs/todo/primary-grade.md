# A primary grade

**When:** not decided.

vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)). A clip cannot be made brighter, warmer or less green.

**Why:**

- A quick edit has to look right without a finishing tool.
- A full grade of a program still finishes in Resolve ([roadmap](../roadmap.md)).
- OpenColorIO loads a studio color configuration. It does not grade a clip.
- A node graph is a second compositor. The picture already has one order ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
- A qualifier selects a range of color. It needs this primary, and the scopes, first.

**In the code:**

- `fs_convert` puts each picture into the timeline color space. The mix is 16-bit float.
- `to_linear` and `encode` in `picture.wgsl` already convert SDR, PQ and HLG.
- `color::shader_colors` supplies the matrices and the scales for that conversion.
- An `Effect` runs on the placed picture, before composite mode and opacity (`fs_blur`).
- The spec puts the clip grade after those effects and before the speed change (§13).
- The spec puts the timeline grade after the tracks mix.
- Scopes are a separate item ([scopes.md](scopes.md)).
- More color spaces are another ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).

**To do:**

- Measure the primary controls Resolve writes, and whether an OpenTimelineIO file keeps them.
- A clip grade, and one timeline grade, with the same math.
- The controls are exposure, contrast, temperature, tint and saturation.
- The math runs in linear light, in the timeline color space, in the same draw.
- The clip grade runs after the clip's effects and before the mix.
- The timeline grade runs on the mixed picture, before subtitles and the encode.
- With the grade at its defaults, the picture and the HDR metadata stay as they are (X19).
- With a grade, the export measures light on the graded frames.
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).
- The CLI, IPC and MCP perform the same action as the GUI.

**Later than this:**

- A qualifier: a second grade on a range of hue, saturation or luma. It needs the scopes.
- A look loaded from a file. OpenColorIO is one way to load a studio configuration, after the extra color spaces exist.
- A node graph stays out. §13 is the order of the picture.
