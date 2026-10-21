# Clip transform: Dynamic Zoom and the OTIO round trip

**When:** next, first in [the roadmap](../roadmap.md#what-comes-next). A slow push on a still is what a simple video most often still needs another editor for.

Transform and Crop are in ([spec §8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), [D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [X65](../deviations.md#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). Two parts are open, and two smaller ones:

- **Titles in a placed compound clip** stay where they are. Placing them needs a title drawn through the compound's maps (`place::Placing`) in the viewer's `fs_title` and the export's title drawing, which move, zoom and turn a title but don't crop or stretch it.
- **`edit.decompose`** drops a placed compound's picture with a warning. Its clips could take it where that is exact: a clip with no picture of its own that fills the frame.

The facts below were measured in Resolve Studio 21.1.1 through its scripting API: values set on clips, OTIO files exported, hand-written ones imported, and stills and a Deliver render read back. Resolve's *picture* is the source fitted into the frame (or sized as its `scale` says, below); its size in the frame is *pw* × *ph*, of a frame *W* × *H*.

## OpenTimelineIO

Resolve writes a clip's Transform as an effect of type 2, Cropping type 3, Dynamic Zoom type 59 and Retime and Scaling type 22, each a `Resolve Effect` listing only the parameters off their defaults. Each value is a fraction, so converting needs the frame's and the source's shapes, not their sizes; Resolve's file holds no frame size (its timeline metadata is only `Resolve OTIO Meta Version`).

| Parameter | Resolve | vidcut |
|---|---|---|
| `transformationZoomX`, `transformationZoomY`, `transformationZoomLink` | zoom across and down in the picture's own axes, 0 to 100 | `zoom` |
| `transformationPan`, `transformationTilt` | the middle's move in picture widths right and picture heights up, −4 to 4 | `position` = [0.5 + Pan · *pw*/*W*, 0.5 − Tilt · *ph*/*H*] |
| `transformationRotationAngle` | degrees counterclockwise, ±100000 | `rotation` |
| `transformationAnchorPoint` | `[x, y]` from the middle in fractions of the picture, y up; zoom and turn go about it, the flips about the middle | folds into `position`: the middle lands at A − R·S·A + Pan/Tilt's move (A the anchor, S the zoom, R the turn) |
| `transformationFlipX`, `transformationFlipY` | mirror about the middle | `flip_horizontal`, `flip_vertical` |
| `cropLeft`, `cropRight`, `cropTop`, `cropBottom` | fractions of the picture, 0 to 1 | `crop` as is |
| Retime and Scaling `scale` | 3 is fill (the scripting API's numbers, only 3 checked in a file: 0 the project's, 1 crop, 2 fit, 3 fill, 4 stretch); the project's is fit in a new project | vidcut fits; crop (source pixel = frame pixel), fill and stretch are a zoom |
| Pitch, Yaw | not measured | not played |

The order is vidcut's: fit, crop, flip, zoom, turn, move (D75). Resolve applies a Transform and Cropping in vidcut's form of a Resolve effect (the parameters `otio::titles` writes) when it imports them.

- **Open:** write a placed clip's picture as these effects next to its `vidcut` metadata, with `scale` 2 so Resolve fits it whatever its project says; read them back as a `Picture` (the anchor folded into `position`, a rotation past ±3600 reduced by whole turns, `scale` 1, 3 and 4 as zooms); carry Pitch and Yaw unplayed and name them in the import's notes; name in an export's notes a position past Resolve's ±4 pictures.

## Dynamic Zoom

- **The Cut page** (p. 768): off until its controls are touched. A green box is the start and a red box the end; each is moved and resized in the viewer, and a path shows the motion. Swap reverses them. The ease is Linear, Ease In, Ease Out or Ease In and Out. Shift keeps a drag to one axis.
- **Resolve:** a box per end: `dynamicZoomCenter`, its middle from the picture's in fractions of the picture, y up, and `dynamicZoomScale`, its size as a fraction of the picture, the same both ways. The picture is drawn at 1/size about the box's middle, after the flips and before Transform's zoom, turn and move; the crop goes with it and the picture's outline grows with the zoom. Turned on, it starts at size 0.8 and ends at 1, both in the middle: a pull out from 125%. Size and middle go in a straight line from the start on the clip's first frame, frame *k* of *n* at *k*/*n*, so the end comes a frame after the last.
- **Resolve's ease:** `dynamicZoomEase` (0 Linear, 1 Ease In, 2 Ease Out, 3 Ease In and Out) is kept and written, but all four play the same straight line, set through scripting or read from a file, in stills and in a Deliver render. Its Inspector menu wasn't tried.
- **Resolve's OTIO:** both parameters are keyed at −*g* and 1000 − *g*, *g* the timeline's `global_start_time`; Resolve reads the earlier key as the start and the later as the end whatever their times (0 and 1000 or −86400 and −85400, with *g* 86400 or 0).
- **vidcut:** the Transform handles' shape for each box, on Alt + T (free now), and Dynamic Zoom in the clip's right-click menu with Swap and Reset. A box per end in fractions of the picture, played as Resolve does; the renderer already places each frame on its own (`place::Placing`).
- **Open:** whether vidcut's eases follow Resolve's render (all a straight line) or curves of their own, a deviation.
