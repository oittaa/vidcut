# A primary grade

**When:** not decided.

The Primaries palette of the Color page is the model. [Spec §14](../spec.md#14-primary-grade-pp-31413157) gives its measured math.

**Why:**

- Auto Color is the only color control of vidcut ([auto-color.md](auto-color.md)). A quick edit has no manual correction.
- Resolve exports no grade in an OpenTimelineIO file (spec §14.12). Thus a grade does not go between vidcut and Resolve in that file.

**In the code:**

- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The mix is `Wide`, format `Rgba32Float`, in the code values of the timeline ([decision D50](../spec.md#12-decisions-on-gaps-and-contradictions)).
- `to_linear` and `encode` run inside `fs_convert`. They are not the stored mix.
- An `Effect` runs on the placed picture, before composite mode and opacity (`fs_blur`).
- The scopes read the canvas after the mix ([gpu](../design/gpu.md#the-scopes)). Thus they show a grade that runs before the mix ends.

**To do:**

- One corrector with the math of spec §14.1–§14.7. It holds Temp, Tint, Lift, Gamma, Gain, Offset and the master wheels.
  It also holds Lum Mix, Saturation, Hue and Contrast below 1.
- The controls show the numbers of the window in §14.1.
- The clip grade runs after the effects of the clip and before the speed change ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)).
- The timeline grade runs on the mixed picture, before subtitles and the encode (§13).
- With the grade at its defaults, the picture and the HDR metadata stay as they are (X19).
- With a grade, the export measures light on the graded frames.
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).
- The CLI, IPC and MCP perform the same action as the GUI.

**To measure:** the items of spec §14.13.

- The formula of the Contrast S-curve, and its place before or after Saturation and Hue. The data is in `$VIDCUT_TEST_DATA/fixtures/color/out/cases/`.
- The formula of Color Boost. The data is in the same folder.
- The curves of Shadows and Highlights, and the filter of the three controls of spec §14.8. The data is in `$VIDCUT_TEST_DATA/fixtures/color/out/flat_cases/`.
- The shapes of the Log wheels between the ranges.
- The result of ColorTrace from CDL, and the import of an ALE with CDL (spec §14.12).

**Not this item:**

- More than one node, and the node mixers ([color-page.md](color-page.md)).
- The display LUT ([luts.md](luts.md)).
- A qualifier, after this primary.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
