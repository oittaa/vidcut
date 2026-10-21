# Clip transform

**When:** next, first in [the roadmap](../roadmap.md#what-comes-next). A picture in a picture, a punch-in and a slow push on a still are what a simple video most often still needs another editor for.

The Cut page's viewer Tools (p. 766) include Transform, Crop and Dynamic Zoom (pp. 767–768). Each has a switch that turns it off without losing its settings, and a Reset (p. 767). vidcut builds those three for footage, stills, image sequences and compound clips. Titles keep their own place, zoom and rotation ([X54](../deviations.md#x54-a-titles-place-is-a-point-in-fractions-of-the-picture)).

## What the Cut page does

- **Transform** (p. 767): drag inside the clip's box to move it, a corner to resize it in proportion, a side to stretch one axis, and the center handle to rotate it. Shift keeps a move to one axis. The values are Zoom X and Y, linked or not, Position X and Y, Rotation Angle about the anchor point, Pitch, Yaw, and Flip Horizontal and Vertical.
- **Crop** (p. 768): a handle on each side, and Crop Left, Right, Top and Bottom in pixels. What is cropped away is transparent, so a lower track shows through. Softness blurs the edges, inside the crop box when negative and outside it when positive.
- **Dynamic Zoom** (p. 768): off until its controls are touched. A green box is the start and a red box the end; each is moved and resized in the viewer, and a path shows the motion. Swap reverses them. The ease is Linear, Ease In, Ease Out or Ease In and Out. Shift keeps a drag to one axis.

## What vidcut does

- **In the viewer.** The same handles as a title's (`onscreen.rs`, [X52](../deviations.md#x52-titles-come-from-a-titles-tab-or-add-title-and-are-edited-in-a-title-window-without-an-inspector)): snapping to the middle and the thirds with a guide drawn, Alt turning snapping off, and one request on release.
- **No new buttons.** A key shows a tool's handles in the viewer for the selected clip, and the same key hides them. The clip's right-click menu lists the three tools with their keys, F1 lists the keys, and each tool has Reset there too. Proposed keys, all free now: T for Transform, Shift + T for Crop, Alt + T for Dynamic Zoom.
- **Left out:** Pitch, Yaw, crop Softness, Composite's opacity and blend modes (p. 769), and keyframes. Dynamic Zoom is the one move.
- **Units** as a title's: `position` is where the picture's middle goes, `[x, y]` in fractions of the frame from its top left (`[0.5, 0.5]` is today's place); rotation is degrees counterclockwise; crop is fractions of the source picture on each side. A clip whose values are all the defaults draws as today, with opaque bars around a picture of another aspect; any other clip composites over the tracks below it.
- **Requests:** `picture.get`, `picture.set` and `picture.clear`, so `vidcut picture set c3 --zoom 0.33 --position 0.8,0.2` and the MCP tools `picture_get`, `picture_set` and `picture_clear`. `edit.copy` and `edit.paste` carry it with the clip.

## OpenTimelineIO

Resolve writes these as clip effects named `Resolve Effect`: Transform is type 2, Cropping type 3, Composite type 1, and Dynamic Zoom type 59, whose `dynamicZoomCenter` and `dynamicZoomScale` are keyed at `"0"` and `"1000"` ([OpenTimelineIO discussion 921](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/921)). vidcut carries them today without playing them; `effects::in_use` already counts a Dynamic Zoom as in use.

The ids of a clip's Transform are not measured. A title's are (`transformationZoomX`, `transformationZoomY`, `transformationRotationAngle`, spec D67). Position, the crop and Dynamic Zoom's ease need one Resolve Studio export measured, as the titles' were.

## Open questions

- The four eases' curves, measured from Resolve's renders.
- The units of Position and the crop in Resolve's OTIO file.
- Whether Resolve applies a Transform that vidcut wrote, as it needed a measured generator before it showed titles.
