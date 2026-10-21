# Clip transform and crop

**When:** next. First of the finishing work in [the roadmap](../roadmap.md#what-comes-next). A placed, cropped, moving picture is what a simple video still leaves for another editor.

A footage clip, a still, an image sequence, or a compound clip gets a place on the timeline frame: position, one zoom or a zoom per axis, rotation, an anchor, a crop on each side with one soft edge, opacity, and a flip on either axis. Any of those but the flips can change along the clip (a picture in picture, a punch-in, a move across a still). The viewer and the export draw the same picture, the way a title already does ([playback](../design/playback.md#the-viewer-thread), [export](../design/export.md#titles)). Every control is a request ([requests](../design/requests.md#one-vocabulary)).

Resolve's Cut page sets the same things in the Inspector's Transform, Cropping and Composite, and on the viewer (pp. 763–764 for a title's on-screen controls; the Video Inspector is the Edit page, p. 764). vidcut has no Inspector ([X43](../deviations.md#x43-a-subtitles-text-is-typed-in-its-own-window), [X52](../deviations.md#x52-titles-come-from-a-titles-tab-or-add-title-and-are-edited-in-a-title-window-without-an-inspector)). The shape Resolve writes is below; it has not been measured in Resolve Studio the way a title's generator was.

## Model

A clip carries `picture`, omitted when every value is the identity, so a project file and an OTIO file stay as they are today ([the `.vidcut` file](../design/model.md#the-vidcut-file)).

| Field | Identity | Meaning |
|---|---|---|
| `position` | `[0, 0]` | fractions of the timeline frame, from its center; +x right, +y up |
| `zoom` | `1` | one scale of the fitted picture; `1` is the fit used today |
| `zoom_x`, `zoom_y` | `1` | extra scale per axis, after `zoom` |
| `rotation` | `0` | degrees, counterclockwise |
| `anchor` | `[0, 0]` | fractions of the fitted picture, from its center; the point that `position` places and that rotation and scale turn about |
| `crop` | all `0` | `left`, `right`, `top`, `bottom`, each a fraction of the source picture |
| `softness` | `0` | one soft edge, a fraction of the shorter side of the cropped picture |
| `opacity` | `1` | `0`–`1` |
| `flip_x`, `flip_y` | false | constants; they are not keyed |

Limits: `position` and `anchor` each component from −4 to 4 (Resolve's scripting API allows pan, tilt and anchor out to four times the frame). `zoom`, `zoom_x` and `zoom_y` from 0 to 100. `rotation` from −3600 to 3600, the range a title already allows. Each crop from 0 to 1, with `left + right < 1` and `top + bottom < 1`. `softness` and `opacity` from 0 to 1. A value outside that is an error, not a silent clip.

The fitted picture is today's fit (`normalize::fit`): the source aspect fitted inside the frame. Cropping runs on the source before that fit. One sample from the cropped source into the final quad, so a punch-in keeps the source's detail. The order, which a test locks:

1. Crop the source window.
2. Flip.
3. Scale about the anchor.
4. Rotate counterclockwise about the anchor.
5. Move the anchor by `position` times the frame size, y up.

Worked example, written into the test: a 1920×1080 frame, a source of the same aspect, `zoom` 0.5 and `position` `[0.25, 0.25]`. The quad is 960×540 and its center is pixel (1440, 270), counting pixels from the top left.

Identity draws what it draws today, including the opaque black bars of a different aspect. Any other picture is composited: outside the quad is clear, so a lower track shows through, and under nothing the frame is black, as under a title. A dissolve mixes after each side is placed; opacity multiplies that side's weight. HDR mixes stay the mismatch [HDR dissolves](hdr-dissolve-preview.md) already records.

Titles keep their own `position`, `zoom`, `rotation` and `opacity` ([X54](../deviations.md#x54-a-titles-place-is-a-point-in-fractions-of-the-picture)). Those place the text. `picture.set` on a title is an error that names `titles.set`. A generator that plays as a gap can store a picture and draws nothing. A compound is one picture, transformed after its own tracks are composed, when the compound or a clip inside it has a picture; otherwise it stays expanded as now (`render::expanded`).

## Keyframes

A field is one constant, or a list of keys. A key is a source frame, a value, and the ease of the segment that follows it. The source frame counts from `Effects.from`, the same origin a carried effect already uses, so a trim, a slip or a split keeps the move on the pictures it belongs to ([OpenTimelineIO](../design/model.md#opentimelineio)). The request's `at` is a timeline time, converted with `Clip::source_at`. Whole frames only; a key that arrives between frames rounds to the nearest and the answer warns.

Before the first key the value holds the first; after the last it holds the last. The last key's ease is unused. `position` is two curves, x and y, because that is how the file stores pan and tilt; one `position` in a request writes both and they share its ease.

Ease, on the key that starts the segment. `u` runs from 0 to 1 along the segment:

| `ease` | Curve |
|---|---|
| `linear` | `u` |
| `hold` | the value stays until the next key |
| `ease_in` | `u²` |
| `ease_out` | `1 − (1 − u)²` |
| `ease` | `u² (3 − 2u)` |

`flip_x` and `flip_y` have no keys.

## Requests

`picture.get`, `picture.set`, `picture.clear`. The CLI is `vidcut picture get`, `vidcut picture set`, `vidcut picture clear` ([commands are requests](../cli.md#commands-are-requests)). MCP tools are `picture_get`, `picture_set`, `picture_clear`.

`picture.set` names `clips`. Without `at` and without `keys`, each field it carries becomes that constant and that field's keys are cleared. With `at`, it sets one key on each field it carries; the first key on a field that was a constant also plants a key at the clip's first source frame with the old value, unless `at` is that frame. `keys` replaces those fields' curves and cannot be combined with `at`. `remove` with `at` deletes the keys at that frame. `picture.clear` removes `picture`. One request is one undo step; a Ken Burns sent as one `apply` batch is one step.

```bash
vidcut picture set c3 --zoom 0.33 --position 0.3,0.25
vidcut picture set c3 --at 0 --zoom 1 --ease ease
vidcut picture set c3 --at 5s --zoom 1.4 --position 0,-0.05
vidcut picture get c3 --json
vidcut picture clear c3
```

```json
{"method":"picture.set","params":{"clips":"c3","zoom":0.33,"position":[0.3,0.25]}}
```

```json
{"method":"picture.set","params":{"clips":"c3","keys":{"zoom":[{"at":"0","value":1,"ease":"ease"},{"at":"5s","value":1.4}],"position":[{"at":"0","value":[0,0],"ease":"ease"},{"at":"5s","value":[0,-0.05]}]}}}
```

`edit.copy` and `edit.paste` already carry a clip's JSON, so they carry `picture`.

## Window

No Inspector. A Picture window, built as the Title window is (`gui/dialog.rs`): labeled rows, a Place / Crop switch, and Advanced for the per-axis zoom, the anchor, the crop, the softness and the flips. The viewer follows the fields. Done sends one `picture.set`. Cancel leaves the clip.

The timeline viewer, for the selected clip, draws the quad and handles, the same idea as a title's handles (`onscreen.rs`). A drag previews; mouse-up sends the request. Dragging a field that already has keys writes a key at the playhead; dragging a constant changes the constant. Each row has a key control. The quad snaps to the center and the thirds, as a title does. The clip under the pointer is the topmost picture there. Labels stay a few words: Zoom, Position, Rotation, Opacity, Crop, Softness, Anchor, Flip Horizontal, Flip Vertical.

## Drawing

One pure placement in vidcut-core. The viewer shader, the CPU resample and a CUDA kernel all call it, so the three agree on the worked example above.

The viewer adds a pass beside `fs_title` in `picture.wgsl`. It samples the cropped source into the quad. A CUDA kernel, next to `cuda::burn` and `cuda::mix`, runs when a frame needs a placement and that GPU is in use. On the other GPUs the frames come down only while a placed picture shows, the same gap [GPU burn-in](gpu-burn-in.md) and [GPU dissolves](gpu-dissolves.md) already have. With `gpu.acceleration` on, a GPU that places correctly does the placement; the answer says which path ran. An identity frame stays on today's fast path and is not resampled again.

A move across a still changes the placement, not the decoded frame. Proxies stay a preview of the source (spec D41); the placement is not baked into them. Export draws the placement into the output frame before a title on that track, then subtitles, in the same order the plan already stacks (`VideoPiece::Titled`).

## OpenTimelineIO

Resolve writes a clip effect named `Resolve Effect` with `Effect Name` Transform (type 2), Cropping (type 3), Composite (type 1) or Dynamic Zoom (type 59). Empty `Parameters` means the defaults. The type numbers and the Dynamic Zoom keys are from a published export (OpenTimelineIO discussion [921](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/921)). Blender's importer reads the parameter ids and the scale factors below; that importer is unreleased, and vidcut has not checked them against Resolve Studio 21.1.1. The writer stays behind a test fixture until one export from that Resolve is saved beside it.

| vidcut | Resolve parameter | Reading of the units |
|---|---|---|
| `zoom` × `zoom_x`, `zoom` × `zoom_y` | `transformationZoomX`, `transformationZoomY` | 1 is the fitted picture; a missing Y is X |
| `position` x, y | `transformationPan`, `transformationTilt` | fractions of the frame; +y up |
| `rotation` | `transformationRotationAngle` | degrees, counterclockwise |
| `anchor` | `transformationAnchorPoint` | a point; the unit is open, below |
| `flip_x`, `flip_y` | `transformationFlipX`, `transformationFlipY` | booleans |
| `crop` | `cropLeft`, `cropRight`, `cropTop`, `cropBottom` | fractions of the source |
| `opacity` | Composite `opacity` | percent in the file, 0–1 in vidcut |

On import, an effect in use becomes `picture` (`effects::in_use` already drops a disabled effect and a parameter left at its default). Dynamic Zoom keys are the strings `"0"` and `"1000"`, the start and the end of the clip, each `{Value, Variant Type}`, plus a center. They become two zoom keys on the first and last source frame. A Transform's `Key Frames` are read as source frame numbers, which is what carried effects already do; a key that is not a frame number warns and is kept in `Kept`. Pitch, yaw, the composite mode, crop retain and any other parameter stay on the effect in `Kept` and are written back. A composite mode other than normal is carried and not played, and the answer names it; opacity still applies.

On export, vidcut updates a Transform, Cropping or Composite effect it already holds, and adds one only for a value that is not the identity. A parameter at its default with no keys is omitted, so a default effect still has empty `Parameters`. Speed stays the first effect. The exact model (the ease, the softness, the anchor as vidcut stores it) also goes in the clip's `vidcut` metadata, the way a title already does. Unknown parameters already in `Kept` are preserved. A split follows the effect's source frames. A join refuses when the two pictures' curves disagree (`effects::join`).

`softness` is metadata only until a Resolve export shows the parameter id. The OTIO export warns on a clip whose softness is not 0, because Resolve will not see that edge. Crop retain stays as read and vidcut always fills the frame from the cropped window.

Dynamic Zoom's ease is an enum. The scripting API's first value is linear; the numeric ids of the other eases are unmeasured, so a Dynamic Zoom vidcut writes carries linear and the vidcut ease in metadata.

Whether Resolve applies a Transform that vidcut synthesized, rather than one Resolve itself wrote, is open. Titles needed a measured generator before Resolve showed them.

## Tests

Unit tests, one behavior each, the expected value written out: the worked example's quad; each ease at `u = 0.5`; a crop that fails `left + right < 1`; a key that stays on its source frame across a split; identity omitted from the project JSON and from the plan; a fixture of the published Dynamic Zoom and a synthesized Transform round-tripping through `effects`. Pixels of the viewer against the export are a hand check (`frame.render`).

## Open questions

- One Resolve Studio export of a Transform, a Cropping and a Dynamic Zoom, measured as the titles were, before the writer is locked. The anchor's unit in particular: Blender's reading is not a plain fraction of the picture.
- The parameter id and the falloff of crop softness.
- The Dynamic Zoom ease enum, past linear.
- Whether a Transform vidcut adds, with empty defaults omitted, is applied by Resolve's importer.

## Deviations

Recorded when this is built, not numbered here. vidcut has no Inspector. Values are fractions, while Resolve's scripting API speaks in pixels for crop and in a zoom of 0–100 with the UI showing 1. A title is not this transform ([X53](../deviations.md#x53-titles-are-text-the-three-lower-thirds-and-scroll-styled-by-markup)). The eases are the five polynomials above. Blend modes, pitch and yaw are carried and not played. Softness is a linear ramp over the fraction above. An identity clip keeps opaque bars; a placed clip composites.
