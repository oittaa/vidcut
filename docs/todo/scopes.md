# Viewer scopes

**When:** not decided.

The viewer has no waveform, parade or vectorscope.

**Why:**

- The Cut page Tools have no scope. Record a deviation if this item is built.
- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The grade is a separate item ([primary-grade.md](primary-grade.md)).

**In the code:**

- A timeline is SDR, HDR PQ or HDR HLG.
- The mix is `Wide`, format `Rgba16Float`, in `screen/mod.rs`.
- Those values are the code values of the timeline ([decision D50](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- `rgba16float` in `rgb.wgsl` is the texture `unpack16` writes when it unpacks stored RGB.
- An export's bit depth follows the footage ([decision D44](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- An HDR export is 10-bit.
- `light::Meter` measures HDR light for metadata.

**To do:**

- Show a waveform, a parade and a vectorscope of the viewer picture.
- Use the timeline color space, PQ and HLG included.
- The viewer turns each scope on, and a request does the same.
- The scopes read the viewer picture, after the grades and after an output LUT ([luts.md](luts.md)).
