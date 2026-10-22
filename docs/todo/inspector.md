# Inspector

**When:** not decided.

The Inspector tab shows and sets the transform, the crop and the Dynamic Zoom of the selected clips ([deviation X70](../deviations.md#x70-the-inspector-is-a-tab-beside-the-media-pool-in-frame-pixels)). It also shows the open title, subtitle and subtitle track. These parts are still to do.

- **Speed.**
  - Now: the Change Clip Speed window sets it (R, spec D48).
  - Why: Resolve's Video panel has Speed Change. The Inspector would show the speed of a clip without a window.
- **Volume.**
  - Now: the GUI has no control for the gain of a clip. Only `clip.set` with `gain_db` sets it.
  - Why: Resolve's Audio panel has the Volume of a clip.
- **Anchor Point.**
  - Now: vidcut has no anchor. It folds the anchor of a Resolve file into the position ([gotchas](../gotchas.md#resolves-otio-files)).
  - Why: Resolve zooms and turns about the anchor point.
- **Pitch, Yaw, crop Softness and Composite.**
  - Now: the spec has them out of scope (§8.15).
  - Why: Resolve's Video panel has them. They need a change of scope first.
