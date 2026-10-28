# Speed ramps

**When:** not decided.

A speed point marks a frame of a clip where its speed can change. The points cut the clip into speed sections, and each section has a speed of its own. An example is a clip at 100% that goes to 25% for a moment, then back to 100%.

- **In vidcut:** a clip has one speed for its whole length (Change Clip Speed, [D48](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). vidcut has no speed points.
- **Why:** speed ramps, freeze frames and rewinds inside one clip are usual edits. The Mark menu's keyframe items also act on speed points (below).

## What Resolve has

- **The Cut page:** the speed tool of the viewer has Add Speed Point. It adds a speed point at the playhead (p. 769).
- **The Edit page:** Clip > Retime Controls (Ctrl+R) shows the speed points on the clip (pp. 1218–1222).
  - A drag on the top handle of a speed point changes the speed of the section before it (p. 1221).
  - Freeze Frame, Rewind and Speed Ramp add preset speed points (pp. 1220–1222).
  - Clear Speed Point removes a speed point (p. 1221).
- **The retime curve:** its points match the speed points of the Retime Controls (p. 1223).
- Speed % and Frames Per Second of Speed Change can have keyframes (p. 737).

## Measured in Resolve Studio 21.1.2

On the Cut page, with the Keyframes panel and the keyframe tray closed, the Mark menu's keyframe items act on speed points. Add Keyframe also does this with the tray open and no row selected.

- Add Keyframe adds a speed point at the playhead on the selected clips. The linked audio gets it too.
  - A new speed point cuts a section into two sections of the same speed.
- Delete Keyframe removes the nearest speed point at or before the playhead.
- Delete All Keyframes removed the speed point of the selected clip, with no question. That clip had one speed point.
- An OTIO export writes the speed points in the clip's `TimeEffect`, in seconds of the media.

`$VIDCUT_TEST_DATA/fixtures/keyframes/results.md` (Q8) has the checks and their exports.

## Open questions

- **Model:** how does a clip hold its speed sections, and how do the trims and the keyframes of the clip follow them?
- **OTIO:** what does each field of Resolve's `TimeEffect` with speed points mean?
