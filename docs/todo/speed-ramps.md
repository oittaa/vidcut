# Speed ramps

**When:** in progress, in four pull requests.

A speed point marks a frame of a clip where its speed can change. The points cut the clip into speed sections, and each section has a speed of its own.

- **In vidcut:** clips have speed points, with their sound, trims, keyframes and OTIO ([D48](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). The `speed.*` requests edit them.
- **Why:** speed ramps and freeze frames inside one clip are usual edits. The Mark menu's keyframe items also act on speed points ([X94](../deviations.md#x94-the-mark-menus-keyframe-items-act-only-on-keyframes)).

`$VIDCUT_TEST_DATA/fixtures/speed/results.md` has the checks in Resolve Studio 21.1.2 and their exports.

## To do

- **Timeline:** a strip above a clip with points, with the speed of each section. A click on a speed opens 20%, 30%, 40%, 50%, 60%, 70%, 100% and Set.
- **Mark menu:** with the panel and the tray closed, the keyframe items act on speed points, as in Resolve.
  - The labels then say Add Speed Point, Delete Speed Point and Delete All Speed Points.
  - Delete All Speed Points removes all the points, after a question.
- **Inspector:** the Speed section edits the section under the playhead: its speed, freeze and reverse.
  - Reverse plays the section's own frames backward. The later sections do not change.
- **Ease:** Ease In, Ease Out and Ease In and Out on a point, and the Speed rows of the keyframe tray.
  - The ease formula is not known. A short check in Resolve comes first.
- **OTIO import:**
  - Eased keys play at the linear speeds of their sections. The import warns.
  - A point on a compound clip, and a section that plays backward, are kept but play at 100%. The import warns.
