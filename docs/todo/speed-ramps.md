# Speed ramps

**When:** in progress, in four pull requests. The first is [oittaa/vidcut#643](https://github.com/oittaa/vidcut/pull/643).

A speed keyframe marks a frame of a clip where its speed can change. The speed keyframes cut the clip into speed sections, and each section has a speed of its own.

- **In vidcut:** clips have speed keyframes, with their sound, trims, keyframes and OTIO ([D48](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The keyframe requests, the speed strip of the timeline, the Speed row and the Mark menu edit them ([X94](../deviations.md#x94-speed-keyframes-and-the-mark-menus-keyframe-items)).
- **Why:** speed ramps and freeze frames inside one clip are usual edits.

`$VIDCUT_TEST_DATA/fixtures/speed/results.md` has the checks in Resolve Studio 21.1.2 and their exports.

## To do

- **Inspector:** the Speed section edits the section under the playhead: its speed, freeze and reverse.
  - Reverse plays the section's own frames backward. The later sections do not change.
- **Ease:** Ease In, Ease Out and Ease In and Out on a speed keyframe, in the ease menu of the Speed row.
  - The ease formula is not known. A short check in Resolve comes first.
- **OTIO import:**
  - Eased keys play at the linear speeds of their sections. The import warns.
  - A speed keyframe on a compound clip, and a section that plays backward, are kept but play at 100%. The import warns.
