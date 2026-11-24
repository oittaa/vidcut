# Auto Color

**When:** not decided.

The Color section of the Cut page viewer Tools has one control, Auto Color. vidcut has none.

**Why:**

- Auto Color balances the blacks and the whites of the current clip.
- It uses the frame at the playhead.
- It is the color item in the Cut page scope ([spec §1](../spec.md#1-scope)).

**In the code:**

- The viewer Tools row has Transform, Crop and Dynamic Zoom ([spec §8.15](../spec.md#815-transform-and-crop-pp-766768)).
- That row has no Color section.
- Each clip has a primary correction on the Color page ([spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)).
- [Spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212) has the measured result, the legacy method and the parts that are not known.

**To do:**

- The owner selects the analysis. The neural analysis of Resolve cannot be matched, so each choice is a deviation:
  - The legacy method of spec §8.25. It writes Lift and Gain, as Resolve does with Use Legacy Auto Color.
  - Another analysis that gives the six numbers E and the three numbers W of the measured result.
- Measure in Resolve whether an OpenTimelineIO file keeps Auto Color.
- One request runs it on the selected clip, from the frame at the playhead.
- A toggle turns that result off and keeps it.
- The CLI, IPC and MCP perform the same action as the GUI.
