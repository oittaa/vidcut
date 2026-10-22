# Inspector

**When:** not decided.

The Inspector tab shows and sets the transform, the crop and the Dynamic Zoom of the selected clips ([deviation X70](../deviations.md#x70-the-inspector-is-a-tab-beside-the-media-pool-in-frame-pixels)).
Its Audio section sets the Volume, the Mute and the Voice Isolation of the selected clips with sound.
It also shows the open title, subtitle and subtitle track. These parts are still to do.

- **Subtitle list.**
  - Now: the Subtitle section shows one subtitle, the subtitle that the user opened. Previous and Next open the subtitle before or after it (X43).
  - Why: Resolve's Captions panel lists every subtitle of the track at its bottom. Prev and Next move through them (p. 1233).
  - To do: a list of the subtitles of the track in the Subtitle section.
  - Why: the user sees the subtitles of the track together, and selects them in the list.
- **Speed.**
  - Now: the Change Clip Speed window sets it (R, spec D48).
  - Why: Resolve's Video panel has Speed Change. The Inspector would show the speed of a clip without a window.
- **Anchor Point.**
  - Now: vidcut has no anchor. It folds the anchor of a Resolve file into the position ([gotchas](../gotchas.md#resolves-otio-files)).
  - Why: Resolve zooms and turns about the anchor point.
- **Pitch, Yaw, crop Softness and Composite.**
  - Now: the spec has them out of scope (§8.15).
  - Why: Resolve's Video panel has them. They need a change of scope first.
