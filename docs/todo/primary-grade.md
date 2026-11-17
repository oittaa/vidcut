# A primary grade

**When:** not decided.

The Primaries palette of the Color page is the model. [Spec §14](../spec.md#14-primary-grade-pp-31413157) gives its measured math.

**Why:**

- Auto Color is the only color control of vidcut ([auto-color.md](auto-color.md)). A quick edit has no manual correction.
- Resolve exports no grade in an OpenTimelineIO file (spec §14.9). Thus a grade does not go between vidcut and Resolve in that file.

**In the code:**

- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The mix is `Wide`, format `Rgba32Float`, in the code values of the timeline ([decision D50](../spec.md#12-decisions-on-gaps-and-contradictions)).
- `to_linear` and `encode` run inside `fs_convert`. They are not the stored mix.
- An `Effect` runs on the placed picture, before composite mode and opacity (`fs_blur`).
- The scopes read the canvas after the mix ([gpu](../design/gpu.md#the-scopes)). Thus they show a grade that runs before the mix ends.

**To do:**

- One corrector with the math of spec §14.2–§14.6: Lift, Gamma, Gain, Offset, the master wheels, Saturation, Hue, Temp and Tint.
- The clip grade runs after the effects of the clip and before the speed change ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)).
- The timeline grade runs on the mixed picture, before subtitles and the encode (§13).
- With the grade at its defaults, the picture and the HDR metadata stay as they are (X19).
- With a grade, the export measures light on the graded frames.
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).
- The CLI, IPC and MCP perform the same action as the GUI.

**To measure:** the items of spec §14.10. They need a grade made in the Resolve window.

- The numbers that the window shows for Lift, Gamma, Gain, Offset, Temp and Tint.
- The S-curve of Contrast, and Pivot.
- Color Boost, Shadows, Highlights, Midtone Detail and Lum Mix.
- The Log wheels: their shapes between the ranges, Low Range and High Range.
- The place of Temp, Tint and Contrast in the order of a node.
- Whether Resolve reads a CDL from an OpenTimelineIO, EDL or ALE import.

**Not this item:**

- More than one node, and the node mixers ([color-page.md](color-page.md)).
- The display LUT ([luts.md](luts.md)).
- A qualifier, after this primary.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
