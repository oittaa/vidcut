# Keyframes

**When:** not decided.

A keyframe is a value of a clip at one frame. In this file, a keyframe is never a video keyframe of a codec.

vidcut has keyframes on the picture, the volume, the pan, the effects, the speed, titles, subtitles and the Four Color Gradient. The spec tells how they work:

- The Keyframes tab and the keyframe tray: [spec §2](../spec.md#2-layout-pp-636637).
- Transform, Crop and Composite: [§8.15](../spec.md#815-transform-and-crop-pp-766768) and D75. The volume and the pan: D81 to D85.
- Effects: [§8.16](../spec.md#816-effects-pp-765-3526-3536). Titles: [§8.14](../spec.md#814-titles-pp-762764-11761178). Subtitles: [§8.12](../spec.md#812-subtitles-pp-645646-702-12271248).
- The model: [design/model.md](../design/model.md#pictures). Resolve's files: [gotchas](../gotchas.md#resolves-otio-files).

This file lists what vidcut does not have yet. The page numbers are those of the manual.

- **Buildable:** the manual or a measurement gives the behavior.
- **Needs a Resolve run:** the item names the run that gives the behavior.

## Buildable

In order of value:

- **Select All and Deselect All.** Ctrl+A in the Keyframes tab or the keyframe tray selects all the keyframes of the shown item (p. 1262). Ctrl+Shift+A clears that selection. Now both keys act on the clips of the timeline.
- **Keyframe buttons on the sections.** The Transform, Crop, Composite and Audio sections get a Keyframe button between Previous and Next. Resolve has one on each group, and it keys each value of the group (p. 747; [§8.15](../spec.md#815-transform-and-crop-pp-766768)).
- **The motion path.** With the Transform handles on, the viewer shows the path of the Position keyframes (pp. 1252–1254). It has a point on each keyframe and a dot on each frame. A drag on a point changes its keyframe. Its right-click menu has Delete Keyframe.
  - Thus the user sees the motion in the picture.
- **Stretch to Fit in Change Clip Speed.** The window gets Maintain Timing and Stretch to Fit (p. 1217). Stretch keeps each Transform, Crop and Composite keyframe on its source frame ([gotchas](../gotchas.md#resolves-otio-files)). Now a speed change always maintains the timing.
- **The time ruler and the curve tooltips.** The ⋯ menu gets Display Time Ruler in Seconds, Display Time Ruler in Frames and Show Curve Tooltips (pp. 1258, 1264). A tooltip gives the name and the value of the curve under the pointer.
- **The parameter filters.** The ⋯ menu gets Display All Video Parameters and Display Selected Parameters (p. 1256). The second opens a check box for each value, in a Video and an Audio group. A clip with effects has many tracks, and the user needs only some of them.
- **Reverse Keyframes.** The ⋯ menu gets Reverse Keyframes (pp. 1264, 1267). It runs the selected keyframes backward over their span. Thus their times change places, and their In and Out handles change places.
- **The parts of the keyframe tray.** The tray gets a divider for its height, a track with the clip's name, and a group track for each section and each effect. It also gets a curve view and a Parameters menu ([spec §2](../spec.md#2-layout-pp-636637), Keyframe tray).
- **The clips of the keyframe tray.** With no clip selected, the tray shows the clip under the playhead. With more than one clip selected, it shows a group for each clip ([spec §2](../spec.md#2-layout-pp-636637), Keyframe tray).
- **Display Speed Keyframes.** Timeline Options gets this switch (p. 645). It shows or hides the speed marks on the clips.
- **Hand Mode.** The header of the graph gets Hand Mode. Then a drag moves the view (p. 1263).

## Needs a Resolve run

- **X and Y keyed apart.** In Resolve's file, Position X and Y have keyframes of their own, and so do Zoom X and Y ([gotchas](../gotchas.md#resolves-otio-files)). vidcut keys X and Y together. Thus an import with keys apart leaves Transform unplayed.
  - Run: key Position X alone after an XY key, and Zoom X alone with the link off. Then look at the tracks, the arrows, a drag and a delete.
- **Smooth points on the motion path.** Smooth gives a point Bezier handles in the viewer, and an acceleration handle sets the ease (pp. 1252–1254).
  - Run: set Smooth on a point, drag its handles, export OTIO and render each frame.
- **Step In and Step Out.** Each holds a value and then jumps (pp. 1263, 1266). The two pages of the manual do not agree on which value holds.
  - Run: set each on Zoom keys, render each frame and export OTIO.
- **Loop and Ping Pong.** Each repeats the selected keyframes to the end of the clip (p. 1267).
  - Run: set each. See if Resolve adds keyframes, how far, and what a trim then does. Export OTIO.
- **The Shape Box.** A box around keyframes stretches them, and Alt on a corner skews them (p. 1265).
  - Run: drag each edge and an Alt corner around three keys. Read the times, the values and the handles.
- **Lock.** The lock of a track prevents accidental changes (p. 1257). Shift-click locks all the tracks, and Alt-click locks all but one.
  - Run: lock a track, then try a drag, Delete, an Inspector change and Reset. Save and open the project again.
- **Show Handles and Flat Handle.** Show Handles shows the handles of all the points, and Flat Handle turns a handle restraint on or off (p. 1263). vidcut always shows all the handles.
  - Run: switch each, and look at the handles and a handle drag.
- **Shift-click in the graph.** The manual says that Shift-click selects the keyframes between two clicks (p. 1262). In the tracks, Shift-click selects one keyframe ([spec §2](../spec.md#2-layout-pp-636637)).
  - Run: Shift-click two points in the Curve Editor.
- **Keyframes of several clips.** The manual shows the keyframes of all the selected clips, side by side or stacked (p. 1268). Resolve Studio 21.1.2 showed the item selected first.
  - Run: select two clips side by side, then stacked, on the Cut page and the Edit page.
- **Film Grain Freeze keyframes.** Freeze takes a curve on the Fusion page ([§8.16](../spec.md#816-effects-pp-765-3526-3536), [X152](../deviations.md#x152-film-grain-freeze-has-no-keyframes)).
  - Run: the Keyframe button of Film Grain in the Inspector of the Cut page, with Freeze on. Also the grain of a frozen frame before the first Freeze keyframe.
- **Dynamic Zoom at other times.** vidcut keeps the two boxes at the ends of the clip.
  - Run: import a file with the two keys at other times, and look for a control that moves them.
- **The shadow and the background of a title.** Resolve's Inspector keys the Drop Shadow and the Background values of a title ([§8.14](../spec.md#814-titles-pp-762764-11761178)).
  - Run: key each value, render and export OTIO.
