# Auto Color

**When:** not decided.

It is not known whether an OpenTimelineIO file of Resolve keeps Auto Color.

**Why:**

- OTIO files go between vidcut and Resolve with as little loss as possible.
- An OTIO export of Resolve holds no grade of a clip. Auto Color itself in the file is not measured ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).

**In the code:**

- vidcut has Auto Color with the legacy method ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
- The corrector keeps the result and the replaced Lift and Gain. An OTIO file of vidcut keeps them in the vidcut metadata of the picture.

**To do:**

- Measure in Resolve whether an OpenTimelineIO file keeps Auto Color.
- If it does, read and write Auto Color in the OTIO import and export.
