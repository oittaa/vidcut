# Speed ramps

**When:** in progress, in four pull requests. The first is [oittaa/vidcut#643](https://github.com/oittaa/vidcut/pull/643).

A speed keyframe marks a frame of a clip where its speed can change. The speed keyframes cut the clip into speed sections, and each section has a speed of its own.

- **In vidcut:** clips have speed keyframes, with their sound, trims, keyframes and OTIO ([D48](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The keyframe requests, the speed strip of the timeline, the Speed row, the Mark menu and the Inspector edit them ([X94](../deviations.md#x94-speed-keyframes-and-the-mark-menus-keyframe-items)).
- **Why:** speed ramps and freeze frames inside one clip are usual edits.

`$VIDCUT_TEST_DATA/fixtures/speed/results.md` has the checks in Resolve Studio 21.1.2 and their exports.

## To do

- **OTIO import:** a speed keyframe on a compound clip is kept but plays at 100%. The import warns.
