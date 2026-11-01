# Viewer scopes

**When:** not decided.

The viewer has no waveform, parade or vectorscope.

**Why:**

- Levels are hard to judge by eye, in SDR and in an HDR timeline.
- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The roadmap leaves the grade to the finishing tool.
- A scope shows the picture of the current draw.

**In the code:**

- A timeline is SDR, HDR PQ or HDR HLG.
- The draw stores the mix as 16-bit floats (`rgba16float` in `rgb.wgsl`).
- An export's bit depth follows the footage ([decision D44](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- An HDR export is 10-bit.
- `light::Meter` measures HDR light for metadata.

**To do:**

- Show a waveform, a parade and a vectorscope of the viewer picture.
- Use the timeline color space, PQ and HLG included.
- The viewer turns each scope on, and a request does the same.
- Leave OpenColorIO, node graphs and qualifiers to the finishing tool.
