# Clip transform: Dynamic Zoom and the OTIO round trip

**When:** next, first in [the roadmap](../roadmap.md#what-comes-next). A slow push on a still is what a simple video most often still needs another editor for.

Transform and Crop are in ([spec §8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), [D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [X64](../deviations.md#x64-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). Two parts are open.

## Dynamic Zoom

- **The Cut page** (p. 768): off until its controls are touched. A green box is the start and a red box the end; each is moved and resized in the viewer, and a path shows the motion. Swap reverses them. The ease is Linear, Ease In, Ease Out or Ease In and Out. Shift keeps a drag to one axis.
- **vidcut:** the Transform handles' shape for each box, on Alt + T (free now), and Dynamic Zoom in the clip's right-click menu with a Reset. A picture per end, in a picture's units ([D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), each frame's placing between them by the ease; the renderer already places each frame on its own (`place::Placing`).
- **Open:** the four eases' curves, measured from Resolve's renders.

## OpenTimelineIO

Resolve writes these as clip effects named `Resolve Effect`: Transform is type 2, Cropping type 3, Composite type 1, and Dynamic Zoom type 59, whose `dynamicZoomCenter` and `dynamicZoomScale` are keyed at `"0"` and `"1000"` ([OpenTimelineIO discussion 921](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/921)). vidcut carries them without playing them, and writes its own placement in the clip's `vidcut` metadata only, which Resolve drops ([gotchas](../gotchas.md#resolves-otio-files)); an export names the clips it places (`otio::placed`).

The ids of a clip's Transform are not measured. A title's are (`transformationZoomX`, `transformationZoomY`, `transformationRotationAngle`, spec D67). Position, the crop and Dynamic Zoom's ease need one Resolve Studio export measured, as the titles' were.

- **Open:** the units of Position and the crop in Resolve's OTIO file; whether Resolve applies a Transform and Cropping that vidcut wrote, as it needed a measured generator before it showed titles; then reading Resolve's back as pictures.
