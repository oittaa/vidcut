# Clip transform: Dynamic Zoom

**When:** next, first in [the roadmap](../roadmap.md#what-comes-next). A slow push on a still is what a simple video most often still needs another editor for.

Transform and Crop are in, in OTIO files too ([spec §8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), [D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [X65](../deviations.md#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). Dynamic Zoom is open, and two smaller parts:

- **Titles in a placed compound clip** stay where they are. Placing them needs a title drawn through the compound's maps (`place::Placing`) in the viewer's `fs_title` and the export's title drawing, which move, zoom and turn a title but don't crop or stretch it.
- **`edit.decompose`** drops a placed compound's picture with a warning. Its clips could take it where that is exact: a clip with no picture of its own that fills the frame.

## Dynamic Zoom

The facts below were measured in Resolve Studio 21.1.1 through its scripting API: values set on clips, OTIO files exported, hand-written ones imported, and stills and a Deliver render read back. Resolve's *picture* is the source sized as its Retime and Scaling says, fitted into the frame by default ([gotchas](../gotchas.md#resolves-otio-files)).

- **The Cut page** (p. 768): off until its controls are touched. A green box is the start and a red box the end; each is moved and resized in the viewer, and a path shows the motion. Swap reverses them. The ease is Linear, Ease In, Ease Out or Ease In and Out. Shift keeps a drag to one axis.
- **Resolve:** a box per end: `dynamicZoomCenter`, its middle from the picture's in fractions of the picture, y up, and `dynamicZoomScale`, its size as a fraction of the picture, the same both ways. The picture is drawn at 1/size about the box's middle, after the flips and before Transform's zoom, turn and move; the crop goes with it and the picture's outline grows with the zoom. Turned on, it starts at size 0.8 and ends at 1, both in the middle: a pull out from 125%. Size and middle go in a straight line from the start on the clip's first frame, frame *k* of *n* at *k*/*n*, so the end comes a frame after the last.
- **Resolve's ease:** `dynamicZoomEase` (0 Linear, 1 Ease In, 2 Ease Out, 3 Ease In and Out) is kept and written, but all four play the same straight line, set through scripting or read from a file, in stills and in a Deliver render. Its Inspector menu wasn't tried.
- **Resolve's OTIO:** a `Resolve Effect` of type 59, both parameters keyed at −*g* and 1000 − *g*, *g* the timeline's `global_start_time`; Resolve reads the earlier key as the start and the later as the end whatever their times (0 and 1000 or −86400 and −85400, with *g* 86400 or 0).
- **vidcut:** the Transform handles' shape for each box, on Alt + T (free now), and Dynamic Zoom in the clip's right-click menu with Swap and Reset. A box per end in fractions of the picture, played as Resolve does; the renderer already places each frame on its own (`place::Placing`).
- **Eases:** vidcut's are real curves, the user's choice, recorded as a deviation when Dynamic Zoom is built.
