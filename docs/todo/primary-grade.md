# A primary grade

**When:** not decided.

The Cut page color control is Auto Color ([auto-color.md](auto-color.md)). These sliders are a deviation from that control, and they wait for a yes.

**Why:**

- Auto Color is one button. It is the color item of the Cut page.
- A quick edit that is delivered from vidcut has no manual correction.
- A program grade still finishes in Resolve ([roadmap](../roadmap.md)).
- Whether an OpenTimelineIO file keeps a grade is not measured.
- A grade the file drops does not come back with the timeline.

**In the code:**

- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The mix is `Wide`, format `Rgba16Float`, in the code values of the timeline ([decision D50](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- `to_linear` and `encode` run inside `fs_convert`. They are not the stored mix.
- An `Effect` runs on the placed picture, before composite mode and opacity (`fs_blur`).
- The spec puts a clip grade after those effects and before the speed change (§13).
- The spec puts a timeline grade after the tracks mix.
- Scopes are a separate item ([scopes.md](scopes.md)).

**To do, if this deviation is accepted:**

- Measure the controls Resolve writes for a primary, and whether an OpenTimelineIO file keeps them.
- Measure whether those controls work on the code values or in linear light.
- A clip grade, and one timeline grade, with the same math.
- The controls to measure are exposure, contrast, temperature, tint and saturation.
- The clip grade runs after the clip's effects and before the mix.
- The timeline grade runs on the mixed picture, before subtitles and the encode.
- With the grade at its defaults, the picture and the HDR metadata stay as they are (X19).
- With a grade, the export measures light on the graded frames.
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).
- The CLI, IPC and MCP perform the same action as the GUI.

**Not this item:**

- An input LUT and an output LUT ([luts.md](luts.md)).
- A qualifier, after this primary and the scopes.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
- A node graph. §13 is the order of the picture.
