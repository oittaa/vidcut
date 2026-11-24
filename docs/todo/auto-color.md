# Auto Color

**When:** not decided.

vidcut has Auto Color with the legacy method only ([X174](../deviations.md#x174-auto-color-uses-the-legacy-method)).
Two parts are open: a machine-learning method, and Auto Color in an OpenTimelineIO file.

**Why:**

- Resolve uses the DaVinci Neural Engine by default. Its legacy method is a preference (pp. 117, 3129).
- OTIO files go between vidcut and Resolve with as little loss as possible.
- An OTIO export of Resolve holds no grade of a clip. Auto Color itself in the file is not measured ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).

**In the code:**

- `auto_color::balance` is the one place of the method. It gives the Lift and Gain from the pixels of the frame.
- The corrector keeps the result and the replaced Lift and Gain. An OTIO file of vidcut keeps them in the vidcut metadata of the picture.

**To do:**

- A machine-learning method, when one is good enough:
  - It becomes the default in place of the legacy method in `auto_color::balance`.
  - The legacy method then stays as a Use Legacy Auto Color preference, as in Resolve (pp. 117, 3129).
  - The neural result of Resolve is not Lift and Gain. It is an Auto Color tool with the numbers E and W ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
  - If the new result is not Lift and Gain, the corrector needs a tool for it.
- Measure in Resolve whether an OpenTimelineIO file keeps Auto Color.
- If it does, read and write Auto Color in the OTIO import and export.
