# Clip transform: two open parts

**When:** with the smaller items ([roadmap](../roadmap.md#what-comes-next)).

vidcut has Transform, Crop and Dynamic Zoom, in OTIO files too ([spec §8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), [D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [X65](../deviations.md#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). Two smaller parts are open:

- **Titles in a placed compound clip** stay where they are.
  - To place them, the viewer's `fs_title` and the export's title drawing must draw a title through the maps of the compound clip (`place::Placing`).
  - Now they move, zoom and turn a title, but they don't crop or stretch it.
- **`edit.decompose`** removes the picture of a placed compound clip, with a warning.
  - Its clips could take the picture where the result is exact: a clip without a picture of its own that fills the frame.
