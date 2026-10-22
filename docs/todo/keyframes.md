# Keyframes

**When:** not decided; after the volume line.

A keyframe is a value of a clip at one frame. Between two keyframes, the value goes from one to the other. In this file, a keyframe is never a video keyframe of a codec.

- **Now:**
  - The spec has keyframes out of scope ([§1](../cut-page-spec.md#1-scope), [§8.15](../cut-page-spec.md#815-transform-and-crop-pp-766768)).
  - vidcut keeps the keyframes of Resolve's clip effects, but it does not play them ([X9](../deviations.md#x9-opentimelineio-interchange)).
  - The exception is Dynamic Zoom. vidcut reads its two keyframes as the start box and the end box.
- **Why:** the user changes a part of a clip, not only the whole clip. Examples: a lower volume under one sentence, or a push in on one moment.
- **To do:** change the scope in three stages, in this order:
  1. Volume keyframes on the volume line.
  2. Transform, Crop and Dynamic Zoom keyframes, with buttons in the Inspector.
  3. A Keyframe Editor.

The volume line is not in vidcut yet. It is a line over the waveform of a clip that sets the gain of the clip.

## What Resolve has

**The Cut page** (Ch. 33–37):

- The Cut page has a Keyframe Editor tab on the left (p. 636).
- A Timeline control at the upper left opens the Keyframe Editor (p. 649).
- Timeline Options has Display Speed Keyframes (p. 645).
- The Volume of the Inspector's Audio panel goes with the volume overlay of each audio clip (p. 743).
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

**The Edit page** (pp. 1109–1113, 1217):

- Each audio clip has a Volume overlay. A drag up or down sets the level of the clip (p. 1109).
- Alt+click (Option on a Mac) on the overlay adds a keyframe (pp. 1109, 1112).
- Two keyframes at least make a change of level (p. 1112).
- Keyframes of the Inspector's Volume also show on the overlay (p. 1111).
- A drag on the line between two keyframes raises or lowers that part (p. 1112).
- A drag on a keyframe changes its level and its time. Shift keeps one direction (p. 1113).
- A right-click on keyframes sets Linear, Ease In, Ease Out or Ease In and Out (p. 1113).
- Cut, copy and paste work on keyframes. Backspace deletes them (p. 1113).
- Audio keyframes can be between frames (p. 1113).
- Change Clip Speed has Maintain Timing and Stretch to fit (p. 1217).
  - Maintain Timing keeps the keyframes where they are.
  - Stretch scales the Composite, Transform and Cropping keyframes with the clip.

## Scope

**Stage 1: volume keyframes on the volume line.**

- Value: the gain of the sound of a clip.
- GUI: the volume line takes the controls of the Edit page's Volume overlay above.
  - Alt+click adds a keyframe. A drag moves a keyframe, or the part between two keyframes.
  - A right-click sets the ease. Delete removes the selected keyframes.
- Spec:
  - §1 takes volume keyframes in. [§9](../cut-page-spec.md#9-audio-pp-642-649650-743744-771-773-778779) gets them, with the pages above.
  - If the Cut page's timeline has no volume overlay (run R7), a deviation records the behavior of the Edit page.
  - [X9](../deviations.md#x9-opentimelineio-interchange) says that keyed volume from a file plays.

**Stage 2: Transform, Crop and Dynamic Zoom keyframes, with Inspector buttons.**

- Values of a `Picture`: `position`, `zoom`, `rotation` and each side of its `crop`.
  - The position has one keyframe for X and Y, and the zoom one for X and Y, as the tracks of p. 1256.
  - The flips have no keyframes.
- Dynamic Zoom: its start box and end box stay at the two ends of the clip ([D75](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - Its values get keyframe buttons only if Resolve's Inspector gives them buttons (run R7).
- GUI, in the Inspector tab ([X70](../deviations.md#x70-the-inspector-is-a-tab-beside-the-media-pool-in-frame-pixels)):
  - A Keyframe button with Previous and Next arrows beside each value. A right-click on it sets the ease.
  - A value with keyframes gets a keyframe at the playhead when it changes. This applies to the Inspector and to the viewer handles.
  - The Reset of a value removes its keyframes. The Reset of a section removes the keyframes of the section.
  - A clip with keyframes shows a keyframe mark in the timeline.
- Spec:
  - §8.15 takes keyframes off its Out list. D75 gets the keyed values.
  - X70 gets the buttons.

**Stage 3: a Keyframe Editor.**

- A Keyframe Editor tab beside the Inspector tab, as the tab of p. 636. The Timeline control of p. 649 opens it.
- A track for each value, in a group track for each section. A drag moves keyframes. A drag on a group track moves the keyframes of the group.
- Spec: [§2](../cut-page-spec.md#2-layout-pp-636637) takes the tab in.
- Not in stage 3: the Curve Editor, Bezier handles, Step, Loop, Ping Pong, Reverse, the Shape Box, the keyframe tray and motion paths.

**Still out of scope:** keyframes of speed, opacity, Pitch, Yaw, the anchor point and titles ([§8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).

## Model

**A keyframe** has a time, a value and an ease.

- **Time:** a source frame of the clip, counted as the `source_in` of a `Clip` is.
  - Thus a keyframe stays on the content of its frame.
  - The kept effects of a file do the same. Their times count from the `from` field of `Effects`, which `carry` sets in `place_imported`.
- **Ease:** an `Ease`, with the curves of `Ease::of` ([X67](../deviations.md#x67-the-eases-of-dynamic-zoom-are-curves)).
  - The ease of a keyframe shapes the change to the next keyframe. The default is Linear.
  - Resolve's meaning of Ease In and Ease Out on one keyframe is not known (run R5).
- Before the first keyframe, the value is that of the first keyframe. After the last keyframe, it is that of the last.
- A value with keyframes takes its value from them only. When its last keyframe goes, the value of that keyframe stays as the plain value.
- **Where:**
  - Stage 1: the gain keyframes are in `ClipAudio`, beside `gain_db`.
  - Stage 2: the keyframes of each value are in `Picture`, beside the plain values.
- **Checks:**
  - `ops::picture::checked` checks the values of each keyframe as it checks plain values.
  - A gain keyframe takes −60 to 24 dB, as `ops::set_clips` does.
  - `Picture::is_usual` is false while a value has keyframes.

**Edits:**

- **Split** (`split_at` in `ops`): both parts keep all the keyframes.
  - A keyframe outside a part stays, as Resolve keeps a keyframe outside a trimmed clip (p. 1258).
  - Thus `edit.join` gives back the clip from before the split.
- **Trim:** the keyframes stay on their source frames. The frames that stay show the same values.
- **Slip:** the keyframes go with the content. Resolve's behavior is not known (run R3).
- **Move and ripple:** the keyframes go with the clip.
- **Speed** ([D48](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)):
  - The keyframes scale with the speed, as Stretch to fit does (p. 1217).
  - A freeze shows one source frame, so it shows one value.
  - A clip that plays backward plays its keyframes backward.
  - What the Cut page does is not known (run R4).
- **Compound clips:** the keyframes of a compound clip count its frames.
  - `edit.decompose` moves them to the source frames of each clip inside.
- **Copy and paste:** `edit.copy` and `edit.paste` carry the keyframes with the clip.
- **Undo** ([§8.10](../cut-page-spec.md#810-undo-pp-651652)):
  - Each request that changes keyframes is one `Change`, so one undo step.
  - A drag is one request on release, as `picture.set` is from the viewer handles.
- **Another rate:** an import scales the keyframe times to the project's rate (`at_project_rate`), as `Effects::retime` does now.

## Playback and export

**Now, the picture:**

- `render::placed_of` makes one `Placed` for each piece, with the `Picture` of its clip (`Placed::of`).
- Only Dynamic Zoom changes from frame to frame.
  - `Placed` has a `Run`, and `Placed::zoomed` gives the box of the frame (`DynamicZoom::at`).
  - `VideoPiece::part` and `Placed::after` move the `Run` with the piece.
- An export makes a `place::Placing` for each frame in `VideoFrames`. The viewer moves the `Run` to each frame (`Plan::placed_at`).
- `layer_size` sizes a layer by `place::detail`, with the closer box of a Dynamic Zoom.
- `Picture::is_usual` decides if a clip that fills the frame takes the place of the tracks below (`render::flatten_lanes`).
- For NVENC, `cuda::burn::Overlay::of` takes the `Placing` of the frame.

**Now, the sound:**

- `AudioPiece::Source` has one `gain_db`.
- `AudioMix` multiplies each sample by that gain, the cross fade (`Fade::gain`), the de-click fades and the ducking (`Dip::gain`).
- The player and the export use the same `AudioMix` ([design](../design/export.md#sound)).
- The gain also goes to these users:
  - `DuckSource`, for ducking by level (`analysis::loud_frames`)
  - `analysis::silent_parts`, for Ripple Delete Silence by level
  - `analysis::Heard`, for the quiet edges of `edit.ripple_delete_words`
  - the meters of the Mixer in the GUI

**To do:**

- **Sound (stage 1):**
  - `AudioPiece::Source` gets the gain keyframes of its clip and its source position.
  - `AudioMix` multiplies each sample by the keyed gain at its source position, as it multiplies by `Fade::gain`.
  - Thus the player and the export play the same keyframes.
  - The other users of the gain take the keyed gain of each frame.
- **Picture (stage 2):**
  - `Placed` gets the keyed values, the source frame of the first frame of its piece, and its `Pace`.
  - `Placed::after` moves that source frame with the piece, as it moves the `Run`.
  - `place::Placing` takes each value at the source frame of the frame.
  - `layer_size` takes the largest zoom of the keyframes in the piece, as it takes the closer box of a Dynamic Zoom.
- The GPU paths need no new work. They take a `Placing` for each frame already ([placed pictures](../design/export.md#placed-pictures)).

## Interchange

**What vidcut reads and writes now** (`otio::effects`, `otio::picture`):

- A clip effect is an OTIO `Effect.1` with the `effect_name` `Resolve Effect`.
- Its data is in `metadata.Resolve_OTIO`: `Effect Name`, `Name`, `Type`, `Enabled`, `Display Type` and `Parameters`.
- A parameter has `Parameter ID`, `Parameter Value`, `Default Parameter Value`, `Variant Type`, and can have `minValue`, `maxValue` and `Key Frames`.
- `Key Frames` is an object. Each name is a keyframe time, a frame count as a string. Each value has `Value` and `Variant Type`.
- `effects::read` keeps an effect that is on when a parameter or a keyframe `Value` is off its default (`in_use`). It keeps a Dynamic Zoom that is on.
- `carry` sets the `from` field of `Effects` to the first source frame of the clip, or of its sound. Thus the code takes time 0 as the first source frame of the item.
- `Effects::written` moves the times by `from` less the first source frame of the written item.
- `otio::picture` reads Transform, Cropping and Retime and Scaling as a `Picture` only when no parameter has keyframes.
  - The center and scale of Dynamic Zoom need exactly two keyframes (`ends`). The earlier keyframe is the start box.
- vidcut writes the two keyframes of Dynamic Zoom at −g and 1000 − g. g is the timeline's `global_start_time` in frames.
  - The [gotchas](../gotchas.md#resolves-otio-files) give this as the form of Resolve's files.
- vidcut writes its other picture parameters with an empty `Key Frames`.
- The clip volume is `Fairlight Clip Volume and Fades` (type 62), with the parameter `volume` in dB.
  - vidcut writes it without `Key Frames`. `effects::gain` reads the gain from its `Parameter Value`.
  - A volume with keyframes off the default stays in the kept effect, unplayed. `effects::sound` writes the gain into its `Parameter Value`.

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

- The `vidcut` metadata of the clip holds the keyframes exactly.
- Resolve's effects get each keyframe in the `Key Frames` of its parameter, in Resolve's units through `Shape`, as `otio::picture::effects` converts plain values.
- The gain keyframes go into the clip volume effect (`effects::sound`).
- An import plays the keyframes of Transform, Cropping and the volume when they fit the model.
- Bezier, Step, or an anchor point with keyed zoom or rotation stays in the kept effects, unplayed, with the warning of the import.

**Unknown:**

- The base of the keyframe times: the first source frame of the item, its first frame in the timeline, or a count with g.
- Why a Dynamic Zoom that is on has keyframes at −g and 1000 − g, and one that is off has keyframes at 0 and 1000.
- How an ease is written, and whether a time can have a fraction.
- Where keyed volume goes, and its `Parameter Value`.
- How Zoom X and Y, and Pan and Tilt, are keyed: together or apart.

### Measurement plan

These runs answer the unknowns. They are for the user to approve, and are not run yet.

- **Setup** (10 min):
  - Resolve Studio 21.1.1. A project at 1920×1080 and 30 fps. The timeline starts at 01:00:00:00, so g is 108000.
  - Media from `$VIDCUT_TEST_DATA/fixtures/`: `bc_h264_1920x1080_30-1_g30b2_900f_aac48k2_o0.mp4` (900 barcode frames with sound) and `bc_h264_320x180_30-1_g30b2_300f_aac48k2tone_o0.mp4` (a tone).
  - Clip A: the barcode clip at timeline frame 60, from source frame 90, 300 frames long.
  - Clip B: the same media at timeline frame 600, with Dynamic Zoom on.
- **R0. The scripting API** (10 min):
  - Look for keyframe calls on timeline items in the documentation of the 21.1.1 scripting API.
  - Answers: which of R1 to R8 a script can do. The other steps are by hand.
- **R1. Volume keyframes** (15 min):
  - Edit page: Alt+click the overlay of clip A at clip frames 0, 30, 60 and 90. Set 0, −12, −12 and 0 dB. Set Ease In on the second keyframe.
  - Export OTIO.
  - Answers: the effect and parameter of volume keyframes, the base of their times, the unit, the form of an ease, and the `Parameter Value`.
- **R2. Transform and Cropping keyframes** (15 min):
  - Inspector, on clip A: keyframes at clip frames 0 and 60.
  - The values: Zoom 1 to 2, Position X 0 to 200, Rotation Angle 0 to 45, and Crop Left 0 to 100.
  - Set Ease Out on one keyframe. Export OTIO.
  - Answers: the shape and units of keyed Transform and Cropping, and whether X and Y are keyed together.
- **R3. Edits** (25 min):
  - From R1 and R2, in four copies: trim the head of clip A by 15 frames; slip it by 15 frames; move it and clip B by 30 frames; split clip A at clip frame 45.
  - Export each copy.
  - Answers: the base of the times; whether keyframes follow the source in a trim and a slip; what a split keeps; whether Dynamic Zoom's times change with the place of clip B.
- **R4. Speed** (20 min):
  - Cut page: Change Clip Speed to 200% and to 50%. Edit page: 200% with Maintain Timing, then with Stretch to fit (p. 1217).
  - Export each.
  - Answers: what the Cut page does to keyframes on a speed change, and the times in the file of a clip at another speed.
- **R5. Curves** (35 min):
  - Position X from 0 to 960 over 30 frames, with Linear.
  - Then Ease In, Ease Out and Ease In and Out on the first keyframe, and then on the second. This gives seven renders.
  - Read the position on each frame, from stills or a Deliver render.
  - Zoom from 1 to 4 over 30 frames, Linear: read the zoom at frame 15.
  - The tone from 0 to −24 dB over 30 frames, Linear: read the level each 10 ms.
  - Answers: the curves; whether the eases play (those of Dynamic Zoom do not); whether gain changes in dB or in amplitude.
- **R6. Read back** (15 min):
  - Change the R1 and R2 files by hand: move each time by 10, and change one value. Import them into Resolve.
  - Read where the keyframes are in the Inspector and the Keyframe Editor.
  - Answers: whether Resolve reads the times in the base that R3 found. Thus vidcut can write them.
- **R7. The Cut page** (15 min):
  - Look for a volume overlay on clips in the Cut page timeline, and for Alt+click on it.
  - Look for keyframe buttons in the Video panel: one for each value, or one for each group, and on Dynamic Zoom.
  - Look at the Keyframe Editor tab, its Timeline control, the keyframe tray, [ and ], and a mark on a clip with keyframes.
  - Answers: which behavior is the Cut page's, and which is only the Edit page's (rule 1).
- **R8. Fractions and silence** (15 min):
  - Edit page, zoomed in: add a volume keyframe between two frames (p. 1113). Export OTIO.
  - Ripple Delete Silence (p. 773) on a clip whose keyframes lower a loud part to −60 dB.
  - Answers: whether a time can have a fraction; whether Ripple Delete Silence hears the keyed volume.

**Total:** 175 min, about 3 hours.

## Scripting

Rule 3: each GUI action has a request. These requests are new:

- `keyframes.list`: the keyframes of clips. For each clip and value, each keyframe has its timeline time, its source frame, its value and its ease.
- `keyframes.add`: clips, a value (`param`), a time (`at`, a timeline time or `playhead`), and a value and an ease if given.
  - Without a value, the keyframe takes the value that the clip has there, as Alt+click does.
- `keyframes.set`: the value or the ease of the keyframes at given times.
- `keyframes.move`: the keyframes at given times, by frames or to a time.
- `keyframes.remove`: the keyframes at given times, or all the keyframes of a value, as a Reset does.

These requests change too:

- `picture.set` and `clip.set` take an `at`. On a value with keyframes, they set a keyframe at `at`, or at the playhead. This is what the Inspector does (p. 1251).
- `timeline.get` names the values of a clip that have keyframes.
- Previous and Next Keyframe are `keyframes.list` and `player.seek`, as `timeline.edit_points` and `player.seek` are for edits.

Each request is one undo step, and its step says what changed ([X33](../deviations.md#x33-each-undo-step-says-what-where-and-how-much)). The CLI and MCP get the same requests.

## Open questions

- **Speed:** if the Cut page keeps keyframe times on a speed change (R4), does vidcut follow it? Source frames cannot do this. A freeze would also hold one value.
- **The ease of a keyframe:** does it shape the change before the keyframe, after it, or both? R5 tells Resolve's curve. vidcut's choice follows it or is a deviation.
- **Fractions:** if audio keyframes can be between frames (R8), does the model hold fractions of a frame?
- **`clip.set` on a keyed gain:** a keyframe at the playhead, or a shift of all keyframes by the same dB?
- **Several selected clips:** the Inspector sets a value on all of them (X70). Does a keyframe go on each clip at the playhead?
- **The other users of the gain:** do the Mixer meters and ducking by level follow keyed gain? R8 answers this for Ripple Delete Silence only.
- **Shortcuts:** [ and ] go from keyframe to keyframe (p. 1251). vidcut uses [ and ] for the stripes of Ripple Delete Silence while it is open.
- **Compound clips:** what does `edit.decompose` do with the keyframes of a compound clip for a clip inside that is frozen or plays backward?
- **Dynamic Zoom:** can its boxes have times other than the ends of the clip?
- **Keyframes alone:** copy and paste of selected keyframes (p. 1113), in stage 1 or later?
- **Later stages:** do keyframes of speed (p. 737), opacity, the anchor point, or titles come in after stage 3?
