# Clip transform: one open part

**When:** with the smaller items ([roadmap](../roadmap.md#what-comes-next)).

vidcut has Transform, Crop and Dynamic Zoom, in OTIO files too ([spec §8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), [D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [X65](../deviations.md#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). One smaller part is open:

- **`edit.decompose`** removes the picture of a placed compound clip, with a warning.
  - Its clips could take the picture where the result is exact: a clip without a picture of its own that fills the frame.
