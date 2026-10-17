# Dissolves: where Resolve puts them

**When:** next time on the Windows computer, which has Resolve Studio and Claude Code.

Resolve Studio 21 on macOS placed dissolves differently from vidcut in three cases. Check each on Resolve's Cut page, then write the rule into spec D50 (or a deviation) and change `ops::dissolve` (`room` in `crates/vidcut-core/src/ops/dissolve.rs`) to match.

1. **Next to empty track or the timeline start.** Resolve put the whole one-second dissolve inside the clip: starting on the edit at a clip's start, ending on it at a clip's end, even where the clip had media past its edge. vidcut centers it on the edit, half of it over the empty track, and at the timeline start shortens it to the half inside.
2. **Selecting that edge.** Clicking a clip's end next to empty track highlighted only the clip's side in Resolve. Check whether vidcut's timeline selects both sides there, which would center Ctrl + T's dissolve.
3. **No frames past the cut.** Where both clips use their whole files, vidcut's Dissolve button says "the clips at the edit have no frames past it to dissolve with". The manual (p. 1160) says Ctrl + T on such an edit asks instead: Trim Clips, Skip Clips or Cancel. Check what Ctrl + T and the Dissolve button do there.

## The timeline

In Resolve, a new project at 30 fps, with `bbb.mp4`, `cam.mov` and `broll.mp4` from `~/Downloads/vidcut-tmp/resolve-import/media` (copied to the Windows computer):

- V1: all of `bbb.mp4` (0 to 20 s), then all of `cam.mov` (20 to 40 s). The cut at 20 s has no frames past it.
- V2: `broll.mp4` from 1 s to 4 s of the file, placed at 13 s. It has media past both its edges, and empty track on both sides.

Export it as OTIO and import it into vidcut, so both tools start from the same edit. Undo between tests.

## Tests

For each, note where every dissolve starts and ends, and any dialog.

A. The timeline start:
1. Put the playhead at 01:00:01:00.
2. Click Dissolve.

B. A V2 clip with empty track on both sides:
1. Click the broll clip on V2.
2. Press Ctrl + T.

C. One edge of that clip:
1. Click the end of the broll clip on V2, and note which side highlights.
2. Press Ctrl + T.

D. A cut with no frames past it:
1. Put the playhead at 01:00:19:00.
2. Click Dissolve.
3. Undo, select the cut at 20 s with V, and press Ctrl + T.
