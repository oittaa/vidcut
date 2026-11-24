# Auto Color

**When:** not decided.

vidcut has Auto Color with the legacy method only ([X174](../deviations.md#x174-auto-color-uses-the-legacy-method)).
Three parts are open: the Auto Color button of the viewer Tools, a machine-learning method, and Auto Color in an OpenTimelineIO file.

**Why:**

- It is not measured what the Auto Color button of the Cut page viewer Tools does in Resolve (p. 771).
  - It is not known whether the button stays lit, what a second click does, and what it does with the legacy preference.
  - The neural result of Resolve is an Auto Color tool with its own switch. Off, the tool gives no change ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
  - The legacy result of Resolve is Lift and Gain, with no switch. Undo takes it back.
- Resolve uses the DaVinci Neural Engine by default. Its legacy method is a preference (pp. 117, 3129).
- OTIO files go between vidcut and Resolve with as little loss as possible.
- An OTIO export of Resolve holds no grade of a clip. Auto Color itself in the file is not measured ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).

**In the code:**

- `grade.auto_color` writes the Lift and Gain of red, green, blue and the master into a corrector. It has no switch.
- The button of the viewer Tools runs the same analysis on node 1. It does not stay lit.
- `auto_color::balance` is the one place of the method. It gives the Lift and Gain from the pixels of the frame.
- An OTIO file of vidcut keeps the result in the vidcut metadata of the grade.

**To do:**

- Measure the Auto Color button of the Cut page viewer Tools in Resolve: two clicks, Ctrl+Z, and the legacy preference off and on.
  - Then make the button of vidcut do the same. Off must not turn the node off.
- A machine-learning method, when one is good enough:
  - It becomes the default in place of the legacy method in `auto_color::balance`.
  - The legacy method then stays as a Use Legacy Auto Color preference, as in Resolve (pp. 117, 3129).
  - The neural result of Resolve is not Lift and Gain. It is an Auto Color tool with the numbers E and W ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
  - If the new result is not Lift and Gain, the corrector needs a tool for it.
- Measure in Resolve whether an OpenTimelineIO file keeps Auto Color.
- If it does, read and write Auto Color in the OTIO import and export.
