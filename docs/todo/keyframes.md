# Keyframes

**When:** not decided.

A keyframe is a value of a clip at one frame. Between two keyframes, the value goes from one to the other. In this file, a keyframe is never a video keyframe of a codec.

- **Now:**
  - A clip has volume keyframes on its volume line ([spec §9](../cut-page-spec.md#9-audio-pp-642-649650-743744-771-773-778779), D81–D85; deviations [X73](../deviations.md#x73-the-volume-line-takes-the-keyframe-controls-of-the-edit-page), [X74](../deviations.md#x74-eases-are-bezier-handles-that-drag-on-the-volume-line)).
  - The `Keyframe` type holds a source frame, a value, an In handle and an Out handle. `ClipAudio::gain_keys` holds the volume keyframes.
  - Stage 2 is built. Transform and Crop values have keyframes on `Picture` ([§8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768), D75).
  - Stage 3 is built. The Keyframes tab shows the keyed values of the first selected clip.
  - Each value has a track and a curve. A drag moves a keyframe, a group or a handle.
  - Position is one keyframe for X and Y. Zoom is one keyframe for X and Y. Rotation is its own value. Each crop side is its own value.
  - The flips have no keyframes. Dynamic Zoom stays the two end boxes and has no Keyframe button.
  - vidcut keeps the keyframes of other clip effects, but it does not play them ([X9](../deviations.md#x9-opentimelineio-interchange)).
  - The exception is Dynamic Zoom. vidcut reads its two keyframes as the start box and the end box.
- **Why:** the user changes a part of a clip, not only the whole clip. An example is a push in on one moment.
- **To do:** keyframes of speed, opacity, Pitch, Yaw, the anchor point and titles.

## What Resolve has

**The Cut page** (Ch. 33–37):

- The Cut page has a Keyframe Editor tab on the left (p. 636).
- A Timeline control at the upper left opens the Keyframe Editor (p. 649).
- Timeline Options has Display Speed Keyframes (p. 645).
- Speed % and Frames Per Second of Speed Change can have keyframes (p. 737).
- The Effects panel of the Inspector has a Keyframe button with Previous and Next buttons on each group of an applied effect (p. 747).
  - The button adds or removes a keyframe at the playhead, on each parameter of the group.
- The text of the Video panel (pp. 734–737) names no keyframe buttons for Transform, Cropping or Dynamic Zoom.

**The keyframing chapter** (Ch. 60, pp. 1249–1269) is for the Edit page and the Cut page (p. 1249):

- Most parameters of the Inspector can have keyframes, for example Zoom, Opacity and the crop (p. 1250).
- Each such parameter has a Keyframe button between Previous and Next arrows. The button is orange on a keyframe (p. 1250).
- After the first keyframe of a parameter, each change at another frame adds a keyframe. The onscreen controls of the viewer also do this (p. 1251).
- A right-click on the button sets Linear, Ease In, Ease Out or Ease In and Out (p. 1251).
- The reset button of a parameter removes its keyframes. The reset button of a group removes the keyframes of the group (p. 1251).
- A clip with keyframes shows a keyframe icon on its name bar (p. 1250).
- Keyframes of Position X and Y make a motion path in the viewer, with Bezier points (pp. 1252–1254).
- Mark > Add Keyframe adds a keyframe, also during playback (pp. 1254–1255).
  - Resolve selects the parameter in this order: the retime controls, the open curve of the Keyframe Editor, the Inspector control changed last, the audio gain.
- The Keyframes tab opens the Keyframe and Curve Editor on the Cut page and on the Edit page (p. 1255).
- **The Keyframe Editor** (pp. 1255–1259):
  - It has a track for each parameter, in a group track for each group, for example Transform and Cropping.
  - Zoom is one track, and Position XY is one track (p. 1256).
  - A move on a group track moves the keyframes at that time of all its parameters (p. 1256).
  - A keyframe can be outside the clip after a trim. An arrow then marks it (p. 1258).
  - A keyframe tray shows the keyframes of the selected clip in the timeline, on the Cut page and the Edit page (p. 1259).
  - It opens in a window of its own only on the Edit page (p. 1260).
- The Curve Editor adds Bezier handles, Step In, Step Out, Loop, Ping Pong, Reverse and a Shape Box (pp. 1261–1267).
- Keyframes can be edited on several selected clips at once (p. 1268).

**The Edit page** (p. 1217):

- Change Clip Speed has Maintain Timing and Stretch to fit.
  - Maintain Timing keeps the keyframes where they are.
  - Stretch scales the Composite, Transform and Cropping keyframes with the clip.

## Measured in Resolve Studio 21.1.1

These facts come from Resolve Studio 21.1.1 on Windows. The test timeline was 1920×1080 at 30 fps, and started at 01:00:00:00.

**Scripting:**

- Resolve's scripting API has no call that adds, reads or removes the keyframes of a clip.
- A scripted value on a parameter with keyframes changes nothing that Resolve shows or plays.
- A script can change the speed, with or without stretching the keyframes, and can export and import OTIO.

**The file** (OTIO, as Resolve exports it):

- A keyframe time is a whole frame. It counts from the first source frame of the clip, the start of its `source_range`.
- Transform: Zoom X and Zoom Y have keyframes of their own. Pan and Tilt are fractions of the width and the height of the timeline. Rotation is in degrees.
  - A keyframe on Position keys X and Y. A later change of X alone adds a keyframe to X only.
- Cropping: Crop Left is a fraction of the width, as its plain value is.
- With keyframes, `Parameter Value` is not the value at the playhead, and not the value of the last keyframe. An import reads the keyframes.
- Dynamic Zoom: Resolve writes its keyframes at the source frames 0 and 1000, on a clip with Dynamic Zoom on or off.
  - In the file, these times are −s and 1000 − s. s is the first source frame of the clip.
- Resolve reads a written file as written. Times, values and eases come back in its Inspector and in its next export.

**Speed:** a speed change that stretches the keyframes keeps each keyframe on its source frame. In the file, a stretched keyframe is at its frame of the clip.

**Curves** (from renders):

- Zoom changes in value. From 0.1 to 0.4 over 30 frames, it is 0.25 at the middle.
- Position, rotation and crop change in value too.
- The eases of the picture are not measured.

**The Cut page:**

- The Inspector has a Keyframe button between Previous and Next arrows on each value and each group. Dynamic Zoom has no button.
  - The groups with buttons: Transform, Cropping, Composite, Speed Change, Lens Correction, Retime and Scaling, Volume and Pan.
- The Keyframes tab shows the Keyframe Editor: a track for each value with its keyframes, Volume and Pan too.
- The Mark menu has Add Keyframe (Ctrl+[), Delete Keyframe (Alt+[), Delete All Keyframes, and Move Selected Keyframes Left, Right, Up and Down.
- The speed tool of the viewer has its own Keyframe button.
- A right-click on a keyframe in the Keyframe Editor opens no menu, on the Cut page and on the Edit page.

**Better than Resolve** (proposals for the stages):

- The eases of the picture use the Bezier curve of the volume ([X74](../deviations.md#x74-eases-are-bezier-handles-that-drag-on-the-volume-line)). Thus Ease In and Out plays an S-curve.
- The ease menu is on each keyframe: on the Keyframe button and in the Keyframe Editor.

## Scope

**Stage 2 is built.** Transform, Crop and the volume line use `Keyframe`. The spec is [§8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768) and D75. The buttons are in [X70](../deviations.md#x70-the-inspector-is-a-tab-beside-the-media-pool-in-frame-pixels).

**Stage 3 is built.** The Keyframes tab sits beside the Inspector. The Timeline menu opens it. The spec is [§2](../cut-page-spec.md#2-layout-pp-636637) and [X87](../deviations.md#x87-the-keyframes-tab-has-an-ease-menu).

- One track for each keyed value, in a group for its section.
- A drag on a keyframe moves it in time. A drag on a curve point also changes the value.
- A drag on a group track moves that group's keyframes at that time.
- Volume is a track in the Audio group.
- Each value has a curve. A drag on an In or Out handle changes that handle.
- One click selects one keyframe. Shift or Ctrl adds to the selection.
- A right-click sets that keyframe's ease, with the volume Bezier ([X74](../deviations.md#x74-eases-are-bezier-handles-that-drag-on-the-volume-line)).
- `keyframes.set` and `keyframes.move` do these edits.
- Not in this stage: Step, Loop, Ping Pong, Reverse, the Shape Box, the keyframe tray and motion paths.

**After stage 3:** keyframes of speed, opacity, Pitch, Yaw, the anchor point and titles ([§8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).

## Model

The picture uses the `Keyframe` of the volume ([D81](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)): the same source frames, handles, curve, eases and edits.

- **Where:** the keyframes of each value are in `Picture`, beside the plain values.
- **Checks:**
  - `ops::picture::checked` checks the values of each keyframe as it checks plain values.
  - `Picture::is_usual` is false while a value has keyframes.
- **Compound clips:** the keyframes of a compound clip count its frames.
  - `edit.decompose` moves them to the source frames of each clip inside.
- **Another rate:** an import scales the keyframe times to the project's rate (`at_project_rate`), as for the volume.

## Playback and export

**Now:**

- `Placed` holds the clip's picture, the source frame of the piece and its pace.
- `Placed::after` moves that source frame with the piece, as it moves a Dynamic Zoom.
- `Picture::sampled` reads each keyed value at that source frame.
- The curve is the volume Bezier curve.
- `place::detail` uses the largest zoom key in the piece.
- The viewer and the export take that placing. The GPU paths take it too.
- Dynamic Zoom still uses its `Run` and `Placed::zoomed`.
- The placed pictures are in [export](../design/export.md#placed-pictures).

## Interchange

**What vidcut reads and writes now for the picture** (`otio::effects`, `otio::picture`):

- A clip effect is an OTIO `Effect.1` with the `effect_name` `Resolve Effect`.
- Its data is in `metadata.Resolve_OTIO`: `Effect Name`, `Name`, `Type`, `Enabled`, `Display Type` and `Parameters`.
- A parameter has `Parameter ID`, `Parameter Value`, `Default Parameter Value`, `Variant Type`, and can have `minValue`, `maxValue` and `Key Frames`.
- `Key Frames` is an object. Each name is a keyframe time, a frame count as a string. Each value has `Value` and `Variant Type`, and can have `InBez` and `OutBez`.
- `effects::read` keeps an effect that is on when a parameter or a keyframe `Value` is off its default (`in_use`). It keeps a Dynamic Zoom that is on.
- `carry` sets the `from` field of `Effects` to the first source frame of the clip, or of its sound. Thus the code takes time 0 as the first source frame of the item.
- `Effects::written` moves the times by `from` less the first source frame of the written item.
- `otio::picture` reads Transform, Cropping and Retime and Scaling as a `Picture` only when no parameter has keyframes.
  - The center and scale of Dynamic Zoom need exactly two keyframes (`ends`). The earlier keyframe is the start box.
- vidcut writes the two keyframes of Dynamic Zoom at −g and 1000 − g. g is the timeline's `global_start_time` in frames.
  - Resolve writes them at −s and 1000 − s. s is the first source frame of the clip ([Measured](#measured-in-resolve-studio-2111)).
  - Resolve reads the earlier key as the start, whatever the times ([gotchas](../gotchas.md#resolves-otio-files)). Thus both forms play the same.
- vidcut writes its other picture parameters with an empty `Key Frames`.

**The one test file:** `$VIDCUT_TEST_DATA/fixtures/resolve-titles/titles.otio` has Resolve's metadata, 24 fps and g = 86400.

- Its keyframes are only on the Dynamic Zoom of its six title clips. That Dynamic Zoom is off.
- Each of these clips has keyframes at `0` and `1000`, also the clips later in the timeline. All have a source start of 0.
- One of its parameters, from the first clip:

```json
{
  "Default Parameter Value": 1.0,
  "Key Frames": {
    "0": { "Value": 0.8, "Variant Type": "Double" },
    "1000": { "Value": 1.0, "Variant Type": "Double" }
  },
  "Parameter ID": "dynamicZoomScale",
  "Parameter Value": 1.0,
  "Variant Type": "Double",
  "maxValue": 100.0,
  "minValue": 0.01
}
```

**What vidcut would write:**

- The `vidcut` metadata of the clip holds the keyframes exactly, as `gain_keys` holds the volume keyframes.
- Resolve's effects get each keyframe in the `Key Frames` of its parameter, in Resolve's units through `Shape`, as `otio::picture::effects` converts plain values.
- An import plays the keyframes of Transform and Cropping when they fit the model.
- Some keyframes stay in the kept effects, unplayed, with the warning of the import:
  - Step
  - an anchor point with keyed zoom or rotation

**Unknown:**

- The curves of the eases of the picture.
- The key times of Dynamic Zoom for media with a timecode. The test media has none.
- The key times of a clip that plays backward, and of a clip at another speed in media with a timecode.
  - vidcut writes the volume keyframes of a clip that plays backward as for speed 1.

## Scripting

Rule 3: each GUI action has a request.

- `keyframes.list`, `keyframes.add`, `keyframes.set`, `keyframes.move` and `keyframes.remove` take the values of the picture in `param`.
- `picture.set` takes an `at`. On a value with keyframes, it sets a keyframe at `at`, or at the playhead, as `clip.set` does for the volume ([D82](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- `timeline.get` names the values of a clip that have keyframes, as it names `volume` now.

## Open questions

- **Compound clips:** what does `edit.decompose` do with the keyframes of a compound clip for a clip inside that is frozen or plays backward?
- **Dynamic Zoom:** can its boxes have times other than the ends of the clip?
- **Keyframes alone:** copy and paste of selected keyframes (p. 1113). vidcut has none yet.
- **Later stages:** do keyframes of speed (p. 737), opacity, the anchor point, or titles come in after stage 3?
