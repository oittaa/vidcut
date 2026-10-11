# Source Tape marks spanning several clips

**When:** later. **In progress:** pull request #25 (needs rework).

Decision D17 limits Source Tape In/Out to the clip under the playhead. Resolve's Source Tape marks can span clips.

**Before #25 can merge:**
- It adds `tape_marks` (with a snapshot of the pool order) to the project file. Check that this is the right model before changing the format.
- Its edits with several pieces reimplement the timeline-mark fitting (`fit_block`) beside the single-clip paths, and its Place On Top snaps to an edit within 5 frames instead of following the manual's alignment rules. They should go through the same edit operations as one clip does.
