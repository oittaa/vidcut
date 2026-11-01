# Auto Color

**When:** not decided.

The Color section of the Cut page viewer Tools has one control, Auto Color. vidcut has none.

**Why:**

- Auto Color balances the blacks and the whites of the current clip.
- It uses the frame at the playhead.
- It is the color item in the Cut page scope ([spec §1](../cut-page-spec.md#1-scope)).
- A five-slider primary is a different control ([primary-grade.md](primary-grade.md)).

**In the code:**

- The viewer Tools row has Transform, Crop and Dynamic Zoom ([deviation X65](../deviations.md#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)).
- That row has no Color section.
- vidcut has no grade ([deviation X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).

**To do:**

- Measure Auto Color in Resolve: what it changes, and whether an OpenTimelineIO file keeps it.
- One request runs it on the selected clip, from the frame at the playhead.
- A toggle turns that result off and keeps it.
- The CLI, IPC and MCP perform the same action as the GUI.
